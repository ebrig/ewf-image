//! Rust library for reading and writing Expert Witness Format forensic images.
//!
//! `ewf_image` provides direct Rust APIs for working with Expert Witness Format
//! images. It can open physical, logical, SMART, and EWF2 segment families,
//! expose metadata and stored hashes, read the logical media stream, walk
//! logical single-file catalogs, and create EWF output. CLI and mount layers
//! are not currently implemented.
//!
//! # Terminology
//!
//! Logical media size is the decoded byte length exposed by [`Image::media_size`]
//! and [`ImageInfo::logical_size`]. Segment set size is the total byte length of
//! the opened EWF container files, as reported by [`Image::segment_set_size`].
//! Chunks are the stored allocation units used by EWF tables. Logical EWF
//! images can also contain a single-file catalog, where each entry describes a
//! file-like object stored inside the image.
//!
//! # Supported container families
//!
//! - EWF1 physical `.E01` / EVF images.
//! - EWF1 logical `.L01` / LVF images.
//! - EWF1 SMART `.S01` images.
//! - EWF2 physical `.Ex01` images.
//! - EWF2 logical `.Lx01` images.
//!
//! # Reading
//!
//! ```no_run
//! use std::io::Read;
//!
//! fn main() -> ewf_image::Result<()> {
//!     let image = ewf_image::Image::open("case.E01")?;
//!     let info = image.info();
//!
//!     println!("{:?}: {} bytes", info.format, info.logical_size);
//!     println!("segments: {}", image.number_of_segments());
//!
//!     let mut sector = vec![0; 512];
//!     image.cursor().read_exact(&mut sector)?;
//!
//!     let mut later_sector = vec![0; 512];
//!     image.read_at(&mut later_sector, 4096)?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Metadata and hashes
//!
//! ```no_run
//! fn main() -> ewf_image::Result<()> {
//!     let image = ewf_image::Image::open("case.E01")?;
//!
//!     if let Some(case_number) = image.header_value("case_number") {
//!         println!("case: {case_number}");
//!     }
//!
//!     if let Some(md5) = image.hash_value("MD5") {
//!         println!("stored MD5: {md5}");
//!     }
//!
//!     #[cfg(feature = "verify")]
//!     {
//!         let verification = image.verify()?;
//!         println!("MD5 match: {:?}", verification.md5_match);
//!         println!("SHA1 match: {:?}", verification.sha1_match);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! # Writing
//!
//! ```no_run
//! use std::fs::File;
//!
//! fn main() -> ewf_image::Result<()> {
//!     let mut input = File::open("disk.raw")?;
//!
//!     let mut options = ewf_image::WriteOptions::default();
//!     options.format = ewf_image::WriteFormat::Ewf2Physical;
//!     options.compression = ewf_image::WriteCompression::Zlib;
//!     options.metadata.set_header_value("case_number", "CASE-001");
//!
//!     let mut writer = ewf_image::EwfWriter::create("case.Ex01", options)?;
//!     std::io::copy(&mut input, &mut writer)?;
//!     writer.finish()?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Feature flags
//!
//! - `verify` is enabled by default and adds `Image::verify()` plus
//!   `VerifyResult` for streamed MD5/SHA1 verification. Stored hash parsing,
//!   EWF2 section integrity checks, and writer hash support are available
//!   without this feature.
//! - `external-fixtures` enables ignored integration tests that require local
//!   EWF corpora and external EWF tools. It does not change library behavior.
//!
//! # Limitations
//!
//! Encrypted EWF2 images are detected and rejected, but decryption and
//! encrypted writing are not implemented. Secondary/shadow target mirroring is
//! supported by the file-backed writer. Base-plus-overlay delta/shadow images
//! are not implemented.

mod codepage;
mod date_time;
mod decode;
mod error;
mod format;
mod image;
mod index;
mod metadata;
mod reader_cache;
mod reader_statistics;
mod segment;
mod signature;
mod single_files;
mod types;
#[cfg(feature = "verify")]
mod verify;
mod writer;

pub use error::{EwfError, Result};
pub use image::{Image, ImageCursor, SegmentReader, SingleFileCursor};
pub use reader_statistics::{ReaderCacheInfo, ReaderStatistics};
pub use signature::{
    check_file_corruption, check_file_encryption, check_file_signature,
    check_segment_files_corruption, check_segment_files_encryption,
};
pub use single_files::SINGLE_FILE_PATH_SEPARATOR;
pub use types::{
    AcquisitionError, ChunkCacheCapacity, CompressionFlags, CompressionLevel, CompressionMethod,
    CompressionValues, DataChunk, DataChunkEncoding, EncodedDataChunk, EwfMetadata, Format,
    FormatProfile, HeaderCodepage, HeaderDateFormat, ImageInfo, MediaFlags, MediaInfo, MediaType,
    MemoryExtent, OpenOptions, OpenStrictness, SectorRange, SegmentFileVersion,
    SingleFileAttribute, SingleFileEntry, SingleFileEntryType, SingleFileExtent,
    SingleFilePermission, SingleFilePermissionGroup, SingleFileSource, SingleFileSubject,
    SingleFilesAuxTables, SingleFilesInfo, StoredHashes,
};
pub use writer::{
    EwfWriter, WriteCompression, WriteCompressionLevel, WriteCompressionValues, WriteFormat,
    WriteHashes, WriteMediaProfile, WriteOptions, WriteResult,
};

#[cfg(feature = "verify")]
pub use types::VerifyResult;
