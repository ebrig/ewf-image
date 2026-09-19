//! Stream writer failure isolation and independent-consumer contracts.
use aff4_image::{Compression, Container, Profile, WriteOptions, Writer};
use std::fs;
use std::io::Cursor;
use std::ops::ControlFlow;

fn proceed(_: u64, _: u64) -> ControlFlow<()> {
    ControlFlow::Continue(())
}
fn data() -> Vec<u8> {
    (0..131079usize)
        .map(|i| {
            if i < 65536 {
                b'A'
            } else {
                (i * 47 + i / 19) as u8
            }
        })
        .collect()
}

#[test]
fn physical_codecs_bevies_padding_and_logical_zip_roundtrip() {
    let data = data();
    for codec in [
        Compression::Stored,
        Compression::Zlib,
        Compression::Snappy,
        Compression::Lz4,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.aff4");
        let mut writer = Writer::create(
            &path,
            Profile::Physical,
            WriteOptions {
                chunk_bytes: 32768,
                chunks_per_bevy: 2,
                compression: codec,
            },
        )
        .unwrap();
        let id = writer
            .add_image(data.len() as u64, &mut Cursor::new(&data), proceed)
            .unwrap();
        let result = writer.finish().unwrap();
        let mut image = Container::open(&path).unwrap();
        let report = image.verify(&id, proceed).unwrap();
        assert_eq!(report.references_match, Some(true));
        assert_eq!(report.sha256, result.streams[0].sha256);
        let mut bytes = vec![0; data.len()];
        image.read_at(&id, &mut bytes, 0).unwrap();
        assert_eq!(bytes, data);
    }
    for codec in [Compression::Stored, Compression::Zlib] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.aff4l");
        let mut writer = Writer::create(
            &path,
            Profile::Logical,
            WriteOptions {
                compression: codec,
                ..WriteOptions::default()
            },
        )
        .unwrap();
        let id = writer
            .add_file(
                "../metadata:only",
                data.len() as u64,
                &mut Cursor::new(&data),
                proceed,
            )
            .unwrap();
        let empty = writer
            .add_file("empty", 0, &mut Cursor::new([]), proceed)
            .unwrap();
        writer.finish().unwrap();
        let mut image = Container::open(path).unwrap();
        assert_eq!(
            image.verify(&id, proceed).unwrap().references_match,
            Some(true)
        );
        assert_eq!(
            image.verify(&empty, proceed).unwrap().references_match,
            Some(true)
        );
    }
}

#[test]
fn failures_cancellation_and_late_collisions_do_not_publish() {
    for profile in [Profile::Physical, Profile::Logical] {
        for cancel in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("case.aff4");
            let mut writer = Writer::create(&path, profile, WriteOptions::default()).unwrap();
            let callback = |_, _| {
                if cancel {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            };
            let result = match profile {
                Profile::Physical => writer.add_image(10, &mut Cursor::new(b"abc"), callback),
                Profile::Logical => writer.add_file("file", 10, &mut Cursor::new(b"abc"), callback),
            };
            assert!(result.is_err());
            assert!(writer.finish().is_err());
            assert!(!path.exists());
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("case.aff4");
    let mut writer = Writer::create(&path, Profile::Physical, WriteOptions::default()).unwrap();
    writer
        .add_image(3, &mut Cursor::new(b"abc"), proceed)
        .unwrap();
    fs::write(&path, b"existing").unwrap();
    assert!(writer.finish().is_err());
    assert_eq!(fs::read(&path).unwrap(), b"existing");
    assert!(Writer::create(&path, Profile::Physical, WriteOptions::default()).is_err());
}

#[test]
#[ignore = "requires the pinned independent aff4tools oracle"]
fn independent_consumer_exports_and_verifies_writer_output() {
    let oracle = std::env::var_os("AFF4_ORACLE").expect("AFF4_ORACLE required");
    let data = data();
    for profile in [Profile::Physical, Profile::Logical] {
        for codec in [
            Compression::Stored,
            Compression::Zlib,
            Compression::Snappy,
            Compression::Lz4,
        ] {
            if profile == Profile::Logical
                && !matches!(codec, Compression::Stored | Compression::Zlib)
            {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("case.aff4");
            let mut writer = Writer::create(
                &path,
                profile,
                WriteOptions {
                    chunk_bytes: 32768,
                    chunks_per_bevy: 2,
                    compression: codec,
                },
            )
            .unwrap();
            match profile {
                Profile::Physical => {
                    writer
                        .add_image(data.len() as u64, &mut Cursor::new(&data), proceed)
                        .unwrap();
                }
                Profile::Logical => {
                    writer
                        .add_file(
                            "evidence.bin",
                            data.len() as u64,
                            &mut Cursor::new(&data),
                            proceed,
                        )
                        .unwrap();
                }
            }
            let written = writer.finish().unwrap();
            let conformance = std::process::Command::new(&oracle)
                .args(["conformance", "--strict"])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                conformance.status.success(),
                "{:?}/{:?}: {} {}",
                profile,
                codec,
                String::from_utf8_lossy(&conformance.stdout),
                String::from_utf8_lossy(&conformance.stderr)
            );
            let verify = std::process::Command::new(&oracle)
                .arg("verify")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                verify.status.success(),
                "{:?}/{:?}: {} {}",
                profile,
                codec,
                String::from_utf8_lossy(&verify.stdout),
                String::from_utf8_lossy(&verify.stderr)
            );
            let output = dir.path().join("export");
            let export = std::process::Command::new(&oracle)
                .arg("export")
                .arg(&path)
                .arg(if profile == Profile::Physical {
                    "--output"
                } else {
                    "--logical"
                })
                .arg(&output)
                .output()
                .unwrap();
            assert!(
                export.status.success(),
                "{} {}",
                String::from_utf8_lossy(&export.stdout),
                String::from_utf8_lossy(&export.stderr)
            );
            let output = if profile == Profile::Physical {
                output
            } else {
                output
                    .join("files")
                    .join(written.streams[0].id.rsplit('/').next().unwrap())
            };
            assert_eq!(fs::read(output).unwrap(), data);
        }
    }
}

#[test]
#[ignore = "requires the pinned independent aff4tools oracle"]
fn independent_producer_images_and_files_are_readable() {
    let oracle = std::env::var_os("AFF4_ORACLE").expect("AFF4_ORACLE required");
    let data = data();
    for logical in [false, true] {
        for codec in ["stored", "zlib", "snappy", "lz4"] {
            let dir = tempfile::tempdir().unwrap();
            let input = dir.path().join("evidence.bin");
            fs::write(&input, &data).unwrap();
            let output = dir.path().join("case.aff4");
            let acquired = std::process::Command::new(&oracle)
                .arg("acquire")
                .arg(if logical { "--logical" } else { "--image" })
                .arg(&input)
                .arg("--output")
                .arg(&output)
                .args([
                    "--compression",
                    codec,
                    "--chunk-size",
                    "32768",
                    "--chunks-per-bevy",
                    "2",
                ])
                .output()
                .unwrap();
            assert!(
                acquired.status.success(),
                "{logical}/{codec}: {} {}",
                String::from_utf8_lossy(&acquired.stdout),
                String::from_utf8_lossy(&acquired.stderr)
            );
            let mut image = Container::open(output).unwrap();
            let streams = image.streams().unwrap();
            let stream = streams
                .iter()
                .find(|s| {
                    s.types
                        .iter()
                        .any(|t| t.ends_with(if logical { "#FileImage" } else { "#DiskImage" }))
                })
                .unwrap();
            let mut bytes = vec![0; data.len()];
            assert_eq!(
                image.read_at(&stream.id, &mut bytes, 0).unwrap(),
                data.len()
            );
            assert_eq!(bytes, data);
            let verified = image.verify(&stream.id, proceed).unwrap();
            assert_ne!(verified.references_match, Some(false));
            assert_eq!(verified.bytes_verified, data.len() as u64);
        }
    }
}
