//! Physical selection and cursor semantics, independent of the AFF4 writer.
use aff4_image::Container;
use std::io::{Read, Seek, SeekFrom, Write};
use zip::{ZipWriter, write::SimpleFileOptions};

fn fixture(metadata: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let mut zip = ZipWriter::new(file.reopen().unwrap());
    zip.set_comment("aff4://volume").unwrap();
    for (name, bytes) in [
        ("version.txt", b"major=1\nminor=0\n".as_slice()),
        ("information.turtle", metadata.as_bytes()),
        ("data", b"abcdefgh"),
        ("map/map", b""),
        ("map/idx", b""),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    file
}

fn metadata() -> String {
    "@prefix a: <http://aff4.org/Schema#> .
    <aff4://volume/disk> a a:DiskImage, a:Image; a:dataStream <aff4://volume/map>; a:size 8; a:blockSize 512 .
    <aff4://volume/map> a a:Image, a:ZipSegment; a:size 8; a:dataStream <aff4://volume/data> .
    <aff4://volume/data> a a:ZipSegment; a:size 8 .".into()
}

#[test]
fn selects_disk_not_storage_and_supports_seek_and_positioned_reads() {
    let file = fixture(&metadata());
    let container = Container::open(file.path()).unwrap();
    assert_eq!(container.disk_images().unwrap().len(), 1);
    let mut reader = container.into_disk_reader(None).unwrap();
    assert_eq!(reader.info().resource_id, "aff4://volume/disk");
    assert_eq!(reader.info().logical_size, 8);
    assert_eq!(reader.info().block_size, Some(512));
    assert_eq!(reader.seek(SeekFrom::End(-3)).unwrap(), 5);
    let mut tail = [0; 3];
    reader.read_exact(&mut tail).unwrap();
    assert_eq!(&tail, b"fgh");
    assert_eq!(reader.read(&mut tail).unwrap(), 0);
    reader.read_at(&mut tail, 1).unwrap();
    assert_eq!(&tail, b"bcd");
    assert_eq!(reader.stream_position().unwrap(), 8);
    assert!(reader.seek(SeekFrom::Start(u64::MAX)).is_ok());
    assert!(reader.seek(SeekFrom::Current(1)).is_err());
    reader.rewind().unwrap();
    assert!(reader.seek(SeekFrom::Current(-1)).is_err());
    assert_eq!(reader.stream_position().unwrap(), 0);
}

#[test]
fn ambiguity_requires_an_explicit_disk_and_storage_cannot_be_selected() {
    let text = format!(
        "{}\n<aff4://volume/other> a <http://aff4.org/Schema#DiskImage>; <http://aff4.org/Schema#dataStream> <aff4://volume/data> .",
        metadata()
    );
    let file = fixture(&text);
    assert!(
        Container::open(file.path())
            .unwrap()
            .into_disk_reader(None)
            .is_err()
    );
    let selected = Container::open(file.path())
        .unwrap()
        .into_disk_reader(Some("aff4://volume/other"))
        .unwrap();
    assert_eq!(selected.info().resource_id, "aff4://volume/other");
    assert!(
        Container::open(file.path())
            .unwrap()
            .into_disk_reader(Some("aff4://volume/data"))
            .is_err()
    );
}

#[test]
fn rejects_conflicting_geometry_and_nondisk_resources() {
    for text in [
        metadata().replace("a:blockSize 512", "a:blockSize 512, 4096"),
        metadata().replace("a:blockSize 512", "a:blockSize 0"),
        metadata().replace("a:DiskImage, a:Image", "a:DiskImage, a:MemoryImage"),
        metadata().replace("a:DiskImage, a:Image", "a:FileImage, a:Image"),
    ] {
        let file = fixture(&text);
        assert!(
            Container::open(file.path())
                .unwrap()
                .into_disk_reader(None)
                .is_err()
        );
    }
}

#[test]
fn unreadable_ranges_remain_errors_and_do_not_advance_cursor() {
    let text = metadata().replace(
        "a:Image, a:ZipSegment; a:size 8; a:dataStream <aff4://volume/data>",
        "a:Map; a:size 8; a:mapGapDefaultStream a:UnreadableData",
    );
    let file = fixture(&text);
    let mut reader = Container::open(file.path())
        .unwrap()
        .into_disk_reader(None)
        .unwrap();
    assert!(reader.read(&mut [0; 4]).is_err());
    assert_eq!(reader.stream_position().unwrap(), 0);
}

#[test]
#[ignore = "requires the SHA256-pinned public canonical reference"]
fn canonical_physical_cursor_uses_mapped_disk_size_and_sector_geometry() {
    let path = std::env::var_os("AFF4_REFERENCE_IMAGE").expect("AFF4_REFERENCE_IMAGE required");
    let bytes = std::fs::read(&path).unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(
        Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        "bcde3297ae95cd9df214bfb79821334628dad08f21ef38374a2c091481e391c0"
    );
    let mut reader = Container::open(path)
        .unwrap()
        .into_disk_reader(None)
        .unwrap();
    assert_eq!(reader.info().logical_size, 268435456);
    assert_eq!(reader.info().block_size, Some(512));
    let mut sector = [0; 512];
    reader.read_exact(&mut sector).unwrap();
    assert_eq!(&sector[510..], &[0x55, 0xaa]);
    reader.seek(SeekFrom::End(-512)).unwrap();
    reader.read_exact(&mut sector).unwrap();
}
