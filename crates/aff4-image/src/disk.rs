//! Explicit physical-image selection and a seekable decoded-media cursor.
use std::io::{self, Read, Seek, SeekFrom};

use crate::{Container, Error, Result};

const DISK: &str = "http://aff4.org/Schema#DiskImage";
const BLOCK_SIZE: &str = "http://aff4.org/Schema#blockSize";

/// Identity and geometry of a selected physical AFF4 image, not its ZIP file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiskImageInfo {
    /// Selected DiskImage resource identifier.
    pub resource_id: String,
    /// Owning container identifier.
    pub volume_id: String,
    /// Decoded image length; this can differ from underlying stream lengths.
    pub logical_size: u64,
    /// Declared `blockSize`, when present. Never inferred from chunk size.
    pub block_size: Option<u32>,
}

impl Container {
    /// Enumerates explicitly typed physical images in a single AFF4 1.0 volume.
    /// Storage streams, logical files, and memory images are not disk candidates.
    /// Invalid image sizes or contradictory block sizes are errors, not omissions.
    pub fn disk_images(&self) -> Result<Vec<DiskImageInfo>> {
        if self.version() != (1, 0) {
            return Err(Error::Unsupported(
                "disk reading requires physical AFF4 1.0; logical collections are not disks".into(),
            ));
        }
        self.streams()?
            .into_iter()
            .filter(|stream| stream.types.iter().any(|kind| kind == DISK))
            .map(|stream| {
                if stream.types.iter().any(|kind| {
                    ["MemoryImage", "FileImage", "FileSubStream"]
                        .iter()
                        .any(|name| {
                            kind == &format!("http://aff4.org/Schema#{name}")
                                || kind == &format!("https://aff4.org/Schema/2022/#{name}")
                                || kind == &format!("http://aff4.org/Schema/2022/#{name}")
                        })
                }) {
                    return Err(crate::malformed("conflicting disk resource types"));
                }
                let mut block_size = None;
                for property in &self.metadata()[&stream.id] {
                    if property.predicate != BLOCK_SIZE {
                        continue;
                    }
                    let value = property
                        .value
                        .parse::<u32>()
                        .ok()
                        .filter(|value| *value > 0)
                        .ok_or_else(|| crate::malformed("invalid disk blockSize"))?;
                    if block_size.is_some_and(|previous| previous != value) {
                        return Err(crate::malformed("conflicting disk blockSize values"));
                    }
                    block_size = Some(value);
                }
                Ok(DiskImageInfo {
                    logical_size: self.size(&stream.id)?,
                    resource_id: stream.id,
                    volume_id: self.volume_id().to_owned(),
                    block_size,
                })
            })
            .collect()
    }

    /// Selects a physical image by identifier and transfers this container to a cursor.
    /// With `None`, exactly one disk candidate is required. Selection never chooses
    /// an underlying ImageStream or Map. Opening does not verify the whole image.
    pub fn into_disk_reader(self, resource: Option<&str>) -> Result<DiskImageReader> {
        let candidates = self.disk_images()?;
        let info = match resource {
            Some(id) => candidates
                .into_iter()
                .find(|info| info.resource_id == id)
                .ok_or_else(|| {
                    Error::Unsupported("selected resource is not a physical DiskImage".into())
                })?,
            None if candidates.len() == 1 => candidates.into_iter().next().unwrap(),
            None => {
                return Err(Error::Unsupported(format!(
                    "expected one physical DiskImage, found {}; select a resource identifier: {}",
                    candidates.len(),
                    candidates
                        .iter()
                        .take(8)
                        .map(|info| info.resource_id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        };
        Ok(DiskImageReader {
            container: self,
            info,
            position: 0,
        })
    }
}

/// Owned read-only cursor over one physical image's decoded bytes.
/// Reads retain normal AFF4 limits and errors, including missing/unreadable ranges.
/// Backing files must remain unchanged. Seeking does not read or verify data.
pub struct DiskImageReader {
    container: Container,
    info: DiskImageInfo,
    position: u64,
}

impl DiskImageReader {
    /// Selected resource identity and declared geometry.
    pub fn info(&self) -> &DiskImageInfo {
        &self.info
    }

    /// Reads logical bytes without changing the cursor position.
    pub fn read_at(&mut self, buffer: &mut [u8], offset: u64) -> Result<usize> {
        self.container
            .read_at(&self.info.resource_id, buffer, offset)
    }
}

impl Read for DiskImageReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self
            .read_at(buffer, self.position)
            .map_err(io::Error::other)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for DiskImageReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => self.info.logical_size.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        }
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "AFF4 seek outside u64 range")
        })?;
        self.position = position;
        Ok(position)
    }
}
