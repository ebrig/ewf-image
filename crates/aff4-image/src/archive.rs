//! Bounded admission of single-disk, unshifted ZIP/ZIP64 directories.
use super::*;
use std::io::{Seek, SeekFrom};

#[derive(Default)]
pub(super) struct Usage {
    pub directory_bytes: u64,
    pub entries: usize,
    pub metadata_bytes: u64,
    pub triples: usize,
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

pub(super) fn open(path: &Path, limits: &Limits) -> Result<(ZipArchive<File>, Usage)> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let tail_size = length.min(22 + u64::from(u16::MAX)) as usize;
    let mut tail = vec![0; tail_size];
    file.seek(SeekFrom::Start(length - tail_size as u64))?;
    file.read_exact(&mut tail)?;
    // Reject ambiguous end records rather than allow the dependency to retry an
    // unbudgeted older directory. Appended ZIP histories are not this profile.
    let mut candidates = tail
        .windows(4)
        .enumerate()
        .filter_map(|(n, b)| (b == b"PK\x05\x06").then_some(n));
    let at = candidates
        .next()
        .ok_or_else(|| malformed("missing ZIP end record"))?;
    if candidates.next().is_some() || tail.len() - at < 22 {
        return Err(malformed("ambiguous or truncated ZIP end record"));
    }
    let end = &tail[at..];
    if end.len() != 22 + usize::from(u16_at(end, 20))
        || u16_at(end, 4) != 0
        || u16_at(end, 6) != 0
        || u16_at(end, 8) != u16_at(end, 10)
    {
        return Err(malformed("unsupported ZIP end layout"));
    }
    let end_offset = length - tail_size as u64 + at as u64;
    let mut entries = u64::from(u16_at(end, 10));
    let mut directory_bytes = u64::from(u32_at(end, 12));
    let mut directory_offset = u64::from(u32_at(end, 16));
    let mut directory_end = end_offset;
    // ZIP64 may be present even when no classic field is saturated.
    let mut locator = [0; 20];
    let zip64 = if end_offset >= 20 {
        file.seek(SeekFrom::Start(end_offset - 20))?;
        file.read_exact(&mut locator)?;
        &locator[..4] == b"PK\x06\x07"
    } else {
        false
    };
    if zip64 {
        if u32_at(&locator, 4) != 0 || u32_at(&locator, 16) != 1 {
            return Err(malformed("multi-disk ZIP64 is unsupported"));
        }
        let offset = u64_at(&locator, 8);
        let mut record = [0; 56];
        if offset.checked_add(56).is_none_or(|n| n > end_offset - 20) {
            return Err(malformed("ZIP64 end record outside archive"));
        }
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut record)?;
        let size = u64_at(&record, 4);
        if &record[..4] != b"PK\x06\x06"
            || size != 44
            || offset.checked_add(12 + size) != Some(end_offset - 20)
            || u32_at(&record, 16) != 0
            || u32_at(&record, 20) != 0
            || u64_at(&record, 24) != u64_at(&record, 32)
        {
            return Err(malformed("unsupported ZIP64 end layout"));
        }
        entries = u64_at(&record, 32);
        directory_bytes = u64_at(&record, 40);
        directory_offset = u64_at(&record, 48);
        directory_end = offset;
    } else if entries == u64::from(u16::MAX)
        || directory_bytes == u64::from(u32::MAX)
        || directory_offset == u64::from(u32::MAX)
    {
        return Err(malformed("missing ZIP64 end record"));
    }
    if entries > limits.archive_entries as u64 || directory_bytes > limits.directory_bytes {
        return Err(malformed("ZIP directory resource limit exceeded"));
    }
    if directory_offset.checked_add(directory_bytes) != Some(directory_end) {
        return Err(malformed("shifted or inconsistent ZIP directory"));
    }
    // Validate real record spans before the ZIP dependency allocates entries.
    // This also ensures the advertised count bounds the actual directory.
    file.seek(SeekFrom::Start(directory_offset))?;
    let mut position = directory_offset;
    for _ in 0..entries {
        if position.checked_add(46).is_none_or(|n| n > directory_end) {
            return Err(malformed("truncated ZIP central header"));
        }
        let mut header = [0; 46];
        file.read_exact(&mut header)?;
        if &header[..4] != b"PK\x01\x02" {
            return Err(malformed("invalid ZIP central header"));
        }
        let variable = u64::from(u16_at(&header, 28))
            + u64::from(u16_at(&header, 30))
            + u64::from(u16_at(&header, 32));
        position = position
            .checked_add(46 + variable)
            .filter(|n| *n <= directory_end)
            .ok_or_else(|| malformed("ZIP central header exceeds directory"))?;
        file.seek(SeekFrom::Start(position))?;
    }
    if position != directory_end {
        return Err(malformed("ZIP directory entry count mismatch"));
    }
    let archive = ZipArchive::with_config(
        zip::read::Config {
            archive_offset: zip::read::ArchiveOffset::Known(0),
        },
        file,
    )?;
    if archive.len() as u64 != entries || archive.central_directory_start() != directory_offset {
        return Err(malformed(
            "ZIP directory changed or contains duplicate names",
        ));
    }
    Ok((
        archive,
        Usage {
            directory_bytes,
            entries: entries as usize,
            ..Usage::default()
        },
    ))
}
