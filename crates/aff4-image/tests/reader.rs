//! AFF4 bounds, map semantics, and independent reference regressions.
use aff4_image::{Container, Error, Limits};
use std::fs::File;
use std::io::Write;
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
