//! Metadata-based physical-image discovery without filename conventions.
use super::*;
use crate::DiskImageInfo;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;

/// A container required by a discovered physical image.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BackingVolume {
    /// AFF4 volume identifier, checked again on reopen.
    pub volume_id: String,
    /// Container path supplied by the caller.
    pub path: PathBuf,
}

/// Stable identity and backing files for one decoded physical disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PhysicalDisk {
    /// Physical image resource and geometry.
    pub image: DiskImageInfo,
    /// Metadata-connected containers, excluding unrelated discovery candidates.
    pub volumes: Vec<BackingVolume>,
}

impl PhysicalDisk {
    /// Reopens exactly the recorded files and checks their identities and geometry.
    /// This does not hash the payload; backing files must remain unchanged.
    pub fn reopen(&self) -> Result<PhysicalDiskReader> {
        let paths: Vec<_> = self.volumes.iter().map(|v| v.path.clone()).collect();
        let set = DiskImageSet::discover(&paths, &[])?;
        set.into_readers()
            .into_iter()
            .find(|reader| reader.info() == self)
            .ok_or_else(|| malformed("AFF4 image or backing volume identity changed"))
    }
}

struct Inventory {
    container: Container,
    path: PathBuf,
    owned: BTreeSet<String>,
    needs: BTreeSet<String>,
    disks: Vec<(DiskImageInfo, bool)>,
}

fn symbolic(id: &str) -> bool {
    volume_set::is_symbolic(id)
}

fn dependencies(
    container: &mut Container,
    id: &str,
    stack: &mut Vec<String>,
    needs: &mut BTreeSet<String>,
) -> Result<()> {
    if symbolic(id) {
        return Ok(());
    }
    if container.inline_data(id)?.is_some() {
        return Ok(());
    }
    if stack.len() >= 32 || stack.iter().any(|previous| previous == id) {
        return Err(malformed("cyclic or excessive disk dependency depth"));
    }
    stack.push(id.to_owned());
    if let Some(target) = container.value(id, "dataStream")? {
        dependencies(container, &target, stack, needs)?;
    } else if container.has_type(id, "Map") {
        container.load_map(id)?;
        let map = container.maps[id].clone();
        let mut targets: BTreeSet<_> = map
            .ranges
            .iter()
            .map(|range| range.target.clone())
            .collect();
        targets.insert(map.gap.clone());
        for target in targets {
            dependencies(container, &target, stack, needs)?;
        }
    } else {
        let path = container.path(id)?;
        let local = container.archive.index_for_name(&path).is_some()
            || container
                .archive
                .index_for_name(&format!("{path}/00000000"))
                .is_some()
            || (container.value(id, "stored")?.as_deref() == Some(&container.volume)
                && container.number(id, "size").ok() == Some(0));
        if !local {
            needs.insert(id.to_owned());
        }
    }
    stack.pop();
    Ok(())
}

impl Inventory {
    fn new(mut container: Container, path: PathBuf) -> Result<Self> {
        let mut owned = BTreeSet::from([container.volume.clone()]);
        let mut needs = BTreeSet::new();
        for id in container.graph.keys() {
            if container.has_type(id, "ImageStream") {
                // ZipVolume::stored may be an original filename, and case
                // metadata is not a disk dependency. Resolve storage owners.
                if let Some(stored) = container.value(id, "stored")?
                    && stored != container.volume
                {
                    needs.insert(stored);
                }
                let member = format!("{}/00000000", container.path(id)?);
                if container.archive.index_for_name(&member).is_some()
                    || (container.number(id, "size").ok() == Some(0)
                        && container.value(id, "stored")?.as_deref() == Some(&container.volume))
                {
                    owned.insert(id.clone());
                }
            }
        }
        let mut disks = Vec::new();
        for disk in container.disk_images()? {
            let mut dependencies_found = BTreeSet::new();
            dependencies(
                &mut container,
                &disk.resource_id,
                &mut Vec::new(),
                &mut dependencies_found,
            )?;
            let mapped = !dependencies_found.is_empty();
            needs.extend(dependencies_found);
            disks.push((disk, mapped));
        }
        container.maps.clear();
        Ok(Self {
            container,
            path,
            owned,
            needs,
            disks,
        })
    }
}

/// Physical disks discovered from explicit inputs and optional nearby containers.
/// Membership follows volume identifiers and map references, never filenames.
pub struct DiskImageSet {
    readers: Vec<PhysicalDiskReader>,
}

impl DiskImageSet {
    /// Discovers every physical disk connected to an input, including when the
    /// input is a companion. Unrelated candidates are not returned as evidence.
    /// Candidates must be local paths; this API never follows network references.
    pub fn discover(inputs: &[PathBuf], candidates: &[PathBuf]) -> Result<Self> {
        Self::discover_with_limits(inputs, candidates, Limits::default())
    }

    /// Discovery inspects at most 128 distinct paths, with aggregate metadata,
    /// directory and triple budgets. Invalid optional candidates are ignored;
    /// unresolved dependencies of selected evidence remain errors.
    pub fn discover_with_limits(
        inputs: &[PathBuf],
        candidates: &[PathBuf],
        limits: Limits,
    ) -> Result<Self> {
        if inputs.is_empty() {
            return Err(malformed("no AFF4 inputs"));
        }
        let mut paths = Vec::new();
        for path in inputs.iter().chain(candidates) {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        if paths.len() > 128 {
            return Err(malformed("AFF4 discovery exceeds 128 container paths"));
        }
        let mut remaining = limits.clone();
        let mut inventory = Vec::new();
        let mut selected = BTreeSet::new();
        for path in paths {
            let explicit = inputs.contains(&path);
            let mut container = match Container::open_with_limits(&path, remaining.clone()) {
                Ok(container) => container,
                Err(error) if explicit => return Err(error),
                Err(_) => continue,
            };
            remaining.metadata_bytes -= container.usage.metadata_bytes;
            remaining.triples -= container.usage.triples;
            remaining.directory_bytes -= container.usage.directory_bytes;
            remaining.archive_entries -= container.usage.entries;
            container.limits = limits.clone();
            match Inventory::new(container, path) {
                Ok(item) => {
                    if explicit {
                        selected.insert(inventory.len());
                    }
                    inventory.push(item);
                }
                Err(error) if explicit => return Err(error),
                Err(_) => continue,
            }
        }
        let mut providers: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, item) in inventory.iter().enumerate() {
            for id in &item.owned {
                providers.entry(id.clone()).or_default().push(index);
            }
        }
        let mut edges = vec![BTreeSet::new(); inventory.len()];
        for (index, item) in inventory.iter().enumerate() {
            for need in &item.needs {
                for &owner in providers.get(need).into_iter().flatten() {
                    edges[index].insert(owner);
                    edges[owner].insert(index);
                }
            }
        }
        loop {
            let previous = selected.len();
            for index in selected.clone() {
                selected.extend(&edges[index]);
            }
            if selected.len() == previous {
                break;
            }
        }
        for &index in &selected {
            for owned in &inventory[index].owned {
                if providers[owned].len() != 1 {
                    return Err(malformed(format!("ambiguous AFF4 owner for {owned}")));
                }
            }
            for need in &inventory[index].needs {
                match providers.get(need).map(Vec::len).unwrap_or(0) {
                    1 => (),
                    0 => return Err(malformed(format!("missing AFF4 companion for {need}"))),
                    _ => return Err(malformed(format!("ambiguous AFF4 companion for {need}"))),
                }
            }
        }
        let mut descriptions = Vec::new();
        let mut images = BTreeSet::new();
        for (primary, &index) in selected.iter().enumerate() {
            let mut component = BTreeSet::from([index]);
            loop {
                let previous = component.len();
                for member in component.clone() {
                    component.extend(&edges[member]);
                }
                if previous == component.len() {
                    break;
                }
            }
            if component
                .iter()
                .all(|&member| inventory[member].disks.is_empty())
            {
                return Err(Error::Unsupported(format!(
                    "no physical DiskImage found for input {}",
                    inventory[index].path.display()
                )));
            }
            let mut volumes: Vec<_> = component
                .iter()
                .map(|&member| BackingVolume {
                    volume_id: inventory[member].container.volume.clone(),
                    path: inventory[member].path.clone(),
                })
                .collect();
            volumes.sort_by(|left, right| left.volume_id.cmp(&right.volume_id));
            for (image, mapped) in &inventory[index].disks {
                if !images.insert(image.resource_id.clone()) {
                    return Err(malformed("ambiguous physical image identifier"));
                }
                if descriptions.len() >= 128 {
                    return Err(malformed("more than 128 physical disk images"));
                }
                descriptions.push((
                    primary,
                    *mapped,
                    PhysicalDisk {
                        image: image.clone(),
                        volumes: volumes.clone(),
                    },
                ));
            }
        }
        if descriptions.is_empty() {
            return Err(Error::Unsupported(
                "no physical DiskImage resources found".into(),
            ));
        }
        let mut containers = Vec::new();
        let mut paths = Vec::new();
        for (index, item) in inventory.into_iter().enumerate() {
            if selected.contains(&index) {
                containers.push(item.container);
                paths.push(item.path);
            }
        }
        let mut backing = VolumeSet::from_containers(containers, paths)?;
        for (primary, mapped, info) in &descriptions {
            if backing.disk_size(*primary, *mapped, &info.image.resource_id)?
                != info.image.logical_size
            {
                return Err(malformed("physical image length changed during discovery"));
            }
        }
        let backing = Arc::new(Mutex::new(backing));
        Ok(Self {
            readers: descriptions
                .into_iter()
                .map(|(primary, mapped, info)| PhysicalDiskReader {
                    backing: backing.clone(),
                    primary,
                    mapped,
                    info,
                    position: 0,
                })
                .collect(),
        })
    }

    /// Transfers all discovered disks to independent cursors with shared caches.
    pub fn into_readers(self) -> Vec<PhysicalDiskReader> {
        self.readers
    }
}

/// Seekable decoded disk, including data spread across companion containers.
pub struct PhysicalDiskReader {
    backing: Arc<Mutex<VolumeSet>>,
    primary: usize,
    mapped: bool,
    info: PhysicalDisk,
    position: u64,
}

impl PhysicalDiskReader {
    /// Identity, geometry and exact backing container paths.
    pub fn info(&self) -> &PhysicalDisk {
        &self.info
    }

    /// Reads decoded bytes without moving the cursor.
    pub fn read_at(&mut self, buffer: &mut [u8], offset: u64) -> Result<usize> {
        self.backing
            .lock()
            .map_err(|_| malformed("AFF4 reader lock poisoned"))?
            .read_disk_at(
                self.primary,
                self.mapped,
                &self.info.image.resource_id,
                buffer,
                offset,
            )
    }
}

impl Read for PhysicalDiskReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self
            .read_at(buffer, self.position)
            .map_err(io::Error::other)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for PhysicalDiskReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => self.info.image.logical_size.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        }
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "AFF4 seek outside u64 range")
        })?;
        self.position = position;
        Ok(position)
    }
}
