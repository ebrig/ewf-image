use std::fs::{self, File};
use std::io::{Read, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use md5::{Digest, Md5};
use sha1::Sha1;
use sha2::Sha256;
use tempfile::NamedTempFile;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{Error, Result, malformed, reader::hex};

/// Container profile chosen before any evidence bytes are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// AFF4 Standard 1.0 physical images with ImageStream storage.
    Physical,
    /// Legacy AFF4-L 1.1 logical files with ZIP segment storage.
    Logical,
}

/// Physical chunk codec. Logical ZIP segments use Stored or Deflate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug)]
pub struct WriteResult {
    /// Final, exclusively created path.
    pub path: PathBuf,
    /// Per-resource source hashes.
    pub streams: Vec<AcquiredStream>,
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
    streams: Vec<AcquiredStream>,
    poisoned: bool,
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
            streams: Vec::new(),
            poisoned: false,
        })
    }

    /// Acquires exactly `size` bytes as a physical ImageStream, preserving
    /// trailing input bytes for the caller. Progress is before/after chunks.
    pub fn add_image(
        &mut self,
        size: u64,
        input: &mut impl Read,
        mut progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<String> {
        self.healthy()?;
        if self.profile != Profile::Physical {
            return Err(Error::Unsupported(
                "physical image in a logical writer".into(),
            ));
        }
        self.poisoned = true;
        let id = identifier();
        let stream = identifier();
        let map = identifier();
        let storage = format!("aff4%3A%2F%2F{}", stream.strip_prefix("aff4://").unwrap());
        let mut hashes = Hashes::new();
        let mut done = 0;
        let mut chunk = 0u64;
        let mut buffer = vec![0; self.options.chunk_bytes as usize];
        let mut index = Vec::new();
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
            }
            let length = (size - done).min(buffer.len() as u64) as usize;
            buffer.fill(0);
            input.read_exact(&mut buffer[..length])?;
            hashes.update(&buffer[..length]);
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
        write_member(zip, &format!("{map_storage}/map"), &range)?;
        write_member(
            zip,
            &format!("{map_storage}/idx"),
            format!("{stream}\n").as_bytes(),
        )?;
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
        self.metadata.push_str(&format!("<{stream}> a a:ImageStream; a:size {size}; a:chunkSize {}; a:chunksInSegment {}; {compression} a:target <{map}> .\n<{id}> a a:Image, a:ContiguousImage, a:DiskImage; a:size {size}; a:dataStream <{map}>; {} .\n<{map}> a a:Map; a:size {size}; a:dependentStream <{stream}>; a:target <{id}>; a:stored <{}> .\n", self.options.chunk_bytes, self.options.chunks_per_bevy, hash_triples(&result),self.volume));
        self.metadata.push_str(&format!(
            "<{stream}> a:stored <{}> .\n<{id}> a:stored <{}> .\n",
            self.volume, self.volume
        ));
        self.streams.push(result);
        self.poisoned = false;
        Ok(id)
    }

    /// Acquires one logical file as a ZIP segment with recorded original path.
    /// Names stay in metadata; ZIP storage uses a generated identifier.
    /// Streams over 1 GiB are rejected for this small-file storage profile.
    pub fn add_file(
        &mut self,
        original_path: &str,
        size: u64,
        input: &mut impl Read,
        mut progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<String> {
        self.healthy()?;
        if self.profile != Profile::Logical {
            return Err(Error::Unsupported(
                "logical file in a physical writer".into(),
            ));
        }
        if size > 1024 * 1024 * 1024 {
            return Err(Error::Unsupported(
                "ZIP logical file exceeds 1 GiB profile limit".into(),
            ));
        }
        if original_path.is_empty() || original_path.chars().any(char::is_control) {
            return Err(malformed(
                "logical path must be nonempty UTF-8 without control characters",
            ));
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
        self.metadata.push_str(&format!("<{id}> a a:FileImage, a:Image, a:zip_segment; a:size {size}; a:fileName {name}; a:originalFileName {path}; {} .\n", hash_triples(&result)));
        self.metadata
            .push_str(&format!("<{id}> a:stored <{}> .\n", self.volume));
        self.streams.push(result);
        self.poisoned = false;
        Ok(id)
    }

    /// Finalizes ZIP metadata, synchronizes bytes, and publishes exclusively.
    /// Unix additionally synchronizes the parent directory. This API does not
    /// certify power-loss durability on every filesystem or device.
    pub fn finish(mut self) -> Result<WriteResult> {
        self.healthy()?;
        let mut zip = self.zip.take().unwrap();
        write_member(&mut zip, "information.turtle", self.metadata.as_bytes())?;
        let file = zip.finish()?;
        file.sync_all()?;
        drop(file);
        self.temporary
            .persist_noclobber(&self.path)
            .map_err(|error| Error::Io(error.error))?;
        #[cfg(unix)]
        File::open(self.path.parent().unwrap())?.sync_all()?;
        Ok(WriteResult {
            path: self.path,
            streams: self.streams,
        })
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
