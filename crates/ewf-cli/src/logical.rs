//! Logical conversion preserves file bytes and hierarchy without host extraction.
use crate::{
    Context, Result, aff4,
    format::{self, Input, Output},
    invalid, transfer,
};
use base64::Engine;
use ewf_image::{Image, LogicalEntryMetadata, LogicalWriter, SingleFileEntry, SingleFileEntryType};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::Path,
};

struct Entry {
    key: String,
    parent: Option<usize>,
    folder: bool,
    size: u64,
    logical: LogicalEntryMetadata,
    aff4: aff4_image::LogicalMetadata,
    ewf: Option<SingleFileEntry>,
}

enum Origin {
    Ewf(Box<Image>),
    Aff4(Box<aff4_image::Container>),
}

impl Origin {
    fn verify(&mut self, entry: &Entry, ctx: &mut Context, report: &mut Value) -> Result<[u8; 32]> {
        match self {
            Self::Ewf(image) => {
                let v = image
                    .verify_single_file_with_progress(entry.ewf.as_ref().unwrap(), |p| {
                        ctx.progress("source file verification", p.bytes_processed, p.bytes_total)
                    })?;
                if v.references_match() == Some(false) {
                    report["exit_code"] = json!(3);
                    return Err(invalid("source logical file hash mismatch"));
                }
                Ok(v.hashes.sha256)
            }
            Self::Aff4(c) => {
                let v = c.verify(&entry.key, |a, b| {
                    ctx.progress("source file verification", a, b)
                })?;
                if v.references_match == Some(false) {
                    report["exit_code"] = json!(3);
                    return Err(invalid("source logical file hash mismatch"));
                }
                format::parse_hash(&v.sha256)
            }
        }
    }
    fn reader<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        match self {
            Self::Ewf(image) => Ok(Box::new(
                image.single_file_cursor(entry.ewf.as_ref().unwrap()),
            )),
            Self::Aff4(c) => Ok(Box::new(Aff4Reader {
                container: c,
                id: entry.key.clone(),
                position: 0,
            })),
        }
    }
}

struct Aff4Reader<'a> {
    container: &'a mut aff4_image::Container,
    id: String,
    position: u64,
}
impl Read for Aff4Reader<'_> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = self
            .container
            .read_at(&self.id, b, self.position)
            .map_err(io::Error::other)?;
        self.position += n as u64;
        Ok(n)
    }
}
struct Hashing<'a> {
    input: Box<dyn Read + 'a>,
    hash: Sha256,
}
impl Read for Hashing<'_> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        let n = self.input.read(b)?;
        self.hash.update(&b[..n]);
        Ok(n)
    }
}

fn ewf_entries(image: &Image) -> Result<Vec<Entry>> {
    let root = image
        .root_file_entry()
        .ok_or_else(|| invalid("missing logical catalog"))?;
    let mut result = Vec::new();
    let mut pending: Vec<_> = if root.name.as_ref().is_some_and(|s| !s.is_empty()) {
        vec![(root, None, String::new())]
    } else {
        root.children
            .iter()
            .rev()
            .map(|e| (e, None, String::new()))
            .collect()
    };
    while let Some((e, parent, prefix)) = pending.pop() {
        let folder = match e.entry_type() {
            Some(SingleFileEntryType::Directory) => true,
            Some(SingleFileEntryType::File) => false,
            _ => {
                return Err(invalid(
                    "unsupported logical entry type; conversion would lose evidence",
                ));
            }
        };
        if !folder && !e.children.is_empty() {
            return Err(invalid(
                "file has child entries; conversion would lose evidence",
            ));
        }
        let name = e
            .name
            .clone()
            .ok_or_else(|| invalid("logical entry has no name"))?;
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let index = result.len();
        result.push(Entry {
            key: index.to_string(),
            parent,
            folder,
            size: if folder {
                0
            } else {
                e.size.ok_or_else(|| invalid("logical file has no size"))?
            },
            logical: LogicalEntryMetadata {
                name,
                creation_time: e.creation_time,
                modification_time: e.modification_time,
                access_time: e.access_time,
                entry_modification_time: e.entry_modification_time,
                deletion_time: e.deletion_time,
            },
            aff4: aff4_image::LogicalMetadata {
                path: path.as_bytes().to_vec(),
                created: e.creation_time.map(|v| i128::from(v) * 1_000_000_000),
                modified: e.modification_time.map(|v| i128::from(v) * 1_000_000_000),
                accessed: e.access_time.map(|v| i128::from(v) * 1_000_000_000),
                changed: e
                    .entry_modification_time
                    .map(|v| i128::from(v) * 1_000_000_000),
                ..Default::default()
            },
            ewf: if folder { None } else { Some(e.clone()) },
        });
        pending.extend(
            e.children
                .iter()
                .rev()
                .map(|c| (c, Some(index), path.clone())),
        );
    }
    Ok(result)
}

fn property<'a>(props: &'a [aff4_image::Property], name: &str) -> Option<&'a str> {
    props
        .iter()
        .find(|p| {
            p.predicate == format!("http://aff4.org/Schema#{name}")
                || p.predicate == format!("https://aff4.org/Schema/2022/#{name}")
                || p.predicate == format!("http://aff4.org/Schema/2022/#{name}")
        })
        .map(|p| p.value.as_str())
}
fn timestamp(props: &[aff4_image::Property], name: &str) -> Result<Option<i128>> {
    property(props, name)
        .map(|v| {
            time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339)
                .map(|t| t.unix_timestamp_nanos())
                .map_err(Into::into)
        })
        .transpose()
}
fn seconds(value: Option<i128>) -> Result<Option<i64>> {
    value
        .map(|v| i64::try_from(v.div_euclid(1_000_000_000)).map_err(Into::into))
        .transpose()
}

fn aff4_entries(c: &aff4_image::Container) -> Result<Vec<Entry>> {
    let streams = c.streams()?;
    if streams
        .iter()
        .any(|s| s.types.iter().any(|t| t.ends_with("#FileSubStream")))
    {
        return Err(invalid(
            "logical conversion of substreams is not supported; refusing to omit their bytes",
        ));
    }
    let nodes: BTreeMap<_, _> = streams
        .iter()
        .filter(|s| {
            s.types
                .iter()
                .any(|t| t.ends_with("#FileImage") || t.ends_with("#FolderImage"))
        })
        .map(|s| (s.id.clone(), s))
        .collect();
    for (id, props) in c.metadata() {
        if props.iter().any(|p| {
            p.predicate == "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
                && (p.value.ends_with("#Folder") || p.value.ends_with("#FolderImage"))
        }) && !nodes.contains_key(id)
        {
            return Err(invalid(
                "unrecognized AFF4 folder resource; refusing to omit its hierarchy",
            ));
        }
    }
    if streams.iter().any(|s| {
        s.types.iter().any(|t| t.ends_with("#Image"))
            && !nodes.contains_key(&s.id)
            && !s
                .types
                .iter()
                .any(|t| t.ends_with("#Map") || t.ends_with("#ImageStream"))
    }) {
        return Err(invalid(
            "logical container includes an unsupported image resource",
        ));
    }
    let mut parents = BTreeMap::new();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, s) in &nodes {
        for p in &c.metadata()[id] {
            if p.predicate == "http://aff4.org/Schema#child" {
                if !s.types.iter().any(|t| t.ends_with("#FolderImage"))
                    || !nodes.contains_key(&p.value)
                {
                    return Err(invalid("unsupported AFF4 child relationship"));
                }
                if parents.insert(p.value.clone(), id.clone()).is_some() {
                    return Err(invalid("AFF4 entry has more than one parent relationship"));
                }
                children
                    .entry(id.clone())
                    .or_default()
                    .push(p.value.clone());
            }
        }
    }
    let mut pending: Vec<_> = nodes
        .keys()
        .rev()
        .filter(|id| !parents.contains_key(*id))
        .map(|id| (id.clone(), None))
        .collect();
    let mut visited = BTreeSet::new();
    let mut result = Vec::new();
    while let Some((id, parent)) = pending.pop() {
        if !visited.insert(id.clone()) {
            return Err(invalid("cyclic AFF4 hierarchy"));
        }
        let stream = nodes[&id];
        let props = &c.metadata()[&id];
        let folder = stream.types.iter().any(|t| t.ends_with("#FolderImage"));
        let path = property(props, "originalFileName")
            .or_else(|| property(props, "originalPathName"))
            .or_else(|| property(props, "fileName"))
            .ok_or_else(|| invalid("AFF4 file has no original name"))?;
        let raw = if let Some(raw) = property(props, "originalPathNameRaw") {
            base64::engine::general_purpose::STANDARD.decode(raw)?
        } else {
            path.as_bytes().to_vec()
        };
        let name = property(props, "fileName")
            .unwrap_or_else(|| path.rsplit(['/', '\\']).next().unwrap_or(path))
            .to_owned();
        let metadata = aff4_image::LogicalMetadata {
            path: raw,
            created: timestamp(props, "birthTime")?,
            modified: timestamp(props, "lastWritten")?,
            accessed: timestamp(props, "lastAccessed")?,
            changed: timestamp(props, "recordChanged")?,
            mode: property(props, "fileMode").map(str::parse).transpose()?,
            ..Default::default()
        };
        let index = result.len();
        result.push(Entry {
            key: id.clone(),
            parent,
            folder,
            size: if folder { 0 } else { c.size(&id)? },
            logical: LogicalEntryMetadata {
                name,
                creation_time: seconds(metadata.created)?,
                modification_time: seconds(metadata.modified)?,
                access_time: seconds(metadata.accessed)?,
                entry_modification_time: seconds(metadata.changed)?,
                ..Default::default()
            },
            aff4: metadata,
            ewf: None,
        });
        if let Some(kids) = children.get(&id) {
            pending.extend(kids.iter().rev().map(|id| (id.clone(), Some(index))));
        }
    }
    if visited.len() != nodes.len() {
        return Err(invalid("cyclic AFF4 hierarchy"));
    }
    Ok(result)
}

pub(crate) fn convert(
    input: &Path,
    output: &Path,
    target: Output,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    if !matches!(target, Output::Aff4 | Output::Lx01) {
        return Err(invalid(
            "logical collections convert to .Lx01 or .aff4; use extract for an individual file",
        ));
    }
    report["input"] = json!(input);
    report["output"] = json!(output);
    let (mut origin, entries, paths, snapshots, metadata) = match format::detect(input)? {
        Input::Ewf => {
            let image = crate::password::open(input, ctx.password.as_ref())?;
            if !image.info().acquisition_complete {
                return Err(invalid("source acquisition is incomplete"));
            }
            let paths = image.info().segment_paths.clone();
            let snapshots = transfer::snapshot(&paths)?;
            let v = image.verify_with_progress(&Default::default(), |p| {
                ctx.progress("source verification", p.bytes_verified, p.bytes_total)
            })?;
            if v.references_match() == Some(false) {
                report["exit_code"] = json!(3);
                return Err(invalid("source image hash mismatch"));
            }
            report["source_verification"] = json!({"references_match":v.references_match()});
            let entries = ewf_entries(&image)?;
            let mut metadata = image.info().metadata.clone();
            metadata.password = None;
            (
                Origin::Ewf(Box::new(image)),
                entries,
                paths,
                snapshots,
                metadata,
            )
        }
        Input::Aff4 => {
            let paths = vec![fs::canonicalize(input)?];
            let snapshots = transfer::snapshot(&paths)?;
            let mut c = aff4::open(input)?;
            let v = c.verify_all(None, |_, a, b| ctx.progress("source verification", a, b))?;
            if aff4::has_mismatch(&v)
                || v.metadata_error.is_some()
                || v.resources.iter().any(|r| r.error.is_some())
                || v.checks
                    .iter()
                    .chain(v.metadata.iter().flat_map(|m| m.checks.iter()))
                    .any(|c| c.outcome == aff4_image::CheckOutcome::Unreadable)
            {
                report["exit_code"] = json!(3);
                return Err(invalid("source AFF4 integrity checks failed"));
            }
            report["source_verification"] = json!(v);
            let entries = aff4_entries(&c)?;
            let mut metadata = ewf_image::EwfMetadata::default();
            for (key, out) in [
                ("caseNumber", &mut metadata.case_number),
                ("evidenceNumber", &mut metadata.evidence_number),
                ("examiner", &mut metadata.examiner),
                ("notes", &mut metadata.notes),
            ] {
                let values: BTreeSet<_> = c
                    .metadata()
                    .values()
                    .filter(|p| {
                        p.iter()
                            .any(|p| p.value == "http://aff4.org/Schema#CaseDetails")
                    })
                    .filter_map(|p| property(p, key))
                    .collect();
                if values.len() == 1 {
                    *out = values.first().map(|s| s.to_string());
                }
            }
            (
                Origin::Aff4(Box::new(c)),
                entries,
                paths,
                snapshots,
                metadata,
            )
        }
        Input::Raw => return Err(invalid("raw images have no logical file catalog")),
    };
    let output = crate::ewf::export::destination(&paths, output)?;
    let size = entries
        .iter()
        .try_fold(0u64, |sum, e| sum.checked_add(e.size))
        .ok_or_else(|| invalid("logical size overflow"))?;
    report["bytes"] = json!(size);
    report["metadata_not_preserved"] = json!([
        "format-specific catalog attributes, source identifiers, and original integrity graph; file bytes, hierarchy, common case fields, and supported timestamps are retained",
        if target == Output::Lx01 {
            "AFF4 nanosecond precision, raw name encoding, and POSIX mode have no equivalent in this Lx01 writer"
        } else {
            "EWF deletion timestamps and extended catalog metadata have no equivalent in this AFF4 writer"
        }
    ]);
    let mut ewf_writer = if target == Output::Lx01 {
        let mut options = ewf_image::SequentialOptions::new(size);
        options.write.format = ewf_image::WriteFormat::Ewf2Logical;
        options.write.metadata = metadata.clone();
        Some(LogicalWriter::create_sequential(&output, options)?)
    } else {
        None
    };
    let mut aff4_writer = if target == Output::Aff4 {
        let mut writer =
            aff4_image::Writer::create(&output, aff4_image::Profile::Logical, Default::default())?;
        writer.add_case_metadata(&aff4_image::CaseMetadata {
            case_number: metadata.case_number.unwrap_or_default(),
            evidence_number: metadata.evidence_number.unwrap_or_default(),
            examiner: metadata.examiner.unwrap_or_default(),
            notes: metadata.notes.unwrap_or_default(),
        })?;
        Some(writer)
    } else {
        None
    };
    let mut ewf_ids = Vec::new();
    let mut aff4_ids: Vec<String> = Vec::new();
    let mut hashes = BTreeMap::new();
    for entry in &entries {
        ctx.check(
            "logical conversion",
            hashes.len() as u64,
            entries.len() as u64,
        )?;
        let expected = if entry.folder {
            None
        } else {
            Some(origin.verify(entry, ctx, report)?)
        };
        let mut reader = if entry.folder {
            None
        } else {
            Some(Hashing {
                input: origin.reader(entry)?,
                hash: Sha256::new(),
            })
        };
        let id = if let Some(writer) = &mut ewf_writer {
            let parent = entry.parent.map(|i| ewf_ids[i]).unwrap_or(1);
            let id = if entry.folder {
                writer.add_directory(parent, entry.logical.clone())?
            } else {
                writer.add_file_with_progress(
                    parent,
                    entry.logical.clone(),
                    entry.size,
                    reader.as_mut().unwrap(),
                    |p| ctx.progress("file conversion", p.bytes_written, p.bytes_total),
                )?
            };
            ewf_ids.push(id);
            id.to_string()
        } else {
            let writer = aff4_writer.as_mut().unwrap();
            let mut metadata = entry.aff4.clone();
            metadata.parent = entry.parent.map(|i| aff4_ids[i].clone());
            let id = if entry.folder {
                writer.add_folder(&metadata)?
            } else {
                writer.add_file_with_metadata(
                    &metadata,
                    entry.size,
                    reader.as_mut().unwrap(),
                    |a, b| ctx.progress("file conversion", a, b),
                )?
            };
            aff4_ids.push(id.clone());
            id
        };
        if let Some(reader) = reader {
            let digest: [u8; 32] = reader.hash.finalize().into();
            if Some(digest) != expected {
                return Err(invalid("source file changed after verification"));
            }
            hashes.insert(id, digest);
        }
    }
    if transfer::snapshot(&paths)? != snapshots {
        return Err(invalid("source changed during logical conversion"));
    }
    ctx.check("finalization", size, size)?;
    if let Some(writer) = ewf_writer {
        report["published"] = Value::Null;
        report["recovery_command"] =
            json!(["ewf-cli", "recover-publication", output.to_string_lossy()]);
        let written = writer.finish()?;
        report["published"] = json!(true);
        report["segments"] = json!(written.segment_paths);
        report["recovery_command"] = Value::Null;
        let image = Image::open(&output)?;
        let v = image.verify_with_progress(&Default::default(), |p| {
            ctx.progress("destination verification", p.bytes_verified, p.bytes_total)
        })?;
        if v.references_match() != Some(true) {
            return Err(invalid("destination logical image verification failed"));
        }
        for entry in ewf_entries(&image)?.iter().filter(|e| !e.folder) {
            let e = entry.ewf.as_ref().unwrap();
            let expected = hashes
                .remove(&e.identifier.unwrap().to_string())
                .ok_or_else(|| invalid("unexpected destination file"))?;
            let v = image.verify_single_file_with_progress(e, |p| {
                ctx.progress(
                    "destination file verification",
                    p.bytes_processed,
                    p.bytes_total,
                )
            })?;
            if v.hashes.sha256 != expected || v.references_match() != Some(true) {
                return Err(invalid("destination file differs from source"));
            }
        }
        if !hashes.is_empty() {
            return Err(invalid("destination omitted logical files"));
        }
    } else {
        let (written, v) = match aff4_writer
            .unwrap()
            .finish_verified(aff4_image::Limits::unrestricted(), |_, a, b| {
                ctx.progress("destination verification", a, b)
            }) {
            Ok(v) => v,
            Err(aff4_image::Error::PublishedButUnsynced { result, source }) => {
                report["published"] = json!(true);
                report["output"] = json!(result.path);
                return Err(source.into());
            }
            Err(e) => return Err(e.into()),
        };
        report["published"] = json!(true);
        report["verification"] = json!(v);
        for stream in written.streams {
            let expected = hashes
                .remove(&stream.id)
                .ok_or_else(|| invalid("unexpected destination file"))?;
            if stream.sha256 != format::hex(&expected) {
                return Err(invalid("destination file differs from source"));
            }
        }
        if !hashes.is_empty() {
            return Err(invalid("destination omitted logical files"));
        }
    }
    report["verified_files"] = json!(entries.iter().filter(|e| !e.folder).count());
    report["destination_matches_source"] = json!(true);
    report["status"] = json!("complete_with_metadata_omissions");
    report["exit_code"] = json!(4);
    Ok(())
}
