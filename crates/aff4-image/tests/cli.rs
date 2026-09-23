//! Collection publication and explicit reader-budget contracts.
use std::{fs, process::Command};

fn collect(
    source: &std::path::Path,
    output: &std::path::Path,
    limits: &[&str],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aff4-image"))
        .arg("collect")
        .arg(source)
        .arg(output)
        .args(limits)
        .output()
        .unwrap()
}

#[test]
fn collection_limit_failure_never_publishes_and_explicit_budget_allows_retry() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), b"independently known bytes").unwrap();
    let output = root.path().join("case.aff4");
    for limits in [
        ["--limit-metadata-bytes", "1"],
        ["--limit-triples", "1"],
        ["--limit-archive-entries", "1"],
        ["--limit-directory-bytes", "1"],
        ["--limit-verification-bytes", "1"],
        ["--limit-collection-entries", "1"],
        ["--limit-collection-depth", "0"],
    ] {
        let result = collect(&source, &output, &limits);
        assert_eq!(result.status.code(), Some(4), "{result:?}");
        let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["published"], false);
        if limits[0] != "--limit-directory-bytes" {
            assert_eq!(report["phase"], "collection");
            assert!(report["limit_error"]["resource"].is_string());
            assert!(
                report["limit_error"]["required"].as_u64().unwrap()
                    > report["limit_error"]["limit"].as_u64().unwrap()
            );
        }
        assert!(!output.exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
    let limits = [
        "--limit-metadata-bytes",
        "1048576",
        "--limit-triples",
        "1000",
    ];
    let result = collect(&source, &output, &limits);
    assert!(result.status.success(), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["published"], true);
    assert_eq!(report["limits"]["metadata_bytes"], 1048576);
    assert_eq!(report["verification_scope"], "finalized staged container");
    let mut container = aff4_image::Container::open(&output).unwrap();
    let id = report["output"]["streams"][0]["id"].as_str().unwrap();
    let mut bytes = vec![0; b"independently known bytes".len()];
    assert_eq!(container.read_at(id, &mut bytes, 0).unwrap(), bytes.len());
    assert_eq!(bytes, b"independently known bytes");
    let verification = Command::new(env!("CARGO_BIN_EXE_aff4-image"))
        .arg("verify")
        .arg(&output)
        .args(limits)
        .output()
        .unwrap();
    assert!(verification.status.success(), "{verification:?}");
}

#[test]
fn staged_verification_failure_reports_resource_checks_without_publication() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::File::create(source.join("chunked"))
        .unwrap()
        .set_len(2 * 1024 * 1024)
        .unwrap();
    let output = root.path().join("case.aff4");
    // The initial linear traversal fits; subsequent block traversal exhausts
    // the shared verification budget, after collection itself has succeeded.
    let result = collect(&source, &output, &["--limit-verification-bytes", "2097152"]);
    assert_eq!(result.status.code(), Some(4), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["published"], false);
    assert_eq!(report["phase"], "finalize and verify before publication");
    assert!(
        report["verification"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["detail"]
                .as_str()
                .is_some_and(|text| text.contains("limit")))
    );
    assert!(!output.exists());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
