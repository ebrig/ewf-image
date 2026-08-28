use std::fmt;

use aes::{Aes128, Aes256};
use ctr::cipher::{KeyIvInit, StreamCipher};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{EwfError, Result};

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

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
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

#[allow(dead_code)]
#[derive(Clone)]
pub(crate) struct XWaysEncryptionMetadata {
    pub(crate) method: EncryptionMethod,
    pub(crate) layout_version: u32,
    pub(crate) salt: [u8; 32],
    pub(crate) initial_counter: [u8; 16],
    pub(crate) password_verifier: Option<[u8; 32]>,
    pub(crate) ewf_header_file_offset: u64,
    pub(crate) encrypted_stream_file_offset: u64,
}

#[allow(dead_code)]
pub(crate) struct EncryptionContext {
    method: EncryptionMethod,
    key: DerivedKey,
    initial_counter: [u8; 16],
}

enum DerivedKey {
    Aes128(Zeroizing<[u8; 16]>),
    Aes256(Zeroizing<[u8; 32]>),
}

impl fmt::Debug for EncryptionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EncryptionContext")
            .field("method", &self.method)
            .field("key", &"[REDACTED]")
            .field("initial_counter", &"[REDACTED]")
            .finish()
    }
}

#[allow(dead_code)]
impl EncryptionContext {
    pub(crate) fn derive(metadata: &XWaysEncryptionMetadata, password: &EwfPassword) -> Self {
        let key = match metadata.method {
            EncryptionMethod::XWaysAes128Ctr => DerivedKey::Aes128(Zeroizing::new(
                derive_aes128_key(password.as_bytes(), &metadata.salt),
            )),
            EncryptionMethod::XWaysAes256Ctr => DerivedKey::Aes256(Zeroizing::new(
                derive_aes256_key(password.as_bytes(), &metadata.salt),
            )),
        };
        Self {
            method: metadata.method,
            key,
            initial_counter: metadata.initial_counter,
        }
    }

    pub(crate) fn apply_keystream(&self, stream_offset: u64, bytes: &mut [u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        let block_index = stream_offset / 16;
        let intra_block =
            usize::try_from(stream_offset % 16).expect("AES intra-block offset is smaller than 16");
        let spanned_bytes = intra_block
            .checked_add(bytes.len())
            .ok_or_else(counter_overflow)?;
        let final_block_delta =
            u64::try_from((spanned_bytes - 1) / 16).map_err(|_| counter_overflow())?;
        let final_block_index = block_index
            .checked_add(final_block_delta)
            .ok_or_else(counter_overflow)?;
        let _ = counter_at(self.method, self.initial_counter, final_block_index)?;
        let counter = counter_at(self.method, self.initial_counter, block_index)?;

        match &self.key {
            DerivedKey::Aes128(key) => {
                type Aes128Ctr = ctr::Ctr128BE<Aes128>;
                let mut cipher = Aes128Ctr::new((&**key).into(), (&counter).into());
                apply_with_intra_block_offset(&mut cipher, intra_block, bytes);
            }
            DerivedKey::Aes256(key) => {
                type Aes256Ctr = ctr::Ctr128LE<Aes256>;
                let mut cipher = Aes256Ctr::new((&**key).into(), (&counter).into());
                apply_with_intra_block_offset(&mut cipher, intra_block, bytes);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn from_test_aes128_key(key: [u8; 16], initial_counter: [u8; 16]) -> Self {
        Self {
            method: EncryptionMethod::XWaysAes128Ctr,
            key: DerivedKey::Aes128(Zeroizing::new(key)),
            initial_counter,
        }
    }
}

fn apply_with_intra_block_offset(
    cipher: &mut impl StreamCipher,
    intra_block: usize,
    bytes: &mut [u8],
) {
    let mut discarded = [0_u8; 15];
    cipher.apply_keystream(&mut discarded[..intra_block]);
    cipher.apply_keystream(bytes);
}

fn counter_at(
    method: EncryptionMethod,
    initial_counter: [u8; 16],
    block_index: u64,
) -> Result<[u8; 16]> {
    let value = match method {
        EncryptionMethod::XWaysAes128Ctr => u128::from_be_bytes(initial_counter),
        EncryptionMethod::XWaysAes256Ctr => u128::from_le_bytes(initial_counter),
    };
    let value = value
        .checked_add(u128::from(block_index))
        .ok_or_else(counter_overflow)?;
    Ok(match method {
        EncryptionMethod::XWaysAes128Ctr => value.to_be_bytes(),
        EncryptionMethod::XWaysAes256Ctr => value.to_le_bytes(),
    })
}

fn counter_overflow() -> EwfError {
    EwfError::Malformed("X-Ways AES counter overflow".into())
}

fn derive_aes256_key(password: &[u8], salt: &[u8; 32]) -> [u8; 32] {
    let password_hash = Sha256::digest(password);
    Sha256::new()
        .chain_update(password_hash)
        .chain_update(salt)
        .finalize()
        .into()
}

fn derive_aes128_key(password: &[u8], salt: &[u8; 32]) -> [u8; 16] {
    let key = derive_aes256_key(password, salt);
    let mut reduced = [0_u8; 16];
    for (output, (first, second)) in reduced.iter_mut().zip(key[..16].iter().zip(&key[16..])) {
        *output = first ^ second;
    }
    reduced
}

#[allow(dead_code)]
fn aes256_password_verifier(password: &[u8], salt: &[u8; 32]) -> [u8; 32] {
    let password_hash = Sha256::digest(password);
    let second_hash = Sha256::new()
        .chain_update(password)
        .chain_update(password_hash)
        .finalize();
    Sha256::new()
        .chain_update(salt)
        .chain_update(password_hash)
        .chain_update(second_hash)
        .finalize()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &[u8] = b"ewf-image-test-only";
    const SALT: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];

    fn hex_vec(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = char::from(pair[0]).to_digit(16).expect("hex digit");
                let low = char::from(pair[1]).to_digit(16).expect("hex digit");
                u8::try_from((high << 4) | low).expect("hex byte")
            })
            .collect()
    }

    fn hex_array<const N: usize>(value: &str) -> [u8; N] {
        hex_vec(value).try_into().expect("hex has expected length")
    }

    fn aes256_context(initial_counter: [u8; 16]) -> EncryptionContext {
        let password = EwfPassword::from_bytes(PASSWORD.to_vec());
        let metadata = XWaysEncryptionMetadata {
            method: EncryptionMethod::XWaysAes256Ctr,
            layout_version: 1,
            salt: SALT,
            initial_counter,
            password_verifier: None,
            ewf_header_file_offset: 0,
            encrypted_stream_file_offset: 0,
        };
        EncryptionContext::derive(&metadata, &password)
    }

    #[test]
    fn derives_documented_xways_keys() {
        assert_eq!(
            derive_aes256_key(PASSWORD, &SALT),
            hex_array("5d8c04beb71ca6914d9dc2aaf68b1d8817023a0d669a818a0e974d5f8a2c40f7")
        );
        assert_eq!(
            derive_aes128_key(PASSWORD, &SALT),
            hex_array("4a8e3eb3d186271b430a8ff57ca75d7f")
        );
    }

    #[test]
    fn computes_documented_aes256_password_verifier() {
        assert_eq!(
            aes256_password_verifier(PASSWORD, &SALT),
            hex_array("23215aeabaff9455ccdd288893346b4392dafd3ff298a09559244567a9ceacb6")
        );
    }

    #[test]
    fn aes128_ctr_matches_nist_big_endian_vector() {
        let mut data = hex_vec("6bc1bee22e409f96e93d7e117393172a");
        let context = EncryptionContext::from_test_aes128_key(
            hex_array("2b7e151628aed2a6abf7158809cf4f3c"),
            hex_array("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff"),
        );

        context.apply_keystream(0, &mut data).unwrap();

        assert_eq!(data, hex_vec("874d6191b620e3261bef6864990db6ce"));
    }

    #[test]
    fn aes256_ctr_increments_the_low_order_first_byte() {
        let context = aes256_context(hex_array("000102030405060708090a0b0c0d0e0f"));
        let mut two_blocks = vec![0_u8; 32];

        context.apply_keystream(0, &mut two_blocks).unwrap();

        assert_eq!(
            &two_blocks[..16],
            hex_vec("e5c8e696d0ed24593152afcd7740a99a")
        );
        assert_eq!(
            &two_blocks[16..],
            hex_vec("363d06a5d8ae94cbb574f597dc511488")
        );
    }

    #[test]
    fn random_access_matches_one_full_ctr_read_at_every_split() {
        let context = aes256_context(hex_array("000102030405060708090a0b0c0d0e0f"));
        let mut expected = vec![0_u8; 48];
        context.apply_keystream(0, &mut expected).unwrap();

        for split in 0..=expected.len() {
            let mut first = vec![0_u8; split];
            let mut second = vec![0_u8; expected.len() - split];
            context.apply_keystream(0, &mut first).unwrap();
            context
                .apply_keystream(u64::try_from(split).unwrap(), &mut second)
                .unwrap();
            first.extend_from_slice(&second);
            assert_eq!(first, expected, "split at byte {split}");
        }
    }

    #[test]
    fn unaligned_random_access_matches_full_ctr_read() {
        let context = aes256_context(hex_array("000102030405060708090a0b0c0d0e0f"));
        let mut expected = vec![0_u8; 48];
        context.apply_keystream(0, &mut expected).unwrap();

        for (offset, length) in [(1_u64, 31_usize), (15, 18), (16, 16), (17, 17)] {
            let mut actual = vec![0_u8; length];
            context.apply_keystream(offset, &mut actual).unwrap();
            let start = usize::try_from(offset).unwrap();
            assert_eq!(actual, expected[start..start + length]);
        }
    }

    #[test]
    fn empty_ctr_read_at_maximum_counter_is_allowed() {
        let context = EncryptionContext::from_test_aes128_key([0; 16], [0xff; 16]);
        let mut empty = [];

        context.apply_keystream(u64::MAX, &mut empty).unwrap();
    }

    #[test]
    fn ctr_read_rejects_counter_wrap() {
        let context = EncryptionContext::from_test_aes128_key([0; 16], [0xff; 16]);
        let mut data = [0_u8; 1];

        let error = context.apply_keystream(16, &mut data).unwrap_err();

        assert!(
            matches!(error, EwfError::Malformed(message) if message == "X-Ways AES counter overflow")
        );
    }
}
