use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Result, invalid};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceKind {
    #[default]
    File,
    Device,
}

fn is_file(kind: &SourceKind) -> bool {
    *kind == SourceKind::File
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceIdentity {
    #[serde(default, skip_serializing_if = "is_file")]
    pub kind: SourceKind,
    pub path: PathBuf,
    pub size: u64,
    pub sector_size: u32,
    pub identity: String,
}

pub(super) struct Source {
    file: File,
    pub identity: SourceIdentity,
    output: PathBuf,
}

impl Source {
    pub fn open(path: &Path, sector_size: Option<u32>, output: &Path) -> Result<Self> {
        #[cfg(windows)]
        if windows::disk_number(path).is_some() {
            return Self::open_device(path, sector_size, output);
        }
        let path = fs::canonicalize(path)?;
        if path == output || path.starts_with(super::sidecar(output, "ewf-acquisition")) {
            return Err(invalid("source overlaps the output or acquisition journal"));
        }
        for suffix in ["ewf-session.json", "ewf-cli.lock"] {
            if path == super::sidecar(output, suffix) {
                return Err(invalid("source overlaps a CLI control file"));
            }
        }
        let named = fs::metadata(&path)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::FileTypeExt;
            if named.file_type().is_block_device() {
                return Self::open_device(&path, sector_size, output);
            }
        }
        // Reject FIFOs and unsupported device paths before a possibly blocking open.
        if !named.is_file() {
            return Err(invalid("unsupported source type"));
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
            kind: SourceKind::File,
            path,
            size: metadata.len(),
            sector_size,
            identity: metadata_identity(&metadata)?,
        };
        Ok(Self {
            file,
            identity,
            output: output.to_path_buf(),
        })
    }

    #[cfg(any(target_os = "linux", windows))]
    fn open_device(path: &Path, sector_size: Option<u32>, output: &Path) -> Result<Self> {
        let identity = device_identity(path, output)?;
        if sector_size.is_some_and(|size| size != identity.sector_size) {
            return Err(invalid(
                "sector-size override disagrees with device geometry",
            ));
        }
        if !matches!(identity.sector_size, 512 | 1024 | 2048 | 4096)
            || identity.size == 0
            || !identity
                .size
                .is_multiple_of(u64::from(identity.sector_size))
        {
            return Err(invalid("unsupported device geometry"));
        }
        let file = File::open(path)?; // Read-only: never request source write access.
        let source = Self {
            file,
            identity,
            output: output.to_path_buf(),
        };
        source.check_unchanged()?;
        Ok(source)
    }

    pub fn check_unchanged(&self) -> Result<()> {
        if self.identity.kind == SourceKind::Device {
            if device_identity(&self.identity.path, &self.output)? != self.identity {
                return Err(invalid("device identity or geometry changed"));
            }
            return Ok(());
        }
        let current = metadata_identity(&self.file.metadata()?)?;
        let named = metadata_identity(&fs::metadata(&self.identity.path)?)?;
        if current != self.identity.identity || current != named {
            return Err(invalid("source metadata changed during acquisition"));
        }
        Ok(())
    }
}

impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.file.read(buffer) {
            Err(error) => {
                // Disconnection is not an unreadable sector. Recheck device
                // presence/identity before allowing the sector substitution policy.
                if self.identity.kind == SourceKind::Device {
                    self.check_unchanged()
                        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.to_string()))?;
                }
                Err(error)
            }
            result => result,
        }
    }
}

impl Seek for Source {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

fn device_identity(path: &Path, output: &Path) -> Result<SourceIdentity> {
    #[cfg(windows)]
    return windows::identity(path, output);
    #[cfg(target_os = "linux")]
    return linux::identity(path, output);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (path, output);
        Err(invalid(
            "device acquisition is supported on Windows and Linux",
        ))
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
