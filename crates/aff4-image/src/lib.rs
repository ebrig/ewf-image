//! AFF4 evidence streams, separate from the EWF format implementation.
//!
//! Supports ZIP AFF4 v1 physical ImageStreams and Maps, legacy AFF4-L 1.1,
//! and a documented subset of the AFF4-L 2.1 draft. Reads never extract
//! archive paths onto the host filesystem. Verification reports linear content,
//! block/map structures, metadata digests, and byte provenance. Physical volume
//! sets support assembled-image SHA256 and contextual metadata/block/map checks
//! with explicit source attribution and caller-supplied stripe order. Neither
//! internal hashes nor matching bytes establish independent authenticity.
#![forbid(unsafe_code)]

mod reader;
mod writer;
pub use reader::{
    ByteCoverage, CheckOutcome, Container, ContainerVerification, IntegrityCheck, Limits,
    MetadataScan, MetadataVerification, Property, ResourceVerification, SetDigest, SetVerification,
    SetVolumeVerification, StreamInfo, Verification, VolumeSet, VolumeSource,
};
pub use writer::{
    AcquiredStream, CaseMetadata, CollectionIssue, CollectionLimits, CollectionOptions,
    CollectionReport, Compression, LogicalMetadata, Profile, SubstreamKind, WriteOptions,
    WriteResult, Writer,
};

/// AFF4 operation result.
pub type Result<T> = std::result::Result<T, Error>;

/// Malformed, unsupported, cancelled, or unreadable evidence.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Finalized staging failed verification and was not published.
    #[error("staged AFF4 container verification failed; inspect the retained verification report")]
    VerificationFailed {
        /// Complete available checks, resource errors and metadata diagnostics.
        /// The temporary container is removed; this report retains the evidence
        /// needed to distinguish mismatches, unreadable data and incomplete checks.
        report: Box<ContainerVerification>,
    },
    /// Collection stopped before exceeding an explicit resource budget.
    #[error("AFF4 collection limit {resource}: requires {required}, limit {limit}")]
    ResourceLimit {
        /// Budget name, matching the CLI report.
        resource: &'static str,
        /// Required bytes or count, including reserved final metadata.
        required: u64,
        /// Configured maximum bytes or count.
        limit: u64,
    },
    /// Backing I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Output was published, but synchronizing its parent directory failed.
    /// Preserve the result and independently verify it before further use.
    #[error("AFF4 output published at {path}, but directory synchronization failed: {source}", path = result.path.display())]
    PublishedButUnsynced {
        /// Published path and source digests; output must not be overwritten.
        result: Box<WriteResult>,
        /// Directory synchronization failure.
        #[source]
        source: std::io::Error,
    },
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
