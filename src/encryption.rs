use std::fmt;

use zeroize::Zeroizing;

/// Password bytes supplied for one encrypted EWF image open attempt.
///
/// The password owns its byte allocation and zeroizes that allocation when it
/// is dropped. Its [`Debug`](fmt::Debug) representation is always redacted.
pub struct EwfPassword {
    bytes: Zeroizing<Vec<u8>>,
}

impl EwfPassword {
    /// Takes ownership of raw password bytes without applying an encoding or
    /// normalization step.
    #[must_use]
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: Zeroizing::new(bytes.into()),
        }
    }

    /// Copies a UTF-8 password into zeroizing owned storage.
    #[must_use]
    pub fn utf8(password: &str) -> Self {
        Self::from_bytes(password.as_bytes().to_vec())
    }
}

impl fmt::Debug for EwfPassword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _secret_length = self.bytes.len();
        formatter.write_str("EwfPassword([REDACTED])")
    }
}

/// Encryption method applied to an opened EWF image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionMethod {
    /// X-Ways AES-128 in big-endian counter mode.
    XWaysAes128Ctr,
    /// X-Ways AES-256 in little-endian counter mode.
    XWaysAes256Ctr,
}

/// Non-secret encryption status for an opened EWF image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptionInfo {
    method: EncryptionMethod,
    password_verifier_present: bool,
}

impl EncryptionInfo {
    /// Returns the image's encryption method.
    #[must_use]
    pub const fn method(self) -> EncryptionMethod {
        self.method
    }

    /// Returns whether the image contained a password verification hash.
    #[must_use]
    pub const fn password_verifier_present(self) -> bool {
        self.password_verifier_present
    }
}
