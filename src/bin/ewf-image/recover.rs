//! Recovery bundles retain partial output and bind complete output to provenance.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use ewf_image::{EwfError, EwfRecovery, RecoveryOptions, RecoveryReport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Progress, Result, export, hex, invalid};

struct HashedWriter<W> {
    inner: W,
    bytes: u64,
    hash: Sha256,
}

impl<W> HashedWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            bytes: 0,
            hash: Sha256::new(),
        }
    }

    fn digest(&self) -> String {
        hex(&self.hash.clone().finalize())
    }
}

impl<W: Write> Write for HashedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.hash.update(&bytes[..count]);
        self.bytes += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub(super) struct Bundle {
    directory: PathBuf,
    raw_path: PathBuf,
    map_path: PathBuf,
    raw: HashedWriter<File>,
    map: HashedWriter<File>,
    stream_complete: bool,
}

pub fn run(
    input: &Path,
    output: &Path,
    maximum: Option<u64>,
    preserve_suspect: bool,
    progress: &mut Progress<'_>,
    report: &mut Value,
    bundle: &mut Option<Bundle>,
) -> Result<()> {
    report["image"] = json!(input);
    report["media_verified"] = json!(false);
    report["recovery_complete"] = json!(false);
    let mut options = RecoveryOptions::default().with_preserve_checksum_suspect(preserve_suspect);
    if let Some(limit) = maximum {
        options = options.with_maximum_output_bytes(limit);
    }
    if progress.stop.load(Ordering::Relaxed) {
        return Err(EwfError::Aborted.into());
    }
    let recovery = EwfRecovery::open(input, options)?;
    let directory = export::destination(recovery.segment_paths(), output)?;
    report["output_directory"] = json!(directory);
    report["media_bytes"] = json!(recovery.media_size());
    report["mapped_bytes"] = json!(0);
    report["preserve_checksum_suspect"] = json!(preserve_suspect);
    report["maximum_output_bytes"] = json!(maximum);
    fs::create_dir(&directory)?;
    report["bundle_created"] = json!(true);
    sync_directory(
        directory
            .parent()
            .ok_or_else(|| invalid("missing recovery parent"))?,
    )?;
    let raw_path = directory.join("image.raw.partial");
    let map_path = directory.join("map.jsonl.partial");
    report["raw_path"] = json!(raw_path);
    report["map_path"] = json!(map_path);
    *bundle = Some(Bundle {
        raw: HashedWriter::new(create(&raw_path)?),
        map: HashedWriter::new(create(&map_path)?),
        directory,
        raw_path,
        map_path,
        stream_complete: false,
    });
    let bundle = bundle.as_mut().expect("initialized recovery bundle");
    record(
        &mut bundle.map,
        &json!({"schema_version": 1, "record": "header",
        "media_bytes": recovery.media_size(), "source_segments": recovery.segment_paths(),
        "preserve_checksum_suspect": preserve_suspect, "maximum_output_bytes": maximum}),
    )?;
    bundle.map.inner.sync_all()?;
    sync_directory(&bundle.directory)?;
    report["phase"] = json!("recovery");
    let recovered = stream(
        &recovery,
        &mut bundle.raw,
        &mut bundle.map,
        progress,
        report,
    )?;
    bundle.stream_complete = true;
    report["recovery"] = serde_json::to_value(&recovered)?;
    bundle.raw.inner.sync_all()?;
    bundle.map.inner.sync_all()?;
    if progress.stop.load(Ordering::Relaxed) {
        return Err(EwfError::Aborted.into());
    }
    report["phase"] = json!("publication");
    publish_partial(&mut bundle.raw_path, &bundle.directory.join("image.raw"))?;
    report["raw_path"] = json!(bundle.raw_path);
    publish_partial(&mut bundle.map_path, &bundle.directory.join("map.jsonl"))?;
    report["map_path"] = json!(bundle.map_path);
    sync_directory(&bundle.directory)?;
    report["published"] = json!(true);
    report["status"] = json!(if recovered.chunks_zero_filled != 0
        || recovered.chunks_checksum_suspect != 0
        || recovered.chunks_redundant != 0
        || !recovered.notices.is_empty()
        || recovered.omitted_notices != 0
    {
        "recovered_with_findings"
    } else {
        "recovered"
    });
    Ok(())
}

fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn record(output: &mut impl Write, value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    output.write_all(&bytes)
}

fn stream(
    recovery: &EwfRecovery,
    raw: &mut impl Write,
    map: &mut impl Write,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<RecoveryReport> {
    let mut previous = 0;
    let mut map_error = None;
    let result = recovery.recover_with_progress(raw, |p| {
        if let Some(status) = p.last_status {
            let row = json!({"record": "chunk", "chunk_index": p.chunks_processed - 1,
                "logical_offset": previous, "byte_count": p.bytes_written - previous,
                "status": status});
            if let Err(error) = record(map, &row) {
                map_error = Some(error);
                return ControlFlow::Break(());
            }
            previous = p.bytes_written;
            report["mapped_bytes"] = json!(previous);
            report["mapped_chunks"] = json!(p.chunks_processed);
        }
        progress.event("recovery", p.bytes_written, p.bytes_total)
    });
    if let Some(error) = map_error {
        return Err(error.into());
    }
    Ok(result?)
}

fn publish_partial(partial: &mut PathBuf, final_path: &Path) -> Result<()> {
    // Both links refer to the owned file. No unrelated destination can be replaced.
    fs::hard_link(&*partial, final_path)?;
    let old = std::mem::replace(partial, final_path.to_path_buf());
    fs::remove_file(old)?;
    Ok(())
}

#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))]
fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl Bundle {
    pub fn finish(&self, report: &mut Value) -> Result<()> {
        report["raw_path"] = json!(self.raw_path);
        report["map_path"] = json!(self.map_path);
        report["raw_bytes_written"] = json!(self.raw.bytes);
        report["raw_prefix_sha256"] = json!(self.raw.digest());
        report["map_bytes_written"] = json!(self.map.bytes);
        report["map_prefix_sha256"] = json!(self.map.digest());
        report["output_sha256"] = if self.stream_complete {
            json!(self.raw.digest())
        } else {
            Value::Null
        };
        let mut sync_errors = Vec::new();
        for result in [self.raw.inner.sync_all(), self.map.inner.sync_all()] {
            if let Err(error) = result {
                sync_errors.push(error.to_string());
            }
        }
        if !sync_errors.is_empty() {
            report["sync_errors"] = json!(sync_errors);
            report["status"] = json!("failed");
            report["exit_code"] = json!(1);
        }
        let complete = sync_errors.is_empty()
            && report["published"] == true
            && matches!(
                report["status"].as_str(),
                Some("recovered" | "recovered_with_findings")
            );
        report["recovery_complete"] = json!(complete);
        report["map_sha256"] = if complete {
            json!(self.map.digest())
        } else {
            Value::Null
        };
        let path = self.directory.join("result.json");
        report["result_path"] = json!(path);
        let saved = (|| -> Result<()> {
            let mut temporary = tempfile::Builder::new()
                .prefix(".result-")
                .tempfile_in(&self.directory)?;
            serde_json::to_writer_pretty(&mut temporary, report)?;
            temporary.write_all(b"\n")?;
            temporary.as_file().sync_all()?;
            temporary.persist_noclobber(&path)?;
            Ok(())
        })();
        if saved.is_err() {
            report["recovery_complete"] = json!(false);
        }
        saved?;
        sync_directory(&self.directory)?;
        if !sync_errors.is_empty() {
            return Err(invalid("could not synchronize partial recovery output"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    struct FullAfter {
        remaining: usize,
    }

    impl Write for FullAfter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::ErrorKind::StorageFull.into());
            }
            let count = self.remaining.min(bytes.len());
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn short_raw_writes_and_map_failures_never_extend_provenance_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.E01");
        let mut writer =
            ewf_image::EwfWriter::create(&path, ewf_image::WriteOptions::default()).unwrap();
        writer.write_all(&vec![0; 65536]).unwrap();
        writer.finish().unwrap();
        let recovery = EwfRecovery::open(path, RecoveryOptions::default()).unwrap();
        let stop = AtomicBool::new(false);
        let mut raw = HashedWriter::new(FullAfter { remaining: 17 });
        let mut map = Vec::new();
        let mut report = json!({"mapped_bytes": 0});
        let error = stream(
            &recovery,
            &mut raw,
            &mut map,
            &mut Progress::new(true, &stop),
            &mut report,
        )
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<EwfError>(),
            Some(EwfError::Io(_))
        ));
        assert_eq!(raw.bytes, 17);
        assert_eq!(report["mapped_bytes"], 0);
        assert!(map.is_empty());
        let mut raw = Vec::new();
        let mut map = HashedWriter::new(FullAfter { remaining: 9 });
        let error = stream(
            &recovery,
            &mut raw,
            &mut map,
            &mut Progress::new(true, &stop),
            &mut report,
        )
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(raw.len(), 32768);
        assert_eq!(map.bytes, 9);
        assert_eq!(report["mapped_bytes"], 0);
    }
}
