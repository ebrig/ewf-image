//! End-to-end acquisition command contracts, including process restarts.
#![cfg(feature = "cli")]

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, Output};

use ewf_image::Image;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn cli(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ewf-image"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

fn result(directory: &Path, args: &[&str], code: i32) -> Value {
    let output = cli(directory, args);
    assert_eq!(
        output.status.code(),
        Some(code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["exit_code"], code);
    report
}

fn source(directory: &Path) -> Vec<u8> {
    let bytes: Vec<_> = (0..32768)
        .map(|n| ((n * 31 + n / 256) % 251) as u8)
        .collect();
    fs::write(directory.join("source.raw"), &bytes).unwrap();
    bytes
}

#[test]
fn cli_info_inspects_metadata_without_certifying_corrupt_media() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    let path = dir.path().join("case.E01");
    let options = ewf_image::WriteOptions {
        metadata: ewf_image::EwfMetadata {
            case_number: Some("CASE-123".into()),
            examiner: Some("Examiner".into()),
            password: Some("do-not-print".into()),
            ..ewf_image::EwfMetadata::default()
        },
        acquisition_errors: vec![ewf_image::AcquisitionError {
            first_sector: 1,
            sector_count: 2,
        }],
        ..ewf_image::WriteOptions::default()
    };
    let mut writer = ewf_image::EwfWriter::create(&path, options).unwrap();
    writer.write_all(&bytes).unwrap();
    writer.finish().unwrap();
    let mut encoded = fs::read(&path).unwrap();
    let offset = encoded
        .windows(bytes.len())
        .position(|v| v == bytes)
        .unwrap();
    encoded[offset] ^= 1;
    fs::write(&path, encoded).unwrap();
    let report = result(dir.path(), &["info", "case.E01"], 0);
    assert_eq!(report["status"], "inspected");
    assert_eq!(report["media_verified"], false);
    assert!(report["verification"].is_null());
    assert_eq!(report["encryption_detected"], false);
    assert_eq!(report["format"], "Ewf1");
    assert_eq!(report["segments"]["count"], 1);
    assert_eq!(report["media"]["logical_bytes"], bytes.len());
    assert_eq!(report["media"]["bytes_per_sector"], 512);
    assert_eq!(report["metadata"]["case_number"], "CASE-123");
    assert_eq!(report["metadata"]["examiner"], "Examiner");
    assert_eq!(report["substituted_sectors"], 2);
    assert!(!report["stored_hashes"]["md5"].is_null());
    assert!(!report.to_string().contains("do-not-print"));
    result(dir.path(), &["verify", "case.E01"], 3);
}

#[test]
fn cli_info_reports_encryption_when_metadata_cannot_be_opened() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let report = result(
        root,
        &["info", "tests/data/xways-encrypted/aes128-compatible.E01"],
        1,
    );
    assert_eq!(report["encryption_detected"], true);
    assert_eq!(report["media_verified"], false);
    assert!(report["metadata"].is_null());
    assert!(report["error"].as_str().unwrap().contains("password"));
}

fn history_records(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(directory.join(".case.E01.ewf-history"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    paths
}

fn export_fixture(directory: &Path, bytes: &[u8], options: ewf_image::WriteOptions) {
    let mut writer = ewf_image::EwfWriter::create(directory.join("case.E01"), options).unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap();
}

fn assert_no_export_temporary(directory: &Path) {
    assert!(fs::read_dir(directory).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".ewf-export-")
    }));
}

#[test]
fn cli_export_streams_formats_split_segments_and_partial_final_chunks() {
    use ewf_image::{WriteCompression as C, WriteFormat as F, WriteOptions};
    let bytes: Vec<_> = (0_u32..3073)
        .flat_map(|v| Sha256::digest(v.to_le_bytes()))
        .collect();
    for (format, compression) in [
        (F::Ewf1Physical, C::None),
        (F::Ewf1Physical, C::Zlib),
        (F::Ewf1Smart, C::None),
        (F::Ewf1Smart, C::Zlib),
        (F::Ewf1Logical, C::Zlib),
        (F::Ewf2Physical, C::None),
        (F::Ewf2Physical, C::Zlib),
        (F::Ewf2Physical, C::Bzip2),
        (F::Ewf2Logical, C::Zlib),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let name = match format {
            F::Ewf1Physical => "case.E01",
            F::Ewf1Smart => "case.S01",
            F::Ewf1Logical => "case.L01",
            F::Ewf2Physical => "case.Ex01",
            F::Ewf2Logical => "case.Lx01",
        };
        let mut writer = ewf_image::EwfWriter::create(
            dir.path().join(name),
            WriteOptions {
                format,
                compression,
                bytes_per_sector: 1,
                sectors_per_chunk: 32768,
                maximum_segment_size: Some(45_000),
                ..WriteOptions::default()
            },
        )
        .unwrap();
        writer.write_all(&bytes).unwrap();
        writer.finish().unwrap();
        let report = result(dir.path(), &["--quiet", "export", name, "case.raw"], 0);
        assert_eq!(report["status"], "exported");
        assert_eq!(report["published"], true);
        assert_eq!(report["exported_bytes"], bytes.len());
        assert_eq!(report["verification"]["references_match"], true);
        assert_eq!(report["verification"]["sha256"], hash(&bytes));
        assert_eq!(fs::read(dir.path().join("case.raw")).unwrap(), bytes);
        assert_no_export_temporary(dir.path());
        assert!(
            result(dir.path(), &["info", name], 0)["segments"]["count"]
                .as_u64()
                .unwrap()
                > 1
        );
    }
}

#[test]
fn cli_export_reports_substitutions_and_handles_chunks_larger_than_write_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = vec![0; 2 * 1024 * 1024 + 512];
    export_fixture(
        dir.path(),
        &bytes,
        ewf_image::WriteOptions {
            sectors_per_chunk: 4096,
            compression: ewf_image::WriteCompression::Zlib,
            acquisition_errors: vec![ewf_image::AcquisitionError {
                first_sector: 2,
                sector_count: 3,
            }],
            ..ewf_image::WriteOptions::default()
        },
    );
    let report = result(dir.path(), &["export", "case.E01", "disk.raw"], 4);
    assert_eq!(report["status"], "exported_with_substitutions");
    assert_eq!(report["substituted_sectors"], 3);
    assert_eq!(report["verification"]["sha256"], hash(&bytes));
    assert_eq!(fs::read(dir.path().join("disk.raw")).unwrap(), bytes);
}

#[test]
fn cli_export_refuses_existing_destinations_aliases_and_control_paths() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    export_fixture(dir.path(), &bytes, ewf_image::WriteOptions::default());
    let original = fs::read(dir.path().join("case.E01")).unwrap();
    fs::hard_link(dir.path().join("case.E01"), dir.path().join("alias.raw")).unwrap();
    fs::create_dir(dir.path().join(".case.E01.ewf-history")).unwrap();
    for output in [
        "case.E01",
        "alias.raw",
        "source.raw",
        ".case.E01.ewf-publication",
        ".case.E01.ewf-session.json",
        ".case.E01.ewf-history/export.raw",
    ] {
        let report = result(dir.path(), &["export", "case.E01", output], 1);
        assert_eq!(report["published"], false);
    }
    assert_eq!(fs::read(dir.path().join("case.E01")).unwrap(), original);
    assert_eq!(fs::read(dir.path().join("source.raw")).unwrap(), bytes);
    assert_no_export_temporary(dir.path());
}

#[test]
fn cli_export_never_publishes_corrupt_media_or_digest_mismatches() {
    for algorithm in ["corrupt", "MD5", "SHA1", "SHA256"] {
        let dir = tempfile::tempdir().unwrap();
        let bytes = source(dir.path());
        let mut options = ewf_image::WriteOptions::default();
        if algorithm != "corrupt" {
            let len = match algorithm {
                "MD5" => 16,
                "SHA1" => 20,
                _ => 32,
            };
            options
                .hashes
                .set_hash_value(algorithm, "00".repeat(len))
                .unwrap();
        }
        export_fixture(dir.path(), &bytes, options);
        if algorithm == "corrupt" {
            let path = dir.path().join("case.E01");
            let mut data = fs::read(&path).unwrap();
            let offset = data.windows(bytes.len()).position(|v| v == bytes).unwrap();
            data[offset] ^= 1;
            fs::write(&path, data).unwrap();
        }
        let report = result(
            dir.path(),
            &["export", "case.E01", "disk.raw"],
            if algorithm == "corrupt" { 1 } else { 3 },
        );
        assert_eq!(report["published"], false);
        assert_eq!(report["media_verified"], false);
        assert!(!dir.path().join("disk.raw").exists());
        assert_no_export_temporary(dir.path());
        if algorithm != "corrupt" {
            assert_eq!(report["verification"]["references_match"], false);
        }
    }
}

#[test]
fn cli_export_distinguishes_absent_references_from_verified_media() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    export_fixture(dir.path(), &bytes, ewf_image::WriteOptions::default());
    let path = dir.path().join("case.E01");
    let image = Image::open(&path).unwrap();
    let mut data = fs::read(&path).unwrap();
    for section in image.sections().iter().filter(|s| matches!(&s.kind, ewf_image::SectionKind::Ewf1(name) if name == "hash" || name == "digest" || name == "xhash")) {
        let offset = section.descriptor_offset as usize;
        data[offset..offset + 16].fill(0);
        data[offset..offset + 7].copy_from_slice(b"unknown");
        data[offset + 72..offset + 76].fill(0);
    }
    drop(image);
    fs::write(path, data).unwrap();
    let report = result(dir.path(), &["export", "case.E01", "disk.raw"], 0);
    assert!(report["verification"]["references_match"].is_null());
    assert_eq!(report["media_verified"], false);
    assert_eq!(report["verification"]["sha256"], hash(&bytes));
    assert_eq!(fs::read(dir.path().join("disk.raw")).unwrap(), bytes);
}

fn running_export(directory: &Path) -> std::process::Child {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_ewf-image"))
        .current_dir(directory)
        .args(["export", "case.E01", "disk.raw"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The first progress event occurs after preflight and before copying.
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    assert!(line.starts_with("export:"), "{line}");
    child.stderr = Some(stderr.into_inner());
    child
}

#[test]
fn cli_export_preserves_a_destination_created_after_preflight() {
    let dir = tempfile::tempdir().unwrap();
    export_fixture(
        dir.path(),
        &vec![0; 32 * 1024 * 1024],
        ewf_image::WriteOptions {
            compression: ewf_image::WriteCompression::Zlib,
            ..ewf_image::WriteOptions::default()
        },
    );
    let child = running_export(dir.path());
    // create_new also makes unexpected early publication fail the test.
    let mut existing = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(dir.path().join("disk.raw"))
        .unwrap();
    existing.write_all(b"unrelated destination").unwrap();
    drop(existing);
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["phase"], "publication");
    assert_eq!(report["published"], false);
    assert_eq!(
        fs::read(dir.path().join("disk.raw")).unwrap(),
        b"unrelated destination"
    );
    assert_no_export_temporary(dir.path());
}

#[cfg(unix)]
#[test]
fn cli_export_cancellation_removes_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    export_fixture(
        dir.path(),
        &vec![0; 32 * 1024 * 1024],
        ewf_image::WriteOptions {
            compression: ewf_image::WriteCompression::Zlib,
            ..ewf_image::WriteOptions::default()
        },
    );
    for signal in ["-INT", "-TERM"] {
        let child = running_export(dir.path());
        assert!(
            Command::new("kill")
                .args([signal, &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(130),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], "cancelled");
        assert_eq!(report["published"], false);
        assert!(report["verification"].is_null());
        assert!(!dir.path().join("disk.raw").exists());
        assert_no_export_temporary(dir.path());
    }
}

#[cfg(unix)]
#[test]
fn cli_export_refuses_dangling_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    export_fixture(dir.path(), &[0; 512], ewf_image::WriteOptions::default());
    std::os::unix::fs::symlink("missing.raw", dir.path().join("link.raw")).unwrap();
    result(dir.path(), &["export", "case.E01", "link.raw"], 1);
    assert!(!dir.path().join("missing.raw").exists());
    assert!(
        fs::symlink_metadata(dir.path().join("link.raw"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn cli_export_refuses_incomplete_and_encrypted_images() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--stop-after",
            "1",
            "--sectors-per-chunk",
            "1",
        ],
        130,
    );
    result(dir.path(), &["export", "case.E01", "disk.raw"], 1);
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/xways-encrypted/aes128-compatible.E01"),
        dir.path().join("encrypted.E01"),
    )
    .unwrap();
    let report = result(dir.path(), &["export", "encrypted.E01", "disk.raw"], 1);
    assert_eq!(report["encryption_detected"], true);
    assert!(!dir.path().join("disk.raw").exists());
    assert_no_export_temporary(dir.path());
}

#[test]
fn cli_history_recovers_unclosed_runs_and_rebuilds_a_stale_report() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--sectors-per-chunk",
            "1",
            "--stop-after",
            "1024",
        ],
        130,
    );
    let saved = dir.path().join(".case.E01.ewf-report.json");
    let previous_report = fs::read(&saved).unwrap();
    let records = history_records(dir.path());
    let last = records.last().unwrap();
    let record: Value = serde_json::from_slice(&fs::read(last).unwrap()).unwrap();
    assert_eq!(record["event"], "run_end");
    // Model death immediately before the closing record becomes visible, with
    // a partial unpublished temporary record left behind.
    fs::remove_file(last).unwrap();
    fs::write(
        dir.path()
            .join(".case.E01.ewf-history/.pending-history-crash"),
        b"{\"schema_version\":",
    )
    .unwrap();
    let read = result(dir.path(), &["report", "case.E01"], 0);
    assert_eq!(read["history"]["latest_run"]["status"], "interrupted");
    assert_eq!(read["history"]["pending_records"], 1);
    assert_eq!(read["history"]["counters_complete"], false);
    assert_eq!(fs::read(&saved).unwrap(), previous_report);
    result(dir.path(), &["report", "case.E01", "--write"], 0);
    assert_ne!(fs::read(&saved).unwrap(), previous_report);
    let completed = result(dir.path(), &["resume", "case.E01", "--retries", "0"], 0);
    assert_eq!(completed["verification"]["sha256"], hash(&bytes));
    fs::remove_file(dir.path().join("source.raw")).unwrap();
    let summary = result(dir.path(), &["report", "case.E01"], 0);
    assert_eq!(summary["history"]["runs"][0]["status"], "interrupted");
    assert_eq!(
        summary["history"]["runs"][1]["start"]["read_policy"]["retries"],
        0
    );
    assert_eq!(summary["history"]["latest_run"]["status"], "complete");
}

#[test]
fn cli_rejects_truncated_committed_history_without_changing_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--sectors-per-chunk",
            "1",
            "--stop-after",
            "1024",
        ],
        130,
    );
    let before = result(dir.path(), &["checkpoint", "validate", "case.E01"], 0);
    let records = history_records(dir.path());
    let path = records.last().unwrap();
    let mut damaged = fs::read(path).unwrap();
    damaged.pop();
    fs::write(path, &damaged).unwrap();
    let failed = result(dir.path(), &["resume", "case.E01"], 1);
    assert!(failed["error"].as_str().unwrap().contains("truncated"));
    result(dir.path(), &["report", "case.E01", "--write"], 1);
    assert_eq!(fs::read(path).unwrap(), damaged);
    assert_eq!(history_records(dir.path()), records);
    let after = result(dir.path(), &["checkpoint", "validate", "case.E01"], 0);
    assert_eq!(before["checkpoint"], after["checkpoint"]);
}

#[test]
fn cli_report_publication_failure_preserves_image_and_committed_result() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    let saved = dir.path().join(".case.E01.ewf-report.json");
    fs::create_dir(&saved).unwrap();
    let failed = result(dir.path(), &["acquire", "source.raw", "case.E01"], 1);
    assert_eq!(failed["status"], "reporting_failed");
    assert_eq!(failed["acquisition_status"], "complete");
    assert_eq!(failed["published"], true);
    assert_eq!(failed["verification"]["sha256"], hash(&bytes));
    let records = history_records(dir.path());
    let recorded = result(dir.path(), &["report", "case.E01"], 0);
    assert_eq!(recorded["history"]["latest_run"]["status"], "complete");
    fs::remove_dir(&saved).unwrap();
    result(dir.path(), &["report", "case.E01", "--write"], 0);
    assert_eq!(history_records(dir.path()), records);
    result(dir.path(), &["verify", "case.E01"], 0);
    // An unrelated existing file is never truncated/replaced.
    fs::write(&saved, b"unrelated").unwrap();
    result(dir.path(), &["report", "case.E01", "--write"], 1);
    assert_eq!(fs::read(&saved).unwrap(), b"unrelated");
}

#[test]
fn cli_acquires_reopens_and_verifies_raw_and_zlib() {
    for compression in ["raw", "zlib"] {
        let dir = tempfile::tempdir().unwrap();
        let bytes = source(dir.path());
        let report = result(
            dir.path(),
            &[
                "--quiet",
                "acquire",
                "source.raw",
                "case.E01",
                "--compression",
                compression,
                "--chunks-per-segment",
                "3",
                "--sectors-per-chunk",
                "2",
                "--case-number",
                "case 123",
                "--read-timeout-ms",
                "10000",
            ],
            0,
        );
        assert_eq!(report["status"], "complete");
        assert_eq!(report["read_policy"]["read_timeout_ms"], 10000);
        assert_eq!(report["published"], true);
        assert_eq!(report["verification"]["references_match"], true);
        assert_eq!(report["verification"]["sha256"], hash(&bytes));
        let image = Image::open(dir.path().join("case.E01")).unwrap();
        assert_eq!(
            image.header_value("case_number").as_deref(),
            Some("case 123")
        );
        let mut actual = Vec::new();
        image.cursor().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        let verified = result(dir.path(), &["verify", "case.E01"], 0);
        assert_eq!(verified["status"], "verified");
        // Resume never mistakes any existing file for a session it can continue.
        result(dir.path(), &["resume", "case.E01"], 1);
    }
}

#[test]
fn cli_repeated_pause_inspect_validate_resume_preserves_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    let paused = result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--sectors-per-chunk",
            "2",
            "--chunks-per-segment",
            "4",
            "--stop-after",
            "3072",
        ],
        130,
    );
    assert_eq!(paused["status"], "cancelled");
    assert_eq!(paused["checkpoint_bytes"], 3072);
    assert!(!dir.path().join("case.E01").exists());
    let inspected = result(dir.path(), &["checkpoint", "inspect", "case.E01"], 0);
    assert_eq!(inspected["checkpoint"]["segment_hashes_validated"], false);
    let validated = result(dir.path(), &["checkpoint", "validate", "case.E01"], 0);
    assert_eq!(validated["checkpoint"]["segment_hashes_validated"], true);
    let paused_again = result(
        dir.path(),
        &[
            "resume",
            "case.E01",
            "--stop-after",
            "9216",
            "--read-timeout-ms",
            "10000",
        ],
        130,
    );
    assert_eq!(paused_again["checkpoint_bytes"], 9216);
    assert_eq!(paused_again["read_policy"]["read_timeout_ms"], 10000);
    let done = result(dir.path(), &["resume", "case.E01"], 0);
    assert_eq!(done["accepted_bytes"], bytes.len());
    assert_eq!(done["verification"]["sha256"], hash(&bytes));
    let summary = result(dir.path(), &["report", "case.E01"], 0);
    let history = &summary["history"];
    assert_eq!(history["runs"].as_array().unwrap().len(), 3);
    assert_eq!(history["runs"][0]["status"], "cancelled");
    assert_eq!(
        history["runs"][1]["start"]["read_policy"]["read_timeout_ms"],
        10000
    );
    assert!(history["runs"][2]["start"]["read_policy"]["read_timeout_ms"].is_null());
    assert_eq!(
        history["latest_run"]["result"]["verification"]["sha256"],
        hash(&bytes)
    );
    assert_eq!(history["counters_complete"], true);
    let saved: Value =
        serde_json::from_slice(&fs::read(dir.path().join(".case.E01.ewf-report.json")).unwrap())
            .unwrap();
    assert_eq!(history, &saved);
}

#[test]
fn cli_rejects_changed_source_before_modifying_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--sectors-per-chunk",
            "2",
            "--stop-after",
            "2048",
        ],
        130,
    );
    let journal = dir.path().join(".case.E01.ewf-acquisition");
    let segment = fs::read(journal.join("case.E01")).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("source.raw"))
        .unwrap()
        .set_len(65536)
        .unwrap();
    let report = result(dir.path(), &["resume", "case.E01"], 1);
    assert!(report["error"].as_str().unwrap().contains("identity"));
    assert_eq!(fs::read(journal.join("case.E01")).unwrap(), segment);
    result(dir.path(), &["checkpoint", "validate", "case.E01"], 0);
}

#[test]
fn cli_checkpoint_inspection_does_not_need_source() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--stop-after",
            "1",
            "--sectors-per-chunk",
            "2",
        ],
        130,
    );
    fs::remove_file(dir.path().join("source.raw")).unwrap();
    result(dir.path(), &["checkpoint", "validate", "case.E01"], 0);
    result(dir.path(), &["resume", "case.E01"], 1);
}

#[test]
fn cli_corrupt_output_is_verification_failure() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    result(dir.path(), &["acquire", "source.raw", "case.E01"], 0);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("case.E01"))
        .unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"corrupt!").unwrap();
    let report = result(dir.path(), &["verify", "case.E01"], 3);
    assert_eq!(report["status"], "verification_failed");
}

#[test]
fn cli_preserves_existing_output_and_manifest() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    fs::write(dir.path().join("case.E01"), b"existing evidence").unwrap();
    result(dir.path(), &["acquire", "source.raw", "case.E01"], 1);
    assert_eq!(
        fs::read(dir.path().join("case.E01")).unwrap(),
        b"existing evidence"
    );
    let manifest = fs::read(dir.path().join(".case.E01.ewf-session.json")).unwrap();
    result(dir.path(), &["acquire", "source.raw", "case.E01"], 1);
    assert_eq!(
        fs::read(dir.path().join(".case.E01.ewf-session.json")).unwrap(),
        manifest
    );
}

#[test]
fn cli_refuses_output_source_aliases_and_invalid_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    fs::rename(dir.path().join("source.raw"), dir.path().join("source.E01")).unwrap();
    result(dir.path(), &["acquire", "source.E01", "source.E01"], 1);
    assert_eq!(fs::read(dir.path().join("source.E01")).unwrap(), bytes);
    assert_eq!(
        cli(
            dir.path(),
            &["acquire", "source.E01", "case.E01", "--retries", "101"]
        )
        .status
        .code(),
        Some(2)
    );
    assert!(!dir.path().join(".case.E01.ewf-session.json").exists());
    for name in [
        ".case.E01.ewf-report.json",
        ".case.E01.ewf-history/source.raw",
    ] {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &bytes).unwrap();
        result(dir.path(), &["acquire", name, "case.E01"], 1);
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert!(!dir.path().join(".case.E01.ewf-session.json").exists());
    }
}

#[test]
fn cli_resume_without_older_history_marks_the_gap() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = source(dir.path());
    result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--sectors-per-chunk",
            "1",
            "--stop-after",
            "1024",
        ],
        130,
    );
    // An earlier binary's checkpoint and manifest have no history directory.
    fs::rename(
        dir.path().join(".case.E01.ewf-history"),
        dir.path().join("retained-history"),
    )
    .unwrap();
    result(dir.path(), &["resume", "case.E01"], 0);
    let summary = result(dir.path(), &["report", "case.E01"], 0);
    assert_eq!(summary["history"]["prior_history_unavailable"], true);
    assert_eq!(summary["history"]["counters_complete"], false);
    assert_eq!(summary["history"]["runs"].as_array().unwrap().len(), 1);
    assert_eq!(
        summary["history"]["latest_run"]["result"]["verification"]["sha256"],
        hash(&bytes)
    );
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn cli_reports_verified_substitutions_with_distinct_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let options = ewf_image::WriteOptions {
        acquisition_errors: vec![ewf_image::AcquisitionError {
            first_sector: 1,
            sector_count: 2,
        }],
        ..ewf_image::WriteOptions::default()
    };
    let mut writer = ewf_image::EwfWriter::create(dir.path().join("case.E01"), options).unwrap();
    writer.write_all(&[0; 4096]).unwrap();
    writer.finish().unwrap();
    let report = result(dir.path(), &["verify", "case.E01"], 4);
    assert_eq!(report["status"], "verified_with_substitutions");
    assert_eq!(report["substituted_sectors"], 2);
    assert_eq!(report["verification"]["references_match"], true);
}

#[test]
fn cli_refuses_active_session_lock() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    let lock = fs::File::create(dir.path().join(".case.E01.ewf-cli.lock")).unwrap();
    lock.lock().unwrap();
    result(dir.path(), &["acquire", "source.raw", "case.E01"], 1);
    assert!(!dir.path().join(".case.E01.ewf-session.json").exists());
}

#[test]
fn cli_rejects_changed_session_options_and_damaged_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    result(
        dir.path(),
        &[
            "acquire",
            "source.raw",
            "case.E01",
            "--sectors-per-chunk",
            "2",
            "--stop-after",
            "2048",
        ],
        130,
    );
    let manifest = dir.path().join(".case.E01.ewf-session.json");
    let original = fs::read(&manifest).unwrap();
    let mut changed: Value = serde_json::from_slice(&original).unwrap();
    changed["compression"] = serde_json::json!("raw");
    fs::write(&manifest, serde_json::to_vec(&changed).unwrap()).unwrap();
    result(dir.path(), &["resume", "case.E01"], 1);
    fs::write(&manifest, original).unwrap();
    let sealed = dir.path().join(".case.E01.ewf-acquisition/case.E01");
    let mut file = fs::OpenOptions::new().write(true).open(sealed).unwrap();
    file.write_all(b"damaged!").unwrap();
    result(dir.path(), &["checkpoint", "validate", "case.E01"], 1);
    result(dir.path(), &["resume", "case.E01"], 1);
    assert!(!dir.path().join("case.E01").exists());
}

#[cfg(unix)]
#[test]
fn cli_handles_real_interrupt_and_termination_signals() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    for signal in ["-INT", "-TERM"] {
        let dir = tempfile::tempdir().unwrap();
        // Sparse synthetic input, never a physical device. Wait for the initial
        // acquisition event so the signal handler and journal are both ready.
        fs::File::create(dir.path().join("source.raw"))
            .unwrap()
            .set_len(512 * 1024 * 1024)
            .unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_ewf-image"))
            .current_dir(dir.path())
            .args([
                "acquire",
                "source.raw",
                "case.E01",
                "--chunks-per-segment",
                "64",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut line = String::new();
        stderr.read_line(&mut line).unwrap();
        assert!(line.starts_with("acquisition:"), "{line}");
        assert!(
            Command::new("kill")
                .args([signal, &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(130));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["status"], "cancelled");
        result(dir.path(), &["checkpoint", "validate", "case.E01"], 0);
        // Resume and pause again proves the OS-signal checkpoint is usable.
        let next = report["checkpoint_bytes"].as_u64().unwrap() + 32768;
        result(
            dir.path(),
            &["resume", "case.E01", "--stop-after", &next.to_string()],
            130,
        );
    }
}

#[test]
#[ignore = "requires pinned EWFEXPORT and EWFVERIFY tools"]
fn external_cli_resumed_acquisition_matches_libewf() {
    let export = std::env::var_os("EWFEXPORT").expect("set EWFEXPORT to pinned ewfexport");
    let verify = std::env::var_os("EWFVERIFY").expect("set EWFVERIFY to pinned ewfverify");
    for (compression, sector_size) in [
        ("raw", "512"),
        ("zlib", "512"),
        ("raw", "4096"),
        ("zlib", "4096"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        // Include incompressible full chunks, compressible chunks and a short
        // final chunk. Repetitive fixtures alone miss expanding zlib frames.
        let mut bytes = Vec::new();
        for counter in 0_u32..8192 {
            bytes.extend_from_slice(&Sha256::digest(counter.to_le_bytes()));
        }
        bytes.resize(524_288, 0);
        bytes.extend_from_within(..4096);
        fs::write(dir.path().join("source.raw"), &bytes).unwrap();
        result(
            dir.path(),
            &[
                "acquire",
                "source.raw",
                "case.E01",
                "--compression",
                compression,
                "--sector-size",
                sector_size,
                "--sectors-per-chunk",
                "64",
                "--chunks-per-segment",
                "3",
                "--stop-after",
                "4096",
            ],
            130,
        );
        result(dir.path(), &["resume", "case.E01"], 0);
        let output = Command::new(&export)
            .current_dir(dir.path())
            .args(["-q", "-f", "raw", "-t", "-", "-u", "case.E01"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, bytes);
        let output = Command::new(&verify)
            .current_dir(dir.path())
            .args(["-q", "case.E01"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
