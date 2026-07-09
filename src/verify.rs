use md5::{Digest, Md5};
use sha1::Sha1;

use crate::EwfError;
use crate::{Image, Result, VerifyResult};

impl Image {
    /// Computes streamed MD5 and SHA1 hashes for the logical media stream.
    ///
    /// The returned [`VerifyResult`] contains computed hashes and optional match
    /// results. Match values are `None` when the image did not store the
    /// corresponding expected hash.
    ///
    /// # Errors
    ///
    /// Returns an error if reading the logical media stream fails, if the read
    /// reaches EOF before [`ImageInfo::logical_size`](crate::ImageInfo::logical_size),
    /// or if the image has been aborted with [`Image::signal_abort`].
    pub fn verify(&self) -> Result<VerifyResult> {
        let mut md5 = Md5::new();
        let mut sha1 = Sha1::new();
        let mut offset = 0_u64;
        let logical_size = self.info().logical_size;
        let mut buf = vec![0; 1024 * 1024];

        while offset < logical_size {
            let remaining = logical_size - offset;
            let request = usize::try_from(remaining.min(buf.len() as u64))
                .map_err(|_| EwfError::Malformed("verify read size does not fit usize".into()))?;
            let read = self.read_at(&mut buf[..request], offset)?;
            if read == 0 {
                return Err(EwfError::Malformed(
                    "verify reached EOF before logical image end".into(),
                ));
            }

            md5.update(&buf[..read]);
            sha1.update(&buf[..read]);
            offset += u64::try_from(read).expect("usize fits u64");
        }

        let computed_md5: [u8; 16] = md5.finalize().into();
        let computed_sha1: [u8; 20] = sha1.finalize().into();
        let stored = &self.info().stored_hashes;

        Ok(VerifyResult {
            computed_md5: Some(computed_md5),
            computed_sha1: Some(computed_sha1),
            md5_match: stored.md5.map(|expected| expected == computed_md5),
            sha1_match: stored.sha1.map(|expected| expected == computed_sha1),
        })
    }
}
