//! Local acquisition, source consistency, and logical metadata contracts.
use aff4_image::{
    CollectionOptions, Container, LogicalMetadata, Profile, SubstreamKind, WriteOptions, Writer,
};
use std::{fs, io::Cursor, ops::ControlFlow};

#[test]
fn rich_metadata_preserves_raw_names_timestamps_hierarchy_and_substreams() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.aff4");
    let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
    let folder = writer
        .add_folder(&LogicalMetadata {
            path: b"root".to_vec(),
            ..Default::default()
        })
        .unwrap();
    let file = writer
        .add_file_with_metadata(
            &LogicalMetadata {
                path: b"root/odd\xff\n".to_vec(),
                parent: Some(folder.clone()),
                modified: Some(1_234_567_890),
                mode: Some(0o100640),
                ..Default::default()
            },
            3,
            &mut Cursor::new(b"abc"),
            |_, _| ControlFlow::Continue(()),
        )
        .unwrap();
    let sub = writer
        .add_substream(
            &file,
            b"Zone.Identifier",
            SubstreamKind::AlternateDataStream,
            b"zone",
        )
        .unwrap();
    let written = writer.finish().unwrap();
    let mut image = Container::open(path).unwrap();
    let props = &image.metadata()[&file];
    assert!(props.iter().any(|p| p.predicate.ends_with("#originalPathNameRaw") && p.value == "cm9vdC9vZGT/Cg=="));
    assert!(props.iter().any(
        |p| p.predicate.ends_with("#lastWritten") && p.value == "1970-01-01T00:00:01.23456789Z"
    ));
    assert!(
        image.metadata()[&folder]
            .iter()
            .any(|p| p.predicate.ends_with("#child") && p.value == file)
    );
    let mut bytes = [0; 4];
    image.read_at(&sub, &mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"zone");
    let report = image
        .verify_all(Some(&written.metadata_sha256), |_, _, _| {
            ControlFlow::Continue(())
        })
        .unwrap();
    assert!(report.all_match(), "{report:#?}");
}

#[test]
fn collector_records_empty_folders_exclusions_and_detects_source_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    fs::write(root.join("file"), b"abc").unwrap();
    fs::write(root.join("skip"), b"omit").unwrap();
    let path = dir.path().join("case.aff4");
    let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
    let report = writer
        .add_directory_tree(
            &root,
            &CollectionOptions {
                exclude: vec!["skip".into()],
                ..Default::default()
            },
            |_, _, _| ControlFlow::Continue(()),
        )
        .unwrap();
    assert_eq!(
        (
            report.files,
            report.folders,
            report.bytes,
            report.issues.len()
        ),
        (1, 2, 3, 1)
    );
    writer.finish().unwrap();
    assert!(
        Container::open(path)
            .unwrap()
            .verify_all(None, |_, _, _| ControlFlow::Continue(()))
            .unwrap()
            .all_match()
    );
    let path = dir.path().join("changed.aff4");
    let mut writer = Writer::create(&path, Profile::Logical, WriteOptions::default()).unwrap();
    let mut changed = false;
    let result =
        writer.add_directory_tree(&root, &CollectionOptions::default(), |path, done, total| {
            if !changed && total == 3 && done == total {
                fs::write(path, b"changed").unwrap();
                changed = true;
            }
            ControlFlow::Continue(())
        });
    assert!(result.is_err());
    assert!(writer.finish().is_err());
    assert!(!path.exists());
}

#[test]
fn cli_collect_verify_and_extract_do_not_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), b"abc").unwrap();
    let output = dir.path().join("case.aff4");
    let collected = std::process::Command::new(env!("CARGO_BIN_EXE_aff4-image"))
        .arg("collect")
        .arg(&root)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        collected.status.success(),
        "{} {}",
        String::from_utf8_lossy(&collected.stdout),
        String::from_utf8_lossy(&collected.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&collected.stdout).unwrap();
    let id = json["output"]["streams"][0]["id"].as_str().unwrap();
    let destination = dir.path().join("recovered");
    let extract = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_aff4-image"))
            .arg("extract")
            .arg(&output)
            .arg(id)
            .arg(&destination)
            .output()
            .unwrap()
    };
    assert!(extract().status.success());
    assert_eq!(fs::read(&destination).unwrap(), b"abc");
    fs::write(&destination, b"keep").unwrap();
    assert!(!extract().status.success());
    assert_eq!(fs::read(destination).unwrap(), b"keep");
    let verify = std::process::Command::new(env!("CARGO_BIN_EXE_aff4-image"))
        .arg("verify")
        .arg(output)
        .arg("--expected-metadata-sha256")
        .arg("0".repeat(64))
        .output()
        .unwrap();
    assert_eq!(verify.status.code(), Some(4));
}

#[cfg(unix)]
#[test]
fn collector_never_follows_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir(&root).unwrap();
    fs::write(dir.path().join("outside"), b"outside").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside"), root.join("link")).unwrap();
    let mut writer = Writer::create(
        dir.path().join("strict.aff4"),
        Profile::Logical,
        WriteOptions::default(),
    )
    .unwrap();
    assert!(
        writer
            .add_directory_tree(&root, &CollectionOptions::default(), |_, _, _| {
                ControlFlow::Continue(())
            })
            .is_err()
    );
    assert!(writer.finish().is_err());
    let mut writer = Writer::create(
        dir.path().join("partial.aff4"),
        Profile::Logical,
        WriteOptions::default(),
    )
    .unwrap();
    let report = writer
        .add_directory_tree(
            &root,
            &CollectionOptions {
                allow_partial: true,
                ..Default::default()
            },
            |_, _, _| ControlFlow::Continue(()),
        )
        .unwrap();
    assert_eq!((report.files, report.issues.len()), (0, 1));
    writer.finish().unwrap();
}
