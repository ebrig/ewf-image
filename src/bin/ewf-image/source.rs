use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Result, invalid};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceIdentity {
    pub path: PathBuf,
    pub size: u64,
    pub sector_size: u32,
    pub identity: String,
}

pub(super) struct Source {
    pub file: File,
    pub identity: SourceIdentity,
}

impl Source {
    pub fn open(path: &Path, sector_size: Option<u32>, output: &Path) -> Result<Self> {
        let path = fs::canonicalize(path)?;
        if path == output || path.starts_with(super::sidecar(output, "ewf-acquisition")) {
            return Err(invalid("source overlaps the output or acquisition journal"));
        }
        for suffix in ["ewf-session.json", "ewf-cli.lock"] {
            if path == super::sidecar(output, suffix) {
                return Err(invalid("source overlaps a CLI control file"));
            }
        }
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("this source is not a regular file"));
        }
        let sector_size = sector_size.unwrap_or(512);
        if !matches!(sector_size, 512 | 1024 | 2048 | 4096)
            || metadata.len() == 0
            || !metadata.len().is_multiple_of(u64::from(sector_size))
        {
            return Err(invalid(
                "source size must be nonzero and sector aligned (512/1024/2048/4096)",
            ));
        }
        let identity = SourceIdentity {
            path,
            size: metadata.len(),
            sector_size,
            identity: metadata_identity(&metadata)?,
        };
        Ok(Self { file, identity })
    }

    pub fn check_unchanged(&self) -> Result<()> {
        let current = metadata_identity(&self.file.metadata()?)?;
        let named = metadata_identity(&fs::metadata(&self.identity.path)?)?;
        if current != self.identity.identity || current != named {
            return Err(invalid("source metadata changed during acquisition"));
        }
        Ok(())
    }
}

impl SourceIdentity {
    pub fn fingerprint(&self) -> Result<[u8; 32]> {
        Ok(Sha256::digest(serde_json::to_vec(self)?).into())
    }
}

fn metadata_identity(metadata: &fs::Metadata) -> Result<String> {
    let mut identity = format!(
        "file:{}:{:?}:{:?}",
        metadata.len(),
        metadata.modified()?,
        metadata.created().ok()
    );
    #[cfg(unix)]
    {
        use std::fmt::Write;
        use std::os::unix::fs::MetadataExt;
        write!(
            identity,
            ":{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )?;
    }
    // The digest is a metadata identity token, not a hash of source content.
    identity = super::hex(&Sha256::digest(identity.as_bytes()));
    Ok(identity)
}
