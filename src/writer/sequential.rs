//! Append-only EWF2 encoding with bounded payload scratch.

use super::{
    ChunkDescriptor, ChunkSpool, EWF2_DONE_SECTION, EWF2_NEXT_SECTION, Ewf2SegmentWriteContext,
    WriteFormat, WriteHashState, WriteOptions, WriteResult, effective_write_hashes, encode_chunk,
    ewf2_segment_path, is_ewf2_format, normalize_maximum_segment_size, normalize_media_size,
    publication_segment_paths, validate_options, validate_secondary_segment_filename,
    validate_session_ranges, write_ewf2_segment, writer_chunk_geometry,
};
use crate::publication::Publication;
use crate::{EwfError, Result};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

/// Settings for a known-length, append-only EWF2 acquisition.
#[derive(Debug, Clone)]
pub struct SequentialOptions {
    /// Exact number of source bytes, excluding final sector padding.
    pub source_size: u64,
    /// Chunks per segment, independent of compressed byte size.
    /// The uncompressed segment capacity must not exceed 512 MiB.
    pub chunks_per_segment: u32,
    /// EWF2 physical or logical settings. `maximum_segment_size` is unsupported.
    pub write: WriteOptions,
}

impl SequentialOptions {
    /// Creates physical EWF2 settings with 16,375 chunks per segment.
    pub fn new(source_size: u64) -> Self {
        Self {
            source_size,
            chunks_per_segment: 16_375,
            write: WriteOptions {
                format: WriteFormat::Ewf2Physical,
                ..WriteOptions::default()
            },
        }
    }
}

/// Streams EWF2 payload into staged native segments without a full raw spool.
///
/// Payload scratch holds one encoded segment, plus one chunk in memory.
/// Logical catalogs are written in the final segment.
/// Catalogs and output path lists still grow with entry and segment counts.
/// Input must match the declared size exactly. Final sector padding is zeroed
/// and included in image digests. Errors poison the writer. Dropping it removes
/// uncommitted staging; after process interruption use `EwfWriter::recover_output`.
/// This writer does not support seeking or checkpoint resume.
pub struct SequentialWriter {
    pub(super) options: WriteOptions,
    path: PathBuf,
    source_size: u64,
    position: u64,
    logical_size: u64,
    chunk_size: u64,
    capacity: usize,
    per_segment: usize,
    total_chunks: u32,
    encoded_chunks: u64,
    pending: Vec<u8>,
    current: Group,
    hashes: WriteHashState,
    paths: Vec<PathBuf>,
    mirrors: Vec<PathBuf>,
    poisoned: bool,
    // Drop spools before the publication journal, including on Windows.
    publication: Publication,
}

struct Group {
    spool: ChunkSpool,
    chunks: Vec<ChunkDescriptor>,
}

impl Group {
    fn new(directory: &Path) -> Result<Self> {
        Ok(Self {
            spool: ChunkSpool {
                file: NamedTempFile::new_in(directory)?,
                len: 0,
            },
            chunks: Vec::new(),
        })
    }
}

impl SequentialWriter {
    /// Starts a publication transaction. Nothing is published until `finish`.
    pub fn create(path: impl AsRef<Path>, settings: SequentialOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let options = settings.write;
        validate_options(&options)?;
        if !is_ewf2_format(options.format)
            || normalize_maximum_segment_size(options.maximum_segment_size).is_some()
        {
            return Err(EwfError::Unsupported(
                "sequential writing requires EWF2 and chunk-count segmentation".into(),
            ));
        }
        let (chunk_size, capacity) =
            writer_chunk_geometry(options.sectors_per_chunk, options.bytes_per_sector)?;
        if chunk_size > 16 * 1024 * 1024
            || settings.chunks_per_segment == 0
            || chunk_size * u64::from(settings.chunks_per_segment) > 512 * 1024 * 1024
        {
            return Err(EwfError::Unsupported(
                "sequential writing requires chunks <=16 MiB and segment capacity <=512 MiB".into(),
            ));
        }
        let sector = u64::from(options.bytes_per_sector);
        let logical_size = settings
            .source_size
            .checked_add(sector - 1)
            .map(|size| size / sector * sector)
            .ok_or_else(|| EwfError::Malformed("sequential source size overflow".into()))?;
        if normalize_media_size(options.media_size).is_some_and(|size| size != logical_size) {
            return Err(EwfError::Unsupported(
                "media_size differs from padded source size".into(),
            ));
        }
        let total_chunks = u32::try_from(logical_size.div_ceil(chunk_size))
            .map_err(|_| EwfError::Unsupported("sequential chunk count exceeds u32".into()))?;
        validate_session_ranges("sessions", &options.sessions, logical_size / sector)?;
        validate_session_ranges("tracks", &options.tracks, logical_size / sector)?;
        validate_secondary_segment_filename(&path, options.secondary_segment_filename.as_deref())?;
        if let Some(secondary) = &options.secondary_segment_filename {
            validate_namespaces(
                &path,
                secondary,
                (total_chunks as usize)
                    .div_ceil(settings.chunks_per_segment as usize)
                    .max(1),
            )?;
        }
        let publication = Publication::begin(
            &path,
            options.secondary_segment_filename.as_deref(),
            options.overwrite_existing,
        )?;
        let staged = publication.stage(0, &path)?;
        let current = Group::new(staged.parent().expect("staged parent"))?;
        Ok(Self {
            options,
            path,
            source_size: settings.source_size,
            position: 0,
            logical_size,
            chunk_size,
            capacity,
            per_segment: settings.chunks_per_segment as usize,
            total_chunks,
            encoded_chunks: 0,
            pending: Vec::with_capacity(capacity),
            current,
            hashes: WriteHashState::new(),
            paths: Vec::new(),
            mirrors: Vec::new(),
            poisoned: false,
            publication,
        })
    }

    /// Source bytes accepted so far; excludes sector padding.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Appends bytes. An oversize write poisons the transaction without publishing.
    pub fn write_all(&mut self, mut bytes: &[u8]) -> Result<()> {
        self.healthy()?;
        self.poisoned = true;
        if bytes.len() as u64 > self.source_size - self.position {
            return Err(EwfError::Malformed(
                "sequential input exceeds declared source size".into(),
            ));
        }
        while !bytes.is_empty() {
            let take = bytes.len().min(self.capacity - self.pending.len());
            self.pending.extend_from_slice(&bytes[..take]);
            self.position += take as u64;
            bytes = &bytes[take..];
            if self.pending.len() == self.capacity {
                self.encode_pending()?;
            }
        }
        self.poisoned = false;
        Ok(())
    }

    fn healthy(&self) -> Result<()> {
        if self.poisoned {
            return Err(EwfError::Aborted);
        }
        Ok(())
    }

    fn encode_pending(&mut self) -> Result<()> {
        let chunk = std::mem::replace(&mut self.pending, Vec::with_capacity(self.capacity));
        self.hashes.update(&chunk);
        let encoded = encode_chunk(
            chunk,
            self.options.compression,
            self.options.compression_values,
            self.chunk_size,
            true,
            false,
        )?;
        self.current
            .chunks
            .push(self.current.spool.append(encoded)?);
        self.encoded_chunks += 1;
        if self.current.chunks.len() == self.per_segment
            && self.encoded_chunks < u64::from(self.total_chunks)
        {
            let staged = self.publication.stage(0, &self.path)?;
            let next = Group::new(staged.parent().expect("staged parent"))?;
            let mut complete = std::mem::replace(&mut self.current, next);
            let number = self.paths.len() + 1;
            self.stage_group(&mut complete, number, false)?;
            self.paths.push(ewf2_segment_path(&self.path, number)?);
        }
        Ok(())
    }

    fn stage_group(&self, group: &mut Group, number: usize, last: bool) -> Result<()> {
        let path = ewf2_segment_path(&self.path, number)?;
        let staged = self.publication.stage(0, &path)?;
        let mut file = File::create(&staged)?;
        write_ewf2_segment(
            &mut file,
            &mut group.spool,
            &group.chunks,
            &self.options,
            Ewf2SegmentWriteContext {
                segment_number: u32::try_from(number)
                    .map_err(|_| EwfError::Unsupported("too many segments".into()))?,
                first_chunk: (number as u64 - 1) * self.per_segment as u64,
                total_chunk_count: self.total_chunks,
                sector_count: self.logical_size / u64::from(self.options.bytes_per_sector),
                terminal_section_type: if last {
                    EWF2_DONE_SECTION
                } else {
                    EWF2_NEXT_SECTION
                },
            },
        )?;
        file.sync_all()?;
        drop(file);
        if let Some(base) = &self.options.secondary_segment_filename {
            let mirror = ewf2_segment_path(base, number)?;
            let destination = self.publication.stage(1, &mirror)?;
            fs::copy(staged, &destination)?;
            fs::OpenOptions::new()
                .write(true)
                .open(destination)?
                .sync_all()?;
        }
        Ok(())
    }

    /// Checks the source length, pads the last sector, and publishes all segments.
    pub fn finish(mut self) -> Result<WriteResult> {
        self.healthy()?;
        if self.position != self.source_size {
            return Err(EwfError::Malformed(
                "sequential input is shorter than declared source size".into(),
            ));
        }
        let padding = (self.logical_size - self.source_size) as usize;
        self.pending.resize(self.pending.len() + padding, 0);
        if !self.pending.is_empty() {
            self.encode_pending()?;
        }
        let (md5, sha1, sha256) = self.hashes.clone().finalize();
        self.options.hashes = effective_write_hashes(&self.options.hashes, md5, sha1, sha256)?;
        let staged = self.publication.stage(0, &self.path)?;
        let empty = Group::new(staged.parent().expect("staged parent"))?;
        let mut last = std::mem::replace(&mut self.current, empty);
        let number = self.paths.len() + 1;
        self.stage_group(&mut last, number, true)?;
        self.paths.push(ewf2_segment_path(&self.path, number)?);
        // Remove scratch handles before publication cleanup on Windows.
        drop(last);
        self.current.spool.file.close()?;
        if let Some(base) = &self.options.secondary_segment_filename {
            for number in 1..=self.paths.len() {
                self.mirrors.push(ewf2_segment_path(base, number)?);
            }
        }
        let mut destinations = vec![publication_segment_paths(&self.paths, true)?];
        if !self.mirrors.is_empty() {
            destinations.push(publication_segment_paths(&self.mirrors, true)?);
        }
        self.publication.publish(&destinations, self.paths.len())?;
        Ok(WriteResult {
            segment_paths: self.paths,
            secondary_segment_paths: self.mirrors,
            logical_size: self.logical_size,
            chunk_size: self.chunk_size,
            chunk_count: u64::from(self.total_chunks),
            computed_sha256: sha256,
        })
    }
}

// Validate the complete namespace once, before creating a journal or accepting
// source bytes. Resolve parent aliases and use conservative Windows case folding.
// O(n log n) comparisons replace the former per-segment O(n^2) filesystem work.
fn validate_namespaces(primary: &Path, secondary: &Path, count: usize) -> Result<()> {
    fn key(path: &Path) -> Result<PathBuf> {
        let path = super::normalized_output_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| EwfError::Malformed("missing output parent".into()))?;
        let key = parent.canonicalize()?.join(
            path.file_name()
                .ok_or_else(|| EwfError::Malformed("missing output name".into()))?,
        );
        #[cfg(windows)]
        let key = PathBuf::from(key.to_string_lossy().to_lowercase());
        Ok(key)
    }
    let primary: BTreeSet<_> = (1..=count)
        .map(|n| key(&ewf2_segment_path(primary, n)?))
        .collect::<Result<_>>()?;
    for number in 1..=count {
        if primary.contains(&key(&ewf2_segment_path(secondary, number)?)?) {
            return Err(EwfError::Unsupported(
                "secondary segment filename overlaps primary output".into(),
            ));
        }
    }
    Ok(())
}

impl Write for SequentialWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Self::write_all(self, bytes).map_err(std::io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.healthy().map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::{SequentialOptions, SequentialWriter, WriteFormat};
    #[test]
    fn scratch_and_descriptors_do_not_grow_with_completed_segments() {
        for format in [WriteFormat::Ewf2Physical, WriteFormat::Ewf2Logical] {
            let dir = tempfile::tempdir().unwrap();
            let sectors_per_chunk = if format == WriteFormat::Ewf2Logical {
                16
            } else {
                1
            };
            let chunk_size = sectors_per_chunk * 512;
            let mut options = SequentialOptions::new(u64::from(chunk_size) * 100);
            options.write.format = format;
            options.write.sectors_per_chunk = sectors_per_chunk;
            options.chunks_per_segment = 3;
            let mut writer = SequentialWriter::create(
                dir.path().join(if format == WriteFormat::Ewf2Physical {
                    "case.Ex01"
                } else {
                    "case.Lx01"
                }),
                options,
            )
            .unwrap();
            let chunk: Vec<u8> = (0..chunk_size).map(|n| (n % 251) as u8).collect();
            for _ in 0..100 {
                writer.write_all(&chunk).unwrap();
                assert!(writer.current.chunks.len() <= 3);
                assert!(writer.current.spool.len <= u64::from(3 * (chunk_size + 4)));
                assert!(writer.pending.len() < chunk_size as usize);
            }
            writer.finish().unwrap();
        }
    }
}
