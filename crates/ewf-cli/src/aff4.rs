use crate::{CaseArgs, Context, Result, invalid};
use aff4_image::{
    CaseMetadata, CollectionLimits, CollectionOptions, Container, Limits, Profile, VolumeSet,
    WriteOptions, Writer,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) fn open(path: &Path) -> aff4_image::Result<Container> {
    Container::open_with_limits(path, Limits::unrestricted())
}

pub(crate) fn case_metadata(case: &CaseArgs) -> CaseMetadata {
    CaseMetadata {
        case_number: case.case_number.clone().unwrap_or_default(),
        evidence_number: case.evidence_number.clone().unwrap_or_default(),
        examiner: case.examiner.clone().unwrap_or_default(),
        ..Default::default()
    }
}

pub(crate) fn info(path: &Path, metadata: bool, report: &mut Value) -> Result<()> {
    let c = open(path)?;
    report["image"] = json!(path);
    report["format"] = json!("AFF4");
    report["version"] = json!(c.version());
    report["volume"] = json!(c.volume_id());
    report["resources"] = json!(c.streams()?);
    report["status"] = json!("inspected");
    if c.version() == (1, 0) {
        report["disks"] = json!(c.disk_images()?);
    }
    if metadata {
        report["resources"] = Value::Null;
        report["metadata"] = json!(c.metadata());
        report["records"] = json!(c.metadata().iter().flat_map(|(subject, properties)| properties.iter().map(move |p| json!({"subject":subject,"predicate":p.predicate,"value":p.value,"datatype":p.datatype,"language":p.language}))).collect::<Vec<_>>());
    }
    Ok(())
}

pub(crate) fn files(path: &Path, offset: usize, limit: usize, report: &mut Value) -> Result<()> {
    let c = open(path)?;
    let entries: Vec<_> = c
        .streams()?
        .into_iter()
        .filter(|s| {
            s.types
                .iter()
                .any(|t| t.ends_with("#FileImage") || t.ends_with("#FolderImage"))
        })
        .collect();
    let page: Vec<_> = entries
        .iter()
        .skip(offset)
        .take(limit)
        .map(|s| {
            let name = c
                .metadata()
                .get(&s.id)
                .and_then(|p| {
                    p.iter().find(|p| {
                        p.predicate.ends_with("#originalPathName")
                            || p.predicate.ends_with("#originalFileName")
                            || p.predicate.ends_with("#fileName")
                    })
                })
                .map(|p| &p.value);
            json!({"id":s.id,"name":name,"size":s.size,"types":s.types})
        })
        .collect();
    report["entries"] = json!(page);
    report["next_offset"] = json!(
        offset
            .checked_add(page.len())
            .filter(|n| *n < entries.len())
    );
    report["status"] = json!("listed");
    Ok(())
}

pub(crate) fn verify(
    path: &Path,
    entry: Option<&str>,
    expected: Option<&str>,
    metadata_hash: Option<&str>,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    let mut c = open(path)?;
    report["image"] = json!(path);
    let selected = if entry.is_none() && expected.is_some() {
        let disks = c.disk_images()?;
        if disks.len() != 1 {
            return Err(invalid(
                "select one AFF4 disk resource when comparing its SHA256",
            ));
        }
        Some(disks[0].resource_id.clone())
    } else {
        None
    };
    let entry = entry.or(selected.as_deref());
    if let Some(entry) = entry {
        let value = c.verify(entry, |a, b| ctx.progress("verification", a, b))?;
        let external = expected.map(|s| s.eq_ignore_ascii_case(&value.sha256));
        let mut complete = value.references_match != Some(false)
            && (value.references_match == Some(true) || external == Some(true))
            && external != Some(false)
            && value.unsupported_hashes.is_empty();
        let mut mismatch = value.references_match == Some(false) || external == Some(false);
        if selected.is_some() {
            let all = c.verify_all(None, |_, a, b| ctx.progress("verification", a, b))?;
            complete = all.all_match() && external == Some(true);
            mismatch |= has_mismatch(&all);
            report["container_verification"] = json!(all);
        }
        report["verification"] = json!(value);
        report["verification"]["scope"] = json!("selected resource");
        report["external_match"] = json!(external);
        report["status"] = json!(if complete {
            "verified"
        } else if mismatch {
            "verification_failed"
        } else {
            "verification_incomplete"
        });
        report["exit_code"] = json!(if complete {
            0
        } else if mismatch {
            3
        } else {
            4
        });
    } else {
        if expected.is_some() {
            return Err(invalid(
                "select a resource to compare an AFF4 decoded-media SHA256",
            ));
        }
        let value = c.verify_all(metadata_hash, |_, a, b| ctx.progress("verification", a, b))?;
        let complete = value.all_match();
        let mismatch = has_mismatch(&value);
        report["summary"] = json!([format!(
            "Resources: {}; checks: {}",
            value.resources.len(),
            value.checks.len()
        )]);
        report["verification"] = json!(value);
        report["status"] = json!(if complete {
            "verified"
        } else if mismatch {
            "verification_failed"
        } else {
            "verification_incomplete"
        });
        report["exit_code"] = json!(if complete {
            0
        } else if mismatch {
            3
        } else {
            4
        });
    }
    Ok(())
}

pub(crate) fn has_mismatch(value: &aff4_image::ContainerVerification) -> bool {
    value
        .checks
        .iter()
        .chain(value.metadata.iter().flat_map(|m| m.checks.iter()))
        .any(|c| c.outcome == aff4_image::CheckOutcome::Mismatch)
}

pub(crate) fn verify_set(
    paths: &[PathBuf],
    image: &str,
    expected: Option<&str>,
    full: bool,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    if let Some(hash) = expected {
        crate::format::parse_hash(hash)?;
    }
    let mut set = VolumeSet::open_with_limits(paths, Limits::unrestricted())?;
    let complete;
    let mismatch;
    if full {
        let value = set.verify_full(image, expected, |_, a, b| {
            ctx.progress("verification", a, b)
        })?;
        complete = value.all_match();
        mismatch = value
            .assembled
            .as_ref()
            .is_some_and(|v| v.external_match == Some(false))
            || value.volumes.iter().any(|v| {
                v.checks
                    .iter()
                    .chain(v.metadata.iter().flat_map(|m| m.checks.iter()))
                    .any(|c| c.outcome == aff4_image::CheckOutcome::Mismatch)
            });
        report["verification"] = json!(value);
    } else {
        let value = set.verify_image(image, expected, |a, b| ctx.progress("verification", a, b))?;
        complete = value.external_match == Some(true);
        mismatch = value.external_match == Some(false);
        report["sha256"] = json!(value.sha256);
        report["verification"] = json!(value);
    }
    report["status"] = json!(if complete {
        "verified"
    } else if mismatch {
        "verification_failed"
    } else {
        "verification_incomplete"
    });
    report["exit_code"] = json!(if complete {
        0
    } else if mismatch {
        3
    } else {
        4
    });
    Ok(())
}

pub(crate) fn collect(
    source: &Path,
    output: &Path,
    case: &CaseArgs,
    exclude: &[PathBuf],
    partial: bool,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    report["output"] = json!(output);
    let mut writer = Writer::create(output, Profile::Logical, WriteOptions::default())?;
    writer.add_case_metadata(&case_metadata(case))?;
    let limits = CollectionLimits {
        reader: Limits::unrestricted(),
        entries: usize::MAX,
        depth: usize::MAX,
    };
    let collection = writer.add_directory_tree_with_limits(
        source,
        &CollectionOptions {
            exclude: exclude.to_vec(),
            allow_partial: partial,
        },
        &limits,
        |_, a, b| ctx.progress("collection", a, b),
    )?;
    let (written, verification) = match writer.finish_verified(Limits::unrestricted(), |_, a, b| {
        ctx.progress("verification", a, b)
    }) {
        Ok(value) => value,
        Err(aff4_image::Error::PublishedButUnsynced { result, source }) => {
            report["published"] = json!(true);
            report["output"] = json!(result.path);
            report["status"] = json!("published_but_unsynced");
            report["error"] = json!(source.to_string());
            report["exit_code"] = json!(4);
            return Ok(());
        }
        Err(aff4_image::Error::VerificationFailed {
            report: verification,
        }) => {
            report["verification"] = json!(verification);
            report["status"] = json!("verification_failed");
            report["exit_code"] = json!(4);
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    report["published"] = json!(true);
    report["output"] = json!(written.path);
    report["bytes"] = json!(collection.bytes);
    report["collection"] = json!(collection);
    report["verification"] = json!(verification);
    report["summary"] = json!([format!(
        "Files: {}; omitted entries: {}",
        collection.files,
        collection.issues.len()
    )]);
    report["status"] = json!(if collection.issues.is_empty() {
        "complete"
    } else {
        "complete_with_omissions"
    });
    report["exit_code"] = json!(if collection.issues.is_empty() { 0 } else { 4 });
    Ok(())
}

pub(crate) fn extract(
    input: &Path,
    entry: &str,
    output: &Path,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    let output = crate::ewf::export::destination(&[input.to_owned()], output)?;
    report["output"] = json!(output);
    let parent = output
        .parent()
        .ok_or_else(|| invalid("missing output parent"))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    let v =
        open(input)?.copy_verified(entry, &mut staged, |a, b| ctx.progress("extraction", a, b))?;
    report["verification"] = json!(v);
    report["verification"]["scope"] = json!("selected resource");
    if v.references_match == Some(false) {
        report["status"] = json!("verification_failed");
        report["exit_code"] = json!(3);
        return Ok(());
    }
    staged.as_file().sync_all()?;
    ctx.check("publication", v.bytes_verified, v.bytes_verified)?;
    staged.persist_noclobber(&output)?;
    report["published"] = json!(true);
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    let complete = v.references_match == Some(true) && v.unsupported_hashes.is_empty();
    report["status"] = json!(if complete {
        "extracted"
    } else {
        "extracted_without_complete_references"
    });
    report["exit_code"] = json!(if complete { 0 } else { 4 });
    Ok(())
}
