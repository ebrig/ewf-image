//! Incremental collection accounting; finalized ZIP verification remains required.
use super::*;

/// Collection discovery bounds and the reader budgets used for staged verification.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CollectionLimits {
    /// Reader budgets; metadata/triples and ZIP entries are checked incrementally.
    pub reader: crate::Limits,
    /// Maximum discovered entries, including root, excluded and skipped entries.
    pub entries: usize,
    /// Maximum source depth; the root has depth zero.
    pub depth: usize,
}

impl Default for CollectionLimits {
    fn default() -> Self {
        Self {
            reader: crate::Limits::default(),
            entries: 100_000,
            depth: 127,
        }
    }
}

pub(super) fn check(resource: &'static str, required: u64, limit: u64) -> Result<()> {
    if required > limit {
        return Err(Error::ResourceLimit {
            resource,
            required,
            limit,
        });
    }
    Ok(())
}

pub(super) struct Budget<'a> {
    limits: &'a CollectionLimits,
    offset: usize,
    roots: usize,
    metadata: u64,
    triples: u64,
    bytes: u64,
    discovered: usize,
}

impl<'a> Budget<'a> {
    pub(super) fn new(writer: &Writer, limits: &'a CollectionLimits) -> Result<Self> {
        let mut budget = Self {
            limits,
            offset: 0,
            roots: 0,
            metadata: 0,
            triples: 0,
            bytes: 0,
            discovered: 0,
        };
        budget.observe(writer)?;
        // Include deferred folder records and relationships already in the writer.
        for (id, (properties, children)) in &writer.folders {
            budget.fragment(&format!("<{id}> {properties} .\n"))?;
            for child in children {
                budget.charge(format!("; a:child <{child}>").len() as u64, 1)?;
            }
        }
        // Reserve the final collection record, including worst-case decimal counts.
        budget.fragment(&super::collect::task_metadata(&CollectionReport {
            files: u64::MAX,
            bytes: u64::MAX,
            ..Default::default()
        }))?;
        for stream in &writer.streams {
            budget.bytes = budget
                .bytes
                .checked_add(stream.size)
                .ok_or_else(|| malformed("collection byte count overflow"))?;
        }
        budget.file(writer, None)?;
        Ok(budget)
    }

    pub(super) fn discover(&mut self, depth: usize) -> Result<()> {
        check("collection_depth", depth as u64, self.limits.depth as u64)?;
        self.discovered = self
            .discovered
            .checked_add(1)
            .ok_or_else(|| malformed("collection entry count overflow"))?;
        check(
            "collection_entries",
            self.discovered as u64,
            self.limits.entries as u64,
        )
    }

    fn charge(&mut self, bytes: u64, triples: u64) -> Result<()> {
        self.metadata = self
            .metadata
            .checked_add(bytes)
            .ok_or_else(|| malformed("metadata byte count overflow"))?;
        self.triples = self
            .triples
            .checked_add(triples)
            .ok_or_else(|| malformed("metadata triple count overflow"))?;
        check(
            "metadata_bytes",
            self.metadata,
            self.limits.reader.metadata_bytes,
        )?;
        check("triples", self.triples, self.limits.reader.triples as u64)
    }

    fn fragment(&mut self, text: &str) -> Result<()> {
        self.charge(text.len() as u64, 0)?;
        let prefixes = b"@prefix a: <http://aff4.org/Schema#> .\n@prefix l: <https://aff4.org/Schema/2022/#> .\n";
        for triple in
            oxttl::TurtleParser::new().for_reader(prefixes.as_slice().chain(text.as_bytes()))
        {
            triple.map_err(|error| malformed(format!("writer metadata: {error}")))?;
            self.charge(0, 1)?;
        }
        Ok(())
    }

    fn observe(&mut self, writer: &Writer) -> Result<()> {
        self.fragment(&writer.metadata[self.offset..])?;
        self.offset = writer.metadata.len();
        if self.roots == 0 && !writer.roots.is_empty() {
            // UUID length is fixed; these are the exact final header byte count.
            let header = format!(
                "<{}> a a:LogicalAcquisitionTask; l:pathSeparator {} .\n",
                identifier(),
                oxrdf::Literal::new_simple_literal(writer.path_separator.to_string())
            );
            self.fragment(&header)?;
        }
        for root in &writer.roots[self.roots..] {
            self.charge(format!("; a:filesystemRoot <{root}>").len() as u64, 1)?;
        }
        self.roots = writer.roots.len();
        Ok(())
    }

    pub(super) fn added(
        &mut self,
        writer: &Writer,
        id: &str,
        folder: bool,
        parent: Option<&str>,
    ) -> Result<()> {
        self.observe(writer)?;
        if folder {
            let properties = &writer.folders[id].0;
            self.fragment(&format!("<{id}> {properties} .\n"))?;
        }
        if parent.is_some() {
            self.charge(format!("; a:child <{id}>").len() as u64, 1)?;
        }
        Ok(())
    }

    pub(super) fn issue(&mut self, issue: &CollectionIssue) -> Result<()> {
        self.charge(super::collect::issue_metadata(issue).len() as u64, 1)
    }

    pub(super) fn file(&mut self, writer: &Writer, size: Option<u64>) -> Result<()> {
        let mut entries = writer
            .archive_entries
            .checked_add(2)
            .ok_or_else(|| malformed("archive entry count overflow"))?; // Final RDF and digest sidecar.
        if let Some(size) = size {
            let members = if size > writer.logical_zip_threshold {
                size.div_ceil(u64::from(writer.options.chunk_bytes))
                    .div_ceil(u64::from(writer.options.chunks_per_bevy))
                    .checked_mul(4)
                    .ok_or_else(|| malformed("archive entry count overflow"))?
            } else {
                1
            };
            entries = entries
                .checked_add(members)
                .ok_or_else(|| malformed("archive entry count overflow"))?;
            self.bytes = self
                .bytes
                .checked_add(size)
                .ok_or_else(|| malformed("collection byte count overflow"))?;
        }
        check(
            "archive_entries",
            entries,
            self.limits.reader.archive_entries as u64,
        )?;
        // This is a lower bound: structural/block verification can traverse bytes again.
        check(
            "verification_bytes",
            self.bytes,
            self.limits.reader.verification_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_for_deferred_hierarchy_and_collection_issues() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("folder")).unwrap();
        fs::write(source.join("folder/file"), b"abc").unwrap();
        fs::write(source.join("skipped"), []).unwrap();
        let mut writer = Writer::create(
            dir.path().join("case.aff4"),
            Profile::Logical,
            WriteOptions::default(),
        )
        .unwrap();
        writer
            .add_case_metadata(&CaseMetadata {
                notes: "quotes \" and slash \\".into(),
                ..Default::default()
            })
            .unwrap();
        writer
            .add_directory_tree(
                &source,
                &CollectionOptions {
                    exclude: vec!["skipped".into()],
                    ..Default::default()
                },
                |_, _, _| ControlFlow::Continue(()),
            )
            .unwrap();
        let limits = CollectionLimits::default();
        let budget = Budget::new(&writer, &limits).unwrap();
        let reserved = super::super::collect::task_metadata(&CollectionReport {
            files: u64::MAX,
            bytes: u64::MAX,
            ..Default::default()
        });
        writer.finish_logical_metadata();
        assert_eq!(
            budget.metadata - reserved.len() as u64,
            writer.metadata.len() as u64
        );
        let triples = oxttl::TurtleParser::new()
            .for_reader(writer.metadata.as_bytes())
            .inspect(|item| assert!(item.is_ok()))
            .count();
        assert_eq!(budget.triples - 4, triples as u64);
    }
}
