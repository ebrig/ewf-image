#![no_main]

use ewf_image::{Image, OpenOptions, SegmentSource, SingleFileEntry};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 1024 * 1024 {
        return;
    }
    let Some((&kind, body)) = data.split_first() else {
        return;
    };

    if kind <= 1 {
        // Feed the UTF-16 catalog parser directly: container checksums would
        // otherwise reject almost every mutation before it reaches the tree.
        let _ = ewf_image::parse_single_files_for_fuzzing(body, kind == 0);
        return;
    }

    let extension = match kind {
        2 | 4 => "L",
        3 | 5 => "Lx",
        _ => return,
    };
    let options = OpenOptions::default()
        .with_chunk_cache_size_bytes(64 * 1024)
        .with_table_entry_cache_size_bytes(64 * 1024);
    let sources = if kind >= 4 {
        let Some((&count, mut remaining)) = body.split_first() else {
            return;
        };
        if !(2..=4).contains(&count) {
            return;
        }
        let mut sources = Vec::with_capacity(count as usize);
        for number in 1..=count {
            let Some(length_bytes) = remaining.get(..4) else {
                return;
            };
            let length = u32::from_le_bytes(length_bytes.try_into().unwrap()) as usize;
            let Some((segment, rest)) = remaining[4..].split_at_checked(length) else {
                return;
            };
            sources.push((
                format!("case.{extension}{number:02}"),
                SegmentSource::from_bytes(segment.to_vec()),
            ));
            remaining = rest;
        }
        if !remaining.is_empty() {
            return;
        }
        sources
    } else {
        vec![(
            format!("case.{extension}01"),
            SegmentSource::from_bytes(body.to_vec()),
        )]
    };

    if let Ok(image) = Image::open_sources_with_options(sources, options)
        && let Some(root) = image.root_file_entry()
    {
        visit_entries(root, |entry| {
            let Some(size) = entry.size.filter(|size| *size > 0) else {
                return;
            };
            let mut buffer = [0; 256];
            let length = size.min(buffer.len() as u64) as usize;
            for offset in [0, size / 2, size - 1] {
                let _ = image.read_single_file_at_strict(entry, &mut buffer[..length], offset);
            }
        });
    }
});

fn visit_entries(root: &SingleFileEntry, mut visit: impl FnMut(&SingleFileEntry)) {
    let mut stack = vec![root];
    for _ in 0..64 {
        let Some(entry) = stack.pop() else {
            break;
        };
        visit(entry);
        stack.extend(entry.children.iter().take(8).rev());
    }
}
