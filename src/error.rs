use std::io;

use thiserror::Error;

/// Result type returned by fallible `ewf_image` APIs.
pub type Result<T> = std::result::Result<T, EwfError>;

/// Error type used by EWF readers, writers, and probe helpers.
#[derive(Debug, Error)]
pub enum EwfError {
    /// An underlying operating-system or stream I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// The input does not start with a recognized EWF file signature.
    #[error("invalid EWF signature")]
    InvalidSignature,

    /// A caller-provided buffer was too short for the requested operation.
    #[error("buffer too short: needed {needed} bytes, found {actual}")]
    BufferTooShort {
        /// Minimum number of bytes required.
        needed: usize,
        /// Number of bytes that were actually available.
        actual: usize,
    },

    /// No segment files were found or supplied.
    #[error("no EWF segments found for {0}")]
    NoSegments(String),

    /// The image uses a valid EWF feature that this crate does not support.
    #[error("unsupported EWF feature: {0}")]
    Unsupported(String),

    /// The operation was cancelled after an abort signal.
    #[error("operation aborted")]
    Aborted,

    /// The image or supplied chunk data is structurally invalid.
    #[error("malformed EWF image: {0}")]
    Malformed(String),
}
