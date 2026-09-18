//! Logical evidence operations. Catalog names never become destination paths.

use std::path::Path;

use ewf_image::{EwfError, Image, SingleFileEntry, SingleFileVerification};
use serde_json::{Value, json};

use super::{Progress, Result, export, hex, inspect, invalid};

fn entries(image: &Image) -> Result<impl Iterator<Item = (usize, &SingleFileEntry)>> {
    let root = image
        .root_file_entry()
        .ok_or_else(|| invalid("image has no logical file catalog"))?;
    let mut stack = vec![root];
    Ok(std::iter::from_fn(move || {
        let entry = stack.pop()?;
        stack.extend(entry.children.iter().rev());
        Some(entry)
    })
    .enumerate())
}

fn entry_json(index: usize, entry: &SingleFileEntry) -> Value {
    json!({"index": index, "identifier": entry.identifier, "name": entry.name,
        "type": entry.entry_type().map(|kind| format!("{kind:?}")), "size": entry.size,
        "children": entry.children.len(), "creation_time": entry.creation_time,
        "modification_time": entry.modification_time, "access_time": entry.access_time,
        "entry_modification_time": entry.entry_modification_time, "deletion_time": entry.deletion_time,
        "md5": entry.md5, "sha1": entry.sha1, "source_identifier": entry.source_identifier,
        "permission_group_index": entry.permission_group_index})
}

pub fn list(
    input: &Path,
    offset: usize,
    limit: usize,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    let image = inspect::open(input, report)?;
    report["phase"] = json!("catalog");
    let mut page = Vec::new();
    let mut next = None;
    for (index, entry) in entries(&image)? {
        if progress.event("catalog", index as u64, 0).is_break() {
            return Err(EwfError::Aborted.into());
        }
        if index < offset {
            continue;
        }
        if page.len() == limit {
            next = Some(index);
            break;
        }
        page.push(entry_json(index, entry));
    }
    report["entries"] = json!(page);
    report["next_offset"] = json!(next);
    report["status"] = json!("listed");
    Ok(())
}

fn verification_json(result: &SingleFileVerification) -> Value {
    json!({"scope": "file", "bytes_verified": result.bytes_verified,
        "md5": hex(&result.hashes.md5), "sha1": hex(&result.hashes.sha1),
        "sha256": hex(&result.hashes.sha256), "references_match": result.references_match(),
        "comparisons": result.comparisons})
}

pub fn read(
    input: &Path,
    index: usize,
    output: Option<&Path>,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    let image = inspect::open(input, report)?;
    let entry = entries(&image)?
        .nth(index)
        .map(|(_, e)| e)
        .ok_or_else(|| invalid("catalog index does not exist"))?;
    report["entry"] = entry_json(index, entry);
    let output = output
        .map(|path| export::destination(image.segment_filenames(), path))
        .transpose()?;
    let mut temporary = output
        .as_ref()
        .map(|path| {
            tempfile::Builder::new()
                .prefix(".ewf-logical-")
                .tempfile_in(path.parent().expect("normalized output"))
        })
        .transpose()?;
    report["phase"] = json!("verification");
    let mut callback = |p: ewf_image::SingleFileProgress| {
        report["file_bytes_processed"] = json!(p.bytes_processed);
        progress.event("file", p.bytes_processed, p.bytes_total)
    };
    let verified = if let Some(file) = &mut temporary {
        image.copy_single_file_with_progress(entry, file, &mut callback)?
    } else {
        image.verify_single_file_with_progress(entry, &mut callback)?
    };
    report["verification"] = verification_json(&verified);
    if verified.references_match() == Some(false) {
        return Err(invalid("logical file hash mismatch"));
    }
    let matched = verified.references_match() == Some(true);
    if let (Some(file), Some(output)) = (temporary, output) {
        report["phase"] = json!("publication");
        file.as_file().sync_all()?;
        if progress
            .event("file", verified.bytes_verified, verified.bytes_verified)
            .is_break()
        {
            return Err(EwfError::Aborted.into());
        }
        file.persist_noclobber(&output)?;
        report["output"] = json!(output);
        report["published"] = json!(true);
        #[cfg(unix)]
        std::fs::File::open(output.parent().expect("normalized output"))?.sync_all()?;
        report["status"] = json!(if matched {
            "extracted"
        } else {
            "extracted_without_reference"
        });
    } else {
        report["status"] = json!(if matched {
            "file_verified"
        } else {
            "file_hashes_missing"
        });
    }
    Ok(())
}
