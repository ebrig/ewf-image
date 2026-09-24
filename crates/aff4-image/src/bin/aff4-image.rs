//! AFF4 inventory, verification, local collection, and selective extraction.
#[path = "aff4-image/output.rs"]
mod output;
use aff4_image::{
    CaseMetadata, CollectionLimits, CollectionOptions, Container, Limits, Profile, VolumeSet,
    WriteOptions, Writer,
};
use clap::{Parser, Subcommand};
use output::{Output, clean};
use serde_json::json;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(serde::Serialize)]
struct CollectionOutput<'a> {
    published: bool,
    output: &'a aff4_image::WriteResult,
    collection: &'a aff4_image::CollectionReport,
    verification: &'a aff4_image::ContainerVerification,
    limits: &'a Limits,
    collection_limits: &'a CollectionLimits,
    verification_scope: &'static str,
}

#[derive(serde::Serialize)]
struct CollectionVerificationFailure<'a> {
    published: bool,
    phase: &'static str,
    collection: &'a aff4_image::CollectionReport,
    limits: &'a Limits,
    collection_limits: &'a CollectionLimits,
    verification_error: String,
    verification: Option<&'a aff4_image::ContainerVerification>,
    verification_scope: &'static str,
}

fn progress(_: u64, _: u64) -> ControlFlow<()> {
    if CANCELLED.load(Ordering::Relaxed) {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

#[derive(Parser)]
#[command(
    version,
    about = "Read, collect, and verify AFF4 evidence",
    disable_help_subcommand = true,
    after_help = "Use aff4-image <command> --help for details."
)]
struct Args {
    /// Print machine-readable results.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List evidence resources.
    Info {
        /// Container path.
        image: PathBuf,
    },
    /// Show metadata records.
    Metadata {
        /// Container path.
        image: PathBuf,
    },
    /// Verify container metadata and content.
    Verify {
        /// Container path.
        image: PathBuf,
        /// Compare an independently recorded metadata digest.
        #[arg(
            long = "metadata-sha256",
            alias = "expected-metadata-sha256",
            value_name = "HASH"
        )]
        expected_metadata_sha256: Option<String>,
    },
    /// Verify an image across supplied volumes.
    VerifySet {
        /// Container paths, primary volume first.
        #[arg(required = true, value_name = "VOLUME")]
        volumes: Vec<PathBuf>,
        /// Image resource ID from info.
        #[arg(long, value_name = "ID")]
        image: String,
        /// Compare an independently recorded image digest.
        #[arg(long = "sha256", alias = "expected-image-sha256", value_name = "HASH")]
        expected_image_sha256: Option<String>,
        /// Also check each volume and its integrity references.
        #[arg(long)]
        full: bool,
    },
    /// Collect a directory into a verified container.
    Collect {
        /// Directory to collect; use a stable snapshot.
        source: PathBuf,
        /// New .aff4 container path.
        output: PathBuf,
        /// Skip a relative path; repeat for multiple exclusions.
        #[arg(long, value_name = "PATH")]
        exclude: Vec<PathBuf>,
        /// Record and skip inaccessible or unsupported entries.
        #[arg(long)]
        allow_partial: bool,
        /// Case identifier.
        #[arg(
            long,
            default_value = "",
            hide_default_value = true,
            value_name = "ID",
            help_heading = "Case details"
        )]
        case_number: String,
        /// Evidence identifier.
        #[arg(
            long,
            default_value = "",
            hide_default_value = true,
            value_name = "ID",
            help_heading = "Case details"
        )]
        evidence_number: String,
        /// Examiner name.
        #[arg(
            long,
            default_value = "",
            hide_default_value = true,
            value_name = "NAME",
            help_heading = "Case details"
        )]
        examiner: String,
        /// Evidence notes.
        #[arg(
            long,
            default_value = "",
            hide_default_value = true,
            value_name = "TEXT",
            help_heading = "Case details"
        )]
        notes: String,
    },
    /// Extract and check one evidence resource.
    Extract {
        /// Container path.
        image: PathBuf,
        /// Resource ID from info.
        resource: String,
        /// New destination file.
        output: PathBuf,
    },
}

fn run(args: Args) -> Result<i32, Box<dyn std::error::Error>> {
    let limits = Limits::unrestricted();
    let display = Output::new(args.json);
    match args.command {
        Command::Info { image } => {
            let container = Container::open_with_limits(image, limits)?;
            let streams = container.streams()?;
            display.emit(&json!({"version": container.version(), "volume": container.volume_id(), "resources": streams}), || {
                let (major, minor) = container.version();
                let mut text = format!("AFF4 {major}.{minor}\nVolume: {}\nResources: {}\n", clean(container.volume_id()), streams.len());
                for stream in &streams {
                    text.push_str(&format!("{}  {}\n", clean(&stream.id), stream.size.map_or_else(|| "size unknown".into(), |size| format!("{size} bytes"))));
                }
                text
            })?;
        }
        Command::Metadata { image } => {
            let mut output_error = None;
            let result = Container::scan_metadata(image, limits, |member, subject, property| {
                if let Err(error) = display.emit(
                    &json!({"member":member,"subject":subject,"property":property}),
                    || {
                        format!(
                            "{}  {}  {}",
                            clean(subject),
                            clean(&property.predicate),
                            clean(&property.value)
                        )
                    },
                ) {
                    output_error = Some(error);
                    return ControlFlow::Break(());
                }
                progress(0, 0)
            });
            if let Some(error) = output_error {
                return Err(error.into());
            }
            let result = result?;
            display.emit(&json!({"summary":result}), || {
                format!(
                    "{} records in {} metadata stores",
                    result.triples, result.stores
                )
            })?;
        }
        Command::Verify {
            image,
            expected_metadata_sha256,
        } => {
            let result = Container::open_with_limits(image, limits)?
                .verify_all(expected_metadata_sha256.as_deref(), |_, done, total| {
                    progress(done, total)
                })?;
            display.emit(&result, || output::container(&result))?;
            if !result.all_match() {
                return Ok(4);
            }
        }
        Command::VerifySet {
            volumes,
            image,
            expected_image_sha256,
            full,
        } => {
            if full {
                let result = VolumeSet::open_with_limits(&volumes, limits)?.verify_full(
                    &image,
                    expected_image_sha256.as_deref(),
                    |_, done, total| progress(done, total),
                )?;
                let matched = result.all_match();
                let external_mismatch = result
                    .assembled
                    .as_ref()
                    .is_some_and(|v| v.external_match == Some(false));
                display.emit(&json!({"scope":"selected image and supplied-volume integrity", "result":result}), || output::set(&result))?;
                return Ok(if external_mismatch {
                    3
                } else if matched {
                    0
                } else {
                    4
                });
            }
            let result = VolumeSet::open_with_limits(&volumes, limits)?.verify_image(
                &image,
                expected_image_sha256.as_deref(),
                progress,
            )?;
            display.emit(&json!({"scope":"assembled image bytes only", "result":result}), || {
                format!("Scope: assembled image bytes\nBytes checked: {}\nExternal SHA256: {}\nSHA256: {}", result.bytes,
                    match result.external_match { Some(true) => "match", Some(false) => "mismatch", None => "not supplied; verification incomplete" }, result.sha256)
            })?;
            match result.external_match {
                Some(true) => {}
                Some(false) => return Ok(3),
                None => return Ok(4),
            }
        }
        Command::Collect {
            source,
            output,
            exclude,
            allow_partial,
            case_number,
            evidence_number,
            examiner,
            notes,
        } => {
            let mut writer = Writer::create(output, Profile::Logical, WriteOptions::default())?;
            writer.add_case_metadata(&CaseMetadata {
                case_number,
                evidence_number,
                examiner,
                notes,
            })?;
            let collection_limits = CollectionLimits {
                reader: limits.clone(),
                entries: usize::MAX,
                depth: usize::MAX,
            };
            let collection = writer.add_directory_tree_with_limits(
                source,
                &CollectionOptions {
                    exclude,
                    allow_partial,
                },
                &collection_limits,
                |_, done, total| progress(done, total),
            );
            let collection = match collection {
                Ok(value) => value,
                Err(error) => {
                    let limit_error = match &error {
                        aff4_image::Error::ResourceLimit {
                            resource,
                            required,
                            limit,
                        } => Some(json!({"resource":resource,"required":required,"limit":limit})),
                        _ => None,
                    };
                    display.emit(&json!({"published":false,"phase":"collection","error":error.to_string(),"limit_error":limit_error,"limits":limits,"collection_limits":collection_limits}), || {
                        format!("Collection failed: {}\nPublished: no", clean(&error.to_string()))
                    })?;
                    return Ok(match error {
                        aff4_image::Error::Aborted => 130,
                        aff4_image::Error::ResourceLimit { .. } => 4,
                        _ => 1,
                    });
                }
            };
            if progress(0, 0).is_break() {
                return Err(aff4_image::Error::Aborted.into());
            }
            let (written, verification) = match writer
                .finish_verified(limits.clone(), |_, done, total| progress(done, total))
            {
                Ok(value) => value,
                Err(aff4_image::Error::PublishedButUnsynced { result, source }) => {
                    display.emit(&json!({"published":true,"output":result,"collection":collection,
                        "limits":limits,"collection_limits":collection_limits,"durability_error":source.to_string(),
                        "verification_scope":"finalized staged container","verification_passed":true}), || {
                            format!("Published and verified: {}\nDirectory synchronization failed: {}", clean(&result.path.display().to_string()), clean(&source.to_string()))
                        })?;
                    return Ok(4);
                }
                Err(error) => {
                    let verification = match &error {
                        aff4_image::Error::VerificationFailed { report } => Some(report.as_ref()),
                        _ => None,
                    };
                    display.emit(
                        &CollectionVerificationFailure {
                            published: false,
                            phase: "finalize and verify before publication",
                            collection: &collection,
                            limits: &limits,
                            collection_limits: &collection_limits,
                            verification_error: error.to_string(),
                            verification,
                            verification_scope: "finalized staged container",
                        },
                        || {
                            let mut text = format!(
                                "Collection failed: {}\nPublished: no",
                                clean(&error.to_string())
                            );
                            if let Some(report) = verification {
                                text.push_str(&format!("\n{}", output::container(report)));
                            }
                            text
                        },
                    )?;
                    return Ok(if matches!(error, aff4_image::Error::Aborted) {
                        130
                    } else {
                        4
                    });
                }
            };
            let complete = verification.all_match() && collection.issues.is_empty();
            // Serialize borrowed reports directly; a second JSON tree duplicates
            // hundreds of thousands of digest records in large collections.
            display.emit(
                &CollectionOutput {
                    published: true,
                    output: &written,
                    collection: &collection,
                    verification: &verification,
                    limits: &limits,
                    collection_limits: &collection_limits,
                    verification_scope: "finalized staged container",
                },
                || {
                    let mut text = format!(
                        "Collected: {}\nFiles: {}\nBytes: {}\nPublished: yes\n{}",
                        clean(&written.path.display().to_string()),
                        collection.files,
                        collection.bytes,
                        output::container(&verification)
                    );
                    if !collection.issues.is_empty() {
                        text.push_str(&format!("\nOmitted entries: {}", collection.issues.len()));
                        for issue in collection.issues.iter().take(5) {
                            text.push_str(&format!(
                                "\n  {}: {}",
                                clean(&issue.path.display().to_string()),
                                clean(&issue.reason)
                            ));
                        }
                        if collection.issues.len() > 5 {
                            text.push_str("\nUse --json for all omissions.");
                        }
                    }
                    text
                },
            )?;
            if !complete {
                return Ok(4);
            }
        }
        Command::Extract {
            image,
            resource,
            output,
        } => {
            let mut container = Container::open_with_limits(image, limits)?;
            let filename = output.file_name().ok_or("missing output name")?;
            #[cfg(windows)]
            if filename.to_string_lossy().contains(':') {
                return Err("alternate-stream output is unsupported".into());
            }
            let parent = std::fs::canonicalize(
                output
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new(".")),
            )?;
            let output = parent.join(filename);
            if std::fs::symlink_metadata(&output).is_ok() {
                return Err("output already exists".into());
            }
            let mut staged = tempfile::Builder::new()
                .prefix(".aff4-extract-")
                .tempfile_in(&parent)?;
            let verification = container.copy_verified(&resource, &mut staged, progress)?;
            if verification.references_match == Some(false) {
                display.emit(
                    &json!({"published":false,"verification":verification}),
                    || {
                        format!(
                            "Extraction failed\nPublished: no\n{}",
                            output::linear(&verification)
                        )
                    },
                )?;
                return Ok(3);
            }
            staged.as_file().sync_all()?;
            if progress(0, 0).is_break() {
                return Err(aff4_image::Error::Aborted.into());
            }
            staged.persist_noclobber(&output)?;
            #[cfg(unix)]
            std::fs::File::open(&parent)?.sync_all()?;
            let complete = verification.references_match == Some(true)
                && verification.unsupported_hashes.is_empty();
            display.emit(&json!({"published":true,"output":output,"scope":"resource linear bytes","verification":verification}), || {
                format!("Extracted: {}\nScope: selected resource\nPublished: yes\n{}", clean(&output.display().to_string()), output::linear(&verification))
            })?;
            if !complete {
                return Ok(4);
            }
        }
    }
    Ok(0)
}

fn main() {
    let args = Args::parse();
    let json = args.json;
    if let Err(error) = ctrlc::set_handler(|| CANCELLED.store(true, Ordering::Relaxed)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
    match run(args) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            if json {
                eprintln!("{}", json!({"error":error.to_string()}));
            } else {
                eprintln!("Error: {}", clean(&error.to_string()));
            }
            std::process::exit(if CANCELLED.load(Ordering::Relaxed) {
                130
            } else {
                1
            });
        }
    }
}
