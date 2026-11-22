//! Segment-chain regressions for complete and incomplete EWF1 output.

use ewf_image::{
    EwfWriter, Image, SequentialOptions, SequentialWriter, WriteCompression, WriteFormat,
    WriteOptions,
};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const FILE_HEADER_SIZE: usize = 13;
const SECTION_DESCRIPTOR_SIZE: usize = 76;

fn media() -> Vec<u8> {
    (0..16_384)
        .map(|i| ((i * 97 + i / 277) % 251) as u8)
        .collect()
}

fn options(compression: WriteCompression) -> WriteOptions {
    WriteOptions {
        format: WriteFormat::Ewf1Physical,
        maximum_segment_size: Some(4096),
        sectors_per_chunk: 1,
        compression,
        ..WriteOptions::default()
    }
}

fn write_file(path: &Path, options: WriteOptions, media: &[u8]) -> Vec<PathBuf> {
    let mut writer = EwfWriter::create(path, options).unwrap();
    writer.write_all(media).unwrap();
    writer.finish().unwrap().segment_paths
}

fn check_segment(path: &Path, expected_marker: &[u8], expect_digest: bool) {
    let bytes = fs::read(path).unwrap();
    let mut offset = FILE_HEADER_SIZE;
    let mut has_digest = false;
    loop {
        let end = offset + SECTION_DESCRIPTOR_SIZE;
        let descriptor = &bytes[offset..end];
        let name = descriptor[..16].split(|&byte| byte == 0).next().unwrap();
        if matches!(name, b"next" | b"done") {
            assert_eq!(name, expected_marker, "{}", path.display());
            assert_eq!(end, bytes.len(), "{}", path.display());
            assert_eq!(has_digest, expect_digest, "{}", path.display());
            return;
        }
        has_digest |= name == b"digest";
        let next = u64::from_le_bytes(descriptor[16..24].try_into().unwrap());
        let next = usize::try_from(next).unwrap();
        assert!(next >= end && next < bytes.len(), "{}", path.display());
        offset = next;
    }
}

fn check_chain(paths: &[PathBuf], expected: &[u8], complete: bool) {
    assert!(paths.len() > 1, "fixture did not split");
    for (index, path) in paths.iter().enumerate() {
        let final_complete_segment = complete && index + 1 == paths.len();
        check_segment(
            path,
            if final_complete_segment {
                b"done"
            } else {
                b"next"
            },
            final_complete_segment,
        );
    }

    let image = Image::open(&paths[0]).unwrap();
    assert_eq!(image.info().segment_paths, paths);
    assert_eq!(image.info().acquisition_complete, complete);
    let mut bytes = Vec::new();
    image.cursor().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, expected);
}

#[test]
fn file_backed_split_ewf1_marks_only_final_segment_done() {
    for compression in [WriteCompression::None, WriteCompression::Zlib] {
        let dir = tempfile::tempdir().unwrap();
        let media = media();
        let paths = write_file(&dir.path().join("split.E01"), options(compression), &media);
        check_chain(&paths, &media, true);
    }
}

#[test]
fn supplied_split_ewf1_marks_only_final_segment_done() {
    for compression in [WriteCompression::None, WriteCompression::Zlib] {
        let dir = tempfile::tempdir().unwrap();
        let media = media();
        let options = options(compression);
        // Determine the number of sinks from identical input and size limits.
        let count = write_file(&dir.path().join("count.E01"), options.clone(), &media).len();
        let sinks = (1..=count)
            .map(|index| {
                let path = dir.path().join(format!("supplied.E{index:02}"));
                let sink = File::create(&path).unwrap();
                (path, sink)
            })
            .collect::<Vec<_>>();
        let mut writer = EwfWriter::create(&sinks[0].0, options).unwrap();
        writer.write_all(&media).unwrap();
        let paths = writer
            .finish_to_segment_writers(sinks)
            .unwrap()
            .segment_paths;
        assert_eq!(paths.len(), count);
        check_chain(&paths, &media, true);
    }
}

#[test]
fn sequential_split_ewf1_marks_only_final_segment_done() {
    for compression in [WriteCompression::None, WriteCompression::Zlib] {
        let dir = tempfile::tempdir().unwrap();
        let media = media();
        let mut settings = SequentialOptions::new(media.len() as u64);
        settings.write = options(compression);
        settings.chunks_per_segment = 3;
        let mut writer = SequentialWriter::create(dir.path().join("seq.E01"), settings).unwrap();
        writer.write_all(&media).unwrap();
        let paths = writer.finish().unwrap().segment_paths;
        check_chain(&paths, &media, true);
    }
}

#[test]
fn incomplete_split_ewf1_keeps_all_segments_open_for_continuation() {
    let dir = tempfile::tempdir().unwrap();
    let media = media();
    let mut writer = EwfWriter::create(
        dir.path().join("partial.E01"),
        options(WriteCompression::None),
    )
    .unwrap();
    writer.write_all(&media).unwrap();
    let paths = writer.finish_incomplete().unwrap().segment_paths;
    check_chain(&paths, &media, false);
}
