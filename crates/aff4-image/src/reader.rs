use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use md5::{Digest, Md5};
use oxrdf::{NamedOrBlankNode, Term};
use oxttl::TurtleParser;
use sha1::Sha1;
use sha2::Sha256;
use zip::{CompressionMethod, ZipArchive};

use crate::{Error, Result, malformed};
use base64::Engine;

#[path = "integrity.rs"]
mod integrity;
pub use integrity::{CheckOutcome, IntegrityCheck, MetadataVerification};

#[path = "verify_all.rs"]
mod verify_all;
pub use verify_all::{ByteCoverage, ContainerVerification, ResourceVerification};

#[path = "scan.rs"]
mod scan;
pub use scan::MetadataScan;

#[path = "archive.rs"]
mod archive;

#[path = "volume_set.rs"]
mod volume_set;
pub use volume_set::{SetDigest, SetVerification, SetVolumeVerification, VolumeSet, VolumeSource};

#[path = "discovery.rs"]
mod discovery;
pub use discovery::{BackingVolume, DiskImageSet, PhysicalDisk, PhysicalDiskReader};

const NS: &str = "http://aff4.org/Schema#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const LOGICAL_NS: &str = "https://aff4.org/Schema/2022/#";
const LEGACY_LOGICAL_NS: &str = "http://aff4.org/Schema/2022/#";
const BASE64: &str = "http://www.w3.org/2001/XMLSchema#base64Binary";
const DECODED_CACHE_CAPACITY_BYTES: usize = 128 * 1024 * 1024;
const LOGICAL_READ_AHEAD_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
/// Cumulative performance counters for one shared AFF4 payload cache.
///
/// Physical readers for disks in the same discovered volume set share these
/// counters and the cache itself.
pub struct ReaderStatistics {
    decoded_cache_hits: u64,
    decoded_cache_misses: u64,
    read_ahead_cache_hits: u64,
    read_ahead_cache_misses: u64,
    read_ahead_prefetches: u64,
    decoded_cache_evictions: u64,
    decoded_bytes: u64,
    decode_nanos: u64,
    stored_member_range_reads: u64,
    stored_member_range_bytes: u64,
}

impl ReaderStatistics {
    /// Returns decoded chunk-cache hits.
    pub fn decoded_cache_hits(&self) -> u64 {
        self.decoded_cache_hits
    }

    /// Returns decoded chunk-cache misses.
    pub fn decoded_cache_misses(&self) -> u64 {
        self.decoded_cache_misses
    }

    /// Returns logical read-ahead page-cache hits.
    pub fn read_ahead_cache_hits(&self) -> u64 {
        self.read_ahead_cache_hits
    }

    /// Returns logical read-ahead page-cache misses.
    pub fn read_ahead_cache_misses(&self) -> u64 {
        self.read_ahead_cache_misses
    }

    /// Returns logical pages fetched automatically after small positioned reads.
    pub fn read_ahead_prefetches(&self) -> u64 {
        self.read_ahead_prefetches
    }

    /// Returns decoded pages or chunks evicted to retain the automatic memory bound.
    pub fn decoded_cache_evictions(&self) -> u64 {
        self.decoded_cache_evictions
    }

    /// Returns logical chunk bytes decoded after cache misses.
    pub fn decoded_bytes(&self) -> u64 {
        self.decoded_bytes
    }

    /// Returns nanoseconds spent decoding chunks after cache misses.
    pub fn decode_nanos(&self) -> u64 {
        self.decode_nanos
    }

    /// Returns direct range reads from uncompressed ZIP members.
    pub fn stored_member_range_reads(&self) -> u64 {
        self.stored_member_range_reads
    }

    /// Returns bytes read directly from uncompressed ZIP members.
    pub fn stored_member_range_bytes(&self) -> u64 {
        self.stored_member_range_bytes
    }

    /// Returns a field-wise saturating delta from an earlier snapshot.
    #[must_use]
    pub fn saturating_delta(self, earlier: Self) -> Self {
        Self {
            decoded_cache_hits: self
                .decoded_cache_hits
                .saturating_sub(earlier.decoded_cache_hits),
            decoded_cache_misses: self
                .decoded_cache_misses
                .saturating_sub(earlier.decoded_cache_misses),
            read_ahead_cache_hits: self
                .read_ahead_cache_hits
                .saturating_sub(earlier.read_ahead_cache_hits),
            read_ahead_cache_misses: self
                .read_ahead_cache_misses
                .saturating_sub(earlier.read_ahead_cache_misses),
            read_ahead_prefetches: self
                .read_ahead_prefetches
                .saturating_sub(earlier.read_ahead_prefetches),
            decoded_cache_evictions: self
                .decoded_cache_evictions
                .saturating_sub(earlier.decoded_cache_evictions),
            decoded_bytes: self.decoded_bytes.saturating_sub(earlier.decoded_bytes),
            decode_nanos: self.decode_nanos.saturating_sub(earlier.decode_nanos),
            stored_member_range_reads: self
                .stored_member_range_reads
                .saturating_sub(earlier.stored_member_range_reads),
            stored_member_range_bytes: self
                .stored_member_range_bytes
                .saturating_sub(earlier.stored_member_range_bytes),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
/// Configured and observed payload bytes for the automatic AFF4 reader cache.
pub struct ReaderCacheInfo {
    capacity_bytes: u64,
    current_bytes: u64,
    peak_bytes: u64,
    entries: u64,
}

impl ReaderCacheInfo {
    /// Returns the automatic decoded payload-cache byte ceiling.
    pub fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }

    /// Returns currently retained decoded page and chunk bytes.
    pub fn current_bytes(&self) -> u64 {
        self.current_bytes
    }

    /// Returns peak retained decoded page and chunk bytes.
    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes
    }

    /// Returns the number of retained decoded pages and chunks.
    pub fn entries(&self) -> u64 {
        self.entries
    }
}

#[derive(Clone, Hash, PartialEq, Eq)]
enum DecodedCacheKey {
    Chunk {
        volume: String,
        resource: String,
        chunk: u64,
    },
    DiskPage {
        volume: String,
        resource: String,
        offset: u64,
    },
}

struct DecodedChunkCache {
    entries: LruCache<DecodedCacheKey, Arc<Vec<u8>>>,
    capacity_bytes: usize,
    current_bytes: usize,
    peak_bytes: usize,
    statistics: ReaderStatistics,
}

impl DecodedChunkCache {
    fn new() -> Self {
        Self::with_capacity(DECODED_CACHE_CAPACITY_BYTES)
    }

    fn with_capacity(capacity_bytes: usize) -> Self {
        Self {
            entries: LruCache::unbounded(),
            capacity_bytes,
            current_bytes: 0,
            peak_bytes: 0,
            statistics: ReaderStatistics::default(),
        }
    }

    fn get_chunk(&mut self, key: &DecodedCacheKey) -> Option<Arc<Vec<u8>>> {
        let result = self.entries.get(key).cloned();
        if result.is_some() {
            self.statistics.decoded_cache_hits =
                self.statistics.decoded_cache_hits.saturating_add(1);
        } else {
            self.statistics.decoded_cache_misses =
                self.statistics.decoded_cache_misses.saturating_add(1);
        }
        result
    }

    fn get_page(&mut self, key: &DecodedCacheKey) -> Option<Arc<Vec<u8>>> {
        let result = self.entries.get(key).cloned();
        if result.is_some() {
            self.statistics.read_ahead_cache_hits =
                self.statistics.read_ahead_cache_hits.saturating_add(1);
        } else {
            self.statistics.read_ahead_cache_misses =
                self.statistics.read_ahead_cache_misses.saturating_add(1);
        }
        result
    }

    fn insert(&mut self, key: DecodedCacheKey, decoded: Arc<Vec<u8>>) {
        let bytes = decoded.len();
        if bytes > self.capacity_bytes {
            return;
        }
        if let Some(previous) = self.entries.put(key, decoded) {
            self.current_bytes = self.current_bytes.saturating_sub(previous.len());
        }
        self.current_bytes = self.current_bytes.saturating_add(bytes);
        while self.current_bytes > self.capacity_bytes {
            let Some((_, evicted)) = self.entries.pop_lru() else {
                break;
            };
            self.current_bytes = self.current_bytes.saturating_sub(evicted.len());
            self.statistics.decoded_cache_evictions =
                self.statistics.decoded_cache_evictions.saturating_add(1);
        }
        self.peak_bytes = self.peak_bytes.max(self.current_bytes);
    }

    fn record_decode(&mut self, bytes: usize, elapsed: Duration) {
        self.statistics.decoded_bytes = self
            .statistics
            .decoded_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        self.statistics.decode_nanos = self
            .statistics
            .decode_nanos
            .saturating_add(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX));
    }

    fn record_read_ahead_prefetch(&mut self) {
        self.statistics.read_ahead_prefetches =
            self.statistics.read_ahead_prefetches.saturating_add(1);
    }

    fn record_stored_member_range_read(&mut self, bytes: usize) {
        self.statistics.stored_member_range_reads =
            self.statistics.stored_member_range_reads.saturating_add(1);
        self.statistics.stored_member_range_bytes = self
            .statistics
            .stored_member_range_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.current_bytes = 0;
    }

    fn info(&self) -> ReaderCacheInfo {
        ReaderCacheInfo {
            capacity_bytes: self.capacity_bytes as u64,
            current_bytes: self.current_bytes as u64,
            peak_bytes: self.peak_bytes as u64,
            entries: self.entries.len() as u64,
        }
    }
}

/// Limits on retained metadata, map/index members, and decoded chunks.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Limits {
    /// Maximum ZIP central-directory bytes, checked before ZIP parsing.
    pub directory_bytes: u64,
    /// Maximum ZIP entries, checked before ZIP parsing.
    pub archive_entries: usize,
    /// Maximum decompressed metadata bytes.
    pub metadata_bytes: u64,
    /// Maximum bytes in an individual retained data/index member.
    pub member_bytes: u64,
    /// Maximum decoded chunk size.
    pub chunk_bytes: u64,
    /// Maximum RDF triples retained.
    pub triples: usize,
    /// Maximum retained map range records and target strings, in bytes.
    pub map_bytes: usize,
    /// Maximum logical bytes traversed by one verification/copy operation.
    /// Container-wide verification shares this across resources and block scans.
    pub verification_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            directory_bytes: 64 * 1024 * 1024,
            archive_entries: 200_000,
            metadata_bytes: 16 * 1024 * 1024,
            member_bytes: 128 * 1024 * 1024,
            chunk_bytes: 16 * 1024 * 1024,
            triples: 200_000,
            map_bytes: 128 * 1024 * 1024,
            verification_bytes: 64 * 1024 * 1024 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// Uses platform capacity instead of application resource quotas.
    ///
    /// Intended for local CLI operations. This does not reserve memory or promise
    /// recovery from process-wide memory exhaustion. Format validation, checked
    /// arithmetic, and dependency-cycle checks still apply. Library callers
    /// processing untrusted inputs should normally retain explicit limits.
    pub const fn unrestricted() -> Self {
        Self {
            directory_bytes: isize::MAX as u64,
            archive_entries: isize::MAX as usize,
            metadata_bytes: isize::MAX as u64,
            member_bytes: isize::MAX as u64,
            chunk_bytes: isize::MAX as u64,
            triples: isize::MAX as usize,
            map_bytes: isize::MAX as usize,
            verification_bytes: u64::MAX,
        }
    }
}

/// Preserved RDF property; values remain associated with their subject.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Property {
    /// Fully expanded predicate IRI.
    pub predicate: String,
    /// IRI, blank-node label, or literal lexical value.
    pub value: String,
    /// Literal datatype IRI; absent for resource references.
    pub datatype: Option<String>,
    /// Literal language tag, when present.
    pub language: Option<String>,
}

/// An explicitly selectable image or underlying data stream.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StreamInfo {
    /// AFF4 resource identifier.
    pub id: String,
    /// RDF classes on the resource.
    pub types: Vec<String>,
    /// Logical byte size when supplied or resolvable from its data stream.
    pub size: Option<u64>,
}

/// Selected-stream linear digest results. Unsupported references are explicit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Verification {
    /// Number of logical bytes read.
    pub bytes_verified: u64,
    /// Computed hexadecimal MD5.
    pub md5: String,
    /// Computed hexadecimal SHA1.
    pub sha1: String,
    /// Computed hexadecimal SHA256.
    pub sha256: String,
    /// Computed hexadecimal SHA512.
    pub sha512: String,
    /// Computed hexadecimal BLAKE2b-512.
    pub blake2b: String,
    /// Available supported linear references all match; None means none exist.
    pub references_match: Option<bool>,
    /// Datatypes of stored hashes not verified, including block-map hashes.
    pub unsupported_hashes: Vec<String>,
}

/// One read-only ZIP container. Methods serialize access through `&mut self`.
/// Open does not certify contents. Backing files must remain unchanged.
pub struct Container {
    archive: ZipArchive<File>,
    stored_source: File,
    volume: String,
    graph: BTreeMap<String, Vec<Property>>,
    limits: Limits,
    cache: Option<(String, Vec<u8>)>,
    index_cache: Option<(String, Vec<u8>)>,
    stored_member: Option<StoredMember>,
    decoded_cache: Arc<Mutex<DecodedChunkCache>>,
    maps: BTreeMap<String, Arc<Map>>,
    version: (u32, u32),
    metadata_members: BTreeMap<String, String>,
    usage: archive::Usage,
}

struct StoredMember {
    name: String,
    size: u64,
    data_start: Option<u64>,
}

struct PositionedReader<'a> {
    container: &'a mut Container,
    id: String,
    position: u64,
}

impl Read for PositionedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self
            .container
            .read_at(&self.id, buffer, self.position)
            .map_err(std::io::Error::other)?;
        self.position += count as u64;
        Ok(count)
    }
}

#[derive(Clone)]
struct Range {
    start: u64,
    end: u64,
    offset: u64,
    target: String,
}
#[derive(Clone)]
struct Map {
    ranges: Vec<Range>,
    gap: String,
}

impl Container {
    /// Opens a standard v1 ZIP container with default resource limits.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limits(path, Limits::default())
    }

    /// Opens a container, validates its version, and parses bounded RDF metadata.
    /// External RDF references are never fetched over the network.
    pub fn open_with_limits(path: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        let (mut archive, mut usage) = archive::open(path.as_ref(), &limits)?;
        let stored_source = File::open(path.as_ref())?;
        let mut names = BTreeSet::new();
        for name in archive.file_names() {
            if !names.insert(name.to_owned()) {
                return Err(malformed("duplicate ZIP member name"));
            }
        }
        let version = member(&mut archive, "version.txt", 4096)?;
        let version =
            std::str::from_utf8(&version).map_err(|_| malformed("invalid version text"))?;
        let mut fields = BTreeMap::new();
        for line in version.lines() {
            if let Some((key, value)) = line.split_once('=')
                && fields.insert(key.trim(), value.trim()).is_some()
            {
                return Err(malformed("duplicate version field"));
            }
        }
        let version = (
            fields
                .get("major")
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| malformed("missing or invalid major version"))?,
            fields
                .get("minor")
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| malformed("missing or invalid minor version"))?,
        );
        if !matches!(version, (1, 0) | (1, 1) | (2, 1)) {
            return Err(Error::Unsupported(
                "container version (supported: 1.0, legacy logical 1.1, draft logical 2.1)".into(),
            ));
        }
        let comment = std::str::from_utf8(archive.comment())
            .map_err(|_| malformed("volume comment is not UTF-8"))?
            .trim_end_matches('\0')
            .trim()
            .to_owned();
        let description = match archive.index_for_name("container.description") {
            Some(_) => String::from_utf8(member(&mut archive, "container.description", 4096)?)
                .map_err(|_| malformed("volume name is not UTF-8"))?,
            None => comment.clone(),
        };
        let volume = description.trim_end_matches('\0').trim().to_owned();
        if !volume.starts_with("aff4://") || volume.len() <= 7 {
            return Err(malformed("missing AFF4 volume URI"));
        }
        if !comment.is_empty() && comment != volume {
            return Err(malformed("conflicting volume identifiers"));
        }
        let mut graph: BTreeMap<String, Vec<Property>> = BTreeMap::new();
        let mut pending = vec!["information.turtle".to_owned()];
        let mut seen = BTreeSet::new();
        let mut metadata_members = BTreeMap::new();
        let mut remaining = limits.metadata_bytes;
        let mut count = 0;
        while let Some(name) = pending.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let turtle = member(&mut archive, &name, remaining)?;
            remaining -= turtle.len() as u64;
            metadata_members.insert(name.clone(), hex(&Sha256::digest(&turtle)));
            for triple in TurtleParser::new().for_reader(turtle.as_slice()) {
                if count >= limits.triples {
                    return Err(malformed("RDF triple limit exceeded"));
                }
                let triple = triple.map_err(|error| malformed(format!("Turtle: {error}")))?;
                count += 1;
                let subject = match triple.subject {
                    NamedOrBlankNode::NamedNode(n) => n.into_string(),
                    NamedOrBlankNode::BlankNode(n) => format!("_:{n}"),
                };
                let (value, datatype, language) = match triple.object {
                    Term::NamedNode(n) => (n.into_string(), None, None),
                    Term::BlankNode(n) => (format!("_:{n}"), None, None),
                    Term::Literal(l) => (
                        l.value().to_owned(),
                        Some(l.datatype().as_str().to_owned()),
                        l.language().map(str::to_owned),
                    ),
                };
                if is_property(triple.predicate.as_str(), "imports") {
                    if datatype.is_some() {
                        return Err(malformed("metadata import must be an IRI"));
                    }
                    pending.push(storage_name(&archive, &volume, &value, version)?);
                }
                graph.entry(subject).or_default().push(Property {
                    predicate: triple.predicate.into_string(),
                    value,
                    datatype,
                    language,
                });
            }
        }
        usage.metadata_bytes = limits.metadata_bytes - remaining;
        usage.triples = count;
        Ok(Self {
            archive,
            stored_source,
            volume,
            graph,
            limits,
            cache: None,
            index_cache: None,
            stored_member: None,
            decoded_cache: Arc::new(Mutex::new(DecodedChunkCache::new())),
            maps: BTreeMap::new(),
            version,
            metadata_members,
            usage,
        })
    }

    /// Preserves all parsed RDF properties, including unsupported extensions.
    pub fn metadata(&self) -> &BTreeMap<String, Vec<Property>> {
        &self.graph
    }

    /// Returns the declared volume identifier.
    pub fn volume_id(&self) -> &str {
        &self.volume
    }

    /// Declared container version; 2.1 refers to the evolving AFF4-L draft.
    pub fn version(&self) -> (u32, u32) {
        self.version
    }

    /// Returns cumulative decoded-chunk reader counters.
    pub fn reader_statistics(&self) -> ReaderStatistics {
        self.decoded_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .statistics
    }

    /// Returns automatic decoded-chunk cache usage.
    pub fn reader_cache_info(&self) -> ReaderCacheInfo {
        self.decoded_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .info()
    }

    /// Enumerates image resources and storage streams. Selection is never implicit.
    pub fn streams(&self) -> Result<Vec<StreamInfo>> {
        self.graph
            .keys()
            .filter(|id| {
                [
                    "Image",
                    "DiskImage",
                    "ImageStream",
                    "Map",
                    "FileImage",
                    "FileSubStream",
                ]
                .iter()
                .any(|t| self.has_type(id, t))
            })
            .map(|id| {
                Ok(StreamInfo {
                    id: id.clone(),
                    types: self
                        .properties(id)
                        .filter(|p| p.predicate == RDF_TYPE)
                        .map(|p| p.value.clone())
                        .collect(),
                    size: self.stream_size(id, &mut Vec::new()).ok(),
                })
            })
            .collect()
    }

    /// Returns the size of a selected resource, resolving its dataStream reference.
    pub fn size(&self, id: &str) -> Result<u64> {
        self.stream_size(id, &mut Vec::new())
    }

    /// Reads a selected stream at a byte offset. Missing targets, cycles,
    /// corrupt chunks, and unknown/unreadable symbolic data return errors.
    pub fn read_at(&mut self, id: &str, buffer: &mut [u8], offset: u64) -> Result<usize> {
        self.read_at_impl(id, buffer, offset, true)
    }

    /// Reads a resource sequentially. ZIP segments, including those behind
    /// identity maps, retain one decompressor across reads; other resources
    /// use positioned reads through their maps.
    /// The caller must read the declared size to detect a truncated stream.
    pub fn sequential_reader(&mut self, id: &str) -> Result<Box<dyn Read + '_>> {
        let size = self.size(id)?;
        self.read_inner(id, &mut [], 0, &mut Vec::new(), false)?;
        let target = self.linear_backing(id)?;
        if self.has_type(&target, "ZipSegment") || self.has_type(&target, "zip_segment") {
            let path = self.path(&target)?;
            let file = self.archive.by_name(&path)?;
            if file.size() != size {
                return Err(malformed("ZIP segment size mismatch"));
            }
            return Ok(Box::new(file));
        }
        Ok(Box::new(PositionedReader {
            container: self,
            id: id.to_owned(),
            position: 0,
        }))
    }

    fn read_at_uncached(&mut self, id: &str, buffer: &mut [u8], offset: u64) -> Result<usize> {
        self.read_at_impl(id, buffer, offset, false)
    }

    fn read_at_impl(
        &mut self,
        id: &str,
        buffer: &mut [u8],
        offset: u64,
        use_decoded_cache: bool,
    ) -> Result<usize> {
        let size = self.size(id)?;
        if offset >= size || buffer.is_empty() {
            return Ok(0);
        }
        let length = (size - offset).min(buffer.len() as u64) as usize;
        self.read_inner(
            id,
            &mut buffer[..length],
            offset,
            &mut Vec::new(),
            use_decoded_cache,
        )?;
        Ok(length)
    }

    /// Computes linear hashes with progress and cancellation. Does not verify
    /// block hashes, map hashes, signatures, or metadata integrity.
    pub fn verify(
        &mut self,
        id: &str,
        progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<Verification> {
        self.copy_verified(id, &mut std::io::sink(), progress)
    }

    /// Copies exact logical bytes while computing and comparing linear hashes.
    /// The caller owns staging/publication and must inspect `references_match`.
    /// Partial output may remain in the sink after any failure or cancellation.
    pub fn copy_verified(
        &mut self,
        id: &str,
        output: &mut impl Write,
        mut progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<Verification> {
        let size = self.size(id)?;
        if size > self.limits.verification_bytes {
            return Err(malformed("verification byte limit exceeded"));
        }
        self.cache = None;
        self.index_cache = None;
        self.decoded_cache
            .lock()
            .map_err(|_| malformed("AFF4 decoded cache lock poisoned"))?
            .clear();
        self.maps.clear();
        self.read_inner(id, &mut [], 0, &mut Vec::new(), false)?;
        let mut md5 = Md5::new();
        let mut sha1 = Sha1::new();
        let mut sha256 = Sha256::new();
        let mut sha512 = sha2::Sha512::new();
        let mut blake2b = <blake2::Blake2b512 as blake2::Digest>::new();
        let mut write_error = None;
        let walked = self.walk_bytes(id, |bytes, done| {
            if let Err(error) = output.write_all(bytes) {
                write_error = Some(error);
                return ControlFlow::Break(());
            }
            md5.update(bytes);
            sha1.update(bytes);
            sha256.update(bytes);
            sha512.update(bytes);
            blake2::Digest::update(&mut blake2b, bytes);
            progress(done, size)
        });
        if let Some(error) = write_error {
            return Err(error.into());
        }
        walked?;
        let result = Verification {
            bytes_verified: size,
            md5: hex(&md5.finalize()),
            sha1: hex(&sha1.finalize()),
            sha256: hex(&sha256.finalize()),
            sha512: hex(&sha512.finalize()),
            blake2b: hex(&blake2::Digest::finalize(blake2b)),
            references_match: None,
            unsupported_hashes: Vec::new(),
        };
        self.with_linear_references(id, result)
    }

    fn with_linear_references(&self, id: &str, mut result: Verification) -> Result<Verification> {
        result.references_match = None;
        result.unsupported_hashes.clear();
        for reference in self
            .properties(id)
            .filter(|p| is_property(&p.predicate, "hash"))
        {
            let datatype = reference.datatype.as_deref().unwrap_or("");
            let expected = match datatype.strip_prefix(NS) {
                Some("MD5") => &result.md5,
                Some("SHA1") => &result.sha1,
                Some("SHA256") => &result.sha256,
                Some("SHA512") => &result.sha512,
                Some("Blake2b" | "blake2b") => &result.blake2b,
                _ => {
                    result.unsupported_hashes.push(datatype.into());
                    continue;
                }
            };
            if reference.value.len() != expected.len()
                || !reference.value.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(malformed("invalid linear hash"));
            }
            result.references_match = Some(
                result.references_match.unwrap_or(true)
                    && reference.value.eq_ignore_ascii_case(expected),
            );
        }
        Ok(result)
    }

    // A dataStream wrapper reads the same bytes as its target. A Map is also
    // equivalent only when one range covers the entire equally sized target
    // from offset zero. Sparse, partial, and reordered maps retain their own
    // traversal in full-container verification.
    fn linear_backing(&mut self, id: &str) -> Result<String> {
        let mut target = id.to_owned();
        let mut visited = Vec::new();
        loop {
            enter(&target, &mut visited)?;
            if self.inline_data(&target)?.is_some() {
                return Ok(target);
            }
            if let Some(next) = self.value(&target, "dataStream")? {
                target = next;
                continue;
            }
            if self.has_type(&target, "Map") {
                self.load_map(&target)?;
                let map = &self.maps[&target];
                if let [range] = map.ranges.as_slice()
                    && range.start == 0
                    && range.offset == 0
                    && range.end == self.size(&target)?
                    && self.size(&range.target).ok() == Some(range.end)
                {
                    target = range.target.clone();
                    continue;
                }
            }
            return Ok(target);
        }
    }

    // ZIP files must be decoded once for sequential verification. Positioned
    // reads remain available, but restarting Deflate for each buffer is quadratic.
    fn walk_bytes(
        &mut self,
        id: &str,
        mut consume: impl FnMut(&[u8], u64) -> ControlFlow<()>,
    ) -> Result<()> {
        let size = self.size(id)?;
        if consume(&[], 0).is_break() {
            return Err(Error::Aborted);
        }
        self.read_inner(id, &mut [], 0, &mut Vec::new(), false)?;
        let target = self.linear_backing(id)?;
        let mut buffer = vec![0; 1024 * 1024];
        if self.has_type(&target, "ZipSegment") || self.has_type(&target, "zip_segment") {
            let path = self.path(&target)?;
            let mut file = self.archive.by_name(&path)?;
            if file.size() != size {
                return Err(malformed("ZIP segment size mismatch"));
            }
            let mut offset = 0;
            while offset < size {
                let take = (size - offset).min(buffer.len() as u64) as usize;
                file.read_exact(&mut buffer[..take])?;
                offset += take as u64;
                if consume(&buffer[..take], offset).is_break() {
                    return Err(Error::Aborted);
                }
            }
            if file.read(&mut buffer[..1])? != 0 {
                return Err(malformed("oversized ZIP segment"));
            }
        } else {
            let mut offset = 0;
            while offset < size {
                let read = self.read_at_uncached(id, &mut buffer, offset)?;
                if read == 0 {
                    return Err(malformed("truncated stream"));
                }
                offset += read as u64;
                if consume(&buffer[..read], offset).is_break() {
                    return Err(Error::Aborted);
                }
            }
        }
        Ok(())
    }

    fn properties<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a Property> {
        self.graph.get(id).into_iter().flatten()
    }
    fn has_type(&self, id: &str, kind: &str) -> bool {
        self.properties(id)
            .any(|p| p.predicate == RDF_TYPE && is_property(&p.value, kind))
    }
    fn value(&self, id: &str, key: &str) -> Result<Option<String>> {
        let values: BTreeSet<_> = self
            .properties(id)
            .filter(|p| is_property(&p.predicate, key))
            .map(|p| p.value.clone())
            .collect();
        if values.len() > 1 {
            return Err(malformed(format!("ambiguous {key}")));
        }
        Ok(values.into_iter().next())
    }
    fn number(&self, id: &str, key: &str) -> Result<u64> {
        self.value(id, key)?
            .ok_or_else(|| malformed(format!("missing {key}")))?
            .parse()
            .map_err(|_| malformed(format!("invalid {key}")))
    }
    fn stream_size(&self, id: &str, visited: &mut Vec<String>) -> Result<u64> {
        enter(id, visited)?;
        if self.inline_data(id)?.is_some() {
            return self.number(id, "size");
        }
        if let Some(target) = self.value(id, "dataStream")? {
            let size = self.stream_size(&target, visited)?;
            if self.value(id, "size")?.is_some() && self.number(id, "size")? != size {
                return Err(malformed("inconsistent stream size"));
            }
            Ok(size)
        } else {
            self.number(id, "size")
        }
    }
    fn path(&self, id: &str) -> Result<String> {
        if !self.has_type(id, "FileImage")
            && let Some(path) = self.value(id, "fileName")?
        {
            return Ok(path);
        }
        storage_name(&self.archive, &self.volume, id, self.version)
    }

    fn inline_data(&self, id: &str) -> Result<Option<Vec<u8>>> {
        let mut result = None;
        for p in self.properties(id).filter(|p| {
            (is_property(&p.predicate, "dataStream") || is_property(&p.predicate, "dataSteam"))
                && p.datatype.as_deref() == Some(BASE64)
        }) {
            if p.value.len() > 1400 {
                return Err(malformed("inline data exceeds 1 KiB limit"));
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&p.value)
                .map_err(|_| malformed("invalid inline base64"))?;
            if bytes.len() > 1024 || bytes.len() as u64 != self.number(id, "size")? {
                return Err(malformed("inline data size mismatch"));
            }
            if result.as_ref().is_some_and(|old| old != &bytes) {
                return Err(malformed("conflicting inline data"));
            }
            result = Some(bytes);
        }
        if result.is_some()
            && self.properties(id).any(|p| {
                (is_property(&p.predicate, "dataStream") || is_property(&p.predicate, "dataSteam"))
                    && p.datatype.as_deref() != Some(BASE64)
            })
        {
            return Err(malformed("conflicting inline and referenced data streams"));
        }
        Ok(result)
    }
    fn load_map(&mut self, id: &str) -> Result<()> {
        if self.maps.contains_key(id) {
            return Ok(());
        }
        let path = self.path(id)?;
        let data = member(
            &mut self.archive,
            &format!("{path}/map"),
            self.limits.member_bytes,
        )?;
        let index = member(
            &mut self.archive,
            &format!("{path}/idx"),
            self.limits.metadata_bytes,
        )?;
        let index =
            std::str::from_utf8(&index).map_err(|_| malformed("map target index is not UTF-8"))?;
        let targets: Vec<_> = index.lines().collect();
        if data.len() % 28 != 0 {
            return Err(malformed("partial map record"));
        }
        let records = data.len() / 28;
        if records > self.limits.map_bytes / std::mem::size_of::<Range>() {
            return Err(malformed("retained map byte limit exceeded"));
        }
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(records)
            .map_err(allocation_error)?;
        let mut retained = 0_usize;
        let mut previous = 0;
        let size = self.number(id, "size")?;
        for record in data.as_chunks::<28>().0 {
            let start = u64::from_le_bytes(record[..8].try_into().unwrap());
            let length = u64::from_le_bytes(record[8..16].try_into().unwrap());
            let offset = u64::from_le_bytes(record[16..24].try_into().unwrap());
            let target = u32::from_le_bytes(record[24..].try_into().unwrap()) as usize;
            let end = start
                .checked_add(length)
                .ok_or_else(|| malformed("map overflow"))?;
            if length == 0 || start < previous || end > size || offset.checked_add(length).is_none()
            {
                return Err(malformed("invalid or overlapping map range"));
            }
            let target = *targets
                .get(target)
                .ok_or_else(|| malformed("missing map target"))?;
            retained = retained
                .checked_add(std::mem::size_of::<Range>())
                .and_then(|n| n.checked_add(target.len()))
                .filter(|n| *n <= self.limits.map_bytes)
                .ok_or_else(|| malformed("retained map byte limit exceeded"))?;
            ranges.push(Range {
                start,
                end,
                offset,
                target: target.to_owned(),
            });
            previous = end;
        }
        // Keep one map at a time, bounding retained range metadata across streams.
        self.maps.clear();
        self.maps.insert(
            id.into(),
            Arc::new(Map {
                ranges,
                gap: self
                    .value(id, "mapGapDefaultStream")?
                    .unwrap_or_else(|| format!("{NS}Zero")),
            }),
        );
        Ok(())
    }

    fn read_stored_member_range(
        &mut self,
        name: &str,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<bool> {
        if self
            .stored_member
            .as_ref()
            .is_none_or(|member| member.name != name)
        {
            let directory_start = self.archive.central_directory_start();
            let file = self.archive.by_name(name)?;
            if file.size() > self.limits.member_bytes {
                return Err(malformed(format!("member exceeds resource limit: {name}")));
            }
            let size = file.size();
            let data_start = (file.compression() == CompressionMethod::Stored
                && !file.encrypted()
                && file.compressed_size() == size)
                .then(|| file.data_start())
                .flatten();
            if data_start
                .and_then(|start| start.checked_add(size))
                .is_some_and(|end| end > directory_start)
            {
                return Err(malformed("stored ZIP member exceeds archive data"));
            }
            self.stored_member = Some(StoredMember {
                name: name.to_owned(),
                size,
                data_start,
            });
        }
        let member = self.stored_member.as_ref().unwrap();
        if offset
            .checked_add(buffer.len() as u64)
            .is_none_or(|end| end > member.size)
        {
            return Err(malformed("member range exceeds stored ZIP entry"));
        }
        let Some(data_start) = member.data_start else {
            return Ok(false);
        };
        let absolute = data_start
            .checked_add(offset)
            .ok_or_else(|| malformed("stored ZIP member offset overflow"))?;
        read_file_exact_at(&self.stored_source, absolute, buffer)?;
        self.decoded_cache
            .lock()
            .map_err(|_| malformed("AFF4 decoded cache lock poisoned"))?
            .record_stored_member_range_read(buffer.len());
        Ok(true)
    }

    fn read_inner(
        &mut self,
        id: &str,
        buffer: &mut [u8],
        offset: u64,
        visited: &mut Vec<String>,
        use_decoded_cache: bool,
    ) -> Result<()> {
        enter(id, visited)?;
        let result = self.read_inner_impl(id, buffer, offset, visited, use_decoded_cache);
        visited.pop();
        result
    }
    fn read_inner_impl(
        &mut self,
        id: &str,
        buffer: &mut [u8],
        offset: u64,
        visited: &mut Vec<String>,
        use_decoded_cache: bool,
    ) -> Result<()> {
        if id == format!("{NS}Zero") {
            buffer.fill(0);
            return Ok(());
        }
        if let Some(value) = id.strip_prefix(&format!("{NS}SymbolicStream")) {
            if value.len() != 2 {
                return Err(malformed("invalid symbolic byte"));
            }
            buffer.fill(
                u8::from_str_radix(value, 16).map_err(|_| malformed("invalid symbolic byte"))?,
            );
            return Ok(());
        }
        if id == format!("{NS}UnknownData") || id == format!("{NS}UnreadableData") {
            return Err(Error::Unsupported(
                "unknown or unreadable evidence range".into(),
            ));
        }
        let size = self.size(id)?;
        if offset
            .checked_add(buffer.len() as u64)
            .is_none_or(|end| end > size)
        {
            return Err(malformed("target range exceeds stream"));
        }
        if let Some(data) = self.inline_data(id)? {
            buffer.copy_from_slice(&data[offset as usize..offset as usize + buffer.len()]);
            return Ok(());
        }
        if let Some(target) = self.value(id, "dataStream")? {
            if self.has_type(id, "ContiguousImage") && self.has_type(&target, "Map") {
                self.load_map(&target)?;
                let map = &self.maps[&target];
                let mut end = 0;
                for range in &map.ranges {
                    if range.start != end {
                        return Err(malformed("gap in declared contiguous image"));
                    }
                    end = range.end;
                }
                if end != size {
                    return Err(malformed("truncated contiguous image map"));
                }
            }
            return self.read_inner(&target, buffer, offset, visited, use_decoded_cache);
        }
        if self.has_type(id, "ZipSegment") || self.has_type(id, "zip_segment") {
            let path = self.path(id)?;
            let mut file = self.archive.by_name(&path)?;
            if file.size() != size {
                return Err(malformed("ZIP segment size mismatch"));
            }
            if std::io::copy(&mut file.by_ref().take(offset), &mut std::io::sink())? != offset {
                return Err(malformed("truncated ZIP segment"));
            }
            file.read_exact(buffer)?;
            if offset + buffer.len() as u64 == size {
                let mut eof = [0];
                if file.read(&mut eof)? != 0 {
                    return Err(malformed("oversized ZIP segment"));
                }
            }
            return Ok(());
        }
        if self.has_type(id, "Map") {
            self.load_map(id)?;
            let map = self.maps[id].clone();
            let mut done = 0;
            while done < buffer.len() {
                let position = offset + done as u64;
                let next = map.ranges.partition_point(|r| r.end <= position);
                let (target, target_offset, end) = match map.ranges.get(next) {
                    Some(range) if range.start <= position => (
                        range.target.as_str(),
                        range.offset + (position - range.start),
                        range.end,
                    ),
                    next => (
                        map.gap.as_str(),
                        position,
                        next.map_or(self.number(id, "size")?, |r| r.start),
                    ),
                };
                let length = (end - position).min((buffer.len() - done) as u64) as usize;
                if length == 0 {
                    return Err(malformed("map makes no progress"));
                }
                self.read_inner(
                    target,
                    &mut buffer[done..done + length],
                    target_offset,
                    visited,
                    use_decoded_cache,
                )?;
                done += length;
            }
            return Ok(());
        }
        if !self.has_type(id, "ImageStream") {
            return Err(Error::Unsupported(format!("stream kind: {id}")));
        }
        let chunk_size = self.number(id, "chunkSize")?;
        let per_bevy = self.number(id, "chunksInSegment")?;
        if chunk_size == 0 || chunk_size > self.limits.chunk_bytes || per_bevy == 0 {
            return Err(malformed("invalid chunk geometry"));
        }
        let path = self.path(id)?;
        let compression = self.value(id, "compressionMethod")?;
        let mut done = 0;
        while done < buffer.len() {
            let position = offset + done as u64;
            let chunk = position / chunk_size;
            let cache_key = DecodedCacheKey::Chunk {
                volume: self.volume.clone(),
                resource: id.to_owned(),
                chunk,
            };
            let cached = if use_decoded_cache {
                self.decoded_cache
                    .lock()
                    .map_err(|_| malformed("AFF4 decoded cache lock poisoned"))?
                    .get_chunk(&cache_key)
            } else {
                None
            };
            if let Some(decoded) = cached {
                let begin = (position % chunk_size) as usize;
                let take = (decoded.len() - begin).min(buffer.len() - done);
                buffer[done..done + take].copy_from_slice(&decoded[begin..begin + take]);
                done += take;
                continue;
            }
            let name = format!("{path}/{:08}", chunk / per_bevy);
            if self
                .index_cache
                .as_ref()
                .is_none_or(|(key, _)| key != &name)
            {
                self.index_cache = Some((
                    name.clone(),
                    member(
                        &mut self.archive,
                        &format!("{name}.index"),
                        self.limits.member_bytes,
                    )?,
                ));
            }
            let index = &self.index_cache.as_ref().unwrap().1;
            if !index.len().is_multiple_of(12) {
                return Err(malformed("partial chunk index record"));
            }
            let index_offset = usize::try_from(chunk % per_bevy)
                .ok()
                .and_then(|n| n.checked_mul(12))
                .ok_or_else(|| malformed("index overflow"))?;
            let index_end = index_offset
                .checked_add(12)
                .ok_or_else(|| malformed("index overflow"))?;
            let record = index
                .get(index_offset..index_end)
                .ok_or_else(|| malformed("missing chunk index"))?;
            let start = u64::from_le_bytes(record[..8].try_into().unwrap());
            let length = u32::from_le_bytes(record[8..].try_into().unwrap()) as u64;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= self.limits.member_bytes)
                .ok_or_else(|| malformed("chunk outside bevy"))?;
            let mut direct = chunk_buffer(length as usize)?;
            let direct_read = self.read_stored_member_range(&name, start, &mut direct)?;
            if direct_read {
                self.cache = None;
            } else if self.cache.as_ref().is_none_or(|(key, _)| key != &name) {
                self.cache = Some((
                    name.clone(),
                    member(&mut self.archive, &name, self.limits.member_bytes)?,
                ));
            }
            let encoded = if direct_read {
                direct.as_slice()
            } else {
                let data = &self.cache.as_ref().unwrap().1;
                if end > data.len() as u64 {
                    return Err(malformed("chunk outside bevy"));
                }
                &data[start as usize..end as usize]
            };
            let decode_started = Instant::now();
            let decoded = if length == chunk_size {
                if direct_read {
                    direct
                } else {
                    encoded.to_vec()
                }
            } else {
                decode(encoded, compression.as_deref(), chunk_size as usize)?
            };
            self.decoded_cache
                .lock()
                .map_err(|_| malformed("AFF4 decoded cache lock poisoned"))?
                .record_decode(decoded.len(), decode_started.elapsed());
            if decoded.len() != chunk_size as usize {
                return Err(malformed("decoded chunk length mismatch"));
            }
            let begin = (position % chunk_size) as usize;
            let take = (decoded.len() - begin).min(buffer.len() - done);
            buffer[done..done + take].copy_from_slice(&decoded[begin..begin + take]);
            done += take;
            if use_decoded_cache {
                self.decoded_cache
                    .lock()
                    .map_err(|_| malformed("AFF4 decoded cache lock poisoned"))?
                    .insert(cache_key, Arc::new(decoded));
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn read_file_exact_at(file: &File, offset: u64, buffer: &mut [u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buffer, offset)
}

#[cfg(windows)]
fn read_file_exact_at(file: &File, offset: u64, mut buffer: &mut [u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut position = offset;
    while !buffer.is_empty() {
        let read = file.seek_read(buffer, position)?;
        if read == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        position = position
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        buffer = &mut buffer[read..];
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_file_exact_at(file: &File, offset: u64, buffer: &mut [u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buffer)
}

fn enter(id: &str, visited: &mut Vec<String>) -> Result<()> {
    if visited.len() >= 32 || visited.iter().any(|v| v == id) {
        return Err(malformed("cyclic or excessively deep stream references"));
    }
    visited.push(id.to_owned());
    Ok(())
}

fn is_property(predicate: &str, local: &str) -> bool {
    [NS, LOGICAL_NS, LEGACY_LOGICAL_NS]
        .iter()
        .any(|ns| predicate.strip_prefix(ns) == Some(local))
}

fn storage_name(
    archive: &ZipArchive<File>,
    volume: &str,
    id: &str,
    version: (u32, u32),
) -> Result<String> {
    if version != (1, 0) && archive.index_for_name(id).is_some() {
        return Ok(id.to_owned());
    }
    if let Some(path) = id.strip_prefix(&format!("{volume}/")) {
        return Ok(if version == (1, 0) {
            escape(path)
        } else {
            path.to_owned()
        });
    }
    let rest = id
        .strip_prefix("aff4://")
        .ok_or_else(|| Error::Unsupported("non-AFF4 storage reference".into()))?;
    // Legacy AFF4-L producers also use the physical ARN escaping for chunked
    // storage. Resolve only an existing prefix; draft 2.1 uses unescaped ARNs.
    let legacy = format!("aff4%3A%2F%2F{}", escape(rest));
    if version == (1, 1)
        && archive
            .file_names()
            .any(|n| n.starts_with(&format!("{legacy}/")))
    {
        return Ok(legacy);
    }
    Ok(if version == (1, 0) {
        format!("aff4%3A%2F%2F{}", escape(rest))
    } else {
        id.to_owned()
    })
}
fn member(archive: &mut ZipArchive<File>, name: &str, limit: u64) -> Result<Vec<u8>> {
    let file = archive.by_name(name)?;
    if file.size() > limit {
        return Err(malformed(format!("member exceeds resource limit: {name}")));
    }
    // Grow from bytes actually decoded, not an untrusted advertised size.
    read_buffer(file, limit)
}

fn allocation_error(error: std::collections::TryReserveError) -> Error {
    std::io::Error::new(std::io::ErrorKind::OutOfMemory, error).into()
}

fn read_buffer(mut reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let take = (limit.saturating_sub(data.len() as u64).saturating_add(1))
            .min(buffer.len() as u64) as usize;
        let count = match reader.read(&mut buffer[..take]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            value => value?,
        };
        if count == 0 {
            return Ok(data);
        }
        if count as u64 > limit.saturating_sub(data.len() as u64) {
            return Err(malformed("decoded member exceeds limit"));
        }
        data.try_reserve(count).map_err(allocation_error)?;
        data.extend_from_slice(&buffer[..count]);
    }
}
fn decode(data: &[u8], method: Option<&str>, size: usize) -> Result<Vec<u8>> {
    match method {
        Some("http://code.google.com/p/snappy/") => {
            if snap::raw::decompress_len(data).map_err(|e| malformed(e.to_string()))? != size {
                return Err(malformed("snappy size mismatch"));
            }
            let mut decoded = chunk_buffer(size)?;
            let count = snap::raw::Decoder::new()
                .decompress(data, &mut decoded)
                .map_err(|e| malformed(e.to_string()))?;
            decoded.truncate(count);
            Ok(decoded)
        }
        Some("https://www.ietf.org/rfc/rfc1950.txt") => {
            read_buffer(flate2::read::ZlibDecoder::new(data), size as u64)
        }
        Some("https://tools.ietf.org/html/rfc1951") => {
            read_buffer(flate2::read::DeflateDecoder::new(data), size as u64)
        }
        Some("https://code.google.com/p/lz4/") => {
            let mut decoded = chunk_buffer(size)?;
            let count = lz4_flex::block::decompress_into(data, &mut decoded)
                .map_err(|e| malformed(e.to_string()))?;
            decoded.truncate(count);
            Ok(decoded)
        }
        _ => Err(Error::Unsupported("chunk compression method".into())),
    }
}

fn chunk_buffer(size: usize) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(size).map_err(allocation_error)?;
    buffer.resize(size, 0);
    Ok(buffer)
}
pub(crate) fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}
fn escape(value: &str) -> String {
    let mut output = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            output.push(char::from(b));
        } else {
            output.push_str(&format!("%{b:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(chunk: u64) -> DecodedCacheKey {
        DecodedCacheKey::Chunk {
            volume: "aff4://volume".into(),
            resource: "aff4://volume/data".into(),
            chunk,
        }
    }

    #[test]
    fn decoded_cache_is_byte_bounded_and_lru() {
        let mut cache = DecodedChunkCache::with_capacity(8);
        cache.insert(chunk(0), Arc::new(vec![0; 4]));
        cache.insert(chunk(1), Arc::new(vec![1; 4]));
        assert!(cache.get_chunk(&chunk(0)).is_some());
        cache.insert(chunk(2), Arc::new(vec![2; 4]));

        assert!(cache.get_chunk(&chunk(0)).is_some());
        assert!(cache.get_chunk(&chunk(1)).is_none());
        assert!(cache.get_chunk(&chunk(2)).is_some());
        assert_eq!(cache.current_bytes, 8);
        assert_eq!(cache.statistics.decoded_cache_evictions, 1);
        assert_eq!(cache.info().capacity_bytes(), 8);
    }

    #[test]
    fn oversized_entry_is_not_retained() {
        let mut cache = DecodedChunkCache::with_capacity(4);
        cache.insert(chunk(0), Arc::new(vec![0; 5]));
        assert_eq!(cache.current_bytes, 0);
        assert_eq!(cache.entries.len(), 0);
    }
}
