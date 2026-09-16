use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use windows_sys::Win32::System::Ioctl::{DISK_EXTENT, VOLUME_DISK_EXTENTS};

use super::{Result, SourceIdentity, SourceKind, invalid};

mod native;
use native::Query;

pub(super) fn disk_number(path: &Path) -> Option<u32> {
    let name = path.to_str()?.to_ascii_lowercase();
    let digits = name.strip_prefix(r"\\.\physicaldrive")?;
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub(super) fn identity(file: &File, path: &Path, output: &Path) -> Result<SourceIdentity> {
    let number = disk_number(path).ok_or_else(|| invalid("use a \\\\.\\PhysicalDriveN source"))?;
    let device = native::query(file, Query::Number)?;
    // STORAGE_DEVICE_NUMBER: FILE_DEVICE_DISK, device number, partition number.
    if device.len() != 12
        || u32_at(&device, 0)? != 7
        || u32_at(&device, 4)? != number
        || !matches!(u32_at(&device, 8)?, 0 | u32::MAX)
    {
        return Err(invalid(
            "opened handle is not the requested whole physical disk",
        ));
    }
    let geometry = native::query(file, Query::Geometry)?;
    let length = native::query(file, Query::Length)?;
    let size = u64_at(&length, 0)?;
    if size == 0 || size > i64::MAX as u64 {
        return Err(invalid("invalid opened-device length"));
    }
    let descriptor = optional_property(file, Query::Descriptor)?;
    let identifiers = optional_property(file, Query::Identifiers)?;
    let token = hardware_token(descriptor.as_deref(), identifiers.as_deref())?;
    let parent = output
        .parent()
        .ok_or_else(|| invalid("missing destination directory"))?;
    let destination = native::destination_volume(parent)
        .and_then(|volume| native::query(&volume, Query::Extents))
        .map_err(|error| invalid(&format!("cannot resolve destination disk extents: {error}")))?;
    reject_overlap(number, &destination)?;
    Ok(SourceIdentity {
        kind: SourceKind::Device,
        path: PathBuf::from(format!(r"\\.\PhysicalDrive{number}")),
        size,
        sector_size: u32_at(&geometry, 20)?,
        identity: super::super::hex(&Sha256::digest(format!(
            "windows-native-v1:{number}:{token}"
        ))),
    })
}

fn optional_property(file: &File, query: Query) -> io::Result<Option<Vec<u8>>> {
    match native::query(file, query) {
        Ok(value) => Ok(Some(value)),
        // Only explicit unsupported-query responses permit identifier fallback.
        // Device removal, permission and I/O errors remain fatal.
        Err(error) if matches!(error.raw_os_error(), Some(1 | 50 | 87)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| invalid("Windows response offset overflow"))?;
    bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| invalid("truncated Windows storage response"))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(field(bytes, offset)?))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(field(bytes, offset)?))
}

fn descriptor(bytes: &[u8], minimum: usize) -> Result<&[u8]> {
    let version = u32_at(bytes, 0)? as usize;
    let size = u32_at(bytes, 4)? as usize;
    if version < minimum || size < minimum || size > bytes.len() {
        return Err(invalid("invalid Windows storage descriptor size"));
    }
    Ok(&bytes[..size])
}

fn descriptor_string(bytes: &[u8], field_offset: usize) -> Result<&str> {
    let offset = u32_at(bytes, field_offset)? as usize;
    if offset == 0 {
        return Ok("");
    }
    if offset < 36 {
        return Err(invalid("storage string overlaps descriptor header"));
    }
    let tail = bytes
        .get(offset..)
        .ok_or_else(|| invalid("storage string offset exceeds descriptor"))?;
    let end = tail
        .iter()
        .position(|&v| v == 0)
        .ok_or_else(|| invalid("unterminated storage string"))?;
    let value = std::str::from_utf8(&tail[..end])?;
    if !value.is_ascii() {
        return Err(invalid("non-ASCII storage identifier"));
    }
    Ok(value.trim())
}

fn hardware_token(device: Option<&[u8]>, ids: Option<&[u8]>) -> Result<String> {
    let mut identifiers = Vec::new();
    if let Some(bytes) = ids {
        let bytes = descriptor(bytes, 12)?;
        let count = u32_at(bytes, 8)? as usize;
        if count > (bytes.len() - 12) / 16 {
            return Err(invalid("invalid storage identifier count"));
        }
        let mut offset = 12;
        for index in 0..count {
            let code_set = u32_at(bytes, offset)?;
            let kind = u32_at(bytes, offset + 4)?;
            let size = u16::from_le_bytes(field(bytes, offset + 8)?) as usize;
            let next = u16::from_le_bytes(field(bytes, offset + 10)?) as usize;
            let association = u32_at(bytes, offset + 12)?;
            let end = offset + 16 + size;
            let value = bytes
                .get(offset + 16..end)
                .ok_or_else(|| invalid("truncated storage identifier"))?;
            if size == 0 {
                return Err(invalid("empty storage identifier"));
            }
            // Port/target IDs describe the connection, not the acquired device.
            if association == 0 && value.iter().any(|&v| v != 0) {
                identifiers.push(format!("{code_set}:{kind}:{}", super::super::hex(value)));
            }
            if index + 1 == count {
                // Hyper-V returns a final stride to the descriptor end rather
                // than zero. The declared count controls iteration; never follow
                // a link beyond the last counted record.
                if next != 0 && (next < 16 + size || offset + next != bytes.len()) {
                    return Err(invalid("invalid final storage identifier link"));
                }
            } else {
                if next < 16 + size {
                    return Err(invalid("overlapping storage identifiers"));
                }
                offset += next;
            }
        }
    }
    // Validate descriptors even when page-83 identifiers are available.
    let serial = if let Some(bytes) = device {
        let bytes = descriptor(bytes, 36)?;
        let vendor = descriptor_string(bytes, 12)?;
        let product = descriptor_string(bytes, 16)?;
        let serial = descriptor_string(bytes, 24)?;
        (!serial.is_empty()).then(|| format!("serial:{vendor:?}:{product:?}:{serial:?}"))
    } else {
        None
    };
    if identifiers.is_empty() {
        serial.ok_or_else(|| invalid("opened device has no stable identifier or serial number"))
    } else {
        identifiers.sort_unstable();
        identifiers.dedup();
        Ok(format!("ids:{}", identifiers.join(";")))
    }
}

fn reject_overlap(number: u32, bytes: &[u8]) -> Result<()> {
    let count = u32_at(bytes, 0)? as usize;
    let first = offset_of!(VOLUME_DISK_EXTENTS, Extents);
    let stride = size_of::<DISK_EXTENT>();
    if count == 0 || count > bytes.len().saturating_sub(first) / stride {
        return Err(invalid("invalid destination disk extent count"));
    }
    for index in 0..count {
        let offset = first + index * stride;
        let disk = u32_at(bytes, offset + offset_of!(DISK_EXTENT, DiskNumber))?;
        let start = u64_at(bytes, offset + offset_of!(DISK_EXTENT, StartingOffset))?;
        let length = u64_at(bytes, offset + offset_of!(DISK_EXTENT, ExtentLength))?;
        if length == 0
            || start > i64::MAX as u64
            || length > i64::MAX as u64
            || start
                .checked_add(length)
                .is_none_or(|end| end > i64::MAX as u64)
        {
            return Err(invalid("invalid destination disk extent geometry"));
        }
        if disk == number {
            return Err(invalid("destination resides on the source disk"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_disk_paths_are_strict() {
        assert_eq!(disk_number(Path::new(r"\\.\PhysicalDrive12")), Some(12));
        assert_eq!(disk_number(Path::new(r"\\.\physicaldrive0")), Some(0));
        for path in [
            r"\\.\PhysicalDrive",
            r"\\.\PhysicalDrive1\extra",
            r"\\.\PhysicalDrive-1",
            r"\\.\C:",
        ] {
            assert_eq!(disk_number(Path::new(path)), None);
        }
    }
    fn serial_descriptor(serial: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 36];
        bytes[..4].copy_from_slice(&36_u32.to_le_bytes());
        bytes[24..28].copy_from_slice(&36_u32.to_le_bytes());
        bytes.extend_from_slice(serial);
        let size = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        bytes
    }
    #[test]
    fn serial_identity_rejects_missing_and_malformed_descriptors() {
        let first = serial_descriptor(b"original\0");
        let other = serial_descriptor(b"replacement\0");
        assert_ne!(
            hardware_token(Some(&first), None).unwrap(),
            hardware_token(Some(&other), None).unwrap()
        );
        assert!(hardware_token(None, None).is_err());
        for value in [b"".as_slice(), b" \0", b"missing terminator", b"\xff\0"] {
            assert!(hardware_token(Some(&serial_descriptor(value)), None).is_err());
        }
        for length in 0..first.len() {
            assert!(hardware_token(Some(&first[..length]), None).is_err());
        }
        let mut bad = first;
        bad[24..28].copy_from_slice(&1_u32.to_le_bytes());
        assert!(hardware_token(Some(&bad), None).is_err());
    }
    fn id_descriptor() -> Vec<u8> {
        let mut bytes = vec![0; 32];
        bytes[..4].copy_from_slice(&16_u32.to_le_bytes());
        bytes[4..8].copy_from_slice(&32_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&1_u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&1_u32.to_le_bytes());
        bytes[20..22].copy_from_slice(&4_u16.to_le_bytes());
        bytes[28..].copy_from_slice(b"test");
        bytes
    }
    #[test]
    fn identifiers_require_bounded_device_associated_records() {
        let ids = id_descriptor();
        assert!(
            hardware_token(None, Some(&ids))
                .unwrap()
                .starts_with("ids:")
        );
        for length in 0..ids.len() {
            assert!(hardware_token(None, Some(&ids[..length])).is_err());
        }
        for (offset, value) in [(8, u32::MAX), (20, 0), (22, 1), (24, 1)] {
            let mut bad = ids.clone();
            bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(hardware_token(None, Some(&bad)).is_err());
        }
        let mut zeros = ids;
        zeros[28..].fill(0);
        assert!(hardware_token(None, Some(&zeros)).is_err());
    }

    #[test]
    fn final_identifier_stride_can_end_at_descriptor_boundary() {
        let mut ids = id_descriptor();
        let expected = hardware_token(None, Some(&ids)).unwrap();
        ids.extend_from_slice(&[0; 4]);
        ids[4..8].copy_from_slice(&36_u32.to_le_bytes());
        ids[22..24].copy_from_slice(&24_u16.to_le_bytes());
        assert_eq!(hardware_token(None, Some(&ids)).unwrap(), expected);
        ids[22..24].copy_from_slice(&25_u16.to_le_bytes());
        assert!(hardware_token(None, Some(&ids)).is_err());
    }

    #[test]
    fn identifier_order_is_not_part_of_device_identity() {
        let mut ids = id_descriptor();
        ids.extend_from_slice(&id_descriptor()[12..]);
        ids[4..8].copy_from_slice(&52_u32.to_le_bytes());
        ids[8..12].copy_from_slice(&2_u32.to_le_bytes());
        ids[22..24].copy_from_slice(&20_u16.to_le_bytes());
        ids[48..52].copy_from_slice(b"next");
        let expected = hardware_token(None, Some(&ids)).unwrap();
        ids[28..32].copy_from_slice(b"next");
        ids[48..52].copy_from_slice(b"test");
        assert_eq!(hardware_token(None, Some(&ids)).unwrap(), expected);
        ids[22..24].copy_from_slice(&19_u16.to_le_bytes());
        assert!(hardware_token(None, Some(&ids)).is_err());
    }
    #[test]
    fn every_destination_extent_is_checked() {
        let first = offset_of!(VOLUME_DISK_EXTENTS, Extents);
        let stride = size_of::<DISK_EXTENT>();
        let mut bytes = vec![0; first + 2 * stride];
        bytes[..4].copy_from_slice(&2_u32.to_le_bytes());
        for (index, disk) in [1_u32, 7].into_iter().enumerate() {
            let offset = first + index * stride;
            bytes[offset..offset + 4].copy_from_slice(&disk.to_le_bytes());
            let length = offset + offset_of!(DISK_EXTENT, ExtentLength);
            bytes[length..length + 8].copy_from_slice(&4096_u64.to_le_bytes());
        }
        assert!(reject_overlap(2, &bytes).is_ok());
        assert!(reject_overlap(1, &bytes).is_err());
        assert!(reject_overlap(7, &bytes).is_err());
        for length in 0..bytes.len() {
            assert!(reject_overlap(2, &bytes[..length]).is_err());
        }
        let length = first + offset_of!(DISK_EXTENT, ExtentLength);
        for value in [0_u64, u64::MAX] {
            let mut bad = bytes.clone();
            bad[length..length + 8].copy_from_slice(&value.to_le_bytes());
            assert!(reject_overlap(2, &bad).is_err());
        }
        let start = first + offset_of!(DISK_EXTENT, StartingOffset);
        for value in [i64::MAX as u64, u64::MAX] {
            let mut bad = bytes.clone();
            bad[start..start + 8].copy_from_slice(&value.to_le_bytes());
            assert!(reject_overlap(2, &bad).is_err());
        }
        bytes[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(reject_overlap(2, &bytes).is_err());
    }
    #[test]
    fn ordinary_file_is_not_a_device_handle() {
        let file = tempfile::tempfile().unwrap();
        assert!(native::query(&file, Query::Number).is_err());
    }
}
