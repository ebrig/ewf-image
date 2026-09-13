//! Streaming acquisition, checkpoint, and crash recovery regressions.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use ewf_image::{
    AcquisitionOptions, AcquisitionWriter, EwfWriter, Image, WriteCompression, WriteOptions,
};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const IDENTITY: [u8; 32] = [0x51; 32];

fn options(size: usize, compression: WriteCompression) -> AcquisitionOptions {
    AcquisitionOptions {
        sectors_per_chunk: 2,
        chunks_per_segment: 3,
        compression,
        ..AcquisitionOptions::new(size as u64)
    }
}

fn data(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| ((index * 139 + index / 13) % 256) as u8)
        .collect()
}

fn check_image(path: &Path, expected: &[u8]) {
    let image = Image::open(path).unwrap();
    assert!(image.info().acquisition_complete);
    assert_eq!(image.media_size(), expected.len() as u64);
    let mut actual = Vec::new();
    image.cursor().read_to_end(&mut actual).unwrap();
    assert_eq!(actual, expected);
    let expected_sha256 = Sha256::digest(expected)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(image.hash_value("SHA256").unwrap(), expected_sha256);
    #[cfg(feature = "verify")]
    {
        let result = image.verify().unwrap();
        assert_eq!(result.md5_match, Some(true));
        assert_eq!(result.sha1_match, Some(true));
        assert_eq!(result.sha256_match, Some(true));
    }
}

#[test]
fn streaming_boundaries_and_final_short_chunk() {
    for compression in [WriteCompression::None, WriteCompression::Zlib] {
        for size in [512, 1024, 3072, 3584, 12 * 1024] {
            let dir = tempdir().unwrap();
            let path = dir.path().join("case.E01");
            let bytes = data(size);
            let mut writer =
                AcquisitionWriter::create(&path, &options(size, compression), IDENTITY).unwrap();
            for part in bytes.chunks(137) {
                writer.write_all(part).unwrap();
            }
            assert_eq!(writer.position(), size as u64);
            assert_eq!(writer.checkpoint_offset(), size as u64);
            assert_eq!(writer.sealed_segments(), size.div_ceil(3072));
            assert!(!path.exists());
            let result = writer.finish().unwrap();
            assert_eq!(
                result.computed_sha256,
                <[u8; 32]>::from(Sha256::digest(&bytes))
            );
            assert_eq!(result.chunk_count, size.div_ceil(1024) as u64);
            check_image(&path, &bytes);
        }
    }
}

#[test]
fn resume_discards_only_unsealed_tail_and_preserves_sealed_bytes() {
    for compression in [WriteCompression::None, WriteCompression::Zlib] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("case.E01");
        let bytes = data(9 * 1024 + 512);
        let opts = options(bytes.len(), compression);
        let mut writer = AcquisitionWriter::create(&path, &opts, IDENTITY).unwrap();
        writer.write_all(&bytes[..4500]).unwrap();
        assert_eq!(writer.checkpoint_offset(), 3072);
        assert_eq!(writer.checkpoint().unwrap(), 4096);
        assert_eq!(writer.position(), 4500);
        drop(writer);
        let state = dir.path().join(".case.E01.ewf-acquisition");
        let first_bytes = fs::read(state.join("case.E01")).unwrap();
        let second_bytes = fs::read(state.join("case.E02")).unwrap();
        // A segment written before its record is not an acknowledged checkpoint.
        fs::write(state.join("case.E03"), b"interrupted seal").unwrap();
        fs::write(state.join("scratch").join("orphan"), b"interrupted scratch").unwrap();
        let mut resumed = AcquisitionWriter::resume(&path, &opts, IDENTITY).unwrap();
        assert_eq!(resumed.position(), 4096);
        assert!(!state.join("case.E03").exists());
        assert!(!state.join("scratch").join("orphan").exists());
        resumed.write_all(&bytes[4096..]).unwrap();
        let result = resumed.finish().unwrap();
        assert_eq!(fs::read(&result.segment_paths[0]).unwrap(), first_bytes);
        assert_eq!(fs::read(&result.segment_paths[1]).unwrap(), second_bytes);
        check_image(&path, &bytes);
    }
}

#[test]
fn resume_zero_and_fully_checkpointed_sources() {
    for initial in [0, 317, 4096] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("case.E01");
        let bytes = data(4096);
        let opts = options(bytes.len(), WriteCompression::Zlib);
        let mut writer = AcquisitionWriter::create(&path, &opts, IDENTITY).unwrap();
        writer.write_all(&bytes[..initial]).unwrap();
        drop(writer);
        let mut writer = AcquisitionWriter::resume(&path, &opts, IDENTITY).unwrap();
        let offset = writer.position() as usize;
        writer.write_all(&bytes[offset..]).unwrap();
        writer.finish().unwrap();
        check_image(&path, &bytes);
    }
}

#[test]
fn identity_configuration_and_active_writer_are_checked() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("case.E01");
    let opts = options(4096, WriteCompression::Zlib);
    let mut writer = AcquisitionWriter::create(&path, &opts, IDENTITY).unwrap();
    writer.write_all(&data(3072)).unwrap();
    assert!(AcquisitionWriter::resume(&path, &opts, IDENTITY).is_err());
    assert!(AcquisitionWriter::create(&path, &opts, IDENTITY).is_err());
    drop(writer);
    assert!(AcquisitionWriter::resume(&path, &opts, [7; 32]).is_err());
    let mut changed = opts.clone();
    changed.source_size += 512;
    assert!(AcquisitionWriter::resume(&path, &changed, IDENTITY).is_err());
    let mut changed = opts.clone();
    changed.chunks_per_segment = 2;
    assert!(AcquisitionWriter::resume(&path, &changed, IDENTITY).is_err());
    let mut changed = opts.clone();
    changed.metadata.set_header_value("case_number", "changed");
    assert!(AcquisitionWriter::resume(&path, &changed, IDENTITY).is_err());
    let mut other = EwfWriter::create(
        &path,
        WriteOptions {
            overwrite_existing: true,
            ..WriteOptions::default()
        },
    )
    .unwrap();
    other.write_all(&data(4096)).unwrap();
    assert!(other.finish().is_err());
    assert!(AcquisitionWriter::resume(&path, &opts, IDENTITY).is_ok());
}

#[test]
fn damaged_sealed_data_or_checkpoint_is_rejected() {
    for corrupt_record in [false, true] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("case.E01");
        let opts = options(4096, WriteCompression::None);
        let mut writer = AcquisitionWriter::create(&path, &opts, IDENTITY).unwrap();
        writer.write_all(&data(3072)).unwrap();
        drop(writer);
        let state = dir.path().join(".case.E01.ewf-acquisition");
        let target = state.join(if corrupt_record {
            "checkpoint-00001"
        } else {
            "case.E01"
        });
        let mut bytes = fs::read(&target).unwrap();
        let index = bytes.len() / 2;
        bytes[index] ^= 1;
        fs::write(&target, &bytes).unwrap();
        assert!(AcquisitionWriter::resume(&path, &opts, IDENTITY).is_err());
        assert_eq!(fs::read(&target).unwrap(), bytes);
    }
}

#[test]
fn refuses_existing_outputs_and_wrong_lengths() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("case.E01");
    let opts = options(4096, WriteCompression::Zlib);
    fs::write(path.with_extension("E04"), b"existing").unwrap();
    assert!(AcquisitionWriter::create(&path, &opts, IDENTITY).is_err());
    fs::remove_file(path.with_extension("E04")).unwrap();
    let mut writer = AcquisitionWriter::create(&path, &opts, IDENTITY).unwrap();
    writer.write_all(&data(100)).unwrap();
    assert!(writer.finish().is_err());
    let mut writer = AcquisitionWriter::resume(&path, &opts, IDENTITY).unwrap();
    writer.write_all(&data(4096)).unwrap();
    assert!(writer.write_all(&[1]).is_err());
    assert!(writer.finish().is_err());
    AcquisitionWriter::resume(&path, &opts, IDENTITY)
        .unwrap()
        .finish()
        .unwrap();
    check_image(&path, &data(4096));
}

#[test]
fn rejects_unsupported_geometry_before_creating_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("case.E01");
    for size in [0, 1, 513] {
        assert!(
            AcquisitionWriter::create(&path, &AcquisitionOptions::new(size), IDENTITY).is_err()
        );
    }
    let mut opts = AcquisitionOptions::new(4096);
    opts.chunks_per_segment = 0;
    assert!(AcquisitionWriter::create(&path, &opts, IDENTITY).is_err());
    let mut opts = AcquisitionOptions::new(4096);
    opts.compression = WriteCompression::Bzip2;
    assert!(AcquisitionWriter::create(&path, &opts, IDENTITY).is_err());
    assert!(!dir.path().join(".case.E01.ewf-acquisition").exists());
}
