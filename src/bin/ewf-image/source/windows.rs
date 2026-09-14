use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{Result, SourceIdentity, SourceKind, invalid};

// User paths are environment values, never interpolated into PowerShell code.
// Get-Volume resolves mounted folders as well as drive-letter destinations.
const QUERY: &str = r"$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$disk = Get-Disk -Number ([uint32]$env:EWF_SOURCE_DISK)
$destinations = @(Get-Volume -FilePath $env:EWF_DESTINATION_DIR | Get-Partition | Get-Disk)
if ($destinations.Count -eq 0) { throw 'Cannot resolve destination disk' }
[pscustomobject]@{
    Number = $disk.Number
    Size = $disk.Size
    LogicalSectorSize = $disk.LogicalSectorSize
    UniqueId = [string]$disk.UniqueId
    SerialNumber = [string]$disk.SerialNumber
    DestinationDisks = @($destinations | ForEach-Object { $_.Number })
} | ConvertTo-Json -Compress";

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Disk {
    number: u32,
    size: u64,
    logical_sector_size: u32,
    unique_id: String,
    serial_number: String,
    destination_disks: Vec<u32>,
}

pub(super) fn disk_number(path: &Path) -> Option<u32> {
    let name = path.to_str()?.to_ascii_lowercase();
    let digits = name.strip_prefix(r"\\.\physicaldrive")?;
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub(super) fn identity(path: &Path, output: &Path) -> Result<SourceIdentity> {
    let number = disk_number(path).ok_or_else(|| invalid("use a \\\\.\\PhysicalDriveN source"))?;
    let system =
        std::env::var_os("SystemRoot").ok_or_else(|| invalid("SystemRoot is unavailable"))?;
    let powershell = PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let parent = output
        .parent()
        .ok_or_else(|| invalid("missing destination directory"))?;
    let parent = parent
        .to_str()
        .ok_or_else(|| invalid("destination path is not Unicode"))?;
    let result = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            QUERY,
        ])
        .env("EWF_SOURCE_DISK", number.to_string())
        .env(
            "EWF_DESTINATION_DIR",
            parent.strip_prefix(r"\\?\").unwrap_or(parent),
        )
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()?;
    if !result.status.success() {
        return Err(invalid(
            "Windows Storage query failed; device acquisition requires Storage cmdlets, sufficient privileges, and a resolvable local destination",
        ));
    }
    let disk: Disk = serde_json::from_slice(&result.stdout)?;
    identity_from_disk(number, &disk)
}

fn identity_from_disk(number: u32, disk: &Disk) -> Result<SourceIdentity> {
    if disk.number != number || disk.destination_disks.is_empty() {
        return Err(invalid(
            "Windows Storage returned inconsistent disk identity",
        ));
    }
    if disk.destination_disks.contains(&number) {
        return Err(invalid("destination resides on the source disk"));
    }
    if disk.unique_id.trim().is_empty() && disk.serial_number.trim().is_empty() {
        return Err(invalid("device has no stable unique ID or serial number"));
    }
    let token = format!(
        "windows:{}:{}:{}",
        number,
        disk.unique_id.trim(),
        disk.serial_number.trim()
    );
    Ok(SourceIdentity {
        kind: SourceKind::Device,
        path: PathBuf::from(format!(r"\\.\PhysicalDrive{number}")),
        size: disk.size,
        sector_size: disk.logical_sector_size,
        identity: super::super::hex(&Sha256::digest(token.as_bytes())),
    })
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

    #[test]
    fn device_identity_checks_destination_and_hardware_identifiers() {
        let mut disk = Disk {
            number: 2,
            size: 4096,
            logical_sector_size: 4096,
            unique_id: "test disk".into(),
            serial_number: String::new(),
            destination_disks: vec![0],
        };
        let first = identity_from_disk(2, &disk).unwrap();
        disk.unique_id = "replacement disk".into();
        assert_ne!(first, identity_from_disk(2, &disk).unwrap());
        disk.destination_disks = vec![2];
        assert!(identity_from_disk(2, &disk).is_err());
        disk.destination_disks = vec![0];
        disk.unique_id.clear();
        assert!(identity_from_disk(2, &disk).is_err());
    }
}
