use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Output},
};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ewf-cli"))
        .args(["--quiet", "--json"])
        .args(args)
        .output()
        .unwrap()
}
fn path(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", output))
}
fn succeeds(args: &[&str]) -> Value {
    let out = run(args);
    assert!(matches!(out.status.code(), Some(0 | 4)), "{out:?}");
    let value = report(&out);
    assert_eq!(value["published"], true, "{value}");
    value
}
fn data() -> Vec<u8> {
    (0..131072)
        .map(|n| ((n * 13 + n / 491) % 256) as u8)
        .collect()
}
fn read_image(file: &Path) -> Vec<u8> {
    let mut result = Vec::new();
    match file.extension().unwrap().to_str().unwrap() {
        "E01" | "Ex01" => {
            ewf_image::Image::open(file)
                .unwrap()
                .cursor()
                .read_to_end(&mut result)
                .unwrap();
        }
        "aff4" => {
            aff4_image::Container::open(file)
                .unwrap()
                .into_disk_reader(None)
                .unwrap()
                .read_to_end(&mut result)
                .unwrap();
        }
        _ => result = fs::read(file).unwrap(),
    }
    result
}

#[test]
fn every_physical_format_can_be_acquired_and_converted() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("source.raw");
    let bytes = data();
    fs::write(&raw, &bytes).unwrap();
    for from in ["E01", "Ex01", "aff4", "raw"] {
        let input = dir.path().join(format!("acquired.{from}"));
        succeeds(&["acquire", path(&raw), path(&input), "--sector-size", "4096"]);
        assert_eq!(read_image(&input), bytes);
        for to in ["E01", "Ex01", "aff4", "raw"] {
            let output = dir.path().join(format!("{from}-converted.{to}"));
            let v = succeeds(&[
                "convert",
                path(&input),
                path(&output),
                "--sector-size",
                "4096",
            ]);
            assert_eq!(v["destination_matches_source"], true, "{v}");
            assert_eq!(read_image(&output), bytes);
            if to == "aff4" {
                let c = aff4_image::Container::open(&output).unwrap();
                assert_eq!(c.disk_images().unwrap()[0].block_size, Some(4096));
            } else if to != "raw" {
                assert_eq!(
                    ewf_image::Image::open(&output)
                        .unwrap()
                        .info()
                        .media
                        .bytes_per_sector,
                    Some(4096)
                );
            }
        }
    }
}

#[test]
fn rejects_aliases_geometry_conflicts_and_unknown_formats_without_publication() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("source.raw");
    fs::write(&raw, data()).unwrap();
    let original = fs::read(&raw).unwrap();
    assert!(!run(&["convert", path(&raw), path(&raw)]).status.success());
    assert_eq!(fs::read(&raw).unwrap(), original);
    let ewf = dir.path().join("disk.Ex01");
    succeeds(&["acquire", path(&raw), path(&ewf), "--sector-size", "4096"]);
    let out = dir.path().join("bad.aff4");
    assert!(
        !run(&["convert", path(&ewf), path(&out), "--sector-size", "512"])
            .status
            .success()
    );
    assert!(!out.exists());
    let unknown = dir.path().join("disk.xyz");
    assert!(
        !run(&["acquire", path(&raw), path(&unknown)])
            .status
            .success()
    );
    assert!(!unknown.exists());
}

#[test]
fn signatures_override_input_extension_and_hash_mismatches_are_failures() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("source.raw");
    fs::write(&raw, data()).unwrap();
    let encoded = dir.path().join("disk.Ex01");
    succeeds(&["acquire", path(&raw), path(&encoded)]);
    let disguised = dir.path().join("disguised.raw");
    fs::copy(&encoded, &disguised).unwrap();
    let decoded = dir.path().join("decoded.raw");
    succeeds(&["convert", path(&disguised), path(&decoded)]);
    assert_eq!(fs::read(decoded).unwrap(), data());
    let expected: String = Sha256::digest(data())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    for extension in ["raw", "Ex01", "aff4"] {
        let file = if extension == "raw" {
            raw.clone()
        } else {
            let file = dir.path().join(format!("verify.{extension}"));
            succeeds(&["acquire", path(&raw), path(&file)]);
            file
        };
        let good = run(&["verify", path(&file), "--sha256", &expected]);
        assert!(good.status.success(), "{good:?}");
        let bad = run(&["verify", path(&file), "--sha256", &"0".repeat(64)]);
        assert_eq!(bad.status.code(), Some(3), "{bad:?}");
    }
}

#[test]
fn shared_help_and_text_output_are_concise() {
    let help = Command::new(env!("CARGO_BIN_EXE_ewf-cli"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    for word in ["acquire", "convert", "collect", "verify", "--json"] {
        assert!(text.contains(word), "{text}");
    }
    assert!(!text.contains("memory-limit"));
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("source.raw");
    fs::write(&raw, data()).unwrap();
    let info = Command::new(env!("CARGO_BIN_EXE_ewf-cli"))
        .args(["info", path(&raw)])
        .output()
        .unwrap();
    let text = String::from_utf8(info.stdout).unwrap();
    assert!(text.contains("Status: inspected"), "{text}");
    assert!(!text.trim_start().starts_with('{'));
}

#[test]
fn multiple_aff4_disks_require_selection_and_preserve_case_fields() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("multiple.aff4");
    let mut writer =
        aff4_image::Writer::create(&input, aff4_image::Profile::Physical, Default::default())
            .unwrap();
    writer
        .add_case_metadata(&aff4_image::CaseMetadata {
            case_number: "case-42".into(),
            ..Default::default()
        })
        .unwrap();
    let bytes = data();
    let first = writer
        .add_image_with_sector_size(bytes.len() as u64, 4096, &mut bytes.as_slice(), |_, _| {
            std::ops::ControlFlow::Continue(())
        })
        .unwrap();
    writer
        .add_image(512, &mut [7u8; 512].as_slice(), |_, _| {
            std::ops::ControlFlow::Continue(())
        })
        .unwrap();
    writer.finish().unwrap();
    let output = dir.path().join("selected.E01");
    assert!(
        !run(&["convert", path(&input), path(&output)])
            .status
            .success()
    );
    assert!(!output.exists());
    let v = succeeds(&["convert", path(&input), path(&output), "--resource", &first]);
    assert!(v["metadata_not_preserved"].as_array().unwrap().len() >= 2);
    let image = ewf_image::Image::open(&output).unwrap();
    assert_eq!(
        image.info().metadata.case_number.as_deref(),
        Some("case-42")
    );
    assert_eq!(read_image(&output), bytes);
}

fn logical_contents(file: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut result = std::collections::BTreeMap::new();
    if file.extension().unwrap() == "Lx01" {
        let image = ewf_image::Image::open(file).unwrap();
        let mut pending: Vec<_> = image
            .root_file_entry()
            .unwrap()
            .children
            .iter()
            .map(|e| (e, String::new()))
            .collect();
        while let Some((entry, prefix)) = pending.pop() {
            let name = entry.name.as_deref().unwrap();
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            if entry.entry_type() == Some(ewf_image::SingleFileEntryType::Directory) {
                result.insert(format!("{path}/"), Vec::new());
                pending.extend(entry.children.iter().map(|e| (e, path.clone())));
            } else {
                let mut bytes = Vec::new();
                image
                    .single_file_cursor(entry)
                    .read_to_end(&mut bytes)
                    .unwrap();
                result.insert(path, bytes);
                assert_eq!(entry.modification_time, Some(1_700_000_000));
            }
        }
    } else {
        let mut c = aff4_image::Container::open(file).unwrap();
        for stream in c.streams().unwrap() {
            let folder = stream.types.iter().any(|s| s.ends_with("#FolderImage"));
            if !folder && !stream.types.iter().any(|s| s.ends_with("#FileImage")) {
                continue;
            }
            let name = c.metadata()[&stream.id]
                .iter()
                .find(|p| p.predicate.ends_with("#originalPathName"))
                .unwrap()
                .value
                .clone();
            if folder {
                result.insert(format!("{name}/"), Vec::new());
            } else {
                let mut bytes = Vec::new();
                c.copy_verified(&stream.id, &mut bytes, |_, _| {
                    std::ops::ControlFlow::Continue(())
                })
                .unwrap();
                result.insert(name, bytes);
            }
        }
    }
    result
}

#[test]
fn logical_conversion_preserves_bytes_hierarchy_empty_folders_and_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("logical.Lx01");
    let mut options = ewf_image::SequentialOptions::new(5);
    options.write.format = ewf_image::WriteFormat::Ewf2Logical;
    let mut writer = ewf_image::LogicalWriter::create_sequential(&input, options).unwrap();
    let folder = writer
        .add_directory(
            1,
            ewf_image::LogicalEntryMetadata {
                name: "folder".into(),
                ..Default::default()
            },
        )
        .unwrap();
    writer
        .add_directory(
            folder,
            ewf_image::LogicalEntryMetadata {
                name: "empty".into(),
                ..Default::default()
            },
        )
        .unwrap();
    writer
        .add_file(
            folder,
            ewf_image::LogicalEntryMetadata {
                name: "héllo.txt".into(),
                modification_time: Some(1_700_000_000),
                ..Default::default()
            },
            5,
            &mut b"hello".as_slice(),
        )
        .unwrap();
    writer
        .add_file(
            1,
            ewf_image::LogicalEntryMetadata {
                name: "zero".into(),
                modification_time: Some(1_700_000_000),
                ..Default::default()
            },
            0,
            &mut io_empty(),
        )
        .unwrap();
    writer.finish().unwrap();
    let expected = logical_contents(&input);
    let aff4 = dir.path().join("logical.aff4");
    succeeds(&["convert", path(&input), path(&aff4)]);
    assert_eq!(logical_contents(&aff4), expected);
    for (index, from) in [&input, &aff4].iter().enumerate() {
        for extension in ["Lx01", "aff4"] {
            let output = dir.path().join(format!("copy-{index}.{extension}"));
            let value = succeeds(&["convert", path(from), path(&output)]);
            assert_eq!(value["verified_files"], 2);
            assert_eq!(logical_contents(&output), expected);
        }
    }
    let invalid = dir.path().join("invalid.raw");
    assert!(
        !run(&["convert", path(&input), path(&invalid)])
            .status
            .success()
    );
    assert!(!invalid.exists());
}
fn io_empty() -> std::io::Empty {
    std::io::empty()
}

#[test]
fn conversion_rejects_bad_source_hashes_and_preserves_ewf_error_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("bad.E01");
    let mut options = ewf_image::WriteOptions::default();
    options.hashes.md5 = Some([0; 16]);
    let mut writer = ewf_image::EwfWriter::create(&input, options).unwrap();
    writer.write_all(&data()).unwrap();
    writer.finish().unwrap();
    for extension in ["E01", "Ex01", "aff4", "raw"] {
        let output = dir.path().join(format!("rejected.{extension}"));
        let result = run(&["convert", path(&input), path(&output)]);
        assert!(!result.status.success());
        assert!(!output.exists());
        assert_eq!(report(&result)["published"], false);
    }
    let input = dir.path().join("substituted.E01");
    let options = ewf_image::WriteOptions {
        acquisition_errors: vec![ewf_image::AcquisitionError {
            first_sector: 1,
            sector_count: 2,
        }],
        ..Default::default()
    };
    let mut writer = ewf_image::EwfWriter::create(&input, options).unwrap();
    writer.write_all(&data()).unwrap();
    writer.finish().unwrap();
    for extension in ["E01", "Ex01"] {
        let output = dir.path().join(format!("substituted-copy.{extension}"));
        let result = succeeds(&["convert", path(&input), path(&output)]);
        assert_eq!(result["exit_code"], 4);
        assert_eq!(
            ewf_image::Image::open(&output)
                .unwrap()
                .info()
                .acquisition_errors,
            vec![ewf_image::AcquisitionError {
                first_sector: 1,
                sector_count: 2
            }]
        );
    }
}

#[test]
fn aff4_metadata_reference_is_checked_and_substreams_are_not_silently_lost() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("logical.aff4");
    let mut writer =
        aff4_image::Writer::create(&input, aff4_image::Profile::Logical, Default::default())
            .unwrap();
    let id = writer
        .add_file("file", 3, &mut b"abc".as_slice(), |_, _| {
            std::ops::ControlFlow::Continue(())
        })
        .unwrap();
    writer
        .add_substream(
            &id,
            b"ads",
            aff4_image::SubstreamKind::AlternateDataStream,
            b"secret",
        )
        .unwrap();
    let written = writer.finish().unwrap();
    assert!(
        run(&[
            "verify",
            path(&input),
            "--metadata-sha256",
            &written.metadata_sha256
        ])
        .status
        .success()
    );
    assert_eq!(
        run(&["verify", path(&input), "--metadata-sha256", &"0".repeat(64)])
            .status
            .code(),
        Some(3)
    );
    let output = dir.path().join("not-created.Lx01");
    let out = run(&["convert", path(&input), path(&output)]);
    assert!(!out.status.success());
    assert!(!output.exists());
    assert!(
        report(&out)["error"]
            .as_str()
            .unwrap()
            .contains("substreams")
    );
}
