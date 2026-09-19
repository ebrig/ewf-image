use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;

use md5::{Digest, Md5};
use oxrdf::{NamedOrBlankNode, Term};
use oxttl::TurtleParser;
use sha1::Sha1;
use sha2::Sha256;
use zip::ZipArchive;

use crate::{Error, Result, malformed};
use base64::Engine;

const NS: &str = "http://aff4.org/Schema#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const LOGICAL_NS: &str = "https://aff4.org/Schema/2022/#";
const LEGACY_LOGICAL_NS: &str = "http://aff4.org/Schema/2022/#";
const BASE64: &str = "http://www.w3.org/2001/XMLSchema#base64Binary";

/// Limits on retained metadata, map/index members, and decoded chunks.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Maximum decompressed metadata bytes.
    pub metadata_bytes: u64,
    /// Maximum bytes in an individual retained data/index member.
    pub member_bytes: u64,
    /// Maximum decoded chunk size.
    pub chunk_bytes: u64,
    /// Maximum RDF triples retained.
    pub triples: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            metadata_bytes: 16 * 1024 * 1024,
            member_bytes: 128 * 1024 * 1024,
            chunk_bytes: 16 * 1024 * 1024,
            triples: 200_000,
        }
    }
}

/// Preserved RDF property; values remain associated with their subject.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// AFF4 resource identifier.
    pub id: String,
    /// RDF classes on the resource.
    pub types: Vec<String>,
    /// Logical byte size when supplied or resolvable from its data stream.
    pub size: Option<u64>,
}

/// Selected-stream linear digest results. Unsupported references are explicit.
#[derive(Debug, Clone)]
pub struct Verification {
    /// Number of logical bytes read.
    pub bytes_verified: u64,
    /// Computed hexadecimal MD5.
    pub md5: String,
    /// Computed hexadecimal SHA1.
    pub sha1: String,
    /// Computed hexadecimal SHA256.
    pub sha256: String,
    /// Available supported linear references all match; None means none exist.
    pub references_match: Option<bool>,
    /// Datatypes of stored hashes not verified, including block-map hashes.
    pub unsupported_hashes: Vec<String>,
}

/// One read-only ZIP container. Methods serialize access through `&mut self`.
/// Open does not certify contents. Backing files must remain unchanged.
pub struct Container {
    archive: ZipArchive<File>,
    volume: String,
    graph: BTreeMap<String, Vec<Property>>,
    limits: Limits,
    cache: Option<(String, Vec<u8>)>,
    maps: BTreeMap<String, Arc<Map>>,
    version: (u32, u32),
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
        let mut archive = ZipArchive::new(File::open(path)?)?;
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
        let mut remaining = limits.metadata_bytes;
        let mut count = 0;
        while let Some(name) = pending.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let turtle = member(&mut archive, &name, remaining)?;
            remaining -= turtle.len() as u64;
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
        Ok(Self {
            archive,
            volume,
            graph,
            limits,
            cache: None,
            maps: BTreeMap::new(),
            version,
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
        let size = self.size(id)?;
        if offset >= size || buffer.is_empty() {
            return Ok(0);
        }
        let length = (size - offset).min(buffer.len() as u64) as usize;
        self.read_inner(id, &mut buffer[..length], offset, &mut Vec::new())?;
        Ok(length)
    }

    /// Computes linear hashes with progress and cancellation. Does not verify
    /// block hashes, map hashes, signatures, or metadata integrity.
    pub fn verify(
        &mut self,
        id: &str,
        mut progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<Verification> {
        let size = self.size(id)?;
        self.cache = None;
        self.maps.clear();
        self.read_inner(id, &mut [], 0, &mut Vec::new())?;
        let mut offset = 0;
        let mut buffer = vec![0; 1024 * 1024];
        let mut md5 = Md5::new();
        let mut sha1 = Sha1::new();
        let mut sha256 = Sha256::new();
        loop {
            if progress(offset, size).is_break() {
                return Err(Error::Aborted);
            }
            if offset == size {
                break;
            }
            let read = self.read_at(id, &mut buffer, offset)?;
            if read == 0 {
                return Err(malformed("truncated stream"));
            }
            md5.update(&buffer[..read]);
            sha1.update(&buffer[..read]);
            sha256.update(&buffer[..read]);
            offset += read as u64;
        }
        let mut result = Verification {
            bytes_verified: offset,
            md5: hex(&md5.finalize()),
            sha1: hex(&sha1.finalize()),
            sha256: hex(&sha256.finalize()),
            references_match: None,
            unsupported_hashes: Vec::new(),
        };
        for reference in self
            .properties(id)
            .filter(|p| is_property(&p.predicate, "hash"))
        {
            let datatype = reference.datatype.as_deref().unwrap_or("");
            let expected = match datatype.strip_prefix(NS) {
                Some("MD5") => &result.md5,
                Some("SHA1") => &result.sha1,
                Some("SHA256") => &result.sha256,
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
        let mut ranges = Vec::new();
        let mut previous = 0;
        let size = self.number(id, "size")?;
        for record in data.chunks_exact(28) {
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
            ranges.push(Range {
                start,
                end,
                offset,
                target: targets
                    .get(target)
                    .ok_or_else(|| malformed("missing map target"))?
                    .to_string(),
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
    fn read_inner(
        &mut self,
        id: &str,
        buffer: &mut [u8],
        offset: u64,
        visited: &mut Vec<String>,
    ) -> Result<()> {
        enter(id, visited)?;
        let result = self.read_inner_impl(id, buffer, offset, visited);
        visited.pop();
        result
    }
    fn read_inner_impl(
        &mut self,
        id: &str,
        buffer: &mut [u8],
        offset: u64,
        visited: &mut Vec<String>,
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
            return self.read_inner(&target, buffer, offset, visited);
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
            let name = format!("{path}/{:08}", chunk / per_bevy);
            let index = member(
                &mut self.archive,
                &format!("{name}.index"),
                self.limits.member_bytes,
            )?;
            if index.len() % 12 != 0 {
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
            if self.cache.as_ref().is_none_or(|(key, _)| key != &name) {
                self.cache = Some((
                    name.clone(),
                    member(&mut self.archive, &name, self.limits.member_bytes)?,
                ));
            }
            let data = &self.cache.as_ref().unwrap().1;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= data.len() as u64)
                .ok_or_else(|| malformed("chunk outside bevy"))?;
            let encoded = &data[start as usize..end as usize];
            let decoded = if length == chunk_size {
                encoded.to_vec()
            } else {
                decode(encoded, compression.as_deref(), chunk_size as usize)?
            };
            if decoded.len() != chunk_size as usize {
                return Err(malformed("decoded chunk length mismatch"));
            }
            let begin = (position % chunk_size) as usize;
            let take = (decoded.len() - begin).min(buffer.len() - done);
            buffer[done..done + take].copy_from_slice(&decoded[begin..begin + take]);
            done += take;
        }
        Ok(())
    }
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
    let mut data = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(malformed("decoded member exceeds limit"));
    }
    Ok(data)
}
fn decode(data: &[u8], method: Option<&str>, size: usize) -> Result<Vec<u8>> {
    match method {
        Some("http://code.google.com/p/snappy/") => {
            if snap::raw::decompress_len(data).map_err(|e| malformed(e.to_string()))? != size {
                return Err(malformed("snappy size mismatch"));
            }
            snap::raw::Decoder::new()
                .decompress_vec(data)
                .map_err(|e| malformed(e.to_string()))
        }
        Some("https://www.ietf.org/rfc/rfc1950.txt") => {
            let mut decoded = Vec::new();
            flate2::read::ZlibDecoder::new(data)
                .take(size as u64 + 1)
                .read_to_end(&mut decoded)?;
            Ok(decoded)
        }
        Some("https://tools.ietf.org/html/rfc1951") => {
            let mut decoded = Vec::new();
            flate2::read::DeflateDecoder::new(data)
                .take(size as u64 + 1)
                .read_to_end(&mut decoded)?;
            Ok(decoded)
        }
        Some("https://code.google.com/p/lz4/") => {
            lz4_flex::block::decompress(data, size).map_err(|e| malformed(e.to_string()))
        }
        _ => Err(Error::Unsupported("chunk compression method".into())),
    }
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
