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
