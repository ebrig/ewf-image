//! One-shot CLI acquisitions; no source substitutions or checkpoint resume.
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Progress, Result, export, hex, invalid, sidecar, source, verify};
use clap::Args;
use ewf_image::{
    EwfError, EwfWriter, Image, LogicalEntryMetadata, LogicalWriter, SequentialOptions,
    SequentialWriter, SingleFileEntryType, WriteCompression, WriteFormat, WriteResult,
};
use serde_json::{Value, json};

#[derive(Args)]
pub(super) struct OutputArgs {
    /// New image path (.Ex01 for acquisition, .Lx01 for collection).
    pub output: PathBuf,
    /// Chunks per segment; chunks use standard 32 KiB geometry.
    #[arg(long, default_value_t = 1024, value_name = "COUNT", help_heading = "Image settings", value_parser = clap::value_parser!(u32).range(1..=16384))]
    chunks_per_segment: u32,
    /// Image compression.
    #[arg(long, default_value = "zlib", help_heading = "Image settings", value_parser = ["raw", "zlib"])]
    compression: String,
    /// Case identifier.
    #[arg(long, value_name = "ID", help_heading = "Case details")]
    case_number: Option<String>,
    /// Evidence identifier.
    #[arg(long, value_name = "ID", help_heading = "Case details")]
    evidence_number: Option<String>,
    /// Examiner name.
    #[arg(long, value_name = "NAME", help_heading = "Case details")]
    examiner: Option<String>,
}

#[derive(Args)]
pub(crate) struct AcquireArgs {
    /// Source file or device.
    source: PathBuf,
    #[command(flatten)]
    output: OutputArgs,
    /// Logical sector size for a regular file.
    #[arg(long, value_name = "BYTES", help_heading = "Image settings", value_parser = clap::value_parser!(u32).range(512..=4096))]
    sector_size: Option<u32>,
    /// Deadline for each source read, in milliseconds.
    #[arg(long, value_name = "MS", value_parser = clap::value_parser!(u64).range(1..))]
    read_timeout_ms: Option<u64>,
}

#[derive(Args)]
pub(crate) struct CollectArgs {
    /// Directory to collect; use a stable snapshot.
    source: PathBuf,
    #[command(flatten)]
    output: OutputArgs,
}

fn destination(path: &Path, extension: &str) -> Result<PathBuf> {
    if path.extension().and_then(|s| s.to_str()) != Some(extension) {
        return Err(invalid(&format!("output must end in .{extension}")));
    }
    export::destination(&[], path)
}

fn settings(args: &OutputArgs, size: u64, format: WriteFormat) -> SequentialOptions {
    let mut settings = SequentialOptions::new(size);
    settings.chunks_per_segment = args.chunks_per_segment;
    settings.write.format = format;
    settings.write.compression = if args.compression == "raw" {
        WriteCompression::None
    } else {
        WriteCompression::Zlib
    };
    settings
        .write
        .metadata
        .case_number
        .clone_from(&args.case_number);
    settings
        .write
        .metadata
        .evidence_number
        .clone_from(&args.evidence_number);
    settings.write.metadata.examiner.clone_from(&args.examiner);
    settings.write.metadata.acquisition_software = Some(env!("CARGO_PKG_NAME").into());
    settings.write.metadata.acquisition_software_version = Some(env!("CARGO_PKG_VERSION").into());
    settings
}

fn check_stop(progress: &mut Progress<'_>, phase: &str, done: u64, total: u64) -> Result<()> {
    if progress.event(phase, done, total).is_break() {
        return Err(EwfError::Aborted.into());
    }
    Ok(())
}

fn start(report: &mut Value, source: &Path, output: &Path) {
    report["source"] = json!(source);
    report["output"] = json!(output);
    report["resumable"] = json!(false);
    report["publication_state"] = json!("not_started");
}

fn published(
    output: &Path,
    outcome: ewf_image::Result<WriteResult>,
    report: &mut Value,
) -> Result<WriteResult> {
    match outcome {
        Ok(result) => {
            report["published"] = json!(true);
            report["publication_state"] = json!("committed");
            report["segments"] = json!(result.segment_paths);
            report["computed_sha256"] = json!(hex(&result.computed_sha256));
            Ok(result)
        }
        Err(error) => {
            // A finish error can follow installation or commit. Never claim
            // rollback/success until the journal has been explicitly recovered.
            report["published"] = Value::Null;
            report["publication_state"] = json!("unresolved");
            report["recovery_command"] = json!([
                env!("CARGO_PKG_NAME"),
                "recover-publication",
                output.to_string_lossy().as_ref()
            ]);
            Err(error.into())
        }
    }
}

pub(super) fn acquire(
    args: &AcquireArgs,
    stop: &Arc<AtomicBool>,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    let output = destination(&args.output.output, "Ex01")?;
    start(report, &args.source, &output);
    let mut input = source::Source::open(&args.source, args.sector_size, &output)?;
    let size = input.identity.size;
    let mut settings = settings(&args.output, size, WriteFormat::Ewf2Physical);
    settings.write.bytes_per_sector = input.identity.sector_size;
    settings.write.sectors_per_chunk = 32768 / input.identity.sector_size;
    report["source_identity"] = json!(input.identity);
    input.configure_reads(
        Arc::clone(stop),
        args.read_timeout_ms.map(Duration::from_millis),
    )?;
    check_stop(progress, "acquisition", 0, size)?;
    let mut writer = SequentialWriter::create(&output, settings)?;
    report["phase"] = json!("acquisition");
    let mut buffer = vec![0; 1024 * 1024];
    while writer.position() < size {
        check_stop(progress, "acquisition", writer.position(), size)?;
        let take = (size - writer.position()).min(buffer.len() as u64) as usize;
        // Do not let read_exact retry Interrupted indefinitely on cancellation.
        let count = match input.read(&mut buffer[..take]) {
            Ok(0) => return Err(invalid("source ended before its declared size")),
            Ok(count) => count,
            Err(error) => {
                check_stop(progress, "acquisition", writer.position(), size)?;
                return Err(error.into());
            }
        };
        writer.write_all(&buffer[..count])?;
        report["accepted_bytes"] = json!(writer.position());
    }
    input.check_unchanged()?;
    check_stop(progress, "publication", size, size)?;
    report["phase"] = json!("publication");
    let written = published(&output, writer.finish(), report)?;
    verify(
        &output,
        Some(written.computed_sha256),
        None,
        progress,
        report,
    )?;
    report["status"] = json!("complete");
    Ok(())
}

struct Entry {
    path: PathBuf,
    parent: Option<usize>,
    metadata: Metadata,
    identity: String,
    logical: LogicalEntryMetadata,
}

fn regular_metadata(path: &Path) -> Result<Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid("reparse points are not collected"));
        }
    }
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err(invalid("links and special files are not collected"));
    }
    Ok(metadata)
}

fn timestamp(time: std::io::Result<SystemTime>) -> Option<i64> {
    let time = time.ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(n) => i64::try_from(n.as_secs()).ok(),
        Err(n) => i64::try_from(n.duration().as_secs()).ok()?.checked_neg(),
    }
}

fn inventory(root: &Path, progress: &mut Progress<'_>) -> Result<(Vec<Entry>, u64)> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut stack = vec![(root.to_path_buf(), None, 0)];
    let mut total = 0u64;
    while let Some((path, parent, depth)) = stack.pop() {
        check_stop(progress, "discovery", entries.len() as u64, 0)?;
        if entries.len() >= 100_000 || depth >= 128 {
            return Err(invalid(
                "collection exceeds 100000 entries or 127 directory levels",
            ));
        }
        let metadata = regular_metadata(&path)?;
        let identity = source::metadata_identity(&metadata)?;
        let name = if parent.is_none() {
            String::new()
        } else {
            path.file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| invalid("collection names must be Unicode"))?
                .to_owned()
        };
        if name.contains(['\0', '\t', '\r', '\n']) {
            return Err(invalid("source name contains a catalog delimiter"));
        }
        let logical = LogicalEntryMetadata {
            name,
            creation_time: timestamp(metadata.created()),
            modification_time: timestamp(metadata.modified()),
            access_time: timestamp(metadata.accessed()),
            ..Default::default()
        };
        if metadata.is_dir() {
            let mut children = Vec::new();
            for entry in fs::read_dir(&path)? {
                if entries.len() + stack.len() + children.len() >= 99_999 {
                    return Err(invalid("collection exceeds 100000 entries"));
                }
                children.push(entry?.path());
            }
            children.sort();
            stack.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|path| (path, Some(entries.len()), depth + 1)),
            );
        } else {
            total = total
                .checked_add(metadata.len())
                .ok_or_else(|| invalid("collection byte count overflow"))?;
        }
        entries.push(Entry {
            path,
            parent,
            metadata,
            identity,
            logical,
        });
    }
    Ok((entries, total))
}

fn unchanged(entry: &Entry) -> Result<()> {
    if source::metadata_identity(&regular_metadata(&entry.path)?)? != entry.identity {
        return Err(invalid("source changed during collection"));
    }
    Ok(())
}

fn open_regular(entry: &Entry) -> Result<File> {
    unchanged(entry)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(&entry.path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || source::metadata_identity(&meta)? != entry.identity {
        return Err(invalid("opened source differs from inventory"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(invalid("reparse points are not collected"));
        }
    }
    Ok(file)
}

pub(super) fn collect(
    args: &CollectArgs,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    if !regular_metadata(&args.source)?.is_dir() {
        return Err(invalid("collection source must be a directory"));
    }
    let root = args.source.canonicalize()?;
    let output = destination(&args.output.output, "Lx01")?;
    if output.starts_with(&root) {
        return Err(invalid("collection output must be outside the source tree"));
    }
    start(report, &root, &output);
    report["phase"] = json!("discovery");
    let (entries, total) = inventory(&root, progress)?;
    let count = entries
        .iter()
        .filter(|entry| entry.metadata.is_file())
        .count();
    report["collection"] = json!({"files":count,"folders":entries.len()-count-1,"bytes":total,
        "snapshot_guaranteed":false,"policy":"strict regular files and directories; no links, special files, ADS or ACL capture"});
    let mut writer = LogicalWriter::create_sequential(
        &output,
        settings(&args.output, total, WriteFormat::Ewf2Logical),
    )?;
    let mut identifiers = vec![1];
    let mut accepted = 0u64;
    report["phase"] = json!("collection");
    for entry in entries.iter().skip(1) {
        check_stop(progress, "collection", accepted, total)?;
        let parent = identifiers[entry.parent.expect("non-root parent")];
        let id = if entry.metadata.is_dir() {
            unchanged(entry)?;
            writer.add_directory(parent, entry.logical.clone())?
        } else {
            // Recheck every ancestor too; do not traverse a substituted link.
            let mut ancestor = entry.parent;
            while let Some(index) = ancestor {
                unchanged(&entries[index])?;
                ancestor = entries[index].parent;
            }
            let mut input = open_regular(entry)?;
            let id = writer.add_file_with_progress(
                parent,
                entry.logical.clone(),
                entry.metadata.len(),
                &mut input,
                |p| {
                    report["accepted_bytes"] = json!(accepted + p.bytes_written);
                    progress.event("collection", accepted + p.bytes_written, total)
                },
            )?;
            if source::metadata_identity(&input.metadata()?)? != entry.identity {
                return Err(invalid("opened source changed during collection"));
            }
            unchanged(entry)?;
            accepted += entry.metadata.len();
            id
        };
        identifiers.push(id);
    }
    for entry in &entries {
        check_stop(progress, "source_validation", accepted, total)?;
        unchanged(entry)?;
    }
    check_stop(progress, "publication", accepted, total)?;
    report["phase"] = json!("publication");
    let written = published(&output, writer.finish(), report)?;
    verify(
        &output,
        Some(written.computed_sha256),
        None,
        progress,
        report,
    )?;
    let image = Image::open(&output)?;
    let mut pending = vec![
        image
            .root_file_entry()
            .ok_or_else(|| invalid("missing published catalog"))?,
    ];
    let mut verified = 0;
    while let Some(entry) = pending.pop() {
        pending.extend(&entry.children);
        if entry.entry_type() == Some(SingleFileEntryType::File) {
            let result = image.verify_single_file_with_progress(
                entry,
                &mut |p: ewf_image::SingleFileProgress| {
                    progress.event("file_verification", p.bytes_processed, p.bytes_total)
                },
            )?;
            if result.references_match() != Some(true) {
                return Err(invalid("published logical file verification failed"));
            }
            verified += 1;
        }
    }
    if verified != count {
        return Err(invalid("published logical file count mismatch"));
    }
    report["verified_files"] = json!(verified);
    report["status"] = json!("complete");
    Ok(())
}

pub(super) fn recover(output: &Path, report: &mut Value) -> Result<()> {
    if !matches!(
        output.extension().and_then(|s| s.to_str()),
        Some("Ex01" | "Lx01")
    ) {
        return Err(invalid("one-shot output must end in .Ex01 or .Lx01"));
    }
    if sidecar(output, "ewf-acquisition").exists() {
        return Err(invalid(
            "use E01 checkpoint resume for acquisition journals",
        ));
    }
    report["output"] = json!(output);
    report["phase"] = json!("publication_recovery");
    let recovered = EwfWriter::recover_output(output, None)?;
    report["recovered"] = json!(recovered);
    report["published"] = json!(output.exists());
    report["verification"] = Value::Null;
    report["status"] = json!("publication_recovered");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn inventory_change_is_rejected_before_opening_payload() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("data"), b"before").unwrap();
        let stop = AtomicBool::new(false);
        let (entries, _) = inventory(root.path(), &mut Progress::new(true, &stop)).unwrap();
        fs::write(root.path().join("data"), b"changed payload").unwrap();
        assert!(open_regular(&entries[1]).is_err());
    }

    #[test]
    fn cancelled_discovery_and_publication_are_explicit() {
        let root = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(true);
        let mut progress = Progress::new(true, &stop);
        assert!(inventory(root.path(), &mut progress).is_err());
        stop.store(false, Ordering::Relaxed);
        assert!(inventory(root.path(), &mut progress).is_ok());
        stop.store(true, Ordering::Relaxed);
        assert!(check_stop(&mut progress, "publication", 0, 0).is_err());
        let mut report = json!({"published":false});
        assert!(
            published(
                &root.path().join("case.Ex01"),
                Err(EwfError::Aborted),
                &mut report
            )
            .is_err()
        );
        assert!(report["published"].is_null());
        assert_eq!(report["publication_state"], "unresolved");
    }
}
