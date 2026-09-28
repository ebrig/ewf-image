//! Human and machine output, resource policy, and exclusive publication.
use std::{
    fs::{self, File, FileTimes},
    path::Path,
    process::{Command, Output},
    time::{Duration, UNIX_EPOCH},
};

fn cli(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ewf-cli"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn collection_and_extraction_have_concise_text_and_explicit_json() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("source")).unwrap();
    let content = b"independently known bytes";
    fs::write(root.path().join("source/file"), content).unwrap();
    let collected = cli(root.path(), &["collect", "source", "case.aff4"]);
    assert!(collected.status.success(), "{collected:?}");
    let text = stdout(&collected);
    assert!(
        text.contains("Published: yes") && text.contains("Files collected: 1"),
        "{text}"
    );
    assert!(text.lines().count() < 12, "{text}");
    assert!(serde_json::from_slice::<serde_json::Value>(&collected.stdout).is_err());
    let info = cli(root.path(), &["info", "case.aff4", "--json"]);
    assert!(info.status.success(), "{info:?}");
    let report: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    let id = report["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["size"] == content.len())
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let extracted = cli(root.path(), &["extract", "case.aff4", id, "file.bin"]);
    assert!(extracted.status.success(), "{extracted:?}");
    assert!(stdout(&extracted).contains("Reference hashes: match"));
    assert_eq!(fs::read(root.path().join("file.bin")).unwrap(), content);
    let collision = cli(root.path(), &["collect", "source", "case.aff4"]);
    assert!(!collision.status.success());
    let verified = cli(root.path(), &["--json", "verify", "case.aff4"]);
    assert!(verified.status.success(), "{verified:?}");
    let report: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert!(report["verification"]["resources"].is_array());
    assert!(
        report["verification"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["outcome"] == "Match")
    );
}

#[test]
fn extraction_restores_recorded_aff4_file_times_when_requested() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("source")).unwrap();
    let source = root.path().join("source/file.txt");
    fs::write(&source, b"known content").unwrap();
    let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(
            FileTimes::new()
                .set_modified(modified)
                .set_accessed(modified),
        )
        .unwrap();
    let collected = cli(root.path(), &["collect", "source", "case.aff4"]);
    assert!(collected.status.success(), "{collected:?}");
    let listed = cli(root.path(), &["info", "case.aff4", "--json"]);
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let id = listed["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["size"] == 13)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let extracted = cli(
        root.path(),
        &[
            "--json",
            "extract",
            "case.aff4",
            id,
            "restored.txt",
            "--restore-times",
        ],
    );
    assert!(extracted.status.success(), "{extracted:?}");
    let report: serde_json::Value = serde_json::from_slice(&extracted.stdout).unwrap();
    assert_eq!(
        report["restored_times"],
        serde_json::json!(["accessed", "modified"])
    );
    assert_eq!(
        fs::metadata(root.path().join("restored.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
}

#[test]
fn omissions_and_mismatches_remain_visible_in_text() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("source")).unwrap();
    fs::write(root.path().join("source/keep"), b"known").unwrap();
    fs::write(root.path().join("source/skip"), b"excluded").unwrap();
    let collected = cli(
        root.path(),
        &["collect", "source", "case.aff4", "--exclude", "skip"],
    );
    assert_eq!(collected.status.code(), Some(4));
    let text = stdout(&collected);
    assert!(
        text.contains("Published: yes") && text.contains("omitted entries: 1"),
        "{text}"
    );
    let verified = cli(
        root.path(),
        &["verify", "case.aff4", "--metadata-sha256", &"0".repeat(64)],
    );
    assert_eq!(verified.status.code(), Some(3));
    let text = stdout(&verified);
    assert!(
        text.contains("Status: verification failed") && text.contains("Check"),
        "{text}"
    );
}

#[test]
fn cli_reads_metadata_above_library_default_without_resource_flags() {
    use aff4_image::{CaseMetadata, Container, Limits, Profile, WriteOptions, Writer};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large.aff4");
    let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
    for _ in 0..3 {
        writer
            .add_case_metadata(&CaseMetadata {
                notes: "n".repeat(Limits::default().metadata_bytes as usize / 2),
                ..Default::default()
            })
            .unwrap();
    }
    writer.finish().unwrap();
    assert!(Container::open(&path).is_err());
    let verified = cli(root.path(), &["verify", "large.aff4"]);
    assert!(verified.status.success(), "{verified:?}");
    assert!(stdout(&verified).contains("Status: verified"));
}

#[test]
fn help_and_errors_are_consistent_and_resource_flags_are_removed() {
    let root = tempfile::tempdir().unwrap();
    for command in [
        None,
        Some("info"),
        Some("metadata"),
        Some("verify"),
        Some("verify-set"),
        Some("collect"),
        Some("extract"),
    ] {
        let mut args = command.into_iter().collect::<Vec<_>>();
        args.push("--help");
        let result = cli(root.path(), &args);
        assert!(result.status.success(), "{result:?}");
        let help = stdout(&result);
        assert!(help.contains("--json"));
        assert!(
            !help.contains("--limit-") && !help.contains("--memory-limit"),
            "{help}"
        );
    }
    assert!(cli(root.path(), &["--version"]).status.success());
    let removed = cli(
        root.path(),
        &["info", "missing.aff4", "--limit-triples", "1"],
    );
    assert_eq!(removed.status.code(), Some(2));
    for args in [
        &["info", "missing.aff4"][..],
        &["--json", "info", "missing.aff4"][..],
    ] {
        let result = cli(root.path(), args);
        assert_eq!(result.status.code(), Some(1));
        if args[0] == "--json" {
            let error: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
            assert!(error["error"].is_string());
        } else {
            assert!(stdout(&result).starts_with("Status: failed"));
        }
    }
}

#[test]
fn collect_empty_directory_and_refuse_extract_overwrite() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("empty")).unwrap();
    let collected = cli(root.path(), &["--json", "collect", "empty", "empty.aff4"]);
    assert!(collected.status.success(), "{collected:?}");
    let report: serde_json::Value = serde_json::from_slice(&collected.stdout).unwrap();
    assert_eq!(report["published"], true);
    assert_eq!(report["collection"]["files"], 0);
    assert_eq!(report["collection"]["folders"], 1);
    assert_eq!(report["verification"]["resources"], serde_json::json!([]));

    fs::create_dir(root.path().join("source")).unwrap();
    fs::write(root.path().join("source/file"), b"abc").unwrap();
    let collected = cli(root.path(), &["--json", "collect", "source", "case.aff4"]);
    assert!(collected.status.success(), "{collected:?}");
    let info = cli(root.path(), &["--json", "files", "case.aff4"]);
    assert!(info.status.success(), "{info:?}");
    let report: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    let id = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["size"] == 3)
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let extract = || {
        cli(
            root.path(),
            &["--json", "extract", "case.aff4", id, "copy.bin"],
        )
    };
    assert!(extract().status.success());
    assert_eq!(fs::read(root.path().join("copy.bin")).unwrap(), b"abc");
    fs::write(root.path().join("copy.bin"), b"keep").unwrap();
    assert!(!extract().status.success());
    assert_eq!(fs::read(root.path().join("copy.bin")).unwrap(), b"keep");
}

#[test]
fn verify_set_distinguishes_missing_matching_and_wrong_external_hashes() {
    use aff4_image::{Profile, WriteOptions, Writer};
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("disk.aff4");
    let bytes = b"known disk bytes";
    let mut writer = Writer::create(&path, Profile::Physical, WriteOptions::default()).unwrap();
    let image = writer
        .add_image(bytes.len() as u64, &mut bytes.as_slice(), |_, _| {
            std::ops::ControlFlow::Continue(())
        })
        .unwrap();
    writer.finish().unwrap();
    let digest = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    for (expected, code) in [
        (None, 4),
        (Some(digest.as_str()), 0),
        (
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
            3,
        ),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ewf-cli"));
        command.args(["--json", "verify-set"]);
        command.arg(&path).args(["--image", &image]);
        if let Some(expected) = expected {
            command.args(["--sha256", expected]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["verification"]["bytes"], bytes.len());
    }
}
