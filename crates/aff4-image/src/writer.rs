use std::collections::BTreeMap;
use std::fs::{self, File};
#[path = "collect.rs"]
mod collect;
#[path = "collection_budget.rs"]
mod collection_budget;
pub use collection_budget::CollectionLimits;
#[path = "logical_metadata.rs"]
mod logical_metadata;
pub use collect::{CollectionIssue, CollectionOptions, CollectionReport};
pub use logical_metadata::{CaseMetadata, LogicalMetadata, SubstreamKind};
use std::io::{Read, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use tempfile::NamedTempFile;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{Error, Result, malformed, reader::hex};

/// Container profile chosen before any evidence bytes are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Profile {
    /// AFF4 Standard 1.0 physical images with ImageStream storage.
    Physical,
    /// Legacy AFF4-L 1.1 logical files with ZIP segment storage.
    Logical,
}

/// Physical chunk codec. Logical ZIP segments use Stored or Deflate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Compression {
    /// Uncompressed bytes.
    Stored,
    /// Zlib chunk compression / ZIP Deflate.
    Zlib,
    /// Raw Snappy chunks (physical profile only).
    Snappy,
    /// Raw LZ4 blocks (physical profile only).
    Lz4,
}

/// Bounded writer geometry. Output is always a new single ZIP volume.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WriteOptions {
    /// Physical chunk bytes (1 through 16 MiB).
    pub chunk_bytes: u32,
    /// Physical chunks per bevy. A bevy cannot exceed 128 MiB uncompressed.
    pub chunks_per_bevy: u32,
    /// Physical codec or logical ZIP codec.
    pub compression: Compression,
}
impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            chunk_bytes: 32768,
            chunks_per_bevy: 1024,
            compression: Compression::Zlib,
        }
    }
}

/// Digests of source bytes accepted for one image or file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AcquiredStream {
    /// Image or FileImage resource identifier.
    pub id: String,
    /// Exact byte count, excluding final chunk padding.
    pub size: u64,
    /// Hexadecimal MD5.
    pub md5: String,
    /// Hexadecimal SHA1.
    pub sha1: String,
    /// Hexadecimal SHA256.
    pub sha256: String,
}

/// Published acquisition result. Caller may reopen to verify independently.
#[derive(Debug, serde::Serialize)]
pub struct WriteResult {
    /// Final, exclusively created path.
    pub path: PathBuf,
    /// Per-resource source hashes.
    pub streams: Vec<AcquiredStream>,
    /// SHA256 of the exact information.turtle bytes; record externally.
    pub metadata_sha256: String,
}

/// Streaming writer with exclusive publication and no full-source spool.
///
/// A failed/cancelled input poisons the writer; finish then refuses publication.
/// Dropping the writer removes its temporary file. Crash leftovers may remain.
/// Inputs must be stable snapshots; device opening, retries, splitting, resume,
/// encryption, and metadata/ACL collection are outside this API.
pub struct Writer {
    zip: Option<ZipWriter<File>>,
    temporary: NamedTempFile,
    path: PathBuf,
    profile: Profile,
    volume: String,
    options: WriteOptions,
    metadata: String,
    archive_entries: u64,
    streams: Vec<AcquiredStream>,
    poisoned: bool,
    logical_zip_threshold: u64,
    folders: BTreeMap<String, (String, Vec<String>)>,
    roots: Vec<String>,
    path_separator: char,
}

impl Writer {
    /// Creates private staging beside a new destination. Existing files,
    /// including symlinks, are never overwritten.
    pub fn create(path: impl AsRef<Path>, profile: Profile, options: WriteOptions) -> Result<Self> {
        if options.chunk_bytes == 0
            || options.chunk_bytes > 16 * 1024 * 1024
            || options.chunks_per_bevy == 0
            || u64::from(options.chunk_bytes) * u64::from(options.chunks_per_bevy)
                > 128 * 1024 * 1024
        {
            return Err(malformed("invalid writer chunk geometry"));
        }
        if profile == Profile::Logical
            && !matches!(options.compression, Compression::Stored | Compression::Zlib)
        {
            return Err(Error::Unsupported(
                "logical ZIP compression must be Stored or Zlib".into(),
            ));
        }
        let input_path = path.as_ref();
        let name = input_path
            .file_name()
            .ok_or_else(|| malformed("missing output filename"))?;
        #[cfg(windows)]
        if name.to_string_lossy().contains(':') {
            return Err(malformed("alternate-stream output is unsupported"));
        }
        let parent = fs::canonicalize(
            input_path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )?;
        let path = parent.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => return Err(malformed("output already exists")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let temporary = tempfile::Builder::new()
            .prefix(".aff4-acquire-")
            .tempfile_in(&parent)?;
        let mut zip = ZipWriter::new(temporary.as_file().try_clone()?);
        let volume = identifier();
        zip.set_comment(volume.as_str())?;
        write_member(&mut zip, "container.description", volume.as_bytes())?;
        write_member(
            &mut zip,
            "version.txt",
            match profile {
                Profile::Physical => b"major=1\nminor=0\ntool=aff4-image 0.1.0\n",
                Profile::Logical => b"major=1\nminor=1\ntool=aff4-image 0.1.0\n",
            },
        )?;
        let metadata = format!(
            "@prefix a: <http://aff4.org/Schema#> .\n@prefix l: <https://aff4.org/Schema/2022/#> .\n<{volume}> a a:ZipVolume .\n"
        );
        Ok(Self {
            zip: Some(zip),
            temporary,
            path,
            profile,
            volume,
            options,
            metadata,
            archive_entries: 2,
            streams: Vec::new(),
            poisoned: false,
            logical_zip_threshold: 1024 * 1024,
            folders: BTreeMap::new(),
            roots: Vec::new(),
            path_separator: '/',
        })
    }

    /// Acquires exactly `size` bytes as a physical ImageStream, preserving
    /// trailing input bytes for the caller. Progress is before/after chunks.
    pub fn add_image(
        &mut self,
        size: u64,
        input: &mut impl Read,
        progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<String> {
        self.healthy()?;
        if self.profile != Profile::Physical {
            return Err(Error::Unsupported(
                "physical image in a logical writer".into(),
            ));
        }
        self.add_chunked(size, input, progress, None, "")
    }

    /// Sets the largest logical file stored as one ZIP segment (at most 1 GiB).
    /// Larger files use chunked ImageStreams with Maps and block integrity hashes.
    pub fn set_logical_zip_threshold(&mut self, bytes: u64) -> Result<()> {
        self.healthy()?;
        if bytes > 1024 * 1024 * 1024 {
            return Err(malformed("ZIP threshold exceeds 1 GiB"));
        }
        self.logical_zip_threshold = bytes;
        Ok(())
    }

    fn add_chunked(
        &mut self,
        size: u64,
        input: &mut impl Read,
        mut progress: impl FnMut(u64, u64) -> ControlFlow<()>,
        logical_path: Option<&str>,
        extra_properties: &str,
    ) -> Result<String> {
        self.poisoned = true;
        let entries = size
            .div_ceil(u64::from(self.options.chunk_bytes))
            .div_ceil(u64::from(self.options.chunks_per_bevy))
            .checked_mul(4)
            .and_then(|n| n.checked_add(if logical_path.is_some() { 0 } else { 2 }))
            .ok_or_else(|| malformed("archive entry count overflow"))?;
        self.archive_entries = self
            .archive_entries
            .checked_add(entries)
            .ok_or_else(|| malformed("archive entry count overflow"))?;
        let id = if logical_path.is_some() {
            format!("{}/files/{}", self.volume, uuid::Uuid::new_v4())
        } else {
            identifier()
        };
        let stream = if logical_path.is_some() {
            id.clone()
        } else {
            identifier()
        };
        let map = identifier();
        let storage = if logical_path.is_some() {
            id.strip_prefix(&format!("{}/", self.volume))
                .unwrap()
                .to_owned()
        } else {
            format!("aff4%3A%2F%2F{}", stream.strip_prefix("aff4://").unwrap())
        };
        let mut hashes = Hashes::new();
        let mut done = 0;
        let mut chunk = 0u64;
        let mut buffer = vec![0; self.options.chunk_bytes as usize];
        let mut index = Vec::new();
        let mut md5_blocks = Vec::new();
        let mut sha256_blocks = Vec::new();
        let mut md5_tree = Sha512::new();
        let mut sha256_tree = Sha512::new();
        let mut index_tree = Sha512::new();
        let mut bevy_bytes = 0u64;
        let zip = self.zip.as_mut().unwrap();
        loop {
            if progress(done, size).is_break() {
                return Err(Error::Aborted);
            }
            if done == size {
                break;
            }
            let bevy = chunk / u64::from(self.options.chunks_per_bevy);
            if chunk.is_multiple_of(u64::from(self.options.chunks_per_bevy)) {
                zip.start_file(format!("{storage}/{bevy:08}"), stored())?;
                bevy_bytes = 0;
                index.clear();
                md5_blocks.clear();
                sha256_blocks.clear();
            }
            let length = (size - done).min(buffer.len() as u64) as usize;
            buffer.fill(0);
            input.read_exact(&mut buffer[..length])?;
            hashes.update(&buffer[..length]);
            md5_blocks.extend_from_slice(&Md5::digest(&buffer[..length]));
            sha256_blocks.extend_from_slice(&Sha256::digest(&buffer[..length]));
            let encoded = encode(&buffer, self.options.compression)?;
            let encoded = if encoded.len() < buffer.len().saturating_sub(16) {
                &encoded
            } else {
                &buffer
            };
            zip.write_all(encoded)?;
            index.extend_from_slice(&bevy_bytes.to_le_bytes());
            index.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            bevy_bytes += encoded.len() as u64;
            done += length as u64;
            chunk += 1;
            if chunk.is_multiple_of(u64::from(self.options.chunks_per_bevy)) || done == size {
                write_member(zip, &format!("{storage}/{bevy:08}.index"), &index)?;
                write_member(
                    zip,
                    &format!("{storage}/{bevy:08}.blockHash.md5"),
                    &md5_blocks,
                )?;
                write_member(
                    zip,
                    &format!("{storage}/{bevy:08}.blockHash.sha256"),
                    &sha256_blocks,
                )?;
                md5_tree.update(&md5_blocks);
                sha256_tree.update(&sha256_blocks);
                index_tree.update(&index);
            }
        }
        let result = hashes.finish(id.clone(), size);
        let map_storage = format!("aff4%3A%2F%2F{}", map.strip_prefix("aff4://").unwrap());
        let mut range = Vec::new();
        if size != 0 {
            range.extend_from_slice(&0u64.to_le_bytes());
            range.extend_from_slice(&size.to_le_bytes());
            range.extend_from_slice(&0u64.to_le_bytes());
            range.extend_from_slice(&0u32.to_le_bytes());
        }
        if logical_path.is_none() {
            write_member(zip, &format!("{map_storage}/map"), &range)?;
            write_member(
                zip,
                &format!("{map_storage}/idx"),
                format!("{stream}\n").as_bytes(),
            )?;
        }
        let index_hash = hex(&index_tree.finalize());
        let md5_tree = md5_tree.finalize();
        let sha256_tree = sha256_tree.finalize();
        let points_hash = Sha512::digest(&range);
        let target_index = format!("{stream}\n");
        let target_hash = Sha512::digest(target_index.as_bytes());
        let empty_hash = Sha512::digest([]);
        let mut composite = Sha512::new();
        if size != 0 {
            composite.update(md5_tree);
            composite.update(sha256_tree);
        }
        composite.update(points_hash);
        composite.update(target_hash);
        composite.update(empty_hash);
        let composite = hex(&composite.finalize());
        let mut map_hash = Sha512::new();
        map_hash.update(&range);
        map_hash.update(target_index.as_bytes());
        let map_hash = hex(&map_hash.finalize());
        let compression: String = match self.options.compression {
            Compression::Stored => {
                "a:compressionMethod <http://aff4.org/Schema#compression/stored>;".into()
            }
            Compression::Zlib => {
                "a:compressionMethod <https://www.ietf.org/rfc/rfc1950.txt>;".into()
            }
            Compression::Snappy => "a:compressionMethod <http://code.google.com/p/snappy/>;".into(),
            Compression::Lz4 => "a:compressionMethod <https://code.google.com/p/lz4/>;".into(),
        };
        let kind = if logical_path.is_some() {
            "FileImage"
        } else {
            "DiskImage"
        };
        let logical_properties = logical_path
            .map(|path| {
                let path_literal = oxrdf::Literal::new_simple_literal(path).to_string();
                let name = oxrdf::Literal::new_simple_literal(
                    path.rsplit(['/', '\\']).next().unwrap_or(path),
                )
                .to_string();
                format!("a:originalFileName {path_literal}; a:fileName {name};")
            })
            .unwrap_or_default();
        let volume = &self.volume;
        if logical_path.is_some() {
            self.metadata.push_str(&format!("<{id}> a a:FileImage, a:Image, a:ImageStream; {extra_properties} {logical_properties} a:size {size}; a:chunkSize {}; a:chunksInSegment {}; {compression} a:stored <{volume}>; a:imageStreamIndexHash \"{index_hash}\"^^a:SHA512; {} .\n", self.options.chunk_bytes, self.options.chunks_per_bevy, hash_triples(&result)));
        } else {
            self.metadata.push_str(&format!("<{stream}> a a:ImageStream; a:size {size}; a:chunkSize {}; a:chunksInSegment {}; {compression} a:target <{map}>; a:stored <{volume}>; a:imageStreamIndexHash \"{index_hash}\"^^a:SHA512 .\n<{id}> a a:Image, a:ContiguousImage, a:{kind}; {logical_properties} a:size {size}; a:dataStream <{map}>; a:stored <{volume}>; {}; a:hash \"{composite}\"^^a:blockMapHashSHA512 .\n<{map}> a a:Map; a:size {size}; a:dependentStream <{stream}>; a:target <{id}>; a:stored <{volume}>; a:blockMapHash \"{composite}\"^^a:SHA512; a:mapPointHash \"{}\"^^a:SHA512; a:mapIdxHash \"{}\"^^a:SHA512; a:mapPathHash \"{}\"^^a:SHA512; a:mapHash \"{map_hash}\"^^a:SHA512 .\n", self.options.chunk_bytes, self.options.chunks_per_bevy, hash_triples(&result), hex(&points_hash), hex(&target_hash), hex(&empty_hash)));
        }
        if size != 0 {
            self.metadata.push_str(&format!("<{stream}/blockhash.md5> a a:BlockHashes; a:hash \"{}\"^^a:SHA512 .\n<{stream}/blockhash.sha256> a a:BlockHashes; a:hash \"{}\"^^a:SHA512 .\n", hex(&md5_tree), hex(&sha256_tree)));
        }
        self.streams.push(result);
        self.poisoned = false;
        Ok(id)
    }

    /// Acquires one logical file as a ZIP segment with recorded original path.
    /// Names stay in metadata; ZIP storage uses a generated identifier.
    /// Files above the configured threshold use ImageStreams with block hashes.
    pub fn add_file(
        &mut self,
        original_path: &str,
        size: u64,
        input: &mut impl Read,
        progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<String> {
        self.add_file_impl(original_path, size, input, progress, "")
    }

    fn add_file_impl(
        &mut self,
        original_path: &str,
        size: u64,
        input: &mut impl Read,
        mut progress: impl FnMut(u64, u64) -> ControlFlow<()>,
        extra_properties: &str,
    ) -> Result<String> {
        self.healthy()?;
        if self.profile != Profile::Logical {
            return Err(Error::Unsupported(
                "logical file in a physical writer".into(),
            ));
        }
        if original_path.is_empty() || original_path.chars().any(char::is_control) {
            return Err(malformed(
                "logical path must be nonempty UTF-8 without control characters",
            ));
        }
        if size > self.logical_zip_threshold {
            return self.add_chunked(size, input, progress, Some(original_path), extra_properties);
        }
        self.poisoned = true;
        let storage = format!("files/{}", uuid::Uuid::new_v4());
        let id = format!("{}/{storage}", self.volume);
        let zip = self.zip.as_mut().unwrap();
        let method = if self.options.compression == Compression::Stored {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        self.archive_entries = self
            .archive_entries
            .checked_add(1)
            .ok_or_else(|| malformed("archive entry count overflow"))?;
        zip.start_file(&storage, stored().compression_method(method))?;
        let mut hashes = Hashes::new();
        let mut buffer = vec![0; 1024 * 1024];
        let mut done = 0;
        loop {
            if progress(done, size).is_break() {
                return Err(Error::Aborted);
            }
            if done == size {
                break;
            }
            let length = (size - done).min(buffer.len() as u64) as usize;
            input.read_exact(&mut buffer[..length])?;
            zip.write_all(&buffer[..length])?;
            hashes.update(&buffer[..length]);
            done += length as u64;
        }
        let result = hashes.finish(id.clone(), size);
        let name = original_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(original_path);
        let name = oxrdf::Literal::new_simple_literal(name).to_string();
        let path = oxrdf::Literal::new_simple_literal(original_path).to_string();
        self.metadata.push_str(&format!("<{id}> a a:FileImage, a:Image, a:zip_segment; {extra_properties} a:size {size}; a:fileName {name}; a:originalFileName {path}; a:stored <{}>; {} .\n", self.volume, hash_triples(&result)));
        self.streams.push(result);
        self.poisoned = false;
        Ok(id)
    }

    /// Finalizes ZIP metadata, synchronizes bytes, and publishes exclusively.
    /// Unix additionally synchronizes the parent directory. This API does not
    /// certify power-loss durability on every filesystem or device.
    /// `Error::PublishedButUnsynced` retains the result if that last sync fails.
    pub fn finish(self) -> Result<WriteResult> {
        self.finish_checked(|_, _| Ok(()))
            .map(|(result, ())| result)
    }

    /// Verifies the finalized temporary container under explicit reader budgets
    /// before exclusive publication. Limit, integrity, or cancellation failures
    /// remove the temporary file and leave the destination absent.
    /// `PublishedButUnsynced` means verification passed but directory sync failed.
    pub fn finish_verified(
        self,
        limits: crate::Limits,
        mut progress: impl FnMut(&str, u64, u64) -> ControlFlow<()>,
    ) -> Result<(WriteResult, crate::ContainerVerification)> {
        self.finish_checked(|path, digest| {
            if progress("staged verification", 0, 0).is_break() {
                return Err(Error::Aborted);
            }
            let verification = crate::Container::open_with_limits(path, limits)?
                .verify_all(Some(digest), &mut progress)?;
            if !verification.all_match() {
                return Err(malformed("staged container verification failed"));
            }
            if progress("before publication", 0, 0).is_break() {
                return Err(Error::Aborted);
            }
            Ok(verification)
        })
    }

    fn finish_checked<T>(
        mut self,
        check: impl FnOnce(&Path, &str) -> Result<T>,
    ) -> Result<(WriteResult, T)> {
        self.healthy()?;
        self.finish_logical_metadata();
        let mut zip = self.zip.take().unwrap();
        write_member(&mut zip, "information.turtle", self.metadata.as_bytes())?;
        let metadata_sha256 = hex(&Sha256::digest(self.metadata.as_bytes()));
        let hashes = format!(
            "@prefix a: <http://aff4.org/Schema#> .\n<{}/information.turtle> a:hash \"{}\"^^a:SHA256 .\n",
            self.volume, metadata_sha256
        );
        write_member(&mut zip, "information.turtle.hashes", hashes.as_bytes())?;
        #[cfg(test)]
        failure_tests::boundary("metadata");
        let file = zip.finish()?;
        #[cfg(test)]
        failure_tests::boundary("zip_closed");
        file.sync_all()?;
        drop(file);
        // The finalized ZIP now owns these bytes. Release the write-side graph
        // before staged verification allocates its bounded read-side graph.
        self.metadata = String::new();
        self.folders.clear();
        self.roots = Vec::new();
        #[cfg(test)]
        failure_tests::boundary("file_synced");
        let checked = check(self.temporary.path(), &metadata_sha256)?;
        self.temporary
            .persist_noclobber(&self.path)
            .map_err(|error| Error::Io(error.error))?;
        let result = WriteResult {
            path: self.path,
            streams: self.streams,
            metadata_sha256,
        };
        #[cfg(test)]
        failure_tests::boundary("published");
        sync_published_directory(&result.path).map_err(|source| Error::PublishedButUnsynced {
            result: Box::new(WriteResult {
                path: result.path.clone(),
                streams: result.streams.clone(),
                metadata_sha256: result.metadata_sha256.clone(),
            }),
            source,
        })?;
        #[cfg(test)]
        failure_tests::boundary("directory_synced");
        Ok((result, checked))
    }

    fn healthy(&self) -> Result<()> {
        if self.poisoned {
            return Err(Error::Unsupported(
                "acquisition failed; discard this writer".into(),
            ));
        }
        Ok(())
    }
}

fn sync_published_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if failure_tests::FAIL_DIRECTORY_SYNC.get() {
        return Err(std::io::Error::other("injected directory sync failure"));
    }
    #[cfg(unix)]
    File::open(path.parent().unwrap())?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[path = "writer_failure_tests.rs"]
mod failure_tests;

fn identifier() -> String {
    format!("aff4://{}", uuid::Uuid::new_v4())
}
fn stored() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true)
}
fn write_member(zip: &mut ZipWriter<File>, name: &str, data: &[u8]) -> Result<()> {
    zip.start_file(name, stored())?;
    zip.write_all(data)?;
    Ok(())
}
fn hash_triples(result: &AcquiredStream) -> String {
    format!(
        "a:hash \"{}\"^^a:MD5, \"{}\"^^a:SHA1, \"{}\"^^a:SHA256",
        result.md5, result.sha1, result.sha256
    )
}
fn encode(data: &[u8], codec: Compression) -> Result<Vec<u8>> {
    match codec {
        Compression::Stored => Ok(data.to_vec()),
        Compression::Zlib => {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(data)?;
            Ok(encoder.finish()?)
        }
        Compression::Snappy => snap::raw::Encoder::new()
            .compress_vec(data)
            .map_err(|e| malformed(e.to_string())),
        Compression::Lz4 => Ok(lz4_flex::block::compress(data)),
    }
}
struct Hashes {
    md5: Md5,
    sha1: Sha1,
    sha256: Sha256,
}
impl Hashes {
    fn new() -> Self {
        Self {
            md5: Md5::new(),
            sha1: Sha1::new(),
            sha256: Sha256::new(),
        }
    }
    fn update(&mut self, data: &[u8]) {
        self.md5.update(data);
        self.sha1.update(data);
        self.sha256.update(data);
    }
    fn finish(self, id: String, size: u64) -> AcquiredStream {
        AcquiredStream {
            id,
            size,
            md5: hex(&self.md5.finalize()),
            sha1: hex(&self.sha1.finalize()),
            sha256: hex(&self.sha256.finalize()),
        }
    }
}
