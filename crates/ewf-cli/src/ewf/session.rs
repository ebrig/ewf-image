use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::source::SourceIdentity;
use super::{Acquire, AcquisitionOptions, Result, invalid, sidecar};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Session {
    pub schema_version: u32,
    pub output: PathBuf,
    pub source: SourceIdentity,
    pub sectors_per_chunk: u32,
    pub chunks_per_segment: u32,
    pub compression: String,
    pub case_number: Option<String>,
    pub evidence_number: Option<String>,
    pub examiner: Option<String>,
    pub software_version: String,
}

impl Session {
    pub fn new(output: PathBuf, source: SourceIdentity, args: &Acquire) -> Self {
        Self {
            schema_version: 1,
            output,
            source,
            sectors_per_chunk: args.sectors_per_chunk,
            chunks_per_segment: args.chunks_per_segment,
            compression: args.compression.clone(),
            case_number: args.case_number.clone(),
            evidence_number: args.evidence_number.clone(),
            examiner: args.examiner.clone(),
            software_version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    pub fn options(&self) -> Result<AcquisitionOptions> {
        if self.schema_version != 1 {
            return Err(invalid("unsupported CLI session schema"));
        }
        let mut options = AcquisitionOptions::new(self.source.size);
        options.bytes_per_sector = self.source.sector_size;
        options.sectors_per_chunk = self.sectors_per_chunk;
        options.chunks_per_segment = self.chunks_per_segment;
        options.compression = match self.compression.as_str() {
            "raw" => ewf_image::WriteCompression::None,
            "zlib" => ewf_image::WriteCompression::Zlib,
            _ => return Err(invalid("unsupported session compression")),
        };
        options.metadata.case_number.clone_from(&self.case_number);
        options
            .metadata
            .evidence_number
            .clone_from(&self.evidence_number);
        options.metadata.examiner.clone_from(&self.examiner);
        options.metadata.acquisition_software = Some(env!("CARGO_PKG_NAME").into());
        options.metadata.acquisition_software_version = Some(self.software_version.clone());
        Ok(options)
    }

    pub fn load(output: &Path) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(sidecar(output, "ewf-session.json"))?
            .take(1_048_577)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 1_048_576 {
            return Err(invalid("CLI session exceeds size limit"));
        }
        let session: Self = serde_json::from_slice(&bytes)?;
        if session.output != output {
            return Err(invalid("session destination does not match this output"));
        }
        session.options()?;
        Ok(session)
    }

    pub fn save(&self) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(
            self.output
                .parent()
                .ok_or_else(|| invalid("missing output directory"))?,
        )?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist_noclobber(sidecar(&self.output, "ewf-session.json"))?;
        #[cfg(unix)]
        sync_parent(&self.output)?;
        Ok(())
    }
}

pub(super) fn lock(output: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sidecar(output, "ewf-cli.lock"))?;
    file.try_lock()
        .map_err(|_| invalid("another CLI operation holds this destination lock"))?;
    Ok(file)
}

pub(super) fn normalize_output(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| invalid("output needs a filename"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let output = fs::canonicalize(parent)?.join(name);
    if output.extension().and_then(|s| s.to_str()) != Some("E01") {
        return Err(invalid("output must end in .E01"));
    }
    Ok(output)
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<()> {
    File::open(
        path.parent()
            .ok_or_else(|| invalid("missing output directory"))?,
    )?
    .sync_all()?;
    Ok(())
}
