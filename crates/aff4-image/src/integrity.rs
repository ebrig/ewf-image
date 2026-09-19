//! Metadata integrity over exact ZIP member bytes, never reserialized RDF.
use super::*;
use sha2::Sha512;

/// Result of an individual integrity check. A missing reference is not a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Computed bytes match the supplied reference.
    Match,
    /// Computed bytes differ from the supplied reference.
    Mismatch,
    /// No reference was recorded.
    Missing,
    /// This implementation cannot perform the requested check.
    Unsupported,
    /// Invalid reference syntax or evidence that could not be read.
    Unreadable,
}

/// One explicitly scoped integrity comparison.
#[derive(Debug, Clone)]
pub struct IntegrityCheck {
    /// Member or evidence resource whose bytes are covered.
    pub resource: String,
    /// Location of the reference, or `external` for a caller-supplied digest.
    pub reference_source: String,
    /// Algorithm name or unsupported datatype IRI.
    pub algorithm: String,
    /// Recorded reference when present.
    pub expected: Option<String>,
    /// Independently computed value when available.
    pub computed: Option<String>,
    /// Comparison result.
    pub outcome: CheckOutcome,
    /// Diagnostic detail for declined or failed checks.
    pub detail: Option<String>,
}

/// Metadata checks, separate from file content verification and authenticity.
#[derive(Debug, Clone)]
pub struct MetadataVerification {
    /// SHA256 of information.turtle, suitable for an external evidence record.
    pub sha256: String,
    /// All available internal and optional external comparisons.
    pub checks: Vec<IntegrityCheck>,
}

impl MetadataVerification {
    /// True only when every requested check has a reference and matches.
    /// Internal matches alone do not establish independent authenticity.
    pub fn all_match(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.outcome == CheckOutcome::Match)
    }
}

pub(super) fn compare(
    resource: &str,
    source: &str,
    algorithm: &str,
    expected: &str,
    bytes: &[u8],
) -> IntegrityCheck {
    let computed = match algorithm {
        "MD5" => Some(hex(&Md5::digest(bytes))),
        "SHA1" => Some(hex(&Sha1::digest(bytes))),
        "SHA256" => Some(hex(&Sha256::digest(bytes))),
        "SHA512" => Some(hex(&Sha512::digest(bytes))),
        "Blake2b" | "BLAKE2B" | "blake2b" => {
            Some(hex(&<blake2::Blake2b512 as blake2::Digest>::digest(bytes)))
        }
        _ => None,
    };
    let (outcome, detail) = match &computed {
        None => (
            CheckOutcome::Unsupported,
            Some("unsupported digest algorithm".into()),
        ),
        Some(value)
            if value.len() != expected.len()
                || !expected.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            (
                CheckOutcome::Unreadable,
                Some("invalid hexadecimal digest".into()),
            )
        }
        Some(value) if value.eq_ignore_ascii_case(expected) => (CheckOutcome::Match, None),
        Some(_) => (CheckOutcome::Mismatch, None),
    };
    IntegrityCheck {
        resource: resource.into(),
        reference_source: source.into(),
        algorithm: algorithm.into(),
        expected: Some(expected.into()),
        computed,
        outcome,
        detail,
    }
}

pub(super) fn absent(resource: &str, source: &str) -> IntegrityCheck {
    IntegrityCheck {
        resource: resource.into(),
        reference_source: source.into(),
        algorithm: String::new(),
        expected: None,
        computed: None,
        outcome: CheckOutcome::Missing,
        detail: Some("no recorded digest".into()),
    }
}

impl Container {
    /// Verifies exact metadata bytes using draft RDF and legacy Gemino JSON hashes.
    /// Both representations are checked when present; conflicts cannot be hidden
    /// by preferring one. Imported metadata is checked against its RDF references.
    /// `expected_sha256` is an independently recorded information.turtle digest.
    /// This does not hash the ZIP archive or verify evidence streams/signatures.
    pub fn verify_metadata(
        &mut self,
        expected_sha256: Option<&str>,
    ) -> Result<MetadataVerification> {
        let primary = member(
            &mut self.archive,
            "information.turtle",
            self.limits.metadata_bytes,
        )?;
        let sha256 = hex(&Sha256::digest(&primary));
        let mut checks = Vec::new();
        if let Some(expected) = expected_sha256 {
            checks.push(compare(
                "information.turtle",
                "external",
                "SHA256",
                expected,
                &primary,
            ));
        }
        let mut found = false;
        if self
            .archive
            .index_for_name("information.turtle.hashes")
            .is_some()
        {
            found = true;
            let bytes = member(
                &mut self.archive,
                "information.turtle.hashes",
                self.limits.metadata_bytes,
            )?;
            let mut count = 0;
            for triple in TurtleParser::new().for_reader(bytes.as_slice()) {
                count += 1;
                if count > self.limits.triples {
                    return Err(malformed("hash reference triple limit exceeded"));
                }
                let triple = triple.map_err(|e| malformed(format!("metadata hashes: {e}")))?;
                let NamedOrBlankNode::NamedNode(subject) = triple.subject else {
                    return Err(malformed("hash subject must be an IRI"));
                };
                let name =
                    storage_name(&self.archive, &self.volume, subject.as_str(), self.version)?;
                if name != "information.turtle" || !is_property(triple.predicate.as_str(), "hash") {
                    return Err(malformed("unexpected metadata hash statement"));
                }
                let Term::Literal(value) = triple.object else {
                    return Err(malformed("hash must be a typed literal"));
                };
                let datatype = value.datatype();
                let algorithm = datatype
                    .as_str()
                    .strip_prefix(NS)
                    .unwrap_or(datatype.as_str());
                let mut check = compare(
                    &name,
                    "information.turtle.hashes",
                    algorithm,
                    value.value(),
                    &primary,
                );
                if matches!(algorithm, "MD5" | "SHA1") {
                    check.outcome = CheckOutcome::Unsupported;
                    check.detail =
                        Some("draft metadata integrity requires SHA256 or stronger".into());
                }
                checks.push(check);
            }
            if count == 0 {
                checks.push(absent("information.turtle", "information.turtle.hashes"));
            }
        }
        if self.archive.index_for_name("container.hashes").is_some() {
            found = true;
            let bytes = member(
                &mut self.archive,
                "container.hashes",
                self.limits.metadata_bytes,
            )?;
            // Reject duplicate keys rather than allowing JSON's last-value wins.
            let values: UniqueHashes = serde_json::from_slice(&bytes)
                .map_err(|e| malformed(format!("container.hashes: {e}")))?;
            if values.0.is_empty() {
                checks.push(absent("information.turtle", "container.hashes"));
            }
            for (algorithm, expected) in values.0 {
                checks.push(compare(
                    "information.turtle",
                    "container.hashes",
                    &algorithm.to_ascii_uppercase(),
                    &expected,
                    &primary,
                ));
            }
        }
        if !found {
            checks.push(absent("information.turtle", "metadata hash files"));
        }
        for (name, opened_hash) in &self.metadata_members {
            let bytes = if name == "information.turtle" {
                primary.clone()
            } else {
                member(&mut self.archive, name, self.limits.metadata_bytes)?
            };
            if hex(&Sha256::digest(&bytes)) != *opened_hash {
                return Err(malformed("metadata changed after opening"));
            }
            if name == "information.turtle" {
                continue;
            }
            // Only the primary store can anchor imported metadata. A hash in
            // an imported file must never authenticate that same file.
            let mut refs = Vec::new();
            for triple in TurtleParser::new().for_reader(primary.as_slice()) {
                let triple = triple.map_err(|e| malformed(e.to_string()))?;
                if !is_property(triple.predicate.as_str(), "hash") {
                    continue;
                }
                if let NamedOrBlankNode::NamedNode(subject) = triple.subject
                    && storage_name(&self.archive, &self.volume, subject.as_str(), self.version)
                        .ok()
                        .as_ref()
                        == Some(name)
                    && let Term::Literal(value) = triple.object
                {
                    refs.push((
                        value.datatype().as_str().to_owned(),
                        value.value().to_owned(),
                    ));
                }
            }
            if refs.is_empty() {
                checks.push(absent(name, "RDF metadata"));
            }
            for (datatype, value) in refs {
                checks.push(compare(
                    name,
                    "information.turtle",
                    datatype.strip_prefix(NS).unwrap_or(&datatype),
                    &value,
                    &bytes,
                ));
            }
        }
        Ok(MetadataVerification { sha256, checks })
    }
}

struct UniqueHashes(BTreeMap<String, String>);
impl<'de> serde::Deserialize<'de> for UniqueHashes {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueHashes;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a unique algorithm-to-digest dictionary")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if values.insert(key.to_ascii_uppercase(), value).is_some() {
                        return Err(serde::de::Error::custom("duplicate hash algorithm"));
                    }
                }
                Ok(UniqueHashes(values))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
