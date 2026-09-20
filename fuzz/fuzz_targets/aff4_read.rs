#![no_main]
use aff4_image::{Container, Limits};
use libfuzzer_sys::fuzz_target;
use std::{io::Write, ops::ControlFlow};

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(data).unwrap();
    let limits = Limits {
        directory_bytes: 64 * 1024,
        archive_entries: 128,
        metadata_bytes: 64 * 1024,
        member_bytes: 64 * 1024,
        chunk_bytes: 64 * 1024,
        triples: 512,
        map_bytes: 64 * 1024,
        verification_bytes: 1024 * 1024,
    };
    let _ = Container::scan_metadata(file.path(), limits.clone(), |_, _, _| {
        ControlFlow::Continue(())
    });
    if let Ok(mut container) = Container::open_with_limits(file.path(), limits) {
        if let Ok(streams) = container.streams() {
            for stream in streams.into_iter().take(8) {
                let _ = container.read_at(&stream.id, &mut [0; 256], 0);
            }
        }
        let _ = container.verify_all(None, |_, _, _| ControlFlow::Continue(()));
    }
});
