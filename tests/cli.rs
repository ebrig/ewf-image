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
            ],
            0,
        );
        assert_eq!(report["status"], "complete");
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
        &["resume", "case.E01", "--stop-after", "9216"],
        130,
    );
    assert_eq!(paused_again["checkpoint_bytes"], 9216);
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
