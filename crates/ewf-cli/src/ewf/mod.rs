//! Command-line acquisition, integrity analysis, recovery, and raw export.

mod analyze;
pub(crate) mod export;
mod history;
mod inspect;
mod logical;
mod recover;
mod sequential;
mod session;
pub(crate) mod source;

use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand};
use ewf_image::{
    AcquisitionOptions, AcquisitionReadOptions, AcquisitionStatus, AcquisitionWriter, EwfError,
    Image, UnreadableSectorPolicy, VerifyOptions,
};
use serde_json::{Value, json};
use session::Session;
use source::Source;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(
    version,
    about = "Read, acquire, and verify EWF evidence",
    disable_help_subcommand = true,
    after_help = "Use ewf-cli ewf <command> --help for details."
)]
struct Cli {
    /// Hide progress.
    #[arg(short, long, global = true)]
    quiet: bool,
    /// Print machine-readable results.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Show image information.
    Info {
        /// First image segment.
        image: PathBuf,
    },
    /// Verify an image or one logical file.
    Verify {
        /// First image segment.
        image: PathBuf,
        /// Entry number from files; omit to verify the whole image.
        entry: Option<usize>,
    },
    /// List logical files and their entry numbers.
    Files {
        /// First logical image segment.
        image: PathBuf,
        /// Start at this entry number.
        #[arg(long, default_value_t = 0, value_name = "ENTRY")]
        offset: usize,
        /// Entries per page.
        #[arg(long, default_value_t = 1000, value_name = "COUNT", value_parser = clap::value_parser!(u32).range(1..=100000))]
        limit: u32,
    },
    /// Verify one logical file (compatibility command).
    #[command(hide = true)]
    VerifyFile { image: PathBuf, entry: usize },
    /// Extract and check one logical file.
    #[command(name = "extract", alias = "extract-file")]
    ExtractFile {
        /// First logical image segment.
        image: PathBuf,
        /// Entry number from files.
        entry: usize,
        /// New destination file.
        output: PathBuf,
    },
    /// Scan for damage and report integrity findings.
    Analyze {
        /// First image segment.
        image: PathBuf,
        /// Maximum retained findings; all findings are still counted.
        #[arg(long, default_value_t = 1024, value_name = "COUNT", value_parser = clap::value_parser!(u32).range(0..=100000))]
        maximum_findings: u32,
    },
    /// Export decoded media to a raw file.
    Export {
        /// First image segment.
        image: PathBuf,
        /// New raw output file.
        output: PathBuf,
    },
    /// Recover damaged EWF1 media with a provenance map.
    Recover {
        /// First damaged EWF1 segment.
        image: PathBuf,
        /// New directory for recovered data and provenance.
        output: PathBuf,
        /// Reject declared media larger than this limit before creating output.
        #[arg(long, value_name = "BYTES", value_parser = clap::value_parser!(u64).range(1..))]
        maximum_output_bytes: Option<u64>,
        /// Retain decodable checksum-suspect bytes when no validated alternate exists.
        #[arg(long)]
        preserve_checksum_suspect: bool,
    },
    /// Acquire and verify a resumable E01 image.
    Acquire(Acquire),
    /// Acquire and verify an Ex01 image (no resume).
    AcquireSequential(sequential::AcquireArgs),
    /// Collect a directory into a verified Lx01 image.
    Collect(sequential::CollectArgs),
    /// Resolve an interrupted Ex01/Lx01 publication.
    RecoverPublication {
        /// First output segment (.Ex01 or .Lx01).
        output: PathBuf,
    },
    /// Resume an interrupted E01 acquisition.
    Resume {
        /// Original .E01 output path.
        output: PathBuf,
        #[command(flatten)]
        read: ReadArgs,
    },
    /// Inspect or validate an E01 checkpoint.
    Checkpoint {
        #[command(subcommand)]
        command: CheckpointCommand,
    },
    /// Show saved acquisition history.
    Report {
        /// Original .E01 output path.
        output: PathBuf,
        /// Update the saved report.
        #[arg(long)]
        write: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum CheckpointCommand {
    /// Show checkpoint information.
    Inspect {
        /// Original .E01 output path.
        output: PathBuf,
    },
    /// Check saved segment hashes.
    Validate {
        /// Original .E01 output path.
        output: PathBuf,
    },
}

#[derive(Args)]
pub(crate) struct Acquire {
    /// Source file or device.
    source: PathBuf,
    /// New .E01 output path.
    output: PathBuf,
    /// Logical sector size for a regular file.
    #[arg(long, value_name = "BYTES", help_heading = "Image settings", value_parser = clap::value_parser!(u32).range(512..=4096))]
    sector_size: Option<u32>,
    /// Sectors in each encoded chunk.
    #[arg(long, default_value_t = 64, value_name = "COUNT", help_heading = "Image settings", value_parser = clap::value_parser!(u32).range(1..=32768))]
    sectors_per_chunk: u32,
    /// Chunks in each segment.
    #[arg(long, default_value_t = 16375, value_name = "COUNT", help_heading = "Image settings", value_parser = clap::value_parser!(u32).range(1..=16375))]
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
    #[command(flatten)]
    read: ReadArgs,
}

#[derive(Args)]
pub(crate) struct ReadArgs {
    /// Per-read timeout; omitted means no deadline.
    #[arg(long, value_name = "MS", help_heading = "Read handling", value_parser = clap::value_parser!(u64).range(1..))]
    read_timeout_ms: Option<u64>,
    /// Additional attempts per failed sector.
    #[arg(long, default_value_t = 2, value_name = "COUNT", help_heading = "Read handling", value_parser = clap::value_parser!(u32).range(0..=100))]
    retries: u32,
    /// Record unreadable sectors and replace them with zeros.
    #[arg(long, help_heading = "Read handling")]
    zero_fill: bool,
    /// Checkpoint interval in bytes; must be a multiple of the chunk size.
    #[arg(long, value_name = "BYTES", help_heading = "Checkpoints", value_parser = clap::value_parser!(u64).range(1..))]
    checkpoint_interval: Option<u64>,
    /// Pause after this many accepted bytes (rounded to a chunk).
    #[arg(long, value_name = "BYTES", help_heading = "Checkpoints", value_parser = clap::value_parser!(u64).range(1..))]
    stop_after: Option<u64>,
}

/// Run an advanced EWF command from the single executable.
pub(crate) fn dispatch_command(
    command: &Command,
    quiet: bool,
    stop: &Arc<AtomicBool>,
) -> (Value, u8) {
    execute_command(command, quiet, stop)
}

// Internal adapter for the format-neutral commands in the same executable.
pub(crate) fn dispatch(
    arguments: &[std::ffi::OsString],
    stop: &Arc<AtomicBool>,
) -> Result<(Value, u8)> {
    let cli = Cli::try_parse_from(arguments)?;
    Ok(execute_command(&cli.command, cli.quiet, stop))
}

fn execute_command(command: &Command, quiet: bool, stop: &Arc<AtomicBool>) -> (Value, u8) {
    let started = Instant::now();
    let mut report = json!({"schema_version": 1, "tool": "ewf-cli", "tool_version": env!("CARGO_PKG_VERSION"),
        "status": "failed", "phase": "preflight", "published": false,
        "verification": null, "checkpoint_bytes": 0, "accepted_bytes": 0});
    let mut history = None;
    let mut recovery = None;
    let result = run(
        command,
        quiet,
        stop,
        &mut report,
        &mut history,
        &mut recovery,
    );
    let mut code = if let Err(error) = result {
        let aborted = error
            .downcast_ref::<EwfError>()
            .is_some_and(|e| matches!(e, EwfError::Aborted))
            && history.as_ref().is_none_or(|h| h.failure.is_none());
        let verification = report["phase"] == "verification";
        report["status"] = json!(if aborted {
            "cancelled"
        } else if verification {
            "verification_failed"
        } else {
            "failed"
        });
        report["error"] = json!(error.to_string());
        if aborted {
            130
        } else if verification {
            3
        } else {
            1
        }
    } else {
        match report["status"].as_str() {
            Some("cancelled") => 130,
            Some("analysis_findings") => 3,
            Some(
                "complete_with_substitutions"
                | "verified_with_substitutions"
                | "exported_with_substitutions"
                | "analysis_warnings"
                | "recovered_with_findings"
                | "file_hashes_missing"
                | "extracted_without_reference",
            ) => 4,
            _ => 0,
        }
    };
    report["exit_code"] = json!(code);
    report["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
    if let Some(bundle) = &recovery {
        let recovery_status = report["status"].clone();
        if let Err(error) = bundle.finish(&mut report) {
            report["recovery_status"] = recovery_status;
            report["recovery_exit_code"] = json!(code);
            report["report_error"] = json!(error.to_string());
            report["status"] = json!("reporting_failed");
            code = 1;
            report["exit_code"] = json!(code);
        }
    }
    if let Some(history) = &mut history
        && let Err(error) = history.finish(&report)
    {
        report["acquisition_status"] = report["status"].clone();
        report["acquisition_exit_code"] = json!(code);
        report["history_error"] = json!(error.to_string());
        if report["error"].is_null() {
            report["error"] = json!(format!(
                "cannot persist acquisition history/report: {error}"
            ));
        }
        report["status"] = json!("reporting_failed");
        code = 1;
        report["exit_code"] = json!(code);
    }
    (report, code)
}

fn run(
    command: &Command,
    quiet: bool,
    stop: &Arc<AtomicBool>,
    report: &mut Value,
    audit: &mut Option<history::History>,
    recovery: &mut Option<recover::Bundle>,
) -> Result<()> {
    let mut progress = Progress::new(quiet, stop);
    match command {
        Command::AcquireSequential(args) => sequential::acquire(args, stop, &mut progress, report),
        Command::Collect(args) => sequential::collect(args, &mut progress, report),
        Command::RecoverPublication { output } => sequential::recover(output, report),
        Command::Info { image } => inspect::info(image, report),
        Command::Files {
            image,
            offset,
            limit,
        } => logical::list(image, *offset, *limit as usize, &mut progress, report),
        Command::VerifyFile { image, entry } => {
            logical::read(image, *entry, None, &mut progress, report)
        }
        Command::ExtractFile {
            image,
            entry,
            output,
        } => logical::read(image, *entry, Some(output), &mut progress, report),
        Command::Analyze {
            image,
            maximum_findings,
        } => analyze::run(image, *maximum_findings as usize, &mut progress, report),
        Command::Export { image, output } => export::run(image, output, &mut progress, report),
        Command::Recover {
            image,
            output,
            maximum_output_bytes,
            preserve_checksum_suspect,
        } => recover::run(
            image,
            output,
            *maximum_output_bytes,
            *preserve_checksum_suspect,
            &mut progress,
            report,
            recovery,
        ),
        Command::Acquire(args) => {
            let output = session::normalize_output(&args.output)?;
            let mut source = Source::open(&args.source, args.sector_size, &output)?;
            source.configure_reads(
                Arc::clone(stop),
                args.read.read_timeout_ms.map(Duration::from_millis),
            )?;
            let lock = session::lock(&output)?;
            let session = Session::new(output, source.identity.clone(), args);
            let options = session.options()?;
            let identity = session.source.fingerprint()?;
            // Persist the reconstruction contract before creating any journal.
            // A crash here leaves a manifest that resume can initialize safely.
            session.save()?;
            *audit = Some(history::History::open(&session, lock, true)?);
            let history = audit.as_mut().expect("history initialized");
            history.start("acquire", &args.read)?;
            report["history_path"] = json!(sidecar(&session.output, "ewf-history"));
            report["report_path"] = json!(sidecar(&session.output, "ewf-report.json"));
            let writer = AcquisitionWriter::create(&session.output, &options, identity)?;
            acquire(
                writer,
                &mut source,
                &session,
                &args.read,
                &mut progress,
                report,
                history,
            )
        }
        Command::Resume { output, read } => {
            let output = session::normalize_output(output)?;
            let session = Session::load(&output)?;
            let lock = session::lock(&output)?;
            *audit = Some(history::History::open(&session, lock, false)?);
            let history = audit.as_mut().expect("history initialized");
            history.start("resume", read)?;
            report["history_path"] = json!(sidecar(&output, "ewf-history"));
            report["report_path"] = json!(sidecar(&output, "ewf-report.json"));
            let mut source = Source::open(
                &session.source.path,
                Some(session.source.sector_size),
                &output,
            )?;
            source.configure_reads(
                Arc::clone(stop),
                read.read_timeout_ms.map(Duration::from_millis),
            )?;
            if source.identity != session.source {
                return Err(invalid(
                    "source identity or geometry differs from the saved session",
                ));
            }
            let options = session.options()?;
            let identity = session.source.fingerprint()?;
            report["phase"] = json!("resume");
            let writer = if sidecar(&output, "ewf-acquisition").try_exists()? {
                AcquisitionWriter::resume_with_progress(&output, &options, identity, |p| {
                    if history.phase(&format!("resume/{:?}", p.phase)).is_err() {
                        return ControlFlow::Break(());
                    }
                    progress.event(&format!("{:?}", p.phase), p.bytes_processed, p.bytes_total)
                })?
            } else {
                // Only a still-uninitialized session can restart without a journal.
                // An existing image cannot be attributed to this session from its
                // filename alone; verification is a separate explicit command.
                if output.try_exists()? {
                    return Err(invalid(
                        "no acquisition journal remains; use verify for an already published image",
                    ));
                }
                AcquisitionWriter::create(&output, &options, identity)?
            };
            acquire(
                writer,
                &mut source,
                &session,
                read,
                &mut progress,
                report,
                history,
            )
        }
        Command::Checkpoint { command } => {
            let (output, validate) = match command {
                CheckpointCommand::Inspect { output } => (output, false),
                CheckpointCommand::Validate { output } => (output, true),
            };
            let output = session::normalize_output(output)?;
            let _lock = session::lock(&output)?;
            let session = Session::load(&output)?;
            report["phase"] = json!("checkpoint");
            let options = session.options()?;
            let identity = session.source.fingerprint()?;
            let checkpoint = if validate {
                AcquisitionWriter::validate_checkpoint(&output, &options, identity, |p| {
                    progress.event("checkpoint validation", p.bytes_processed, p.bytes_total)
                })?
            } else {
                AcquisitionWriter::inspect_checkpoint(&output, &options, identity)?
            };
            report["checkpoint"] = json!({"source_size": checkpoint.source_size,
                "checkpoint_bytes": checkpoint.checkpoint_bytes, "chunk_size": checkpoint.chunk_size,
                "sealed_segments": checkpoint.sealed_segments, "stored_bytes": checkpoint.stored_bytes,
                "ready_to_finish": checkpoint.ready_to_finish, "publication_started": checkpoint.publication_started,
                "segment_hashes_validated": checkpoint.segment_hashes_validated,
                "substituted_sectors": checkpoint.substituted_sectors,
                "acquisition_errors": error_ranges(&checkpoint.acquisition_errors)});
            report["checkpoint_bytes"] = json!(checkpoint.checkpoint_bytes);
            report["status"] = json!(if validate {
                "checkpoint_validated"
            } else {
                "checkpoint_inspected"
            });
            Ok(())
        }
        Command::Verify { image, entry } => match entry {
            Some(entry) => logical::read(image, *entry, None, &mut progress, report),
            None => verify(image, None, &mut progress, report),
        },
        Command::Report { output, write } => {
            let output = session::normalize_output(output)?;
            let _lock = session::lock(&output)?;
            let session = Session::load(&output)?;
            report["phase"] = json!("report");
            report["history"] = history::report(&session, *write)?;
            report["status"] = json!("history_report");
            Ok(())
        }
    }
}

fn acquire(
    mut writer: AcquisitionWriter,
    source: &mut Source,
    session: &Session,
    args: &ReadArgs,
    progress: &mut Progress<'_>,
    report: &mut Value,
    history: &mut history::History,
) -> Result<()> {
    report["source"] = serde_json::to_value(&session.source)?;
    report["output"] = json!(session.output);
    report["phase"] = json!("acquisition");
    let options = AcquisitionReadOptions {
        retries: args.retries,
        unreadable_sector_policy: if args.zero_fill {
            UnreadableSectorPolicy::ZeroFill
        } else {
            UnreadableSectorPolicy::Stop
        },
        checkpoint_interval: args.checkpoint_interval,
        ..AcquisitionReadOptions::default()
    };
    report["read_policy"] = history::read_policy(args);
    history.phase("acquisition")?;
    let result = writer.acquire_with_progress(source, &options, |p| {
        report["read_attempts"] = json!(p.read_attempts);
        report["retry_attempts"] = json!(p.retry_attempts);
        if history.progress(p).is_err() {
            return ControlFlow::Break(());
        }
        if args
            .stop_after
            .is_some_and(|limit| p.bytes_written >= limit)
        {
            ControlFlow::Break(())
        } else {
            progress.event("acquisition", p.bytes_written, p.source_size)
        }
    });
    report["accepted_bytes"] = json!(writer.position());
    report["checkpoint_bytes"] = json!(writer.checkpoint_offset());
    report["acquisition_errors"] = error_ranges(writer.acquisition_errors());
    report["substituted_sectors"] = json!(substituted_sectors(writer.acquisition_errors())?);
    if let Some(kind) = source.read_failure() {
        report["source_read_stop"] = json!(if kind == io::ErrorKind::TimedOut {
            "timeout"
        } else {
            "cancelled"
        });
    }
    // The library may seal a final checkpoint after the last callback (on a
    // stop or source error). Record that actual offset before closing the run.
    let _ = history.event("checkpoint", json!({
        "accepted_bytes": report["accepted_bytes"], "checkpoint_bytes": report["checkpoint_bytes"],
        "substituted_sectors": report["substituted_sectors"], "acquisition_errors": report["acquisition_errors"],
        "read_attempts": report["read_attempts"], "retry_attempts": report["retry_attempts"],
        "source_read_stop": report["source_read_stop"],
    }));
    let outcome = result?;
    if let Some(error) = &history.failure {
        return Err(io::Error::other(format!("cannot record acquisition history: {error}")).into());
    }
    if outcome.status == AcquisitionStatus::Cancelled {
        report["status"] = json!("cancelled");
        return Ok(());
    }
    source.check_unchanged()?;
    report["phase"] = json!("publication");
    history.phase("publication")?;
    let finished = writer.finish_with_progress(|p| {
        if history
            .phase(&format!("publication/{:?}", p.phase))
            .is_err()
        {
            return ControlFlow::Break(());
        }
        progress.event(&format!("{:?}", p.phase), p.bytes_processed, p.bytes_total)
    })?;
    report["published"] = json!(true);
    report["segments"] = json!(finished.segment_paths);
    history.event(
        "published",
        json!({"segments": report["segments"], "sha256": hex(&finished.computed_sha256)}),
    )?;
    history.phase("verification")?;
    verify(
        &session.output,
        Some(finished.computed_sha256),
        progress,
        report,
    )?;
    report["status"] = json!(if outcome.progress.substituted_sectors == 0 {
        "complete"
    } else {
        "complete_with_substitutions"
    });
    Ok(())
}

fn verify(
    path: &Path,
    expected: Option<[u8; 32]>,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    report["phase"] = json!("verification");
    let image = Image::open(path)?;
    let mut options = VerifyOptions::default();
    if let Some(hash) = expected {
        options = options.with_expected_sha256(hash);
    }
    let verified = image.verify_with_progress(&options, |p| {
        progress.event("verification", p.bytes_verified, p.bytes_total)
    })?;
    report["verification"] = json!({"bytes_verified": verified.bytes_verified,
        "md5": hex(&verified.hashes.md5), "sha1": hex(&verified.hashes.sha1),
        "sha256": hex(&verified.hashes.sha256), "references_match": verified.references_match(),
        "comparisons": verified.comparisons});
    report["acquisition_errors"] = error_ranges(image.acquisition_errors());
    let substituted = substituted_sectors(image.acquisition_errors())?;
    report["substituted_sectors"] = json!(substituted);
    if verified.references_match() != Some(true) {
        return Err(invalid(
            "media hashes mismatch or no supported reference digest exists",
        ));
    }
    report["status"] = json!(if substituted == 0 {
        "verified"
    } else {
        "verified_with_substitutions"
    });
    Ok(())
}

fn error_ranges(errors: &[ewf_image::AcquisitionError]) -> Value {
    json!(errors.iter().map(|range| json!({"first_sector": range.first_sector, "sector_count": range.sector_count})).collect::<Vec<_>>())
}

fn substituted_sectors(errors: &[ewf_image::AcquisitionError]) -> Result<u64> {
    errors.iter().try_fold(0u64, |total, range| {
        total
            .checked_add(range.sector_count)
            .ok_or_else(|| invalid("acquisition-error sector count overflow"))
    })
}

fn sidecar(output: &Path, suffix: &str) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(output.file_name().unwrap_or_default());
    name.push(format!(".{suffix}"));
    output.with_file_name(name)
}

fn invalid(message: &str) -> Box<dyn std::error::Error> {
    io::Error::new(io::ErrorKind::InvalidInput, message).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Progress<'a> {
    quiet: bool,
    stop: &'a AtomicBool,
    last: Option<Instant>,
}

impl<'a> Progress<'a> {
    fn new(quiet: bool, stop: &'a AtomicBool) -> Self {
        Self {
            quiet,
            stop,
            last: None,
        }
    }

    fn event(&mut self, phase: &str, done: u64, total: u64) -> ControlFlow<()> {
        if self.stop.load(Ordering::Relaxed) {
            return ControlFlow::Break(());
        }
        if !self.quiet
            && self
                .last
                .is_none_or(|time| time.elapsed() >= Duration::from_secs(1))
        {
            eprintln!("{phase}: {done}/{total} bytes");
            self.last = Some(Instant::now());
        }
        ControlFlow::Continue(())
    }
}
