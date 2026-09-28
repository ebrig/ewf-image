//! Decoded-byte transfers with source checks and destination readback.
use crate::{
    CaseArgs, Context, Result, aff4,
    format::{self, Input, Output},
    invalid,
};
use ewf_image::{EwfMetadata, SequentialOptions, SequentialWriter, VerifyOptions};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

struct Source {
    reader: Box<dyn Read>,
    size: u64,
    sector: u32,
    metadata: EwfMetadata,
    ewf_options: Option<ewf_image::WriteOptions>,
    expected: Option<[u8; 32]>,
    snapshots: Vec<(PathBuf, String)>,
    paths: Vec<PathBuf>,
    warnings: Vec<String>,
    omissions: Vec<String>,
}

pub(crate) fn snapshot(paths: &[PathBuf]) -> Result<Vec<(PathBuf, String)>> {
    paths
        .iter()
        .map(|p| {
            Ok((
                p.clone(),
                crate::ewf::source::metadata_identity(&fs::metadata(p)?)?,
            ))
        })
        .collect()
}

impl Source {
    fn check_unchanged(&self) -> Result<()> {
        for (path, identity) in &self.snapshots {
            let m = fs::metadata(path)?;
            if crate::ewf::source::metadata_identity(&m)? != *identity {
                return Err(invalid("source changed during conversion"));
            }
        }
        Ok(())
    }
}

fn sector(known: Option<u32>, supplied: Option<u32>) -> Result<u32> {
    if known.zip(supplied).is_some_and(|(a, b)| a != b) {
        return Err(invalid("sector size disagrees with source geometry"));
    }
    let value = known.or(supplied).unwrap_or(512);
    if !matches!(value, 512 | 1024 | 2048 | 4096) {
        return Err(invalid(
            "supported sector sizes are 512, 1024, 2048, and 4096",
        ));
    }
    Ok(value)
}

fn read_source(
    input: &Path,
    resource: Option<&str>,
    supplied_sector: Option<u32>,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<Source> {
    match format::detect(input)? {
        Input::Raw => {
            if resource.is_some() {
                return Err(invalid("raw images do not have resource selectors"));
            }
            let paths = vec![fs::canonicalize(input)?];
            let snapshots = snapshot(&paths)?;
            let file = File::open(input)?;
            let size = file.metadata()?.len();
            Ok(Source {
                reader: Box::new(file),
                size,
                sector: sector(None, supplied_sector)?,
                metadata: Default::default(),
                ewf_options: None,
                expected: None,
                snapshots,
                paths,
                warnings: vec![],
                omissions: vec![],
            })
        }
        Input::Ewf => {
            if resource.is_some() {
                return Err(invalid(
                    "resource selection applies to AFF4 physical containers",
                ));
            }
            let image = crate::password::open(input, ctx.password.as_ref())?;
            if image.info().single_files.is_some() {
                return Err(invalid(
                    "logical containers require logical conversion, not physical-media conversion",
                ));
            }
            if !image.info().acquisition_complete {
                return Err(invalid("source acquisition is incomplete"));
            }
            let paths = image.info().segment_paths.clone();
            let snapshots = snapshot(&paths)?;
            let v = image.verify_with_progress(&VerifyOptions::default(), |p| {
                ctx.progress("source verification", p.bytes_verified, p.bytes_total)
            })?;
            report["source_verification"] = json!({"references_match":v.references_match(),"sha256":format::hex(&v.hashes.sha256)});
            if v.references_match() == Some(false) {
                report["exit_code"] = json!(3);
                return Err(invalid("source reference hashes do not match"));
            }
            let mut options = ewf_image::WriteOptions::default();
            options.copy_media_values_from_image(&image)?;
            options.copy_header_values_from_image(&image);
            options.metadata.password = None;
            options.header_codepage = image.info().header_codepage;
            options.header_values_date_format = image.info().header_values_date_format;
            options.acquisition_errors = image.info().acquisition_errors.clone();
            options.sessions = image.info().sessions.clone();
            options.tracks = image.info().tracks.clone();
            options.memory_extents = image.info().memory_extents.clone();
            let mut omissions = Vec::new();
            if !image.info().ewf2_increment_data.is_empty()
                || image.info().ewf2_final_information.is_some()
                || image.info().ewf2_restart_data.is_some()
                || image.info().ewf2_analytical_data.is_some()
            {
                omissions
                    .push("opaque EWF2 auxiliary sections tied to the original container".into());
            }
            let metadata = options.metadata.clone();
            let warnings = if v.references_match().is_none() {
                vec!["Source has no supported reference hash; destination is compared with decoded source bytes.".into()]
            } else {
                vec![]
            };
            Ok(Source {
                reader: Box::new(image.cursor()),
                size: image.media_size(),
                sector: sector(Some(options.bytes_per_sector), supplied_sector)?,
                metadata,
                ewf_options: Some(options),
                expected: Some(v.hashes.sha256),
                snapshots,
                paths,
                warnings,
                omissions,
            })
        }
        Input::Aff4 => {
            let paths = vec![fs::canonicalize(input)?];
            let snapshots = snapshot(&paths)?;
            let mut c = aff4::open(input)?;
            let disks = c.disk_images()?;
            let disk = match resource {
                Some(id) => disks
                    .iter()
                    .find(|d| d.resource_id == id)
                    .ok_or_else(|| invalid("selected resource is not a physical disk"))?,
                None if disks.len() == 1 => &disks[0],
                _ => {
                    return Err(invalid(
                        "select one physical disk using --resource ID; use info to list disks",
                    ));
                }
            }
            .clone();
            let mut metadata = EwfMetadata::default();
            let mut warnings = Vec::new();
            for (name, destination) in [
                ("caseNumber", &mut metadata.case_number),
                ("evidenceNumber", &mut metadata.evidence_number),
                ("examiner", &mut metadata.examiner),
                ("notes", &mut metadata.notes),
            ] {
                let mut values: Vec<_> = c
                    .metadata()
                    .values()
                    .filter(|properties| {
                        properties
                            .iter()
                            .any(|p| p.value == "http://aff4.org/Schema#CaseDetails")
                    })
                    .flat_map(|properties| properties.iter())
                    .filter(|p| p.predicate == format!("http://aff4.org/Schema#{name}"))
                    .map(|p| p.value.clone())
                    .collect();
                values.sort();
                values.dedup();
                if values.len() == 1 {
                    *destination = values.pop().filter(|s| !s.is_empty());
                } else if values.len() > 1 {
                    warnings.push(format!(
                        "Multiple AFF4 {name} values cannot be represented in one EWF header."
                    ));
                }
            }
            let all = c.verify_all(None, |_, a, b| ctx.progress("source verification", a, b))?;
            let bad = all.metadata_error.is_some()
                || all.resources.iter().any(|r| r.error.is_some())
                || all
                    .metadata
                    .iter()
                    .flat_map(|m| m.checks.iter())
                    .chain(all.checks.iter())
                    .any(|c| {
                        matches!(
                            c.outcome,
                            aff4_image::CheckOutcome::Mismatch
                                | aff4_image::CheckOutcome::Unreadable
                        )
                    });
            if bad {
                report["exit_code"] = json!(3);
                report["source_verification"] = json!(all);
                return Err(invalid("AFF4 source integrity checks failed"));
            }
            if !all.all_match() {
                warnings.push(
                    "Some source AFF4 integrity references are missing or unsupported.".into(),
                );
            }
            let v = c.verify(&disk.resource_id, |a, b| {
                ctx.progress("source verification", a, b)
            })?;
            if v.references_match == Some(false) {
                report["exit_code"] = json!(3);
                return Err(invalid("source disk reference hashes do not match"));
            }
            report["source_verification"] = json!(all);
            let mut omissions = Vec::new();
            if disks.len() > 1 {
                omissions.push("unselected AFF4 disk resources".into());
            }
            // Re-encoding maps to contiguous bytes does not preserve their graph,
            // allocation provenance, identifiers, or producer-specific properties.
            omissions.push("AFF4 resource graph and producer-specific metadata; decoded disk bytes and common case fields are retained".into());
            if disk.block_size.is_none() && supplied_sector.is_none() {
                warnings.push("Source has no sector geometry; using 512-byte sectors.".into());
            }
            Ok(Source {
                reader: Box::new(c.into_disk_reader(Some(&disk.resource_id))?),
                size: disk.logical_size,
                sector: sector(disk.block_size, supplied_sector)?,
                metadata,
                ewf_options: None,
                expected: Some(format::parse_hash(&v.sha256)?),
                snapshots,
                paths,
                warnings,
                omissions,
            })
        }
    }
}

struct Reading<'a> {
    input: &'a mut dyn Read,
    ctx: &'a mut Context,
    size: u64,
    done: u64,
    hash: Sha256,
}

impl Read for Reading<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.ctx
            .check("transfer", self.done, self.size)
            .map_err(|e| io::Error::other(e.to_string()))?;
        if self.done == self.size || bytes.is_empty() {
            return Ok(0);
        }
        let take = (self.size - self.done).min(bytes.len() as u64) as usize;
        let n = self.input.read(&mut bytes[..take])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source ended before its declared size",
            ));
        }
        self.hash.update(&bytes[..n]);
        self.done += n as u64;
        Ok(n)
    }
}

fn copy(
    input: &mut dyn Read,
    output: &mut impl Write,
    size: u64,
    ctx: &mut Context,
) -> Result<[u8; 32]> {
    let mut reading = Reading {
        input,
        ctx,
        size,
        done: 0,
        hash: Sha256::new(),
    };
    io::copy(&mut reading, output)?;
    Ok(reading.hash.finalize().into())
}

fn before_finish(source: &Source, digest: [u8; 32], ctx: &mut Context) -> Result<()> {
    source.check_unchanged()?;
    if source.expected.is_some_and(|expected| expected != digest) {
        return Err(invalid("source bytes changed after source verification"));
    }
    ctx.check("finalization", source.size, source.size)
}

pub(crate) fn convert(
    input: &Path,
    output: &Path,
    resource: Option<&str>,
    supplied_sector: Option<u32>,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    let target = Output::from_path(output)?;
    let logical = match format::detect(input)? {
        Input::Ewf => crate::password::open(input, ctx.password.as_ref())?
            .info()
            .single_files
            .is_some(),
        Input::Aff4 => aff4::open(input)?.version() != (1, 0),
        Input::Raw => false,
    };
    if logical {
        if resource.is_some() || supplied_sector.is_some() {
            return Err(invalid(
                "resource and sector selection apply to physical images",
            ));
        }
        return crate::logical::convert(input, output, target, ctx, report);
    }
    if target == Output::Lx01 {
        return Err(invalid(
            "physical images cannot be converted into logical collections",
        ));
    }
    report["input"] = json!(input);
    report["output"] = json!(output);
    let mut source = read_source(input, resource, supplied_sector, ctx, report)?;
    let output = crate::ewf::export::destination(&source.paths, output)?;
    transfer(&mut source, &output, target, ctx, report)
}

pub(crate) fn acquire(
    input: &Path,
    output: &Path,
    supplied_sector: Option<u32>,
    case: &CaseArgs,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    let target = Output::from_path(output)?;
    if !matches!(target, Output::Aff4 | Output::Raw) {
        return Err(invalid(
            "physical acquisition requires .E01, .Ex01, .aff4, or .raw",
        ));
    }
    let output = crate::ewf::export::destination(&[], output)?;
    let mut device = crate::ewf::source::Source::open(input, supplied_sector, &output)?;
    device.configure_reads(Arc::clone(&ctx.stop), None)?;
    let size = device.identity.size;
    let sector = device.identity.sector_size;
    report["source_identity"] = json!(device.identity);
    report["input"] = json!(input);
    report["output"] = json!(output);
    // Check the exact opened source again on its final read, before publication.
    struct Checked {
        source: crate::ewf::source::Source,
        remaining: u64,
    }
    impl Read for Checked {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Ok(0);
            }
            let n = self.source.read(bytes)?;
            self.remaining = self.remaining.saturating_sub(n as u64);
            if self.remaining == 0 {
                self.source
                    .check_unchanged()
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }
            Ok(n)
        }
    }
    let mut source = Source {
        reader: Box::new(Checked {
            source: device,
            remaining: size,
        }),
        size,
        sector,
        metadata: EwfMetadata {
            case_number: case.case_number.clone(),
            evidence_number: case.evidence_number.clone(),
            examiner: case.examiner.clone(),
            ..Default::default()
        },
        ewf_options: None,
        expected: None,
        snapshots: vec![],
        paths: vec![],
        warnings: vec![],
        omissions: vec![],
    };
    transfer(&mut source, &output, target, ctx, report)
}

fn transfer(
    source: &mut Source,
    output: &Path,
    target: Output,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    ctx.check("preflight", 0, source.size)?;
    if source.size == 0 || !source.size.is_multiple_of(u64::from(source.sector)) {
        return Err(invalid(
            "physical image size must be nonzero and sector aligned",
        ));
    }
    report["bytes"] = json!(source.size);
    report["sector_size"] = json!(source.sector);
    report["source_metadata"] = json!({"case_number":source.metadata.case_number,"evidence_number":source.metadata.evidence_number,"examiner":source.metadata.examiner,"notes":source.metadata.notes,"description":source.metadata.description});
    report["warnings"] = json!(source.warnings);
    report["format"] = json!(format!("{target:?}"));
    let mut losses = source.omissions.clone();
    if target == Output::Raw
        && report["source_metadata"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| !v.is_null())
    {
        losses.push("case metadata (raw has no metadata container)".into());
    }
    if matches!(target, Output::Aff4 | Output::Raw) && source.ewf_options.is_some() {
        losses.push("EWF format-specific headers, media flags, and auxiliary metadata; common case fields are retained only in AFF4".into());
    }
    if let Some(options) = &source.ewf_options {
        if matches!(target, Output::Aff4 | Output::Raw) && !options.acquisition_errors.is_empty() {
            losses.push("native acquisition-error ranges; retained in this report".into());
            report["source_acquisition_errors"] = json!(
                options
                    .acquisition_errors
                    .iter()
                    .map(|r| json!({"first_sector":r.first_sector,"sector_count":r.sector_count}))
                    .collect::<Vec<_>>()
            );
        }
        if matches!(target, Output::Aff4 | Output::Raw)
            && (!options.sessions.is_empty() || !options.tracks.is_empty())
        {
            losses.push("session/track ranges".into());
        }
    }
    report["metadata_not_preserved"] = json!(losses);
    let digest;
    match target {
        Output::E01 => {
            // Conversion has a known length and writes forward only. The
            // transactional sequential writer bounds scratch to one segment.
            let mut options = source.ewf_options.clone().unwrap_or_default();
            options.format = ewf_image::WriteFormat::Ewf1Physical;
            options.bytes_per_sector = source.sector;
            options.sectors_per_chunk = 32768 / source.sector;
            options.media_size = Some(source.size);
            options.compression = ewf_image::WriteCompression::Zlib;
            options.maximum_segment_size = Some(512 * 1024 * 1024);
            options.metadata = source.metadata.clone();
            let mut settings = SequentialOptions::new(source.size);
            settings.write = options;
            let mut writer = SequentialWriter::create(output, settings)?;
            struct Sink<'a>(&'a mut SequentialWriter);
            impl Write for Sink<'_> {
                fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                    self.0.write_all(b).map_err(io::Error::other)?;
                    Ok(b.len())
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            digest = copy(
                source.reader.as_mut(),
                &mut Sink(&mut writer),
                source.size,
                ctx,
            )?;
            before_finish(source, digest, ctx)?;
            report["published"] = Value::Null;
            report["recovery_command"] =
                json!(["ewf-cli", "recover-publication", output.to_string_lossy()]);
            let written = writer.finish()?;
            report["published"] = json!(true);
            report["recovery_command"] = Value::Null;
            report["segments"] = json!(written.segment_paths);
            verify_written_ewf(output, digest, ctx, report)?;
        }
        Output::Ex01 => {
            let mut options = SequentialOptions::new(source.size);
            options.write = source.ewf_options.clone().unwrap_or_default();
            options.write.format = ewf_image::WriteFormat::Ewf2Physical;
            options.write.bytes_per_sector = source.sector;
            options.write.sectors_per_chunk = 32768 / source.sector;
            options.write.metadata = source.metadata.clone();
            options.write.compression = ewf_image::WriteCompression::Zlib;
            let mut writer = SequentialWriter::create(output, options)?;
            struct Sink<'a>(&'a mut SequentialWriter);
            impl Write for Sink<'_> {
                fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                    self.0.write_all(b).map_err(io::Error::other)?;
                    Ok(b.len())
                }
                fn flush(&mut self) -> io::Result<()> {
                    Ok(())
                }
            }
            digest = copy(
                source.reader.as_mut(),
                &mut Sink(&mut writer),
                source.size,
                ctx,
            )?;
            before_finish(source, digest, ctx)?;
            report["published"] = Value::Null;
            report["recovery_command"] =
                json!(["ewf-cli", "recover-publication", output.to_string_lossy()]);
            let written = writer.finish()?;
            report["published"] = json!(true);
            report["segments"] = json!(written.segment_paths);
            report["recovery_command"] = Value::Null;
            verify_written_ewf(output, digest, ctx, report)?;
        }
        Output::Aff4 => {
            let mut writer = aff4_image::Writer::create(
                output,
                aff4_image::Profile::Physical,
                aff4_image::WriteOptions::default(),
            )?;
            writer.add_case_metadata(&aff4_image::CaseMetadata {
                case_number: source.metadata.case_number.clone().unwrap_or_default(),
                evidence_number: source.metadata.evidence_number.clone().unwrap_or_default(),
                examiner: source.metadata.examiner.clone().unwrap_or_default(),
                notes: source.metadata.notes.clone().unwrap_or_default(),
            })?;
            let mut reader = Reading {
                input: source.reader.as_mut(),
                ctx,
                size: source.size,
                done: 0,
                hash: Sha256::new(),
            };
            writer.add_image_with_sector_size(
                source.size,
                source.sector,
                &mut reader,
                |_, _| std::ops::ControlFlow::Continue(()),
            )?;
            digest = reader.hash.finalize().into();
            before_finish(source, digest, ctx)?;
            match writer.finish_verified(aff4_image::Limits::unrestricted(), |_, a, b| {
                ctx.progress("destination verification", a, b)
            }) {
                Ok((written, verified)) => {
                    report["published"] = json!(true);
                    if written.streams.len() != 1
                        || written.streams[0].sha256 != format::hex(&digest)
                    {
                        report["exit_code"] = json!(3);
                        return Err(invalid("destination digest differs from source"));
                    }
                    report["verification"] = json!(verified);
                }
                Err(aff4_image::Error::PublishedButUnsynced { result, source }) => {
                    report["published"] = json!(true);
                    report["output"] = json!(result.path);
                    return Err(source.into());
                }
                Err(aff4_image::Error::VerificationFailed {
                    report: verification,
                }) => {
                    report["verification"] = json!(verification);
                    report["exit_code"] = json!(3);
                    return Err(invalid("destination verification failed"));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Output::Raw => {
            let parent = output
                .parent()
                .ok_or_else(|| invalid("missing output parent"))?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            digest = copy(source.reader.as_mut(), &mut file, source.size, ctx)?;
            before_finish(source, digest, ctx)?;
            file.as_file().sync_all()?;
            let verified = hash_reader(&mut File::open(file.path())?, source.size, ctx)?;
            if verified != digest {
                report["exit_code"] = json!(3);
                return Err(invalid("destination readback digest differs from source"));
            }
            ctx.check("publication", source.size, source.size)?;
            file.persist_noclobber(output)?;
            report["published"] = json!(true);
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            report["verification"] = json!({"scope":"decoded media","bytes_verified":source.size,"sha256":format::hex(&verified),"references_match":true});
        }
        Output::Lx01 => return Err(invalid("physical transfer cannot create a logical image")),
    }
    report["sha256"] = json!(format::hex(&digest));
    report["destination_matches_source"] = json!(true);
    let substitutions = source
        .ewf_options
        .as_ref()
        .is_some_and(|o| !o.acquisition_errors.is_empty());
    report["status"] = json!(if substitutions {
        "complete_with_source_acquisition_errors"
    } else if losses.is_empty() {
        "complete"
    } else {
        "complete_with_metadata_omissions"
    });
    report["exit_code"] = json!(if losses.is_empty() && !substitutions {
        0
    } else {
        4
    });
    Ok(())
}

fn verify_written_ewf(
    path: &Path,
    expected: [u8; 32],
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    let result = crate::password::open(path, ctx.password.as_ref())?.verify_with_progress(
        &VerifyOptions::default().with_expected_sha256(expected),
        |p| ctx.progress("destination verification", p.bytes_verified, p.bytes_total),
    )?;
    report["verification"] = json!({"scope":"decoded media","bytes_verified":result.bytes_verified,"sha256":format::hex(&result.hashes.sha256),"references_match":result.references_match()});
    if result.references_match() != Some(true) {
        report["exit_code"] = json!(3);
        return Err(invalid("destination readback verification failed"));
    }
    Ok(())
}

fn hash_reader(reader: &mut dyn Read, size: u64, ctx: &mut Context) -> Result<[u8; 32]> {
    copy(reader, &mut io::sink(), size, ctx)
}

pub(crate) fn verify_raw(
    path: &Path,
    expected: Option<&str>,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    report["image"] = json!(path);
    let paths = [path.to_owned()];
    let initial = snapshot(&paths)?;
    let size = fs::metadata(path)?.len();
    let digest = hash_reader(&mut File::open(path)?, size, ctx)?;
    if snapshot(&paths)? != initial {
        return Err(invalid("raw source changed during verification"));
    }
    let matched = expected.map(|e| e.eq_ignore_ascii_case(&format::hex(&digest)));
    report["verification"] = json!({"scope":"raw bytes","bytes_verified":size,"sha256":format::hex(&digest),"references_match":matched});
    report["status"] = json!(if matched == Some(true) {
        "verified"
    } else if matched == Some(false) {
        "verification_failed"
    } else {
        "computed_without_reference"
    });
    report["exit_code"] = json!(match matched {
        Some(true) => 0,
        Some(false) => 3,
        None => 4,
    });
    Ok(())
}

pub(crate) fn verify_ewf(
    path: &Path,
    entry: Option<&str>,
    expected: Option<&str>,
    ctx: &mut Context,
    report: &mut Value,
) -> Result<()> {
    report["image"] = json!(path);
    let image = crate::password::open(path, ctx.password.as_ref())?;
    if !image.info().acquisition_complete {
        return Err(invalid("image acquisition is incomplete"));
    }
    if let Some(index) = entry {
        let index = index
            .parse::<usize>()
            .map_err(|_| invalid("EWF entry must be a catalog index from files"))?;
        let root = image
            .root_file_entry()
            .ok_or_else(|| invalid("image has no logical catalog"))?;
        let mut pending = vec![root];
        let mut selected = None;
        for current in 0..=index {
            let Some(entry) = pending.pop() else { break };
            if current == index {
                selected = Some(entry);
                break;
            }
            pending.extend(entry.children.iter().rev());
        }
        let e = selected.ok_or_else(|| invalid("catalog index does not exist"))?;
        let v = image.verify_single_file_with_progress(e, |p| {
            ctx.progress("file verification", p.bytes_processed, p.bytes_total)
        })?;
        let external = v.hashes.sha256 == format::parse_hash(expected.unwrap())?;
        let matched = external && v.references_match() != Some(false);
        report["verification"] = json!({"scope":"selected file","bytes_verified":v.bytes_verified,"sha256":format::hex(&v.hashes.sha256),"references_match":v.references_match()});
        report["external_match"] = json!(external);
        report["status"] = json!(if matched {
            "verified"
        } else {
            "verification_failed"
        });
        report["exit_code"] = json!(if matched { 0 } else { 3 });
        return Ok(());
    }
    verify_written_ewf(path, format::parse_hash(expected.unwrap())?, ctx, report)?;
    let substitutions = !image.info().acquisition_errors.is_empty();
    report["status"] = json!(if substitutions {
        "verified_with_substitutions"
    } else {
        "verified"
    });
    report["exit_code"] = json!(if substitutions { 4 } else { 0 });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    #[test]
    fn cancellation_discards_unpublished_conversion_staging() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("source.raw");
        fs::write(&raw, [0x51; 65536]).unwrap();
        for extension in ["E01", "Ex01", "aff4", "raw"] {
            let output = dir.path().join(format!("cancelled.{extension}"));
            let mut ctx = Context {
                stop: Arc::new(AtomicBool::new(true)),
                quiet: true,
                last: None,
                password: None,
            };
            let mut report = json!({"published":false});
            assert!(convert(&raw, &output, None, None, &mut ctx, &mut report).is_err());
            assert!(!output.exists());
            assert_eq!(report["published"], false);
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
