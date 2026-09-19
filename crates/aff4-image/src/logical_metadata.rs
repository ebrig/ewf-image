//! Caller-owned logical metadata. Original paths are never ZIP storage names.
use super::*;
use base64::Engine;

/// Metadata captured from a source, independent of the examiner's filesystem.
#[derive(Debug, Clone, Default)]
pub struct LogicalMetadata {
    /// Exact original path bytes. Non-UTF8/control bytes receive a raw-name field.
    pub path: Vec<u8>,
    /// Previously added folder, or None for an acquisition root.
    pub parent: Option<String>,
    /// Creation time in nanoseconds since the Unix epoch.
    pub created: Option<i128>,
    /// Last content modification in nanoseconds since the Unix epoch.
    pub modified: Option<i128>,
    /// Last access in nanoseconds since the Unix epoch.
    pub accessed: Option<i128>,
    /// Last metadata change in nanoseconds since the Unix epoch.
    pub changed: Option<i128>,
    /// Native Unix mode bits when available.
    pub mode: Option<u32>,
}

/// Descriptive provenance, not proof of origin or a digital signature.
#[derive(Debug, Clone, Default)]
pub struct CaseMetadata {
    /// Case identifier supplied by the operator.
    pub case_number: String,
    /// Evidence identifier supplied by the operator.
    pub evidence_number: String,
    /// Examiner name supplied by the operator.
    pub examiner: String,
    /// Collection notes, including source/snapshot constraints.
    pub notes: String,
}

/// Relationship of a non-primary file stream to its parent.
#[derive(Debug, Clone, Copy)]
pub enum SubstreamKind {
    /// NTFS alternate data stream.
    AlternateDataStream,
    /// Filesystem extended attribute, including ACL bytes supplied by callers.
    ExtendedAttribute,
}

fn literal(value: &str) -> String {
    oxrdf::Literal::new_simple_literal(value).to_string()
}
fn normalized(bytes: &[u8]) -> (String, Option<String>) {
    if let Ok(value) = std::str::from_utf8(bytes)
        && !value.chars().any(char::is_control)
    {
        return (value.into(), None);
    }
    let mut text = String::new();
    for byte in bytes {
        if *byte < 32 || *byte >= 127 || *byte == b'%' {
            text.push_str(&format!("%{byte:02X}"));
        } else {
            text.push(*byte as char);
        }
    }
    (
        text,
        Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
    )
}

impl LogicalMetadata {
    fn properties(&self) -> Result<(String, String)> {
        if self.path.is_empty() {
            return Err(malformed("empty original path"));
        }
        let name = {
            self.path
                .rsplit(|b| *b == b'/' || *b == b'\\')
                .next()
                .unwrap()
        };
        let (path, raw_path) = normalized(&self.path);
        let (_, raw_name) = normalized(name);
        let mut properties = format!("l:originalPathName {};", literal(&path));
        for (key, value) in [("originalPathNameRaw", raw_path), ("fileNameRaw", raw_name)] {
            if let Some(value) = value {
                properties.push_str(&format!(
                    " l:{key} {}^^<http://www.w3.org/2001/XMLSchema#base64Binary>;",
                    literal(&value)
                ));
            }
        }
        for (key, value) in [
            ("birthTime", self.created),
            ("lastWritten", self.modified),
            ("lastAccessed", self.accessed),
            ("recordChanged", self.changed),
        ] {
            if let Some(value) = value {
                let time = time::OffsetDateTime::from_unix_timestamp_nanos(value)
                    .map_err(|_| malformed("timestamp outside supported calendar"))?
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|_| malformed("timestamp cannot be represented as RFC3339"))?;
                properties.push_str(&format!(
                    " a:{key} {}^^<http://www.w3.org/2001/XMLSchema#dateTime>;",
                    literal(&time)
                ));
            }
        }
        if let Some(mode) = self.mode {
            properties.push_str(&format!(" l:fileMode {mode};"));
        }
        Ok((path, properties))
    }
}

impl Writer {
    pub(super) fn logical_parent(&self, parent: Option<&str>) -> Result<()> {
        self.healthy()?;
        if self.profile != Profile::Logical {
            return Err(Error::Unsupported(
                "logical metadata in physical profile".into(),
            ));
        }
        if parent.is_some_and(|p| !self.folders.contains_key(p)) {
            return Err(malformed("parent is not a writer-owned folder"));
        }
        Ok(())
    }

    fn register_child(&mut self, parent: Option<&str>, id: &str) {
        if let Some(parent) = parent {
            self.folders.get_mut(parent).unwrap().1.push(id.into());
        } else {
            self.roots.push(id.into());
        }
    }

    /// Adds a folder and returns its stable resource identifier, preserving empties.
    pub fn add_folder(&mut self, metadata: &LogicalMetadata) -> Result<String> {
        self.logical_parent(metadata.parent.as_deref())?;
        let (path, properties) = metadata.properties()?;
        let id = identifier();
        let name = {
            metadata
                .path
                .rsplit(|b| *b == b'/' || *b == b'\\')
                .next()
                .unwrap()
        };
        let (name, _) = normalized(name);
        let properties = format!(
            "a a:Folder, a:FolderImage, a:Image; a:fileName {}; a:originalFileName {}; {properties} a:stored <{}>",
            literal(&name),
            literal(&path),
            self.volume
        );
        self.register_child(metadata.parent.as_deref(), &id);
        self.folders.insert(id.clone(), (properties, Vec::new()));
        Ok(id)
    }

    /// Acquires a file and preserves timestamps, mode, raw names, and its parent.
    pub fn add_file_with_metadata(
        &mut self,
        metadata: &LogicalMetadata,
        size: u64,
        input: &mut impl Read,
        progress: impl FnMut(u64, u64) -> ControlFlow<()>,
    ) -> Result<String> {
        self.logical_parent(metadata.parent.as_deref())?;
        let (path, properties) = metadata.properties()?;
        let id = self.add_file_impl(&path, size, input, progress, &properties)?;
        self.register_child(metadata.parent.as_deref(), &id);
        Ok(id)
    }

    /// Records source/operator context as supplied, without claiming authentication.
    pub fn add_case_metadata(&mut self, metadata: &CaseMetadata) -> Result<String> {
        self.healthy()?;
        let id = identifier();
        self.metadata.push_str(&format!("<{id}> a a:CaseDetails; a:target <{}>; a:caseNumber {}; a:evidenceNumber {}; a:examiner {}; a:notes {} .\n", self.volume, literal(&metadata.case_number), literal(&metadata.evidence_number), literal(&metadata.examiner), literal(&metadata.notes)));
        Ok(id)
    }

    /// Preserves a named substream's bytes. Callers capture ADS/xattrs/ACLs using
    /// source-specific APIs; this method does not infer them from a host path.
    pub fn add_substream(
        &mut self,
        parent: &str,
        name: &[u8],
        kind: SubstreamKind,
        bytes: &[u8],
    ) -> Result<String> {
        self.healthy()?;
        if !self.streams.iter().any(|s| s.id == parent) && !self.folders.contains_key(parent) {
            return Err(malformed("substream parent is not writer-owned"));
        }
        if bytes.len() > 1024 {
            return Err(Error::Unsupported(
                "inline substreams are limited to 1 KiB; use a file stream for larger attributes"
                    .into(),
            ));
        }
        let id = identifier();
        let (name, raw) = normalized(name);
        let raw = raw
            .map(|s| {
                format!(
                    "l:fileNameRaw {}^^<http://www.w3.org/2001/XMLSchema#base64Binary>;",
                    literal(&s)
                )
            })
            .unwrap_or_default();
        let content = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.metadata.push_str(&format!("<{id}> a a:FileSubStream, a:Image; a:target <{parent}>; a:name {}; {raw} a:size {}; l:dataStream {}^^<http://www.w3.org/2001/XMLSchema#base64Binary>; a:hash {}^^a:SHA256 .\n", literal(&name), bytes.len(), literal(&content), literal(&hex(&Sha256::digest(bytes)))));
        let property = match kind {
            SubstreamKind::AlternateDataStream => "alternateDataStream",
            SubstreamKind::ExtendedAttribute => "extendedAttribute",
        };
        // RDF permits a subject to reappear. Never rewrite its previously emitted metadata.
        self.metadata
            .push_str(&format!("<{parent}> l:{property} <{id}> .\n"));
        Ok(id)
    }

    /// Sets the source path separator recorded with acquisition roots.
    pub fn set_path_separator(&mut self, separator: char) -> Result<()> {
        self.healthy()?;
        if !matches!(separator, '/' | '\\') {
            return Err(malformed("unsupported path separator"));
        }
        self.path_separator = separator;
        Ok(())
    }

    pub(super) fn finish_logical_metadata(&mut self) {
        for (id, (properties, children)) in &self.folders {
            self.metadata.push_str(&format!("<{id}> {properties}"));
            for child in children {
                self.metadata.push_str(&format!("; a:child <{child}>"));
            }
            self.metadata.push_str(" .\n");
        }
        if !self.roots.is_empty() {
            self.metadata.push_str(&format!(
                "<{}> a a:LogicalAcquisitionTask; l:pathSeparator {}",
                identifier(),
                literal(&self.path_separator.to_string())
            ));
            for root in &self.roots {
                self.metadata
                    .push_str(&format!("; a:filesystemRoot <{root}>"));
            }
            self.metadata.push_str(" .\n");
        }
    }
}
