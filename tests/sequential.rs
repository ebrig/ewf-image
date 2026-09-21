//! Bounded EWF2 writer behavior and publication regressions.
use ewf_image::{
    Image, LogicalEntryMetadata, LogicalWriter, SequentialOptions, SequentialWriter,
    WriteCompression, WriteFormat,
};
use std::io::{Cursor, Read};
use std::ops::ControlFlow;

fn data(size: usize) -> Vec<u8> {
    (0..size)
        .map(|i| ((i * 71 + i / 257) % 251) as u8)
        .collect()
}

fn options(size: usize) -> SequentialOptions {
    let mut options = SequentialOptions::new(size as u64);
    options.write.sectors_per_chunk = 1;
    options.chunks_per_segment = 2;
    options
}

#[test]
fn sequential_split_padding_compression_and_mirror() {
    for compression in [
        WriteCompression::None,
        WriteCompression::Zlib,
        WriteCompression::Bzip2,
    ] {
        for size in [0, 1, 512, 1024, 1025, 4096, 4107] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("case.Ex01");
            let mirror = dir.path().join("mirror.Ex01");
            let mut options = options(size);
            options.write.compression = compression;
            options.write.secondary_segment_filename = Some(mirror.clone());
            let mut writer = SequentialWriter::create(&path, options).unwrap();
            let mut source = data(size);
            for part in source.chunks(113) {
                writer.write_all(part).unwrap();
            }
            assert_eq!(writer.position(), size as u64);
            assert!(!path.exists());
            let result = writer.finish().unwrap();
            assert_eq!(result.segment_paths.len(), size.div_ceil(1024).max(1));
            source.resize(size.div_ceil(512) * 512, 0);
            for path in [&path, &mirror] {
                let image = Image::open(path).unwrap();
                let mut decoded = Vec::new();
                image.cursor().read_to_end(&mut decoded).unwrap();
                assert_eq!(decoded, source);
                #[cfg(feature = "verify")]
                assert_eq!(image.verify().unwrap().md5_match, Some(true));
            }
        }
    }
}

#[test]
fn sequential_logical_catalog_finishes_after_first_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.Lx01");
    let source = data(5001);
    let mut options = options(source.len());
    options.write.format = WriteFormat::Ewf2Logical;
    let mut writer = LogicalWriter::create_sequential(&path, options).unwrap();
    let folder = writer
        .add_directory(
            1,
            LogicalEntryMetadata {
                name: "folder".into(),
                ..Default::default()
            },
        )
        .unwrap();
    for (name, bytes) in [
        ("first", &source[..4000]),
        ("empty", &[][..]),
        ("last", &source[4000..]),
    ] {
        writer
            .add_file(
                folder,
                LogicalEntryMetadata {
                    name: name.into(),
                    ..Default::default()
                },
                bytes.len() as u64,
                &mut Cursor::new(bytes),
            )
            .unwrap();
    }
    let result = writer.finish().unwrap();
    assert_eq!(result.segment_paths.len(), 5);
    let image = Image::open(path).unwrap();
    for (name, bytes) in [
        ("first", &source[..4000]),
        ("empty", &[][..]),
        ("last", &source[4000..]),
    ] {
        let entry = image
            .file_entry_by_path(&format!("folder\t{name}"))
            .unwrap()
            .unwrap();
        let mut decoded = Vec::new();
        image
            .single_file_cursor(entry)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, bytes);
        #[cfg(feature = "verify")]
        assert_eq!(
            image.verify_single_file(entry).unwrap().references_match(),
            Some(true)
        );
    }
}

#[test]
fn sequential_failed_input_drop_and_no_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.Ex01");
    for oversize in [false, true] {
        let mut writer = SequentialWriter::create(&path, options(2048)).unwrap();
        writer.write_all(&data(1536)).unwrap();
        if oversize {
            assert!(writer.write_all(&data(1024)).is_err());
        }
        assert!(writer.finish().is_err());
        assert!(!path.exists());
        // Drop must release locks and remove staging so another create succeeds.
        drop(SequentialWriter::create(&path, options(2048)).unwrap());
    }
    std::fs::write(&path, b"existing").unwrap();
    let mut writer = SequentialWriter::create(&path, options(2048)).unwrap();
    writer.write_all(&data(2048)).unwrap();
    assert!(writer.finish().is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"existing");
}

#[test]
fn sequential_logical_cancel_never_publishes_staged_segments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.Lx01");
    let mut options = options(2 * 1024 * 1024);
    options.write.format = WriteFormat::Ewf2Logical;
    let mut writer = LogicalWriter::create_sequential(&path, options).unwrap();
    assert!(
        writer
            .add_file_with_progress(
                1,
                LogicalEntryMetadata {
                    name: "cancel".into(),
                    ..Default::default()
                },
                2 * 1024 * 1024,
                &mut std::io::repeat(3),
                |p| if p.bytes_written > 0 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            )
            .is_err()
    );
    assert!(writer.finish().is_err());
    assert!(!path.exists());
    assert!(!ewf_image::EwfWriter::recover_output(&path, None).unwrap());
}

#[test]
fn sequential_crash_worker() {
    let Some(path) = std::env::var_os("EWF_SEQUENTIAL_CRASH_PATH") else {
        return;
    };
    let mut writer = SequentialWriter::create(path, options(8192)).unwrap();
    writer.write_all(&data(4096)).unwrap();
    // Abrupt termination: do not run Drop for any staging or lock handles.
    std::process::exit(73);
}

#[test]
fn sequential_process_interruption_recovers_unpublished_staging() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.Ex01");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sequential_crash_worker"])
        .env("EWF_SEQUENTIAL_CRASH_PATH", &path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    assert!(!path.exists());
    assert!(SequentialWriter::create(&path, options(8192)).is_err());
    assert!(ewf_image::EwfWriter::recover_output(&path, None).unwrap());
    drop(SequentialWriter::create(&path, options(8192)).unwrap());
}
