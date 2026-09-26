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
fn discovery_groups_by_identity_from_either_volume_and_reopens() {
    use aff4_image::DiskImageSet;
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let paths = pair(
        dir.path(),
        PRIMARY,
        COMPANION,
        &[
            map(0, 2, 0, 0),
            map(2, 2, 0, 1),
            map(4, 2, 2, 0),
            map(6, 2, 2, 1),
        ]
        .concat(),
    );
    let unrelated = dir.path().join("unrelated.aff4");
    fixture(
        &unrelated,
        "aff4://unrelated",
        &COMPANION.replace("v2", "unrelated"),
        &[("b/00000000", b"1234"), ("b/00000000.index", &index(4))],
    );
    for start in &paths {
        let mut candidates = paths.clone();
        candidates.push(unrelated.clone());
        let mut readers = DiskImageSet::discover(std::slice::from_ref(start), &candidates)
            .unwrap()
            .into_readers();
        assert_eq!(readers.len(), 1);
        let reader = &mut readers[0];
        assert_eq!(reader.info().volumes.len(), 2);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abEFcdGH");
        let mut reopened = reader.info().reopen().unwrap();
        bytes.clear();
        reopened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abEFcdGH");
    }
    assert!(DiskImageSet::discover(&paths[..1], &[]).is_err());
    // An explicitly supplied orphan cannot disappear beside a valid image.
    assert!(DiskImageSet::discover(&[paths[0].clone(), paths[1].clone(), unrelated], &[]).is_err());
    let duplicate = dir.path().join("duplicate.aff4");
    std::fs::copy(&paths[1], &duplicate).unwrap();
    assert!(DiskImageSet::discover(&paths[..1], &[paths[1].clone(), duplicate]).is_err());
    assert!(
        DiskImageSet::discover_with_limits(
            &paths,
            &[],
            Limits {
                metadata_bytes: PRIMARY.len() as u64,
                ..Limits::default()
            }
        )
        .is_err()
    );
}

#[test]
fn discovery_keeps_each_primary_and_cursor_in_a_shared_set() {
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let companion = format!(
        "{COMPANION}\n<aff4://v2/disk> a <http://aff4.org/Schema#DiskImage>; <http://aff4.org/Schema#dataStream> <aff4://v2/b> ."
    );
    let paths = pair(
        dir.path(),
        PRIMARY,
        &companion,
        &[map(0, 4, 0, 0), map(4, 4, 0, 1)].concat(),
    );
    let mut readers = aff4_image::DiskImageSet::discover(&paths[..1], &paths[1..])
        .unwrap()
        .into_readers();
    assert_eq!(readers.len(), 2);
    let mut bytes = [0; 4];
    readers[0].read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"abcd");
    readers[1].read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"EFGH");
    readers[0].read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"EFGH");
    assert_eq!(
        readers[1].info().reopen().unwrap().info(),
        readers[1].info()
    );
}

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
    let report = set
        .verify_full(
            "aff4://951b3e29-6549-4266-8e81-3f88ddba61ae",
            Some("d7d6df4534f06568eb90a06e252592c9b79378b95bb9a7e01db3a388feda6c13"),
            |_, _, _| ControlFlow::Continue(()),
        )
        .unwrap();
    let coverage = report.coverage.as_ref().unwrap();
    assert_eq!(
        coverage.stored + coverage.described + coverage.gap_filled,
        268435456
    );
    assert!(coverage.stored > 0 && coverage.described > 0);
    for volume in &report.volumes {
        for check in &volume.checks {
            if check.reference_source.ends_with("#imageStreamHash") {
                assert_eq!(check.outcome, aff4_image::CheckOutcome::Unsupported);
                continue;
            }
            assert_eq!(
                check.outcome,
                aff4_image::CheckOutcome::Match,
                "{}: {check:?}",
                volume.volume
            );
        }
    }
    assert!(
        report
            .volumes
            .iter()
            .flat_map(|v| &v.checks)
            .filter(|c| c.algorithm == "SHA512" && c.resource == report.image)
            .count()
            >= 2
    );
    // This historical canonical image has no metadata integrity sidecars.
    assert!(!report.all_match());
}

#[test]
fn full_set_report_retains_missing_references_coverage_and_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let paths = pair(
        dir.path(),
        PRIMARY,
        COMPANION,
        &[map(0, 4, 0, 0), map(4, 4, 0, 1)].concat(),
    );
    let mut set = VolumeSet::open(&paths).unwrap();
    let report = set
        .verify_full("aff4://v1/image", None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    assert_eq!(report.coverage.as_ref().unwrap().stored, 8);
    assert_eq!(report.streams.len(), 2);
    assert_eq!(report.volumes.len(), 2);
    assert!(!report.all_match());
    assert!(
        report
            .volumes
            .iter()
            .flat_map(|v| &v.checks)
            .any(|c| c.outcome == aff4_image::CheckOutcome::Missing)
    );
    assert!(matches!(
        set.verify_full("aff4://v1/image", None, |_, _, _| ControlFlow::Break(())),
        Err(Error::Aborted)
    ));
}

#[test]
fn full_set_verifies_foreign_block_references_and_metadata_in_their_own_context() {
    use sha2::{Digest, Sha256};
    let hash = |data: &[u8]| {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let dir = tempfile::tempdir().unwrap();
    let paths = [dir.path().join("one.aff4"), dir.path().join("two.aff4")];
    let ranges = [map(0, 4, 0, 0), map(4, 4, 0, 1)].concat();
    let targets = b"aff4://v1/a\naff4://v2/b\n";
    let primary = format!(
        "{PRIMARY}\n<aff4://v1/a> <http://aff4.org/Schema#hash> \"{}\"^^<http://aff4.org/Schema#SHA256> .\n<aff4://v1/image> <http://aff4.org/Schema#hash> \"{}\"^^<http://aff4.org/Schema#SHA256> .\n<aff4://v1/map> <http://aff4.org/Schema#mapHash> \"{}\"^^<http://aff4.org/Schema#SHA256> .",
        hash(b"abcd"),
        hash(b"abcdEFGH"),
        hash(&[ranges.as_slice(), targets].concat())
    );
    let companion = format!(
        "{COMPANION}\n<aff4://v2/b> <http://aff4.org/Schema#hash> \"{}\"^^<http://aff4.org/Schema#SHA256> .",
        hash(b"EFGH")
    );
    let metadata_hash = |volume: &str, metadata: &str| {
        format!(
            "<{volume}/information.turtle> <http://aff4.org/Schema#hash> \"{}\"^^<http://aff4.org/Schema#SHA256> .",
            hash(metadata.as_bytes())
        )
    };
    let primary_hash = metadata_hash("aff4://v1", &primary);
    let companion_hash = metadata_hash("aff4://v2", &companion);
    for changed in [false, true] {
        let foreign = Sha256::digest(if changed { b"efgh" } else { b"EFGH" });
        fixture(
            &paths[0],
            "aff4://v1",
            &primary,
            &[
                ("a/00000000", b"abcd"),
                ("a/00000000.index", &index(4)),
                ("map/map", &ranges),
                ("map/idx", targets),
                ("aff4%3A%2F%2Fv2/b/00000000.blockHash.sha256", &foreign),
                ("information.turtle.hashes", primary_hash.as_bytes()),
            ],
        );
        fixture(
            &paths[1],
            "aff4://v2",
            &companion,
            &[
                ("b/00000000", b"EFGH"),
                ("b/00000000.index", &index(4)),
                ("information.turtle.hashes", companion_hash.as_bytes()),
            ],
        );
        let report = VolumeSet::open(&paths)
            .unwrap()
            .verify_full("aff4://v1/image", Some(&hash(b"abcdEFGH")), |_, _, _| {
                ControlFlow::Continue(())
            })
            .unwrap();
        assert_eq!(
            report.assembled.as_ref().unwrap().external_match,
            Some(true)
        );
        assert_eq!(report.all_match(), !changed, "{report:?}");
        assert!(
            report.volumes[0]
                .checks
                .iter()
                .any(|c| c.resource == "aff4://v2/b"
                    && c.outcome
                        == if changed {
                            aff4_image::CheckOutcome::Mismatch
                        } else {
                            aff4_image::CheckOutcome::Match
                        })
        );
    }
}
