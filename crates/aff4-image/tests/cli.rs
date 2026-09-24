//! Human and machine output, resource policy, and exclusive publication.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn cli(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aff4-image"))
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
        text.contains("Published: yes") && text.contains("Verification: passed"),
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
    assert!(report["resources"].is_array());
    assert!(
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["outcome"] == "Match")
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
        text.contains("Published: yes") && text.contains("Omitted entries: 1"),
        "{text}"
    );
    let verified = cli(
        root.path(),
        &["verify", "case.aff4", "--metadata-sha256", &"0".repeat(64)],
    );
    assert_eq!(verified.status.code(), Some(4));
    let text = stdout(&verified);
    assert!(
        text.contains("Verification: failed or incomplete") && text.contains("mismatched"),
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
    assert!(stdout(&verified).contains("Verification: passed"));
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
        assert!(result.stdout.is_empty());
        if args[0] == "--json" {
            let error: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
            assert!(error["error"].is_string());
        } else {
            assert!(String::from_utf8_lossy(&result.stderr).starts_with("Error:"));
        }
    }
}
