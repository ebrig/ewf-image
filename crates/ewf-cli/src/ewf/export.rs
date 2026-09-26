//! Strict streaming raw export with exclusive final publication.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ewf_image::{EwfError, Image};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::Sha256;

use super::{Progress, Result, error_ranges, hex, inspect, invalid, substituted_sectors};

const BUFFER_BYTES: usize = 1024 * 1024;

pub(super) fn run(
    input: &Path,
    output: &Path,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    report["output"] = json!(output);
    let image = inspect::open(input, report)?;
    if !image.info().acquisition_complete {
        return Err(invalid("cannot export an incomplete acquisition"));
    }
    let output = destination(image.segment_filenames(), output)?;
    report["output"] = json!(output);
    report["media_bytes"] = json!(image.media_size());
    report["exported_bytes"] = json!(0);
    report["acquisition_errors"] = error_ranges(image.acquisition_errors());
    let substituted = substituted_sectors(image.acquisition_errors())?;
    report["substituted_sectors"] = json!(substituted);
    let parent = output
        .parent()
        .ok_or_else(|| invalid("missing output parent"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".ewf-export-")
        .tempfile_in(parent)?;
    report["phase"] = json!("export");
    copy_media(&image, &mut temporary, progress, report)?;
    report["phase"] = json!("publication");
    temporary.as_file().sync_all()?;
    check_stop(progress, image.media_size(), image.media_size())?;
    temporary.persist_noclobber(&output)?;
    report["published"] = json!(true);
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    report["status"] = json!(if substituted == 0 {
        "exported"
    } else {
        "exported_with_substitutions"
    });
    Ok(())
}

pub(crate) fn destination(segments: &[PathBuf], output: &Path) -> Result<PathBuf> {
    let name = output
        .file_name()
        .ok_or_else(|| invalid("missing output filename"))?;
    #[cfg(windows)]
    if name.to_string_lossy().contains(':') {
        return Err(invalid(
            "alternate data streams are not output destinations",
        ));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let output = fs::canonicalize(parent)?.join(name);
    match fs::symlink_metadata(&output) {
        Ok(_) => return Err(invalid("output destination already exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for segment in segments {
        let segment = fs::canonicalize(segment)?;
        // Protect absent control files too: creating one can hide or invalidate
        // otherwise healthy evidence. Include paths inside control directories.
        let parent = segment
            .parent()
            .ok_or_else(|| invalid("missing segment parent"))?;
        let prefix = format!(".{}.ewf-", segment.file_name().unwrap().to_string_lossy());
        if let Ok(relative) = output.strip_prefix(parent)
            && let Some(first) = relative.components().next()
            && first
                .as_os_str()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with(&prefix.to_ascii_lowercase())
        {
            return Err(invalid("output destination overlaps an image control path"));
        }
    }
    Ok(output)
}

fn check_stop(progress: &mut Progress<'_>, done: u64, total: u64) -> Result<()> {
    if progress.event("export", done, total).is_break() {
        return Err(EwfError::Aborted.into());
    }
    Ok(())
}

fn copy_media(
    image: &Image,
    output: &mut impl Write,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut offset = 0;
    let mut chunk_index = 0;
    check_stop(progress, offset, image.media_size())?;
    while offset < image.media_size() {
        let chunk = image.read_data_chunk(chunk_index)?;
        let expected = image.chunk_size().min(image.media_size() - offset);
        if chunk.logical_offset != offset || chunk.data.len() as u64 != expected || chunk.corrupted
        {
            return Err(
                io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete media stream").into(),
            );
        }
        for bytes in chunk.data.chunks(BUFFER_BYTES) {
            check_stop(progress, offset, image.media_size())?;
            output.write_all(bytes)?;
            md5.update(bytes);
            sha1.update(bytes);
            sha256.update(bytes);
            offset += bytes.len() as u64;
            report["exported_bytes"] = json!(offset);
            check_stop(progress, offset, image.media_size())?;
        }
        chunk_index += 1;
    }
    output.flush()?;
    let digests = [
        hex(&md5.finalize()),
        hex(&sha1.finalize()),
        hex(&sha256.finalize()),
    ];
    let stored = [
        image.md5_hash().map(|v| hex(&v)),
        image.sha1_hash().map(|v| hex(&v)),
        image.hash_value("SHA256").map(str::to_owned),
    ];
    let comparisons: Vec<_> = ["MD5", "SHA1", "SHA256"]
        .into_iter()
        .zip(stored)
        .zip(&digests)
        .filter_map(|((algorithm, expected), computed)| {
            expected.map(|expected| {
                json!({"algorithm": algorithm, "reference": "stored",
                "matches": expected.eq_ignore_ascii_case(computed),
                "expected": expected, "computed": computed})
            })
        })
        .collect();
    let matches =
        (!comparisons.is_empty()).then(|| comparisons.iter().all(|v| v["matches"] == true));
    report["verification"] = json!({"bytes_verified": offset, "md5": digests[0],
        "sha1": digests[1], "sha256": digests[2],
        "references_match": matches, "comparisons": comparisons});
    if matches == Some(false) {
        report["phase"] = json!("verification");
        return Err(invalid("exported media differs from a stored digest"));
    }
    report["media_verified"] = json!(matches == Some(true));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    struct Sink<'a> {
        stop: &'a AtomicBool,
        fault: &'static str,
        accepted: usize,
    }

    impl Write for Sink<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = if self.fault == "short" {
                if self.accepted == 17 {
                    return Err(io::ErrorKind::StorageFull.into());
                }
                17.min(bytes.len())
            } else {
                bytes.len()
            };
            self.accepted += count;
            if self.fault == "cancel" {
                self.stop.store(true, Ordering::Relaxed);
            }
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fault == "flush" {
                Err(io::ErrorKind::StorageFull.into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn interrupted_copies_never_report_complete_hashes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("case.E01");
        let mut writer =
            ewf_image::EwfWriter::create(&path, ewf_image::WriteOptions::default()).unwrap();
        writer.write_all(&vec![0; 65536]).unwrap();
        writer.finish().unwrap();
        let image = Image::open(path).unwrap();
        for fault in ["short", "flush", "cancel"] {
            let stop = AtomicBool::new(false);
            let mut sink = Sink {
                stop: &stop,
                fault,
                accepted: 0,
            };
            let mut report = json!({"verification": null});
            let error = copy_media(
                &image,
                &mut sink,
                &mut Progress::new(true, &stop),
                &mut report,
            )
            .unwrap_err();
            assert!(report["verification"].is_null());
            match fault {
                "cancel" => {
                    assert!(matches!(
                        error.downcast_ref::<EwfError>(),
                        Some(EwfError::Aborted)
                    ));
                    assert_eq!(sink.accepted, 32768);
                }
                "short" => assert_eq!(sink.accepted, 17),
                _ => assert_eq!(sink.accepted, 65536),
            }
        }
    }
}
