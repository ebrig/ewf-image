//! Streaming metadata inventory without retaining the RDF graph.
use super::*;

/// Work performed by a streaming metadata scan. Triples include repeated subjects.
#[derive(Debug, Clone, Default)]
pub struct MetadataScan {
    /// Total triples delivered, including imported stores.
    pub triples: usize,
    /// Number of metadata ZIP members parsed.
    pub stores: usize,
    /// Uncompressed bytes of those members.
    pub bytes: u64,
}

impl Container {
    /// Streams primary and imported RDF statements without constructing a graph.
    /// Subject order is arbitrary; callers must not assume adjacent statements
    /// describe a complete object. The callback receives source-member provenance.
    /// This is an inventory operation, not content or metadata verification.
    /// ZIP central-directory allocation is still performed by the ZIP dependency.
    pub fn scan_metadata(
        path: impl AsRef<Path>,
        limits: Limits,
        mut visit: impl FnMut(&str, &str, &Property) -> ControlFlow<()>,
    ) -> Result<MetadataScan> {
        let mut archive = ZipArchive::new(File::open(path)?)?;
        let mut names = BTreeSet::new();
        for name in archive.file_names() {
            if !names.insert(name.to_owned()) {
                return Err(malformed("duplicate ZIP member name"));
            }
        }
        let description = if archive.index_for_name("container.description").is_some() {
            String::from_utf8(member(&mut archive, "container.description", 4096)?)
                .map_err(|_| malformed("volume UTF8"))?
        } else {
            std::str::from_utf8(archive.comment())
                .map_err(|_| malformed("volume UTF8"))?
                .to_owned()
        };
        let volume = description.trim_end_matches('\0').trim();
        if !volume.starts_with("aff4://") {
            return Err(malformed("invalid AFF4 volume"));
        }
        let mut pending = vec!["information.turtle".to_owned()];
        let mut seen = BTreeSet::new();
        let mut result = MetadataScan::default();
        while let Some(name) = pending.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let file = archive.by_name(&name)?;
            let remaining = limits.metadata_bytes.saturating_sub(result.bytes);
            if file.size() > remaining {
                return Err(malformed("metadata byte limit exceeded"));
            }
            result.bytes += file.size();
            result.stores += 1;
            for triple in TurtleParser::new().for_reader(file.take(remaining.saturating_add(1))) {
                if result.triples >= limits.triples {
                    return Err(malformed("RDF triple limit exceeded"));
                }
                let triple = triple.map_err(|e| malformed(e.to_string()))?;
                result.triples += 1;
                let subject = match triple.subject {
                    NamedOrBlankNode::NamedNode(n) => n.into_string(),
                    NamedOrBlankNode::BlankNode(n) => format!("_:{n}"),
                };
                let (value, datatype, language) = match triple.object {
                    Term::NamedNode(n) => (n.into_string(), None, None),
                    Term::BlankNode(n) => (format!("_:{n}"), None, None),
                    Term::Literal(l) => (
                        l.value().to_owned(),
                        Some(l.datatype().as_str().to_owned()),
                        l.language().map(str::to_owned),
                    ),
                };
                let property = Property {
                    predicate: triple.predicate.into_string(),
                    value,
                    datatype,
                    language,
                };
                if is_property(&property.predicate, "imports") {
                    if property.datatype.is_some() {
                        return Err(malformed("metadata import must be an IRI"));
                    }
                    let prefix = format!("{volume}/");
                    let imported = property
                        .value
                        .strip_prefix(&prefix)
                        .ok_or_else(|| Error::Unsupported("nonlocal metadata import".into()))?;
                    pending.push(imported.to_owned());
                }
                if visit(&name, &subject, &property).is_break() {
                    return Err(Error::Aborted);
                }
            }
        }
        Ok(result)
    }
}
