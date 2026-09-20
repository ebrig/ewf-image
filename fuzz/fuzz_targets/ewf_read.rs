#![no_main]
use ewf_image::{Image, OpenOptions, SegmentSource};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    let options = OpenOptions::default()
        .with_chunk_cache_size_bytes(64 * 1024)
        .with_table_entry_cache_size_bytes(64 * 1024);
    if let Ok(image) = Image::open_sources_with_options(
        [("input.E01", SegmentSource::from_bytes(data.to_vec()))],
        options,
    ) {
        let mut buffer = [0; 4096];
        for offset in [
            0,
            image.media_size() / 2,
            image.media_size().saturating_sub(1),
        ] {
            let _ = image.read_at(&mut buffer, offset);
        }
    }
});
