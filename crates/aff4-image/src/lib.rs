//! AFF4 evidence streams, separate from the EWF format implementation.
//!
//! Supports ZIP AFF4 v1 physical ImageStreams and Maps, legacy AFF4-L 1.1,
//! and a documented subset of the AFF4-L 2.1 draft. Reads never extract
//! archive paths onto the host filesystem. Verification covers selected stream
//! bytes and available linear hashes. Metadata hashes are checked separately;
//! neither internal hashes nor matching bytes establish independent authenticity.
#![forbid(unsafe_code)]

mod reader;
mod writer;
pub use reader::{
    ByteCoverage, CheckOutcome, Container, ContainerVerification, IntegrityCheck, Limits,
    MetadataScan, MetadataVerification, Property, ResourceVerification, StreamInfo, Verification,
};
pub use writer::{AcquiredStream, Compression, Profile, WriteOptions, WriteResult, Writer};

/// AFF4 operation result.
pub type Result<T> = std::result::Result<T, Error>;

/// Malformed, unsupported, cancelled, or unreadable evidence.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Backing I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// ZIP structure or decoding error.
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    /// Invalid metadata, offsets, or references.
    #[error("malformed AFF4: {0}")]
    Malformed(String),
    /// Recognized feature outside this implementation's supported profile.
    #[error("unsupported AFF4: {0}")]
    Unsupported(String),
    /// Cooperative cancellation.
    #[error("operation cancelled")]
    Aborted,
}

pub(crate) fn malformed(message: impl Into<String>) -> Error {
    Error::Malformed(message.into())
}
