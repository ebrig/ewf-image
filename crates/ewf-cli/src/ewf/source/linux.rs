use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use super::{Result, SourceIdentity, SourceKind, invalid};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum BackingDevice {
    Block(PathBuf),
    File { device: u64, inode: u64 },
}

pub(super) fn identity(path: &Path, output: &Path) -> Result<SourceIdentity> {
    let path = fs::canonicalize(path)?;
    let metadata = fs::metadata(&path)?;
    if !metadata.file_type().is_block_device() {
        return Err(invalid("source is no longer a block device"));
    }
    let device = sys_device(metadata.rdev())?;
    let source_disks = backing_devices(&device, false, 0)?;
    let parent = output
        .parent()
        .ok_or_else(|| invalid("missing destination directory"))?;
    let destination = sys_device(fs::metadata(parent)?.dev())
        .map_err(|_| invalid("cannot resolve destination block storage; device acquisition requires a local block-backed destination"))?;
    let destination_disks = backing_devices(&destination, true, 0)?;
    if !source_disks.is_disjoint(&destination_disks) {
        return Err(invalid(
            "destination resides on the source device or shared backing storage",
        ));
    }
    let (size, sector_size) = geometry(&device)?;
    // Partitions inherit their parent hardware ID, and include their own offset.
    let disk = if device.join("partition").exists() {
        device
            .parent()
            .ok_or_else(|| invalid("missing parent disk"))?
    } else {
        &device
    };
    let mut hardware_id = ["wwid", "device/wwid", "device/serial", "dm/uuid"]
        .iter()
        .find_map(|name| {
            fs::read_to_string(disk.join(name))
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .map_or_else(|| loop_identity(disk), Ok)?;
    // A DM UUID alone does not identify a reassigned loop backing file.
    for backing in &source_disks {
        if let BackingDevice::Block(path) = backing
            && path != disk
            && path.join("loop/backing_file").exists()
        {
            hardware_id.push_str(&loop_identity(path)?);
        }
    }
    let start = fs::read_to_string(device.join("start")).unwrap_or_else(|_| "0".into());
    let identity = super::super::hex(&sha2::Sha256::digest(
        format!(
            "linux:{}:{}:{}",
            hardware_id.trim(),
            start.trim(),
            metadata.rdev()
        )
        .as_bytes(),
    ));
    Ok(SourceIdentity {
        kind: SourceKind::Device,
        path,
        size,
        sector_size,
        identity,
    })
}

// Loop sources have no hardware serial. Bind them to the backing file and
// mapping geometry; changing the file or reassigning the loop rejects resume.
fn loop_identity(device: &Path) -> Result<String> {
    let path = fs::read_to_string(device.join("loop/backing_file"))
        .map_err(|_| invalid("device has no stable hardware or loop-file identity"))?;
    let path = fs::canonicalize(path.trim_end_matches('\n'))?;
    let metadata = fs::metadata(&path)?;
    let offset = fs::read_to_string(device.join("loop/offset"))?;
    let limit = fs::read_to_string(device.join("loop/sizelimit"))?;
    Ok(format!(
        "loop:{}:{}:{}:{}",
        path.display(),
        super::metadata_identity(&metadata)?,
        offset.trim(),
        limit.trim()
    ))
}

pub(super) fn validate_handle(mut file: &File, identity: &SourceIdentity) -> Result<()> {
    let opened = file.metadata()?;
    let named = fs::metadata(&identity.path)?;
    if !opened.file_type().is_block_device() || opened.rdev() != named.rdev() {
        return Err(invalid(
            "opened source handle differs from the named block device",
        ));
    }
    let sector_size = rustix::fs::ioctl_blksszget(file)?;
    let position = file.stream_position()?;
    let size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(position))?;
    if sector_size != identity.sector_size || size != identity.size {
        return Err(invalid(
            "opened source handle geometry differs from discovery",
        ));
    }
    Ok(())
}

use sha2::Digest;

fn sys_device(device: u64) -> Result<PathBuf> {
    Ok(fs::canonicalize(format!(
        "/sys/dev/block/{}:{}",
        rustix::fs::major(device),
        rustix::fs::minor(device)
    ))?)
}

fn geometry(device: &Path) -> Result<(u64, u32)> {
    let sectors: u64 = fs::read_to_string(device.join("size"))?.trim().parse()?;
    // sysfs size always uses 512-byte units, independently of logical block size.
    let size = sectors
        .checked_mul(512)
        .ok_or_else(|| invalid("device size overflow"))?;
    let disk = if device.join("partition").exists() {
        device
            .parent()
            .ok_or_else(|| invalid("missing parent disk"))?
    } else {
        device
    };
    let sector_size = fs::read_to_string(disk.join("queue/logical_block_size"))?
        .trim()
        .parse()?;
    Ok((size, sector_size))
}

fn backing_devices(
    device: &Path,
    follow_backing_file: bool,
    depth: usize,
) -> Result<BTreeSet<BackingDevice>> {
    if depth >= 32 {
        return Err(invalid("block storage ancestry is too deep"));
    }
    let device = fs::canonicalize(device)?;
    let mut devices = BTreeSet::from([BackingDevice::Block(device.clone())]);
    let partition = device.join("partition").exists();
    if partition {
        devices.extend(backing_devices(
            device
                .parent()
                .ok_or_else(|| invalid("missing parent disk"))?,
            follow_backing_file,
            depth + 1,
        )?);
    }
    // Partitions inherit ancestry from the parent; kernels need not expose a
    // slaves directory on individual partition objects.
    if !partition {
        for entry in fs::read_dir(device.join("slaves"))? {
            devices.extend(backing_devices(
                &entry?.path(),
                follow_backing_file,
                depth + 1,
            )?);
        }
    }
    let backing = device.join("loop/backing_file");
    // A loop source represents file extents, not the entire host disk. Follow
    // backing storage on destinations to still reject a loop destination stored
    // inside a physical source being acquired.
    if backing.exists() {
        let path = fs::read_to_string(backing)?;
        let metadata = fs::metadata(path.trim_end_matches('\n'))?;
        devices.insert(BackingDevice::File {
            device: metadata.dev(),
            inode: metadata.ino(),
        });
        if follow_backing_file {
            devices.extend(backing_devices(
                &sys_device(metadata.dev())?,
                true,
                depth + 1,
            )?);
        }
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysfs_geometry_uses_512_byte_units_and_partition_parent() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("queue")).unwrap();
        fs::write(root.path().join("queue/logical_block_size"), "4096\n").unwrap();
        fs::write(root.path().join("size"), "8192\n").unwrap();
        assert_eq!(geometry(root.path()).unwrap(), (4_194_304, 4096));
        let part = root.path().join("partition1");
        fs::create_dir(&part).unwrap();
        fs::write(part.join("partition"), "1").unwrap();
        fs::write(part.join("size"), "1024\n").unwrap();
        assert_eq!(geometry(&part).unwrap(), (524_288, 4096));
        fs::write(part.join("size"), u64::MAX.to_string()).unwrap();
        assert!(geometry(&part).is_err());
    }

    #[test]
    fn shared_partition_and_mapper_backing_are_detected() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let disk = root.path().join("disk");
        let part = disk.join("part");
        let mapper = root.path().join("mapper");
        fs::create_dir_all(disk.join("slaves")).unwrap();
        fs::create_dir_all(&part).unwrap();
        fs::write(part.join("partition"), "1").unwrap();
        fs::create_dir_all(mapper.join("slaves")).unwrap();
        symlink(&part, mapper.join("slaves/part")).unwrap();
        let source = backing_devices(&disk, false, 0).unwrap();
        assert!(!source.is_disjoint(&backing_devices(&mapper, true, 0).unwrap()));
        symlink(&mapper, disk.join("slaves/cycle")).unwrap();
        assert!(backing_devices(&mapper, true, 0).is_err());
    }

    #[test]
    fn loop_identity_and_backing_aliases_track_the_file_not_just_its_path() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.raw");
        let alias = root.path().join("alias.raw");
        fs::write(&source, [0; 4096]).unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let mut nodes = Vec::new();
        for (name, backing) in [("loopA", &source), ("loopB", &alias)] {
            let node = root.path().join(name);
            fs::create_dir_all(node.join("loop")).unwrap();
            fs::create_dir(node.join("slaves")).unwrap();
            fs::write(node.join("loop/backing_file"), backing.to_str().unwrap()).unwrap();
            fs::write(node.join("loop/offset"), "0").unwrap();
            fs::write(node.join("loop/sizelimit"), "0").unwrap();
            nodes.push(node);
        }
        let before = loop_identity(&nodes[0]).unwrap();
        let first = backing_devices(&nodes[0], false, 0).unwrap();
        let second = backing_devices(&nodes[1], false, 0).unwrap();
        assert!(!first.is_disjoint(&second));
        fs::write(&source, [1; 8192]).unwrap();
        assert_ne!(before, loop_identity(&nodes[0]).unwrap());
        fs::write(nodes[0].join("loop/offset"), "512").unwrap();
        assert_ne!(before, loop_identity(&nodes[0]).unwrap());
    }
}
