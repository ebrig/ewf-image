//! EWF1 writer tests for standard compression labels and SMART volume padding.

use std::fs;

use ewf_image::{
    EwfWriter, Image, WriteCompression, WriteCompressionLevel, WriteCompressionValues, WriteFormat,
    WriteOptions,
};
use tempfile::tempdir;

fn section_payload<'a>(segment: &'a [u8], name: &[u8]) -> &'a [u8] {
    const FILE_HEADER_SIZE: usize = 13;
    const SECTION_HEADER_SIZE: usize = 76;

    let mut offset = FILE_HEADER_SIZE;
    loop {
        let header = &segment[offset..offset + SECTION_HEADER_SIZE];
        let next = u64::from_le_bytes(header[16..24].try_into().unwrap()) as usize;
        if header[..16].split(|&byte| byte == 0).next().unwrap() == name {
            return &segment[offset + SECTION_HEADER_SIZE..next];
        }
        assert!(next > offset, "section {name:?} was not written");
        offset = next;
    }
}

#[test]
fn standard_ewf1_volume_labels_match_configured_media_compression() {
    use WriteCompression::{None as Stored, Zlib};
    use WriteCompressionLevel::{Best, Default, Fast, None as LevelNone};
    let dir = tempdir().unwrap();
    let data = vec![0x41; 32_768];

    for (format, extension) in [
        (WriteFormat::Ewf1Physical, "E01"),
        (WriteFormat::Ewf1Logical, "L01"),
    ] {
        for (index, (method, level, expected)) in [
            (Stored, Default, 0),
            (Stored, Best, 0),
            (Zlib, LevelNone, 0),
            (Zlib, Default, 1),
            (Zlib, Fast, 1),
            (Zlib, Best, 2),
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.path().join(format!("volume-{index}.{extension}"));
            let mut writer = EwfWriter::create(
                &path,
                WriteOptions {
                    format,
                    compression: method,
                    compression_values: WriteCompressionValues {
                        level,
                        empty_block: false,
                    },
                    ..WriteOptions::default()
                },
            )
            .unwrap();
            writer.write_all(&data).unwrap();
            writer.finish().unwrap();

            let bytes = fs::read(&path).unwrap();
            let volume = section_payload(&bytes, b"volume");
            assert_eq!(volume.len(), 1052);
            assert_eq!(volume[52], expected, "{format:?}: {method:?}/{level:?}");

            let table = section_payload(&bytes, b"table");
            let entry = u32::from_le_bytes(table[24..28].try_into().unwrap());
            assert_eq!(
                entry >> 31 != 0,
                method == Zlib && level != LevelNone,
                "{format:?}: {method:?}/{level:?}"
            );
        }
    }
}

#[test]
fn smart_volume_keeps_byte_52_reserved_even_with_compression() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("volume.S01");
    let data = vec![0x53; 32_768];
    let mut writer = EwfWriter::create(
        &path,
        WriteOptions {
            format: WriteFormat::Ewf1Smart,
            compression: WriteCompression::Zlib,
            compression_values: WriteCompressionValues {
                level: WriteCompressionLevel::Best,
                empty_block: false,
            },
            ..WriteOptions::default()
        },
    )
    .unwrap();
    writer.write_all(&data).unwrap();
    writer.finish().unwrap();

    let bytes = fs::read(&path).unwrap();
    let volume = section_payload(&bytes, b"volume");
    assert_eq!(volume.len(), 94);
    assert_eq!(&volume[85..90], b"SMART");
    assert_eq!(volume[52], 0);

    let image = Image::open(&path).unwrap();
    let mut decoded = vec![0; data.len()];
    assert_eq!(image.read_at(&mut decoded, 0).unwrap(), data.len());
    assert_eq!(decoded, data);
}
