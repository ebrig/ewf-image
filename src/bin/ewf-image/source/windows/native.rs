//! The CLI's only unsafe boundary: synchronous, read-only Windows queries.
//!
//! No arbitrary control codes, caller-provided pointers, struct casts, or handle
//! ownership transfers cross this boundary. Response decoding stays in safe Rust.
#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    IOCTL_DISK_GET_DRIVE_GEOMETRY, IOCTL_DISK_GET_LENGTH_INFO, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_QUERY_PROPERTY,
};

#[derive(Clone, Copy)]
pub(super) enum Query {
    Number,
    Geometry,
    Length,
    Descriptor,
    Identifiers,
    Extents,
}

/// Only used with synchronous handles opened by this CLI. Every listed IOCTL
/// uses `METHOD_BUFFERED` and has pointer-free input/output wire structures.
pub(super) fn query(file: &File, query: Query) -> io::Result<Vec<u8>> {
    let mut input = [0_u8; 12]; // STORAGE_PROPERTY_QUERY, PropertyStandardQuery.
    let (code, input_len, capacity) = match query {
        Query::Number => (IOCTL_STORAGE_GET_DEVICE_NUMBER, 0, 12),
        Query::Geometry => (IOCTL_DISK_GET_DRIVE_GEOMETRY, 0, 24),
        Query::Length => (IOCTL_DISK_GET_LENGTH_INFO, 0, 8),
        Query::Descriptor => (IOCTL_STORAGE_QUERY_PROPERTY, 12, 65536),
        Query::Identifiers => {
            input[..4].copy_from_slice(&2_u32.to_le_bytes()); // StorageDeviceIdProperty.
            (IOCTL_STORAGE_QUERY_PROPERTY, 12, 65536)
        }
        Query::Extents => (IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, 0, 1024 * 1024),
    };
    let mut output = vec![0_u8; capacity];
    let mut returned = 0;
    // SAFETY: file owns a live synchronous handle throughout this call. Both
    // buffers are initialized, distinct, and live for their stated lengths.
    // These fixed METHOD_BUFFERED queries contain no embedded user pointers.
    // No OVERLAPPED operation can outlive the buffers; the last argument is null.
    let success = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            if input_len == 0 {
                std::ptr::null()
            } else {
                input.as_ptr().cast()
            },
            input_len,
            output.as_mut_ptr().cast(),
            capacity as u32,
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    if returned as usize > output.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized Windows query response",
        ));
    }
    output.truncate(returned as usize);
    Ok(output)
}

pub(super) fn destination_volume(parent: &Path) -> io::Result<File> {
    let path: Vec<u16> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
    if path[..path.len() - 1].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination contains NUL",
        ));
    }
    let mut mount = vec![0_u16; 32768];
    // SAFETY: path is terminated and both distinct buffers stay alive for the
    // synchronous call. The output capacity is supplied in UTF-16 code units.
    if unsafe { GetVolumePathNameW(path.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if !mount.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unterminated volume mount path",
        ));
    }
    let mut name = [0_u16; 64]; // Volume GUID path, including terminating NUL.
    // SAFETY: mount contains a NUL and both buffers remain valid for the call.
    if unsafe {
        GetVolumeNameForVolumeMountPointW(mount.as_ptr(), name.as_mut_ptr(), name.len() as u32)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let end = name
        .iter()
        .position(|&v| v == 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unterminated volume GUID"))?;
    let name = &name[..end];
    let name = name
        .strip_suffix(&[u16::from(b'\\')])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid volume GUID path"))?;
    // Metadata-only access; never request destination volume write access.
    OpenOptions::new()
        .access_mode(0)
        .share_mode(3)
        .open(std::ffi::OsString::from_wide(name))
}
