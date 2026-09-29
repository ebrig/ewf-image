//! AFF4 bounds, map semantics, and independent reference regressions.
use aff4_image::{Container, Error, Limits};
use std::fs::File;
use std::io::{Read, Write};
use std::ops::ControlFlow;
use zip::{ZipWriter, write::SimpleFileOptions};

fn fixture(turtle: &str, members: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
    fixture_version(turtle, members, b"major=1\nminor=0\n")
}

fn fixture_version(
    turtle: &str,
    members: &[(&str, &[u8])],
    version: &[u8],
) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut zip = ZipWriter::new(File::create(file.path()).unwrap());
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);
    zip.set_comment("aff4://volume").unwrap();
    for (name, data) in [
        ("version.txt", version),
        ("information.turtle", turtle.as_bytes()),
    ]
    .into_iter()
    .chain(members.iter().copied())
    {
        zip.start_file(name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
    file
}

fn metadata() -> String {
    r#"@prefix a: <http://aff4.org/Schema#> .
    <aff4://volume/data> a a:ImageStream; a:chunkSize 4; a:chunksInSegment 1; a:size 6;
      a:hash "e80b5017098950fc58aad83c8c14978e"^^a:MD5 .
    <aff4://volume/disk> a a:DiskImage, a:Image; a:dataStream <aff4://volume/data> ."#
        .into()
}

#[test]
fn directory_and_map_limits_apply_to_small_valid_inputs() {
    let range = [
        0u64.to_le_bytes().as_slice(),
        &4u64.to_le_bytes(),
        &0u64.to_le_bytes(),
        &0u32.to_le_bytes(),
    ]
    .concat();
    let file = fixture(
        "@prefix a: <http://aff4.org/Schema#> . <aff4://volume/map> a a:Map; a:size 4 .",
        &[
            ("map/map", &range),
            ("map/idx", b"http://aff4.org/Schema#Zero\n"),
        ],
    );
    for limits in [
        Limits {
            archive_entries: 3,
            ..Limits::default()
        },
        Limits {
            directory_bytes: 16,
            ..Limits::default()
        },
    ] {
        assert!(Container::open_with_limits(file.path(), limits.clone()).is_err());
        let mut visits = 0;
        assert!(
            Container::scan_metadata(file.path(), limits, |_, _, _| {
                visits += 1;
                ControlFlow::Continue(())
            })
            .is_err()
        );
        assert_eq!(visits, 0);
    }
    let mut image = Container::open_with_limits(
        file.path(),
        Limits {
            archive_entries: 4,
            map_bytes: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(image.read_at("aff4://volume/map", &mut [0; 4], 0).is_err());
    assert_eq!(
        Container::open(file.path())
            .unwrap()
            .read_at("aff4://volume/map", &mut [0; 4], 0)
            .unwrap(),
        4
    );
}

#[test]
fn verification_work_is_bounded_across_resources() {
    let file = fixture(
        &metadata(),
        &[
            ("data/00000000", b"abcd"),
            ("data/00000001", b"ef\0\0"),
            ("data/00000000.index", &index(4)),
            ("data/00000001.index", &index(4)),
        ],
    );
    let mut image = Container::open_with_limits(
        file.path(),
        Limits {
            verification_bytes: 6,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(
        image
            .verify("aff4://volume/data", |_, _| ControlFlow::Continue(()))
            .is_ok()
    );
    let report = image
        .verify_all(None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    assert!(!report.all_match());
    assert!(report.resources.iter().any(|r| {
        r.error
            .as_deref()
            .is_some_and(|e| e.contains("verification byte limit"))
    }));
}

#[test]
fn direct_data_stream_alias_reuses_bytes_but_checks_its_own_reference_and_limit() {
    use aff4_image::CheckOutcome;
    use sha2::{Digest, Sha256};
    let bytes = vec![42; 2 * 1024 * 1024];
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let metadata = format!(
        "@prefix a: <http://aff4.org/Schema#> . \
         <aff4://volume/data> a a:Image, a:ZipSegment; a:size {}; a:hash \"{digest}\"^^a:SHA256 . \
         <aff4://volume/disk> a a:Image, a:DiskImage; a:size {}; \
         a:dataStream <aff4://volume/data>; a:hash \"{}\"^^a:SHA256 .",
        bytes.len(),
        bytes.len(),
        "0".repeat(64),
    );
    let file = fixture(&metadata, &[("data", &bytes)]);
    let mut image = Container::open(file.path()).unwrap();
    let mut intermediate = Vec::new();
    let report = image
        .verify_all(None, |id, done, total| {
            if done > 0 && done < total {
                intermediate.push(id.to_owned());
            }
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(intermediate, ["aff4://volume/data"]);
    assert_eq!(report.resources.len(), 2);
    assert!(report.resources.iter().all(|r| r.coverage.is_some()));
    let linear: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.reference_source.ends_with("#hash"))
        .collect();
    assert_eq!(linear.len(), 2);
    assert_eq!(linear[0].outcome, CheckOutcome::Match);
    assert_eq!(linear[1].outcome, CheckOutcome::Mismatch);

    let mut bounded = Container::open_with_limits(
        file.path(),
        Limits {
            verification_bytes: bytes.len() as u64 * 2 - 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let report = bounded
        .verify_all(None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    assert!(report.resources.iter().any(|r| {
        r.error
            .as_deref()
            .is_some_and(|error| error.contains("verification byte limit"))
    }));
    assert!(matches!(
        image.verify_all(None, |id, done, total| {
            if id.ends_with("/disk") && done == total {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }),
        Err(Error::Aborted)
    ));
}

fn index(length: u32) -> Vec<u8> {
    let mut data = 0u64.to_le_bytes().to_vec();
    data.extend_from_slice(&length.to_le_bytes());
    data
}

#[test]
fn chunks_padding_and_per_resource_hashes() {
    let file = fixture(
        &metadata(),
        &[
            ("data/00000000", b"abcd"),
            ("data/00000001", b"ef\0\0"),
            ("data/00000000.index", &index(4)),
            ("data/00000001.index", &index(4)),
        ],
    );
    let mut image = Container::open(file.path()).unwrap();
    let mut bytes = [0; 6];
    assert_eq!(
        image.read_at("aff4://volume/disk", &mut bytes, 2).unwrap(),
        4
    );
    assert_eq!(&bytes[..4], b"cdef");
    assert_eq!(
        image
            .verify("aff4://volume/data", |_, _| ControlFlow::Continue(()))
            .unwrap()
            .references_match,
        Some(true)
    );
    assert_eq!(
        image
            .verify("aff4://volume/disk", |_, _| ControlFlow::Continue(()))
            .unwrap()
            .references_match,
        None
    );
    assert!(matches!(
        image.verify("aff4://volume/data", |_, _| ControlFlow::Break(())),
        Err(Error::Aborted)
    ));
    assert!(
        Container::open_with_limits(
            file.path(),
            Limits {
                metadata_bytes: 1,
                ..Limits::default()
            }
        )
        .is_err()
    );
}

#[test]
fn map_sparse_ranges_and_invalid_targets() {
    let meta = format!("{}\n<aff4://volume/map> a a:Map; a:size 10 .", metadata());
    let mut range = 2u64.to_le_bytes().to_vec();
    range.extend_from_slice(&4u64.to_le_bytes());
    range.extend_from_slice(&1u64.to_le_bytes());
    range.extend_from_slice(&0u32.to_le_bytes());
    let file = fixture(
        &meta,
        &[
            ("data/00000000", b"abcd"),
            ("data/00000001", b"ef\0\0"),
            ("data/00000000.index", &index(4)),
            ("data/00000001.index", &index(4)),
            ("map/map", &range),
            ("map/idx", b"aff4://volume/data\n"),
        ],
    );
    let mut image = Container::open(file.path()).unwrap();
    let mut bytes = [1; 10];
    image.read_at("aff4://volume/map", &mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"\0\0bcde\0\0\0\0");
    let file = fixture(
        &meta,
        &[("map/map", &range), ("map/idx", b"aff4://volume/map\n")],
    );
    assert!(
        Container::open(file.path())
            .unwrap()
            .read_at("aff4://volume/map", &mut bytes, 0)
            .is_err()
    );
    range[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    let file = fixture(
        &meta,
        &[("map/map", &range), ("map/idx", b"aff4://volume/data\n")],
    );
    assert!(
        Container::open(file.path())
            .unwrap()
            .read_at("aff4://volume/map", &mut bytes, 0)
            .is_err()
    );
}

#[test]
fn bounded_compression_and_unknown_data() {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"aaaa").unwrap();
    let data = encoder.finish().unwrap();
    let meta = r#"@prefix a: <http://aff4.org/Schema#> . <aff4://volume/data> a a:ImageStream; a:chunkSize 4; a:chunksInSegment 1; a:size 4; a:compressionMethod <https://www.ietf.org/rfc/rfc1950.txt> ."#;
    let file = fixture(
        meta,
        &[
            ("data/00000000", &data),
            ("data/00000000.index", &index(data.len() as u32)),
        ],
    );
    let mut image = Container::open(file.path()).unwrap();
    let mut bytes = [0; 4];
    image.read_at("aff4://volume/data", &mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"aaaa");
    let mut image = Container::open_with_limits(
        file.path(),
        Limits {
            chunk_bytes: 3,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(image.read_at("aff4://volume/data", &mut bytes, 0).is_err());
    let file = fixture(
        r#"@prefix a: <http://aff4.org/Schema#> . <aff4://volume/map> a a:Map; a:size 4; a:mapGapDefaultStream a:UnreadableData ."#,
        &[("map/map", b""), ("map/idx", b"")],
    );
    assert!(
        Container::open(file.path())
            .unwrap()
            .read_at("aff4://volume/map", &mut bytes, 0)
            .is_err()
    );
}

#[test]
#[ignore = "requires the pinned AFF4 canonical Base-Linear image"]
fn canonical_reference_matches_producer_hashes() {
    let path = std::env::var_os("AFF4_REFERENCE_IMAGE").expect("AFF4_REFERENCE_IMAGE required");
    let mut image = Container::open(path).unwrap();
    let verified = image
        .verify("aff4://c215ba20-5648-4209-a793-1f918c723610", |_, _| {
            ControlFlow::Continue(())
        })
        .unwrap();
    assert_eq!(verified.bytes_verified, 3_964_928);
    assert_eq!(verified.md5, "d5825dc1152a42958c8219ff11ed01a3");
    assert_eq!(verified.sha1, "fbac22cca549310bc5df03b7560afcf490995fbb");
    assert_eq!(verified.references_match, Some(true));
    let mut header = [0; 512];
    image
        .read_at(
            "aff4://cf853d0b-5589-4c7c-8358-2ca1572b87eb",
            &mut header,
            0,
        )
        .unwrap();
    assert_eq!(&header[510..], &[0x55, 0xaa]);
}

#[test]
fn logical_zip_inline_imports_and_metadata() {
    let meta = r#"@prefix a: <http://aff4.org/Schema#> . @prefix l: <https://aff4.org/Schema/2022/#> .
    <aff4://volume> l:imports <aff4://volume/extra.turtle> .
    <aff4://file> a l:FileImage, a:Image, a:ZipSegment; a:size 3; l:fileName "../source:stream";
      a:hash "900150983cd24fb0d6963f7d28e17f72"^^a:MD5 ."#;
    let extra = r#"@prefix a: <http://aff4.org/Schema#> . @prefix l: <https://aff4.org/Schema/2022/#> .
    <aff4://inline> a l:FileSubStream, a:Image; a:size 3; a:dataStream "YWJj"^^<http://www.w3.org/2001/XMLSchema#base64Binary> ."#;
    let file = fixture_version(
        meta,
        &[("extra.turtle", extra.as_bytes()), ("aff4://file", b"abc")],
        b"major=2\nminor=1\n",
    );
    let mut image = Container::open(file.path()).unwrap();
    assert_eq!(image.version(), (2, 1));
    assert_eq!(image.streams().unwrap().len(), 2);
    assert_eq!(
        image
            .verify("aff4://file", |_, _| ControlFlow::Continue(()))
            .unwrap()
            .references_match,
        Some(true)
    );
    let mut buffer = [0; 3];
    image.read_at("aff4://inline", &mut buffer, 0).unwrap();
    assert_eq!(&buffer, b"abc");
    let mut sequential = Vec::new();
    image
        .sequential_reader("aff4://file")
        .unwrap()
        .read_to_end(&mut sequential)
        .unwrap();
    assert_eq!(sequential, b"abc");
    sequential.clear();
    image
        .sequential_reader("aff4://inline")
        .unwrap()
        .read_to_end(&mut sequential)
        .unwrap();
    assert_eq!(sequential, b"abc");
    assert!(
        image.metadata()["aff4://file"]
            .iter()
            .any(|p| p.value == "../source:stream")
    );
    assert!(
        Container::open_with_limits(
            file.path(),
            Limits {
                metadata_bytes: meta.len() as u64,
                ..Limits::default()
            }
        )
        .is_err()
    );
    let bad = fixture_version(
        &extra.replace("a:size 3", "a:size 4"),
        &[],
        b"major=2\nminor=1\n",
    );
    assert!(
        Container::open(bad.path())
            .unwrap()
            .size("aff4://inline")
            .is_err()
    );
}

#[test]
fn rejects_missing_versions_ambiguous_inline_and_missing_empty_members() {
    let missing = fixture_version(&metadata(), &[], b"major=1\n");
    assert!(Container::open(missing.path()).is_err());
    let empty = r#"@prefix a: <http://aff4.org/Schema#> . <aff4://volume/file> a a:Image, a:ZipSegment; a:size 0 ."#;
    let file = fixture_version(empty, &[], b"major=2\nminor=1\n");
    assert!(
        Container::open(file.path())
            .unwrap()
            .verify("aff4://volume/file", |_, _| ControlFlow::Continue(()))
            .is_err()
    );
    let mixed = r#"@prefix a: <http://aff4.org/Schema#> . <aff4://inline> a a:Image; a:size 3; a:dataStream "YWJj"^^<http://www.w3.org/2001/XMLSchema#base64Binary>, <aff4://other> ."#;
    let file = fixture_version(mixed, &[], b"major=2\nminor=1\n");
    assert!(
        Container::open(file.path())
            .unwrap()
            .size("aff4://inline")
            .is_err()
    );
}

#[test]
fn raw_deflate_is_distinct_from_zlib() {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&[42; 1024]).unwrap();
    let compressed = encoder.finish().unwrap();
    let meta = r#"@prefix a: <http://aff4.org/Schema#> . <aff4://volume/data> a a:ImageStream; a:chunkSize 1024; a:chunksInSegment 1; a:size 1024; a:compressionMethod <https://tools.ietf.org/html/rfc1951> ."#;
    let file = fixture(
        meta,
        &[
            ("data/00000000", &compressed),
            ("data/00000000.index", &index(compressed.len() as u32)),
        ],
    );
    let mut bytes = [0; 1024];
    Container::open(file.path())
        .unwrap()
        .read_at("aff4://volume/data", &mut bytes, 0)
        .unwrap();
    assert_eq!(bytes, [42; 1024]);
    let wrong = fixture(
        &meta.replace(
            "https://tools.ietf.org/html/rfc1951",
            "https://www.ietf.org/rfc/rfc1950.txt",
        ),
        &[
            ("data/00000000", &compressed),
            ("data/00000000.index", &index(compressed.len() as u32)),
        ],
    );
    assert!(
        Container::open(wrong.path())
            .unwrap()
            .read_at("aff4://volume/data", &mut bytes, 0)
            .is_err()
    );
}

#[test]
#[ignore = "requires the pinned legacy AFF4-L canonical dream image"]
fn canonical_logical_reference_matches_producer_hashes() {
    let path = std::env::var_os("AFF4_LOGICAL_REFERENCE_IMAGE")
        .expect("AFF4_LOGICAL_REFERENCE_IMAGE required");
    let mut image = Container::open(path).unwrap();
    let streams = image.streams().unwrap();
    assert_eq!(streams.len(), 1);
    let result = image
        .verify(&streams[0].id, |_, _| ControlFlow::Continue(()))
        .unwrap();
    assert_eq!(result.bytes_verified, 8688);
    assert_eq!(result.md5, "75d83773f8d431a3ca91bfb8859e486d");
    assert_eq!(result.sha1, "9ae1b46bead70c322eef7ac8bc36a8ea2055595c");
    assert_eq!(result.references_match, Some(true));
}

#[test]
fn metadata_hashes_cover_exact_bytes_and_external_reference() {
    use aff4_image::CheckOutcome;
    use sha2::{Digest, Sha256};
    let primary = "@prefix a: <http://aff4.org/Schema#> .\n";
    let digest = Sha256::digest(primary.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let rdf = format!(
        "<aff4://volume/information.turtle> <http://aff4.org/Schema#hash> \"{digest}\"^^<http://aff4.org/Schema#SHA256> ."
    );
    let legacy = format!("{{\"sha256\":\"{digest}\"}}");
    let file = fixture(
        primary,
        &[
            ("information.turtle.hashes", rdf.as_bytes()),
            ("container.hashes", legacy.as_bytes()),
        ],
    );
    let mut image = Container::open(file.path()).unwrap();
    let report = image.verify_metadata(Some(&digest)).unwrap();
    assert!(report.all_match());
    assert_eq!(report.checks.len(), 3);
    assert_eq!(
        image.verify_metadata(Some(&"0".repeat(64))).unwrap().checks[0].outcome,
        CheckOutcome::Mismatch
    );
    // Whitespace preserves RDF semantics but changes the signed/hashed bytes.
    let changed = fixture(
        &format!("{primary} "),
        &[("information.turtle.hashes", rdf.as_bytes())],
    );
    assert_eq!(
        Container::open(changed.path())
            .unwrap()
            .verify_metadata(None)
            .unwrap()
            .checks[0]
            .outcome,
        CheckOutcome::Mismatch
    );
    let conflict = fixture(
        primary,
        &[
            ("information.turtle.hashes", rdf.as_bytes()),
            (
                "container.hashes",
                br#"{"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
            ),
        ],
    );
    assert!(
        !Container::open(conflict.path())
            .unwrap()
            .verify_metadata(None)
            .unwrap()
            .all_match()
    );
    let duplicate = fixture(
        primary,
        &[("container.hashes", br#"{"sha256":"a","SHA256":"b"}"#)],
    );
    assert!(
        Container::open(duplicate.path())
            .unwrap()
            .verify_metadata(None)
            .is_err()
    );
    let missing = fixture(primary, &[]);
    assert_eq!(
        Container::open(missing.path())
            .unwrap()
            .verify_metadata(None)
            .unwrap()
            .checks[0]
            .outcome,
        CheckOutcome::Missing
    );
}

#[test]
fn imported_metadata_requires_a_primary_store_hash() {
    use aff4_image::CheckOutcome;
    use sha2::{Digest, Sha256};
    let secondary = "<aff4://file> <http://aff4.org/Schema#size> 0 .";
    let hash = Sha256::digest(secondary)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let primary = format!(
        "@prefix a: <http://aff4.org/Schema#> . <aff4://volume> a:imports <aff4://volume/more> . <aff4://volume/more> a:hash \"{hash}\"^^a:SHA256 ."
    );
    let file = fixture(&primary, &[("more", secondary.as_bytes())]);
    let report = Container::open(file.path())
        .unwrap()
        .verify_metadata(None)
        .unwrap();
    assert_eq!(report.checks.last().unwrap().outcome, CheckOutcome::Match);
    assert!(!report.all_match()); // the primary hash itself is absent
    let file = fixture(&primary, &[("more", format!("{secondary} ").as_bytes())]);
    assert_eq!(
        Container::open(file.path())
            .unwrap()
            .verify_metadata(None)
            .unwrap()
            .checks
            .last()
            .unwrap()
            .outcome,
        CheckOutcome::Mismatch
    );
}

#[test]
fn full_verification_reports_block_corruption_and_gap_coverage() {
    use aff4_image::CheckOutcome;
    use md5::{Digest, Md5};
    let mut hashes = Md5::digest(b"abcd").to_vec();
    hashes.extend_from_slice(&Md5::digest(b"ef"));
    for corrupt in [false, true] {
        let mut recorded = hashes.clone();
        if corrupt {
            recorded[0] ^= 1;
        }
        let file = fixture(
            &metadata(),
            &[
                ("data/00000000", b"abcd"),
                ("data/00000001", b"ef\0\0"),
                ("data/00000000.index", &index(4)),
                ("data/00000001.index", &index(4)),
                ("data/00000000.blockHash.md5", &recorded[..16]),
                ("data/00000001.blockHash.md5", &recorded[16..]),
            ],
        );
        let mut image = Container::open(file.path()).unwrap();
        let report = image
            .verify_all(None, |_, _, _| ControlFlow::Continue(()))
            .unwrap();
        let blocks: Vec<_> = report
            .checks
            .iter()
            .filter(|c| c.reference_source.contains("blockHash"))
            .collect();
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0].outcome,
            if corrupt {
                CheckOutcome::Mismatch
            } else {
                CheckOutcome::Match
            }
        );
        assert_eq!(blocks[1].outcome, CheckOutcome::Match);
        assert!(
            report
                .resources
                .iter()
                .all(|r| r.coverage.as_ref().unwrap().stored == 6)
        );
        assert!(!report.all_match()); // no metadata hash reference
        assert!(matches!(
            image.verify_all(None, |_, _, _| ControlFlow::Break(())),
            Err(Error::Aborted)
        ));
    }
    let meta = "@prefix a: <http://aff4.org/Schema#> . <aff4://volume/map> a a:Map; a:size 5 .";
    let mut range = 1u64.to_le_bytes().to_vec();
    range.extend(2u64.to_le_bytes());
    range.extend(0u64.to_le_bytes());
    range.extend(0u32.to_le_bytes());
    let file = fixture(
        meta,
        &[
            ("map/map", &range),
            ("map/idx", b"http://aff4.org/Schema#Zero\n"),
        ],
    );
    let report = Container::open(file.path())
        .unwrap()
        .verify_all(None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    let c = report.resources[0].coverage.as_ref().unwrap();
    assert_eq!((c.stored, c.described, c.gap_filled), (0, 2, 3));
}

#[test]
fn paired_block_hashes_report_each_algorithm_and_preserve_work_limit() {
    use aff4_image::CheckOutcome;
    use md5::{Digest, Md5};
    use sha2::Sha256;
    let first_md5 = Md5::digest(b"abcd");
    let mut second_md5 = Md5::digest(b"ef").to_vec();
    second_md5[0] ^= 1;
    let first_sha256 = Sha256::digest(b"abcd");
    let mut second_sha256 = Sha256::digest(b"ef").to_vec();
    second_sha256[0] ^= 1;
    let file = fixture(
        &metadata(),
        &[
            ("data/00000000", b"abcd"),
            ("data/00000001", b"ef\0\0"),
            ("data/00000000.index", &index(4)),
            ("data/00000001.index", &index(4)),
            ("data/00000000.blockHash.md5", &first_md5),
            ("data/00000001.blockHash.md5", &second_md5),
            ("data/00000000.blockHash.sha256", &first_sha256),
            ("data/00000001.blockHash.sha256", &second_sha256),
        ],
    );
    let mut image = Container::open(file.path()).unwrap();
    let report = image
        .verify_all(None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    let blocks: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.reference_source.contains("blockHash"))
        .collect();
    assert_eq!(blocks.len(), 4);
    assert_eq!(
        blocks
            .iter()
            .map(|check| check.algorithm.as_str())
            .collect::<Vec<_>>(),
        ["MD5", "MD5", "SHA256", "SHA256"]
    );
    assert_eq!(blocks[0].outcome, CheckOutcome::Match);
    assert_eq!(blocks[1].outcome, CheckOutcome::Mismatch);
    assert_eq!(blocks[2].outcome, CheckOutcome::Match);
    assert_eq!(blocks[3].outcome, CheckOutcome::Mismatch);

    let mut bounded = Container::open_with_limits(
        file.path(),
        Limits {
            verification_bytes: 23,
            ..Limits::default()
        },
    )
    .unwrap();
    let report = bounded
        .verify_all(None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    assert!(report.checks.iter().any(|check| {
        check.reference_source == "block hashes"
            && check
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("verification byte limit"))
    }));
}

#[test]
#[ignore = "requires pinned Base-Linear-AllHashes canonical image"]
fn canonical_full_integrity_tree_matches_independent_producer() {
    use aff4_image::CheckOutcome;
    let path =
        std::env::var_os("AFF4_ALL_HASHES_REFERENCE").expect("AFF4_ALL_HASHES_REFERENCE required");
    let report = Container::open(path)
        .unwrap()
        .verify_all(None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    assert!(
        report.resources.iter().all(|r| r.error.is_none()),
        "{report:#?}"
    );
    assert!(report.checks.len() >= 20);
    for check in &report.checks {
        if check.reference_source.ends_with("#imageStreamHash") {
            assert_eq!(check.outcome, CheckOutcome::Unsupported);
        } else {
            assert_eq!(check.outcome, CheckOutcome::Match, "{check:#?}");
        }
    }
    assert!(report.checks.iter().any(|c| c.algorithm == "Blake2b"));
    assert!(
        report
            .resources
            .iter()
            .any(|r| r.coverage.as_ref().unwrap().described > 0)
    );
    assert!(!report.all_match()); // unidentified digest and absent metadata anchor
}

#[test]
fn streaming_metadata_preserves_repeated_subjects_and_import_provenance() {
    let primary = "@prefix a: <http://aff4.org/Schema#> . <aff4://one> a:size 1 . <aff4://two> a:size 2 . <aff4://one> a:fileName \"late\" . <aff4://volume> a:imports <aff4://volume/more> .";
    let file = fixture(
        primary,
        &[("more", b"<aff4://three> <http://aff4.org/Schema#size> 3 .")],
    );
    let mut records = Vec::new();
    let result = Container::scan_metadata(
        file.path(),
        Limits::default(),
        |source, subject, property| {
            records.push((source.to_owned(), subject.to_owned(), property.clone()));
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!((result.triples, result.stores), (5, 2));
    assert_eq!(
        records
            .iter()
            .filter(|(_, subject, _)| subject == "aff4://one")
            .count(),
        2
    );
    assert_eq!(records.last().unwrap().0, "more");
    assert!(
        Container::scan_metadata(
            file.path(),
            Limits {
                triples: 4,
                ..Limits::default()
            },
            |_, _, _| ControlFlow::Continue(())
        )
        .is_err()
    );
    assert!(matches!(
        Container::scan_metadata(
            file.path(),
            Limits::default(),
            |_, _, _| ControlFlow::Break(())
        ),
        Err(Error::Aborted)
    ));
}

#[test]
fn matching_resource_does_not_hide_another_streams_missing_references() {
    use aff4_image::CheckOutcome;
    use sha2::{Digest, Sha256};
    let primary = format!(
        "{}\n<aff4://volume/unchecked> a a:ImageStream; a:chunkSize 4; a:chunksInSegment 1; a:size 4 .",
        metadata()
    );
    let digest = Sha256::digest(primary.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let legacy = format!("{{\"sha256\":\"{digest}\"}}");
    let file = fixture(
        &primary,
        &[
            ("container.hashes", legacy.as_bytes()),
            ("data/00000000", b"abcd"),
            ("data/00000000.index", &index(4)),
            ("data/00000001", b"ef\0\0"),
            ("data/00000001.index", &index(4)),
            ("unchecked/00000000", b"free"),
            ("unchecked/00000000.index", &index(4)),
        ],
    );
    let report = Container::open(file.path())
        .unwrap()
        .verify_all(None, |_, _, _| ControlFlow::Continue(()))
        .unwrap();
    assert!(report.metadata.as_ref().unwrap().all_match());
    assert!(report.resources.iter().all(|r| r.error.is_none()));
    assert!(
        report
            .checks
            .iter()
            .any(|c| c.resource == "aff4://volume/unchecked" && c.outcome == CheckOutcome::Missing)
    );
    assert!(!report.all_match());
}
