//! Explicit companion resolution, ownership conflicts, and independent stripes.
use aff4_image::{Container, Error, Limits, VolumeSet};
use std::{
    fs::File,
    io::Write,
    ops::ControlFlow,
    path::{Path, PathBuf},
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn fixture(path: &Path, volume: &str, metadata: &str, members: &[(&str, &[u8])]) {
    let mut zip = ZipWriter::new(File::create(path).unwrap());
    zip.set_comment(volume).unwrap();
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in [
        ("version.txt", b"major=1\nminor=0\n".as_slice()),
        ("information.turtle", metadata.as_bytes()),
    ]
    .into_iter()
    .chain(members.iter().copied())
    {
        zip.start_file(name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}
fn map(start: u64, length: u64, offset: u64, target: u32) -> Vec<u8> {
    [
        start.to_le_bytes().as_slice(),
        &length.to_le_bytes(),
        &offset.to_le_bytes(),
        &target.to_le_bytes(),
    ]
    .concat()
}
fn index(length: u32) -> Vec<u8> {
    [0u64.to_le_bytes().as_slice(), &length.to_le_bytes()].concat()
}
const PRIMARY: &str = r#"@prefix a: <http://aff4.org/Schema#> .
<aff4://v1/a> a a:ImageStream; a:stored <aff4://v1>; a:size 4; a:chunkSize 4; a:chunksInSegment 1 .
<aff4://v2/b> a a:ImageStream; a:stored <aff4://v2> .
<aff4://v1/map> a a:Map; a:size 8 .
<aff4://v1/image> a a:DiskImage, a:ContiguousImage; a:dataStream <aff4://v1/map>; a:size 8 ."#;
const COMPANION: &str = r#"@prefix a: <http://aff4.org/Schema#> .
<aff4://v2/b> a a:ImageStream; a:stored <aff4://v2>; a:size 4; a:chunkSize 4; a:chunksInSegment 1 ."#;

#[test]
fn companion_metadata_and_directory_budgets_are_shared() {
    let directory = tempfile::tempdir().unwrap();
    let paths = pair(
        directory.path(),
        PRIMARY,
        COMPANION,
        &[map(0, 4, 0, 0), map(4, 4, 0, 1)].concat(),
    );
    for limits in [
        Limits {
            metadata_bytes: PRIMARY.len() as u64,
            ..Limits::default()
        },
        Limits {
            archive_entries: 6,
            ..Limits::default()
        },
        Limits {
            triples: 17,
            ..Limits::default()
        },
    ] {
        for path in &paths {
            assert!(Container::open_with_limits(path, limits.clone()).is_ok());
        }
        assert!(VolumeSet::open_with_limits(&paths, limits).is_err());
    }
    let limits = Limits {
        metadata_bytes: (PRIMARY.len() + COMPANION.len()) as u64,
        archive_entries: 10,
        ..Limits::default()
    };
    let mut set = VolumeSet::open_with_limits(&paths, limits).unwrap();
    let mut bytes = [0; 8];
    set.read_at("aff4://v1/image", &mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"abcdEFGH");
}
fn pair(directory: &Path, primary: &str, companion: &str, ranges: &[u8]) -> Vec<PathBuf> {
    let paths = vec![
        directory.join("arbitrary-z.zip"),
        directory.join("arbitrary-a.zip"),
    ];
    fixture(
        &paths[0],
        "aff4://v1",
        primary,
        &[
            ("a/00000000", b"abcd"),
            ("a/00000000.index", &index(4)),
            ("map/map", ranges),
            ("map/idx", b"aff4://v1/a\naff4://v2/b\n"),
        ],
    );
    fixture(
        &paths[1],
        "aff4://v2",
        companion,
        &[("b/00000000", b"EFGH"), ("b/00000000.index", &index(4))],
    );
    paths
}
#[test]
fn split_and_striped_reads_missing_volumes_conflicts_and_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    for (ranges, expected) in [
        ([map(0, 4, 0, 0), map(4, 4, 0, 1)].concat(), b"abcdEFGH"),
        (
            [
                map(0, 2, 0, 0),
                map(2, 2, 0, 1),
                map(4, 2, 2, 0),
                map(6, 2, 2, 1),
            ]
            .concat(),
            b"abEFcdGH",
        ),
    ] {
        let paths = pair(dir.path(), PRIMARY, COMPANION, &ranges);
        let mut set = VolumeSet::open(&paths).unwrap();
        let id = "aff4://v1/image";
        assert_eq!(set.images(), [id]);
        assert_eq!(set.size(id).unwrap(), 8);
        assert_eq!(set.sources().len(), 2);
        let mut bytes = [0; 10];
        assert_eq!(set.read_at(id, &mut bytes, 0).unwrap(), 8);
        assert_eq!(&bytes[..8], expected);
        assert_eq!(set.read_at(id, &mut bytes, 1).unwrap(), 7);
        assert_eq!(&bytes[..7], &expected[1..]);
        assert_eq!(set.read_at(id, &mut bytes, 8).unwrap(), 0);
        let hash = set
            .verify_image(id, None, |_, _| ControlFlow::Continue(()))
            .unwrap();
        assert_eq!(hash.external_match, None);
        assert_eq!(
            set.verify_image(id, Some(&hash.sha256), |_, _| ControlFlow::Continue(()))
                .unwrap()
                .external_match,
            Some(true)
        );
        assert_eq!(
            set.verify_image(id, Some(&"0".repeat(64)), |_, _| ControlFlow::Continue(()))
                .unwrap()
                .external_match,
            Some(false)
        );
        assert!(matches!(
            set.verify_image(id, None, |_, _| ControlFlow::Break(())),
            Err(Error::Aborted)
        ));
        assert!(VolumeSet::open(&paths[..1]).is_err());
        assert!(VolumeSet::open(&[paths[0].clone(), paths[0].clone()]).is_err());
    }
    let ranges = [map(0, 4, 0, 0), map(4, 4, 0, 1)].concat();
    let conflict = PRIMARY.replace("a:stored <aff4://v2> .", "a:stored <aff4://v2>; a:size 5 .");
    assert!(VolumeSet::open(&pair(dir.path(), &conflict, COMPANION, &ranges)).is_err());
    let bad_owner = COMPANION.replace("a:stored <aff4://v2>", "a:stored <aff4://v1>");
    assert!(VolumeSet::open(&pair(dir.path(), PRIMARY, &bad_owner, &ranges)).is_err());
    let paths = pair(dir.path(), PRIMARY, COMPANION, &map(0, 5, 0, 0));
    assert!(
        VolumeSet::open(&paths)
            .unwrap()
            .size("aff4://v1/image")
            .is_err()
    );
}
#[test]
fn contiguous_images_reject_implicit_holes_in_single_and_multi_volume_readers() {
    let dir = tempfile::tempdir().unwrap();
    for ranges in [map(1, 3, 0, 0), map(0, 4, 0, 0)] {
        let paths = pair(dir.path(), PRIMARY, COMPANION, &ranges);
        assert!(
            VolumeSet::open(&paths)
                .unwrap()
                .read_at("aff4://v1/image", &mut [0; 8], 0)
                .is_err()
        );
        assert!(
            Container::open(&paths[0])
                .unwrap()
                .read_at("aff4://v1/image", &mut [0; 8], 0)
                .is_err()
        );
    }
}
#[test]
#[ignore = "requires pinned canonical striped reference pair"]
fn canonical_striped_image_matches_independent_export() {
    let paths = ["AFF4_STRIPE1", "AFF4_STRIPE2"]
        .map(|key| PathBuf::from(std::env::var_os(key).expect(key)));
    let mut set = VolumeSet::open(&paths).unwrap();
    let result = set
        .verify_image(
            "aff4://951b3e29-6549-4266-8e81-3f88ddba61ae",
            Some("d7d6df4534f06568eb90a06e252592c9b79378b95bb9a7e01db3a388feda6c13"),
            |_, _| ControlFlow::Continue(()),
        )
        .unwrap();
    assert_eq!(result.bytes, 268435456);
    assert_eq!(result.external_match, Some(true));
    assert_eq!(result.sources.len(), 2);
}

#[test]
fn set_cli_distinguishes_computed_matched_and_mismatched_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let paths = pair(
        dir.path(),
        PRIMARY,
        COMPANION,
        &[map(0, 4, 0, 0), map(4, 4, 0, 1)].concat(),
    );
    let digest = VolumeSet::open(&paths)
        .unwrap()
        .verify_image("aff4://v1/image", None, |_, _| ControlFlow::Continue(()))
        .unwrap()
        .sha256;
    for (expected, code) in [(None, 4), (Some(digest), 0), (Some("0".repeat(64)), 3)] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_aff4-image"));
        command
            .arg("verify-set")
            .args(&paths)
            .args(["--image", "aff4://v1/image"]);
        if let Some(expected) = expected {
            command.args(["--expected-image-sha256", &expected]);
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["scope"], "assembled image bytes only");
        assert_eq!(report["result"]["bytes"], 8);
    }
}
