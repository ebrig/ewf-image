//! AFF4 inventory, verification, local collection, and selective extraction.
use aff4_image::{
    CaseMetadata, CollectionLimits, CollectionOptions, Container, Limits, Profile, VolumeSet,
    WriteOptions, Writer,
};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);
fn progress(_: u64, _: u64) -> ControlFlow<()> {
    if CANCELLED.load(Ordering::Relaxed) {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

#[derive(Parser)]
#[command(about = "Inspect, verify, collect, and extract AFF4 evidence; JSON output")]
struct Args {
    #[command(flatten)]
    limits: LimitArgs,
    #[command(subcommand)]
    command: Command,
}
/// Explicit resource budgets shared by all read and collect operations.
#[derive(clap::Args)]
struct LimitArgs {
    #[arg(
        long = "limit-directory-bytes",
        global = true,
        help = "Override directory_bytes reader budget"
    )]
    directory_bytes: Option<u64>,
    #[arg(
        long = "limit-archive-entries",
        global = true,
        help = "Override archive_entries reader budget"
    )]
    archive_entries: Option<usize>,
    #[arg(
        long = "limit-metadata-bytes",
        global = true,
        help = "Override metadata_bytes reader budget"
    )]
    metadata_bytes: Option<u64>,
    #[arg(
        long = "limit-member-bytes",
        global = true,
        help = "Override member_bytes reader budget"
    )]
    member_bytes: Option<u64>,
    #[arg(
        long = "limit-chunk-bytes",
        global = true,
        help = "Override chunk_bytes reader budget"
    )]
    chunk_bytes: Option<u64>,
    #[arg(
        long = "limit-triples",
        global = true,
        help = "Override triples reader budget"
    )]
    triples: Option<usize>,
    #[arg(
        long = "limit-map-bytes",
        global = true,
        help = "Override map_bytes reader budget"
    )]
    map_bytes: Option<usize>,
    #[arg(
        long = "limit-verification-bytes",
        global = true,
        help = "Override verification_bytes reader budget"
    )]
    verification_bytes: Option<u64>,
}
impl LimitArgs {
    fn resolve(self) -> Limits {
        let defaults = Limits::default();
        Limits {
            directory_bytes: self.directory_bytes.unwrap_or(defaults.directory_bytes),
            archive_entries: self.archive_entries.unwrap_or(defaults.archive_entries),
            metadata_bytes: self.metadata_bytes.unwrap_or(defaults.metadata_bytes),
            member_bytes: self.member_bytes.unwrap_or(defaults.member_bytes),
            chunk_bytes: self.chunk_bytes.unwrap_or(defaults.chunk_bytes),
            triples: self.triples.unwrap_or(defaults.triples),
            map_bytes: self.map_bytes.unwrap_or(defaults.map_bytes),
            verification_bytes: self
                .verification_bytes
                .unwrap_or(defaults.verification_bytes),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// List explicitly selectable resources.
    Info { image: PathBuf },
    /// Stream metadata as JSON lines without retaining the RDF graph.
    Metadata { image: PathBuf },
    /// Verify all resources, recorded integrity structures, and metadata hashes.
    Verify {
        image: PathBuf,
        #[arg(long)]
        expected_metadata_sha256: Option<String>,
    },
    /// Hash a physical image across explicitly supplied volumes (primary first).
    VerifySet {
        #[arg(required = true)]
        volumes: Vec<PathBuf>,
        #[arg(long)]
        image: String,
        #[arg(long)]
        expected_image_sha256: Option<String>,
        /// Also check per-volume metadata, owned streams, maps and striped roots.
        #[arg(long)]
        full: bool,
    },
    /// Acquire a local directory. Use a stable snapshot for cross-file consistency.
    Collect {
        source: PathBuf,
        output: PathBuf,
        #[arg(long)]
        exclude: Vec<PathBuf>,
        #[arg(long)]
        allow_partial: bool,
        /// Bound discovered entries, including root, excluded and skipped entries.
        #[arg(long, default_value_t = 100_000)]
        limit_collection_entries: usize,
        /// Bound directory depth; root has depth zero.
        #[arg(long, default_value_t = 127)]
        limit_collection_depth: usize,
        #[arg(long, default_value = "")]
        case_number: String,
        #[arg(long, default_value = "")]
        evidence_number: String,
        #[arg(long, default_value = "")]
        examiner: String,
        #[arg(long, default_value = "")]
        notes: String,
    },
    /// Extract one resource to a caller-chosen new file, checking linear hashes.
    Extract {
        image: PathBuf,
        resource: String,
        output: PathBuf,
    },
}

fn run(args: Args) -> Result<i32, Box<dyn std::error::Error>> {
    let limits = args.limits.resolve();
    match args.command {
        Command::Info { image } => {
            let container = Container::open_with_limits(image, limits)?;
            println!(
                "{}",
                json!({"version": container.version(), "volume": container.volume_id(), "resources": container.streams()?})
            );
        }
        Command::Metadata { image } => {
            let result = Container::scan_metadata(image, limits, |member, subject, property| {
                println!(
                    "{}",
                    json!({"member":member,"subject":subject,"property":property})
                );
                progress(0, 0)
            })?;
            println!("{}", json!({"summary":result}));
        }
        Command::Verify {
            image,
            expected_metadata_sha256,
        } => {
            let result = Container::open_with_limits(image, limits)?
                .verify_all(expected_metadata_sha256.as_deref(), |_, done, total| {
                    progress(done, total)
                })?;
            println!("{}", serde_json::to_string(&result)?);
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
                println!(
                    "{}",
                    json!({"scope":"selected image and supplied-volume integrity", "result":result})
                );
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
            println!(
                "{}",
                json!({"scope":"assembled image bytes only", "result":result})
            );
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
            limit_collection_entries,
            limit_collection_depth,
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
                entries: limit_collection_entries,
                depth: limit_collection_depth,
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
                    println!(
                        "{}",
                        json!({"published":false,"phase":"collection","error":error.to_string(),"limit_error":limit_error,"limits":limits,"collection_limits":collection_limits})
                    );
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
                    println!(
                        "{}",
                        json!({"published":true,"output":result,"collection":collection,
                        "limits":limits,"collection_limits":collection_limits,"durability_error":source.to_string(),
                        "verification_scope":"finalized staged container","verification_passed":true})
                    );
                    return Ok(4);
                }
                Err(error) => {
                    println!(
                        "{}",
                        json!({"published":false,"collection":collection,"limits":limits,"collection_limits":collection_limits,
                        "phase":"finalize and verify before publication","verification_error":error.to_string()})
                    );
                    return Ok(if matches!(error, aff4_image::Error::Aborted) {
                        130
                    } else {
                        4
                    });
                }
            };
            let complete = verification.all_match() && collection.issues.is_empty();
            println!(
                "{}",
                json!({"published":true,"output":written,"collection":collection,"verification":verification,"limits":limits,"collection_limits":collection_limits,
                "verification_scope":"finalized staged container"})
            );
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
                println!("{}", json!({"published":false,"verification":verification}));
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
            println!(
                "{}",
                json!({"published":true,"output":output,"scope":"resource linear bytes","verification":verification})
            );
            if !complete {
                return Ok(4);
            }
        }
    }
    Ok(0)
}

fn main() {
    let args = Args::parse();
    if let Err(error) = ctrlc::set_handler(|| CANCELLED.store(true, Ordering::Relaxed)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
    match run(args) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{}", json!({"error":error.to_string()}));
            std::process::exit(if CANCELLED.load(Ordering::Relaxed) {
                130
            } else {
                1
            });
        }
    }
}
