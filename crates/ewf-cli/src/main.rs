//! Unified local evidence operations. Format libraries remain independent.
mod aff4;
mod ewf;
mod format;
mod logical;
mod output;
mod password;
mod prefetch;
mod timestamps;
mod transfer;

use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(
    version,
    about = "Acquire, convert, and verify EWF, AFF4, and raw evidence",
    disable_help_subcommand = true,
    after_help = "Output formats follow filenames: .E01, .Ex01, .aff4, .raw; collections: .Lx01 or .aff4."
)]
struct Cli {
    /// Print machine-readable results.
    #[arg(long, global = true)]
    json: bool,
    /// Hide progress.
    #[arg(short, long, global = true)]
    quiet: bool,
    /// Read an EWF password from a file, or from stdin with '-'.
    #[arg(long, global = true, value_name = "PATH")]
    password_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Default)]
struct CaseArgs {
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

#[derive(Subcommand)]
enum Command {
    /// Advanced EWF acquisition, recovery, and diagnostic commands.
    Ewf {
        #[command(subcommand)]
        command: ewf::Command,
    },
    /// Show image information and selectable resources.
    Info { image: PathBuf },
    /// Verify an image or a selected logical file.
    Verify {
        image: PathBuf,
        /// Entry number (EWF) or resource ID (AFF4).
        entry: Option<String>,
        /// Independently recorded decoded-media SHA256.
        #[arg(long, value_name = "HASH")]
        sha256: Option<String>,
        /// Independently recorded AFF4 metadata SHA256 (whole-container checks).
        #[arg(long, value_name = "HASH", conflicts_with_all = ["entry", "sha256"])]
        metadata_sha256: Option<String>,
        /// EWF whole-image verification workers (default: 1).
        #[arg(long, value_name = "COUNT", value_parser = clap::value_parser!(u32).range(1..=64))]
        workers: Option<u32>,
    },
    /// Acquire a file or physical disk to one image.
    Acquire {
        /// File or physical device to read.
        source: PathBuf,
        /// New image; extension selects its format.
        output: PathBuf,
        /// Sector size for raw files; devices supply their own geometry.
        #[arg(long, value_name = "BYTES")]
        sector_size: Option<u32>,
        /// Compression for the selected image format. EWF: raw/zlib; AFF4: stored/zlib/snappy/lz4.
        #[arg(long, value_name = "CODEC", value_parser = ["raw", "stored", "zlib", "snappy", "lz4"])]
        compression: Option<String>,
        /// AFF4 physical chunk size in bytes (default: 32 KiB).
        #[arg(long, value_name = "BYTES")]
        chunk_bytes: Option<u32>,
        #[command(flatten)]
        case: CaseArgs,
    },
    /// Convert decoded evidence to another container or raw image.
    Convert {
        /// Existing image or collection.
        input: PathBuf,
        /// New image; extension selects its format.
        output: PathBuf,
        /// Select a physical disk in a container with multiple disks.
        #[arg(long, value_name = "ID")]
        resource: Option<String>,
        /// Supply missing raw/AFF4 sector geometry.
        #[arg(long, value_name = "BYTES")]
        sector_size: Option<u32>,
    },
    /// Collect a directory into a logical image.
    Collect {
        /// Directory to collect.
        source: PathBuf,
        /// New .Lx01 or .aff4 collection.
        output: PathBuf,
        #[command(flatten)]
        case: CaseArgs,
        /// Skip a relative path (AFF4); repeat for several paths.
        #[arg(long, value_name = "PATH")]
        exclude: Vec<PathBuf>,
        /// Record skipped inaccessible entries (AFF4).
        #[arg(long)]
        allow_partial: bool,
    },
    /// List logical files and their selectors.
    Files {
        image: PathBuf,
        /// First entry to list.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Entries per page.
        #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..=100000))]
        limit: u32,
    },
    /// Extract and check one logical file or AFF4 resource.
    Extract {
        image: PathBuf,
        entry: String,
        output: PathBuf,
        /// Restore recorded access and modification times on the output file.
        #[arg(long)]
        restore_times: bool,
    },
    /// Show recorded metadata.
    Metadata { image: PathBuf },
    /// Verify an AFF4 image across supplied volumes.
    VerifySet {
        #[arg(required = true)]
        volumes: Vec<PathBuf>,
        #[arg(long, value_name = "ID")]
        image: String,
        #[arg(long, value_name = "HASH")]
        sha256: Option<String>,
        /// Include each volume's integrity references.
        #[arg(long)]
        full: bool,
    },
    /// Scan an EWF image for damage.
    Analyze { image: PathBuf },
    /// Export an EWF media stream, including a logical image's flat stream.
    #[command(hide = true)]
    Export { image: PathBuf, output: PathBuf },
    /// Recover damaged EWF1 media with a provenance map.
    Recover {
        image: PathBuf,
        output: PathBuf,
        /// Retain decodable bytes with suspect checksums.
        #[arg(long)]
        preserve_checksum_suspect: bool,
    },
    /// Resume an E01 acquisition from its saved session.
    Resume { output: PathBuf },
    /// Inspect or validate an E01 checkpoint.
    Checkpoint {
        #[command(subcommand)]
        command: Checkpoint,
    },
    /// Resolve an interrupted EWF publication.
    RecoverPublication { output: PathBuf },
    /// Show saved E01 acquisition history.
    Report {
        output: PathBuf,
        #[arg(long)]
        write: bool,
    },
}

#[derive(Subcommand)]
enum Checkpoint {
    /// Show checkpoint information.
    Inspect { output: PathBuf },
    /// Check saved segment hashes.
    Validate { output: PathBuf },
}

struct Context {
    stop: Arc<AtomicBool>,
    quiet: bool,
    last: Option<Instant>,
    password: Option<ewf_image::EwfPassword>,
}

impl Context {
    fn check(&mut self, phase: &str, done: u64, total: u64) -> Result<()> {
        if self.stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "operation cancelled").into());
        }
        if !self.quiet && self.last.is_none_or(|t| t.elapsed().as_secs() >= 1) {
            eprintln!("{phase}: {done}/{total} bytes");
            self.last = Some(Instant::now());
        }
        Ok(())
    }
    fn progress(&mut self, phase: &str, done: u64, total: u64) -> std::ops::ControlFlow<()> {
        if self.check(phase, done, total).is_err() {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    }
    fn ewf(&self, mut args: Vec<OsString>, report: &mut Value) -> Result<()> {
        args.insert(0, "ewf-cli ewf".into());
        if self.quiet {
            args.insert(1, "--quiet".into());
        }
        let (value, code) = ewf::dispatch(&args, &self.stop, self.password.as_ref())?;
        *report = value;
        report["exit_code"] = json!(code);
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> Box<dyn std::error::Error> {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}

fn case_arguments(case: &CaseArgs, args: &mut Vec<OsString>) {
    for (name, value) in [
        ("--case-number", &case.case_number),
        ("--evidence-number", &case.evidence_number),
        ("--examiner", &case.examiner),
    ] {
        if let Some(value) = value {
            args.extend([name.into(), value.into()]);
        }
    }
}

fn ewf_only(path: &Path) -> Result<()> {
    if format::detect(path)? != format::Input::Ewf {
        return Err(invalid("this operation requires an EWF image"));
    }
    Ok(())
}

fn validate_password_target(command: &Command) -> Result<()> {
    let path = match command {
        Command::Ewf {
            command:
                ewf::Command::Info { image }
                | ewf::Command::Verify { image, .. }
                | ewf::Command::Files { image, .. }
                | ewf::Command::VerifyFile { image, .. }
                | ewf::Command::ExtractFile { image, .. }
                | ewf::Command::Analyze { image, .. }
                | ewf::Command::Export { image, .. },
        } => image,
        Command::Info { image }
        | Command::Metadata { image }
        | Command::Verify { image, .. }
        | Command::Files { image, .. }
        | Command::Extract { image, .. }
        | Command::Analyze { image }
        | Command::Export { image, .. } => image,
        Command::Convert { input, .. } => input,
        _ => return Err(invalid("--password-file applies only to EWF image reads")),
    };
    ewf_only(path)
}

fn run(cli: &Cli, ctx: &mut Context, report: &mut Value) -> Result<()> {
    use format::{Input, Output};
    match &cli.command {
        Command::Ewf { command } => {
            let (value, code) =
                ewf::dispatch_command(command, ctx.quiet, &ctx.stop, ctx.password.as_ref());
            *report = value;
            report["exit_code"] = json!(code);
            Ok(())
        }
        Command::Acquire {
            source,
            output,
            sector_size,
            compression,
            chunk_bytes,
            case,
        } => {
            let format = Output::from_path(output)?;
            if matches!(format, Output::E01 | Output::Ex01) {
                if chunk_bytes.is_some() {
                    return Err(invalid("--chunk-bytes applies only to AFF4 acquisition"));
                }
                if compression
                    .as_deref()
                    .is_some_and(|value| !matches!(value, "raw" | "zlib"))
                {
                    return Err(invalid("EWF compression must be raw or zlib"));
                }
                let mut args = vec![
                    if format == Output::E01 {
                        "acquire".into()
                    } else {
                        "acquire-sequential".into()
                    },
                    source.as_os_str().into(),
                    output.as_os_str().into(),
                ];
                if let Some(size) = sector_size {
                    args.extend(["--sector-size".into(), size.to_string().into()]);
                }
                if let Some(codec) = compression {
                    args.extend(["--compression".into(), codec.into()]);
                }
                case_arguments(case, &mut args);
                ctx.ewf(args, report)
            } else {
                transfer::acquire(
                    source,
                    output,
                    *sector_size,
                    transfer::AcquireOptions {
                        compression: compression.as_deref(),
                        chunk_bytes: *chunk_bytes,
                    },
                    case,
                    ctx,
                    report,
                )
            }
        }
        Command::Convert {
            input,
            output,
            resource,
            sector_size,
        } => transfer::convert(
            input,
            output,
            resource.as_deref(),
            *sector_size,
            ctx,
            report,
        ),
        Command::Collect {
            source,
            output,
            case,
            exclude,
            allow_partial,
        } => match Output::from_path(output)? {
            Output::Lx01 => {
                if !exclude.is_empty() || *allow_partial {
                    return Err(invalid(
                        "exclusions and partial collection are currently supported for AFF4 output",
                    ));
                }
                let mut args = vec![
                    "collect".into(),
                    source.as_os_str().into(),
                    output.as_os_str().into(),
                ];
                case_arguments(case, &mut args);
                ctx.ewf(args, report)
            }
            Output::Aff4 => {
                aff4::collect(source, output, case, exclude, *allow_partial, ctx, report)
            }
            _ => Err(invalid("collection output must be .Lx01 or .aff4")),
        },
        Command::Info { image } | Command::Metadata { image } => {
            let metadata = matches!(cli.command, Command::Metadata { .. });
            match format::detect(image)? {
                Input::Ewf => ctx.ewf(vec!["info".into(), image.as_os_str().into()], report),
                Input::Aff4 => aff4::info(image, metadata, report),
                Input::Raw => {
                    report["image"] = json!(image);
                    report["bytes"] = json!(std::fs::metadata(image)?.len());
                    report["status"] = json!("inspected");
                    report["format"] = json!("raw");
                    Ok(())
                }
            }
        }
        Command::Verify {
            image,
            entry,
            sha256,
            metadata_sha256,
            workers,
        } => {
            if let Some(hash) = sha256 {
                format::parse_hash(hash)?;
            }
            if let Some(hash) = metadata_sha256 {
                format::parse_hash(hash)?;
                if format::detect(image)? != Input::Aff4 {
                    return Err(invalid("--metadata-sha256 requires an AFF4 container"));
                }
            }
            if workers.is_some() && entry.is_some() {
                return Err(invalid(
                    "--workers applies only to whole-image EWF verification",
                ));
            }
            match format::detect(image)? {
                Input::Ewf if sha256.is_none() => {
                    let mut args = vec!["verify".into(), image.as_os_str().into()];
                    if let Some(entry) = entry {
                        args.push(entry.into());
                    }
                    if let Some(count) = workers {
                        args.extend(["--workers".into(), count.to_string().into()]);
                    }
                    ctx.ewf(args, report)
                }
                Input::Ewf => transfer::verify_ewf(
                    image,
                    entry.as_deref(),
                    sha256.as_deref(),
                    workers.unwrap_or(1) as usize,
                    ctx,
                    report,
                ),
                Input::Aff4 => {
                    if workers.is_some() {
                        return Err(invalid("--workers applies only to EWF verification"));
                    }
                    aff4::verify(
                        image,
                        entry.as_deref(),
                        sha256.as_deref(),
                        metadata_sha256.as_deref(),
                        ctx,
                        report,
                    )
                }
                Input::Raw => {
                    if workers.is_some() {
                        return Err(invalid("--workers applies only to EWF verification"));
                    }
                    if entry.is_some() {
                        return Err(invalid("raw images do not have file selectors"));
                    }
                    transfer::verify_raw(image, sha256.as_deref(), ctx, report)
                }
            }
        }
        Command::Files {
            image,
            offset,
            limit,
        } => match format::detect(image)? {
            Input::Ewf => ctx.ewf(
                vec![
                    "files".into(),
                    image.as_os_str().into(),
                    "--offset".into(),
                    offset.to_string().into(),
                    "--limit".into(),
                    limit.to_string().into(),
                ],
                report,
            ),
            Input::Aff4 => aff4::files(image, *offset, *limit as usize, report),
            Input::Raw => Err(invalid("raw images do not contain a logical file catalog")),
        },
        Command::Extract {
            image,
            entry,
            output,
            restore_times,
        } => match format::detect(image)? {
            Input::Ewf => {
                let mut args = vec![
                    "extract".into(),
                    image.as_os_str().into(),
                    entry.into(),
                    output.as_os_str().into(),
                ];
                if *restore_times {
                    args.push("--restore-times".into());
                }
                ctx.ewf(args, report)
            }
            Input::Aff4 => aff4::extract(image, entry, output, *restore_times, ctx, report),
            Input::Raw => Err(invalid("raw images do not contain a logical file catalog")),
        },
        Command::VerifySet {
            volumes,
            image,
            sha256,
            full,
        } => aff4::verify_set(volumes, image, sha256.as_deref(), *full, ctx, report),
        Command::Analyze { image } => {
            ewf_only(image)?;
            ctx.ewf(vec!["analyze".into(), image.as_os_str().into()], report)
        }
        Command::Export { image, output } => {
            ewf_only(image)?;
            ctx.ewf(
                vec![
                    "export".into(),
                    image.as_os_str().into(),
                    output.as_os_str().into(),
                ],
                report,
            )
        }
        Command::Recover {
            image,
            output,
            preserve_checksum_suspect,
        } => {
            let mut args = vec![
                "recover".into(),
                image.as_os_str().into(),
                output.as_os_str().into(),
            ];
            if *preserve_checksum_suspect {
                args.push("--preserve-checksum-suspect".into());
            }
            ctx.ewf(args, report)
        }
        Command::Resume { output } => {
            ctx.ewf(vec!["resume".into(), output.as_os_str().into()], report)
        }
        Command::RecoverPublication { output } => ctx.ewf(
            vec!["recover-publication".into(), output.as_os_str().into()],
            report,
        ),
        Command::Checkpoint { command } => {
            let (name, path) = match command {
                Checkpoint::Inspect { output } => ("inspect", output),
                Checkpoint::Validate { output } => ("validate", output),
            };
            ctx.ewf(
                vec!["checkpoint".into(), name.into(), path.as_os_str().into()],
                report,
            )
        }
        Command::Report { output, write } => {
            let mut args = vec!["report".into(), output.as_os_str().into()];
            if *write {
                args.push("--write".into());
            }
            ctx.ewf(args, report)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut ctx = Context {
        stop: Arc::new(AtomicBool::new(false)),
        quiet: cli.quiet,
        last: None,
        password: None,
    };
    let flag = Arc::clone(&ctx.stop);
    let started = Instant::now();
    let mut report = json!({"schema_version":1,"status":"failed","published":false,"exit_code":0});
    let result = ctrlc::set_handler(move || flag.store(true, Ordering::Relaxed))
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        .and_then(|()| {
            if let Some(path) = &cli.password_file {
                validate_password_target(&cli.command)?;
                ctx.password = Some(password::read(path)?);
            }
            run(&cli, &mut ctx, &mut report)
        });
    if let Err(error) = result {
        let cancelled = ctx.stop.load(Ordering::Relaxed);
        report["status"] = json!(if cancelled { "cancelled" } else { "failed" });
        report["error"] = json!(error.to_string());
        report["exit_code"] = json!(if cancelled {
            130
        } else {
            report["exit_code"]
                .as_u64()
                .filter(|n| *n != 0)
                .unwrap_or(1)
        });
    }
    report["schema_version"] = json!(1);
    report["tool"] = json!("ewf-cli");
    report["tool_version"] = json!(env!("CARGO_PKG_VERSION"));
    if report["elapsed_seconds"].is_null() {
        report["elapsed_seconds"] = json!(started.elapsed().as_secs_f64());
    }
    let code = report["exit_code"].as_u64().unwrap_or(1) as u8;
    if let Err(error) = output::print(&report, cli.json) {
        eprintln!("cannot write result: {error}");
        return ExitCode::from(1);
    }
    ExitCode::from(code)
}
