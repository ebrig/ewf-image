//! Explicit multi-volume physical reading. Graphs retain their source context.
use super::*;
use std::path::PathBuf;

/// Attribution of a stored ImageStream to its owning ZIP volume.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VolumeSource {
    /// Data stream resource.
    pub stream: String,
    /// Owning volume identifier.
    pub volume: String,
    /// Supplied path to that volume.
    pub path: PathBuf,
}

/// Whole-image digest over a resolved volume set, distinct from block-map hashes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SetDigest {
    /// Image identifier.
    pub image: String,
    /// Logical bytes traversed.
    pub bytes: u64,
    /// SHA256 of the assembled logical image.
    pub sha256: String,
    /// Comparison with an independently supplied whole-image SHA256.
    pub external_match: Option<bool>,
    /// Stream owners, sorted by stream identifier.
    pub sources: Vec<VolumeSource>,
}

/// A primary physical container plus explicitly supplied companion volumes.
/// No filenames are guessed, no graphs are flattened, and ambiguous owners fail.
/// This reader does not yet certify a striped image's composite block-map digest.
pub struct VolumeSet {
    volumes: Vec<Container>,
    paths: Vec<PathBuf>,
    owners: BTreeMap<String, usize>,
    validated: BTreeMap<String, (Arc<Map>, u64)>,
    cached_owner: Option<usize>,
}

impl VolumeSet {
    /// Opens the primary and companions with a shared metadata/directory budget.
    /// The primary must contain the selected image's complete Map. Named foreign
    /// volume references must be supplied; duplicate owners/volume IDs are errors.
    pub fn open(paths: &[PathBuf]) -> Result<Self> {
        Self::open_with_limits(paths, Limits::default())
    }

    /// Opens at most 128 volumes. Metadata bytes, triples, ZIP directory bytes,
    /// and entry counts are aggregate limits across the entire set. Chunk and
    /// member limits apply per object; only one volume retains payload caches.
    pub fn open_with_limits(paths: &[PathBuf], limits: Limits) -> Result<Self> {
        if paths.is_empty() || paths.len() > 128 {
            return Err(malformed("volume set must contain 1 through 128 paths"));
        }
        let mut volumes = Vec::new();
        let mut ids = BTreeSet::new();
        let mut remaining = limits.clone();
        for path in paths {
            let mut volume = Container::open_with_limits(path, remaining.clone())?;
            remaining.metadata_bytes -= volume.usage.metadata_bytes;
            remaining.triples -= volume.usage.triples;
            remaining.directory_bytes -= volume.usage.directory_bytes;
            remaining.archive_entries -= volume.usage.entries;
            volume.limits = limits.clone();
            if volume.version != (1, 0) {
                return Err(Error::Unsupported(
                    "volume sets currently support physical AFF4 1.0".into(),
                ));
            }
            if !ids.insert(volume.volume.clone()) {
                return Err(malformed("duplicate volume identifier"));
            }
            volumes.push(volume);
        }
        let mut owners = BTreeMap::new();
        for (index, volume) in volumes.iter().enumerate() {
            for id in volume
                .graph
                .keys()
                .filter(|id| volume.has_type(id, "ImageStream"))
            {
                if let Some(stored) = volume.value(id, "stored")?
                    && !ids.contains(&stored)
                {
                    return Err(malformed(format!("missing source volume {stored}")));
                }
                let Ok(size) = volume.number(id, "size") else {
                    continue;
                };
                let path = volume.path(id)?;
                let owns = if size == 0 {
                    volume.value(id, "stored")?.as_deref() == Some(&volume.volume)
                } else {
                    volume
                        .archive
                        .index_for_name(&format!("{path}/00000000"))
                        .is_some()
                };
                if owns {
                    if volume
                        .value(id, "stored")?
                        .is_some_and(|v| v != volume.volume)
                    {
                        return Err(malformed("local stream conflicts with declared owner"));
                    }
                    if owners.insert(id.clone(), index).is_some() {
                        return Err(malformed(format!("ambiguous stream owner: {id}")));
                    }
                }
            }
        }
        // Validate supplied geometry against the owner's metadata. A foreign stub
        // may omit it, but an explicit disagreement is not silently overridden.
        for (id, &owner) in &owners {
            for volume in &volumes {
                if !volume.graph.contains_key(id) {
                    continue;
                }
                for key in [
                    "size",
                    "chunkSize",
                    "chunksInSegment",
                    "compressionMethod",
                    "stored",
                ] {
                    if let Some(value) = volume.value(id, key)? {
                        let expected = if key == "stored" {
                            Some(volumes[owner].volume.clone())
                        } else {
                            volumes[owner].value(id, key)?
                        };
                        if expected.as_ref() != Some(&value) {
                            return Err(malformed(format!("conflicting {key} for {id}")));
                        }
                    }
                }
            }
        }
        Ok(Self {
            volumes,
            paths: paths.to_vec(),
            owners,
            validated: BTreeMap::new(),
            cached_owner: None,
        })
    }

    /// Identifiers of physical images described by the primary volume.
    pub fn images(&self) -> Vec<String> {
        self.volumes[0]
            .graph
            .keys()
            .filter(|id| self.volumes[0].has_type(id, "DiskImage"))
            .cloned()
            .collect()
    }

    /// Data stream ownership; metadata itself remains scoped to each volume.
    pub fn sources(&self) -> Vec<VolumeSource> {
        self.owners
            .iter()
            .map(|(id, &n)| VolumeSource {
                stream: id.clone(),
                volume: self.volumes[n].volume.clone(),
                path: self.paths[n].clone(),
            })
            .collect()
    }

    /// Opens a primary map and validates all referenced storage before reading.
    fn image_map(&mut self, id: &str) -> Result<(Arc<Map>, u64)> {
        if let Some(result) = self.validated.get(id) {
            return Ok(result.clone());
        }
        if !self.volumes[0].has_type(id, "DiskImage") {
            return Err(Error::Unsupported("select a primary DiskImage".into()));
        }
        let target = self.volumes[0]
            .value(id, "dataStream")?
            .ok_or_else(|| malformed("image has no dataStream"))?;
        self.volumes[0].load_map(&target)?;
        let map = self.volumes[0].maps[&target].clone();
        let size = self.volumes[0].number(&target, "size")?;
        if self.volumes[0].value(id, "size")?.is_some()
            && self.volumes[0].number(id, "size")? != size
        {
            return Err(malformed("image/map size mismatch"));
        }
        let contiguous = self.volumes[0].has_type(id, "ContiguousImage");
        let mut end = 0;
        for range in &map.ranges {
            if contiguous && range.start != end {
                return Err(malformed("gap in contiguous image"));
            }
            end = range.end;
            if is_symbolic(&range.target) {
                continue;
            }
            let owner = *self
                .owners
                .get(&range.target)
                .ok_or_else(|| malformed(format!("missing owner for {}", range.target)))?;
            let size = self.volumes[owner].number(&range.target, "size")?;
            if range
                .offset
                .checked_add(range.end - range.start)
                .is_none_or(|end| end > size)
            {
                return Err(malformed("map exceeds owned stream"));
            }
        }
        if contiguous && end != size {
            return Err(malformed("truncated contiguous map"));
        }
        if !is_symbolic(&map.gap) && !self.owners.contains_key(&map.gap) {
            return Err(Error::Unsupported("external/nested gap stream".into()));
        }
        self.validated.clear();
        self.validated.insert(id.into(), (map.clone(), size));
        Ok((map, size))
    }

    /// Logical size of a selected primary image, after dependency validation.
    pub fn size(&mut self, id: &str) -> Result<u64> {
        self.image_map(id).map(|(_, size)| size)
    }

    /// Reads assembled image bytes using the primary Map and explicit owners.
    /// Unknown/unreadable symbolic data and missing companions fail closed.
    pub fn read_at(&mut self, id: &str, buffer: &mut [u8], offset: u64) -> Result<usize> {
        let (map, size) = self.image_map(id)?;
        if offset >= size {
            return Ok(0);
        }
        let length = (size - offset).min(buffer.len() as u64) as usize;
        let mut done = 0;
        while done < length {
            let position = offset + done as u64;
            let next = map.ranges.partition_point(|r| r.end <= position);
            let (target, start, end) = match map.ranges.get(next) {
                Some(range) if range.start <= position => (
                    range.target.as_str(),
                    range.offset + position - range.start,
                    range.end,
                ),
                next => (map.gap.as_str(), position, next.map_or(size, |r| r.start)),
            };
            let take = (end - position).min((length - done) as u64) as usize;
            let owner = if is_symbolic(target) {
                0
            } else {
                *self
                    .owners
                    .get(target)
                    .ok_or_else(|| malformed("missing target volume"))?
            };
            if self.cached_owner != Some(owner) {
                if let Some(previous) = self.cached_owner {
                    self.volumes[previous].cache = None;
                    self.volumes[previous].index_cache = None;
                }
                self.cached_owner = Some(owner);
            }
            self.volumes[owner].read_inner(
                target,
                &mut buffer[done..done + take],
                start,
                &mut Vec::new(),
            )?;
            done += take;
        }
        Ok(length)
    }

    /// Computes whole-image SHA256 across companions. This does not certify the
    /// per-volume metadata or striped composite hashes. Optional external digest
    /// must cover the assembled image, not information.turtle or a ZIP file.
    pub fn verify_image(
        &mut self,
        id: &str,
        expected_sha256: Option<&str>,
        mut progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<SetDigest> {
        if expected_sha256
            .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(malformed("invalid external SHA256"));
        }
        let size = self.size(id)?;
        if size > self.volumes[0].limits.verification_bytes {
            return Err(malformed("verification byte limit exceeded"));
        }
        let mut buffer = vec![0; 1024 * 1024];
        let mut offset = 0;
        let mut hash = Sha256::new();
        loop {
            if progress(offset, size).is_break() {
                return Err(Error::Aborted);
            }
            if offset == size {
                break;
            }
            let read = self.read_at(id, &mut buffer, offset)?;
            if read == 0 {
                return Err(malformed("truncated volume set"));
            }
            hash.update(&buffer[..read]);
            offset += read as u64;
        }
        let sha256 = hex(&hash.finalize());
        Ok(SetDigest {
            image: id.into(),
            bytes: size,
            external_match: expected_sha256.map(|s| s.eq_ignore_ascii_case(&sha256)),
            sha256,
            sources: self.sources(),
        })
    }
}

fn is_symbolic(id: &str) -> bool {
    id == format!("{NS}Zero")
        || id.starts_with(&format!("{NS}SymbolicStream"))
        || id == format!("{NS}UnknownData")
        || id == format!("{NS}UnreadableData")
}
