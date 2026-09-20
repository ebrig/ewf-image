//! AFF4 inventory, verification, local collection, and selective extraction.
use aff4_image::{
    CaseMetadata, CollectionOptions, Container, Limits, Profile, VolumeSet, WriteOptions, Writer,
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
    #[command(subcommand)]
    command: Command,
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
    },
    /// Acquire a local directory. Use a stable snapshot for cross-file consistency.
    Collect {
        source: PathBuf,
        output: PathBuf,
        #[arg(long)]
        exclude: Vec<PathBuf>,
        #[arg(long)]
        allow_partial: bool,
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
    match args.command {
        Command::Info { image } => {
            let container = Container::open(image)?;
            println!(
                "{}",
                json!({"version": container.version(), "volume": container.volume_id(), "resources": container.streams()?})
            );
        }
        Command::Metadata { image } => {
            let result =
                Container::scan_metadata(image, Limits::default(), |member, subject, property| {
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
            let result = Container::open(image)?
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
        } => {
            let result = VolumeSet::open(&volumes)?.verify_image(
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
            let collection = writer.add_directory_tree(
                source,
                &CollectionOptions {
                    exclude,
                    allow_partial,
                },
                |_, done, total| progress(done, total),
            )?;
            if progress(0, 0).is_break() {
                return Err(aff4_image::Error::Aborted.into());
            }
            let written = writer.finish()?;
            let verification = Container::open(&written.path).and_then(|mut container| {
                container.verify_all(Some(&written.metadata_sha256), |_, done, total| {
                    progress(done, total)
                })
            });
            let verification = match verification {
                Ok(value) => value,
                Err(error) => {
                    println!(
                        "{}",
                        json!({"published":true,"output":written,"collection":collection,"verification_error":error.to_string()})
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
                json!({"published":true,"output":written,"collection":collection,"verification":verification})
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
            let mut container = Container::open(image)?;
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
