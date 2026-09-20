//! Complete inventory of checks; unsupported constructions remain visible.
use super::integrity::{absent, compare};
use super::*;
use sha2::Sha512;

/// Byte provenance for one fully traversed resource. Counts sum to its size.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ByteCoverage {
    /// Bytes read from ZIP members or ImageStreams.
    pub stored: u64,
    /// Bytes represented by inline metadata or explicit symbolic ranges.
    pub described: u64,
    /// Bytes supplied by a map's default stream for unmapped addresses.
    pub gap_filled: u64,
}

/// Content traversal of one resource, kept separate from metadata checks.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResourceVerification {
    /// Selected image, file, map, or storage stream.
    pub resource: String,
    /// Linear digests, when every byte was read successfully.
    pub verification: Option<Verification>,
    /// Provenance of the logical bytes; absent if traversal could not finish.
    pub coverage: Option<ByteCoverage>,
    /// Diagnostic for a traversal that could not complete.
    pub error: Option<String>,
}

/// Container-wide inventory. No single matching digest substitutes for coverage.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContainerVerification {
    /// Metadata integrity report when its hash files can be interpreted.
    pub metadata: Option<MetadataVerification>,
    /// Metadata verification failure, if any.
    pub metadata_error: Option<String>,
    /// Every supported data-bearing resource, including files without hashes.
    pub resources: Vec<ResourceVerification>,
    /// Recorded linear, structural, block, and composite references.
    pub checks: Vec<IntegrityCheck>,
}

impl ContainerVerification {
    /// True only with matching metadata and all inventoried content checks.
    /// Metadata-only containers have no content checks to perform.
    /// Does not establish authenticity without independently trusted references.
    pub fn all_match(&self) -> bool {
        self.metadata
            .as_ref()
            .is_some_and(MetadataVerification::all_match)
            && self.metadata_error.is_none()
            && self
                .resources
                .iter()
                .all(|r| r.verification.is_some() && r.coverage.is_some())
            && self.checks.iter().all(|c| c.outcome == CheckOutcome::Match)
    }
}

fn algorithm(p: &Property) -> &str {
    let datatype = p.datatype.as_deref().unwrap_or("");
    datatype.strip_prefix(NS).unwrap_or(datatype)
}

fn declined(id: &str, p: &Property, error: &Error) -> IntegrityCheck {
    IntegrityCheck {
        resource: id.into(),
        reference_source: p.predicate.clone(),
        algorithm: algorithm(p).into(),
        expected: Some(p.value.clone()),
        computed: None,
        outcome: if matches!(error, Error::Unsupported(_)) {
            CheckOutcome::Unsupported
        } else {
            CheckOutcome::Unreadable
        },
        detail: Some(error.to_string()),
    }
}

impl Container {
    /// Checks metadata and all data resources, continuing after individual failures.
    /// Progress cancellation aborts the entire operation. Unknown digest semantics
    /// are reported as unsupported, never inferred from the digest length.
    pub fn verify_all(
        &mut self,
        expected_metadata_sha256: Option<&str>,
        mut progress: impl FnMut(&str, u64, u64) -> ControlFlow<()>,
    ) -> Result<ContainerVerification> {
        if progress("metadata", 0, 0).is_break() {
            return Err(Error::Aborted);
        }
        let (metadata, metadata_error) = match self.verify_metadata(expected_metadata_sha256) {
            Ok(value) => (Some(value), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let mut report = ContainerVerification {
            metadata,
            metadata_error,
            resources: Vec::new(),
            checks: Vec::new(),
        };
        let ids: Vec<_> = self
            .streams()?
            .into_iter()
            .filter(|s| !self.has_type(&s.id, "Folder") && !self.has_type(&s.id, "FolderImage"))
            .map(|s| s.id)
            .collect();
        let mut remaining = self.limits.verification_bytes;
        for id in &ids {
            let value = self.size(id).and_then(|size| {
                remaining = remaining
                    .checked_sub(size)
                    .ok_or_else(|| malformed("verification byte limit exceeded"))?;
                self.verify(id, |done, size| progress(id, done, size))
            });
            let (verification, coverage, error) = match value {
                Err(Error::Aborted) => return Err(Error::Aborted),
                Err(e) => (None, None, Some(e.to_string())),
                Ok(value) => match self.byte_coverage(id, 0, value.bytes_verified, &mut Vec::new())
                {
                    Ok(coverage) => (Some(value), Some(coverage), None),
                    Err(e) => (Some(value), None, Some(e.to_string())),
                },
            };
            let hashes: Vec<_> = self
                .properties(id)
                .filter(|p| is_property(&p.predicate, "hash"))
                .cloned()
                .collect();
            for p in &hashes {
                if algorithm(p).starts_with("blockMapHash") {
                    continue;
                }
                let computed = verification.as_ref().and_then(|v| match algorithm(p) {
                    "MD5" => Some(&v.md5),
                    "SHA1" => Some(&v.sha1),
                    "SHA256" => Some(&v.sha256),
                    "SHA512" => Some(&v.sha512),
                    "Blake2b" | "blake2b" => Some(&v.blake2b),
                    _ => None,
                });
                let mut check =
                    declined(id, p, &Error::Unsupported("unknown linear digest".into()));
                if let Some(value) = computed {
                    check.computed = Some(value.clone());
                    check.detail = None;
                    check.outcome = if p.value.len() != value.len()
                        || !p.value.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        CheckOutcome::Unreadable
                    } else if value.eq_ignore_ascii_case(&p.value) {
                        CheckOutcome::Match
                    } else {
                        CheckOutcome::Mismatch
                    };
                } else if verification.is_none() {
                    check.outcome = CheckOutcome::Unreadable;
                    check.detail = error.clone();
                }
                report.checks.push(check);
            }
            // Do not require a redundant linear hash on a Map or an ImageStream
            // when a composite protects it, but do retain missing references on files.
            if hashes.is_empty()
                && (self.has_type(id, "FileImage") || self.has_type(id, "FileSubStream"))
                && self.inline_data(id)?.is_none()
            {
                report.checks.push(absent(id, "linear file digest"));
            }
            report.resources.push(ResourceVerification {
                resource: id.clone(),
                verification,
                coverage,
                error,
            });
        }
        let subjects: Vec<_> = self.graph.keys().cloned().collect();
        for id in subjects {
            if progress(&id, 0, 0).is_break() {
                return Err(Error::Aborted);
            }
            let properties = self.graph[&id].clone();
            let block_hash_object = self.has_type(&id, "BlockHashes");
            for p in properties.iter().filter(|p| {
                is_property(&p.predicate, "hash")
                    && (block_hash_object || algorithm(p).starts_with("blockMapHash"))
                    || [
                        "mapHash",
                        "mapIdxHash",
                        "mapPointHash",
                        "mapPathHash",
                        "blockMapHash",
                        "imageStreamHash",
                        "imageStreamIndexHash",
                    ]
                    .iter()
                    .any(|key| is_property(&p.predicate, key))
            }) {
                let check = match self.structural_input(&id, p) {
                    Ok((bytes, alg)) => compare(&id, &p.predicate, &alg, &p.value, &bytes),
                    Err(e) => declined(&id, p, &e),
                };
                report.checks.push(check);
            }
            if self.has_type(&id, "Map")
                && !properties.iter().any(|p| {
                    is_property(&p.predicate, "hash")
                        || is_property(&p.predicate, "mapHash")
                        || is_property(&p.predicate, "blockMapHash")
                })
            {
                let path = self.path(&id)?;
                for (member, key) in [
                    ("map", "mapPointHash"),
                    ("idx", "mapIdxHash"),
                    ("mapPath", "mapPathHash"),
                ] {
                    if (member != "mapPath"
                        || self
                            .archive
                            .index_for_name(&format!("{path}/{member}"))
                            .is_some())
                        && !properties.iter().any(|p| is_property(&p.predicate, key))
                    {
                        report.checks.push(absent(&id, key));
                    }
                }
            }
            if self.has_type(&id, "ImageStream") {
                match self.check_blocks(&id, &mut remaining, &mut progress) {
                    Ok(checks) => {
                        if checks.is_empty()
                            && !self
                                .properties(&id)
                                .any(|p| is_property(&p.predicate, "hash"))
                        {
                            report
                                .checks
                                .push(absent(&id, "linear or block content digest"));
                        }
                        report.checks.extend(checks);
                    }
                    Err(Error::Aborted) => return Err(Error::Aborted),
                    Err(e) => {
                        let p = Property {
                            predicate: "block hashes".into(),
                            value: String::new(),
                            datatype: None,
                            language: None,
                        };
                        report.checks.push(declined(&id, &p, &e));
                    }
                }
            }
        }
        Ok(report)
    }

    fn byte_coverage(
        &mut self,
        id: &str,
        offset: u64,
        length: u64,
        visited: &mut Vec<String>,
    ) -> Result<ByteCoverage> {
        enter(id, visited)?;
        let result = (|| {
            let mut count = ByteCoverage::default();
            if id == format!("{NS}Zero")
                || id.starts_with(&format!("{NS}SymbolicStream"))
                || self.inline_data(id)?.is_some()
            {
                count.described = length;
            } else if let Some(target) = self.value(id, "dataStream")? {
                return self.byte_coverage(&target, offset, length, visited);
            } else if self.has_type(id, "Map") {
                self.load_map(id)?;
                let map = self.maps[id].clone();
                let mut position = offset;
                let end = offset
                    .checked_add(length)
                    .ok_or_else(|| malformed("coverage overflow"))?;
                while position < end {
                    let next = map.ranges.partition_point(|r| r.end <= position);
                    if let Some(range) = map.ranges.get(next).filter(|r| r.start <= position) {
                        let take = (range.end - position).min(end - position);
                        let child = self.byte_coverage(
                            &range.target,
                            range.offset + position - range.start,
                            take,
                            visited,
                        )?;
                        count.stored += child.stored;
                        count.described += child.described;
                        count.gap_filled += child.gap_filled;
                        position += take;
                    } else {
                        let take =
                            map.ranges.get(next).map_or(end, |r| r.start.min(end)) - position;
                        count.gap_filled += take;
                        position += take;
                    }
                }
            } else {
                count.stored = length;
            }
            Ok(count)
        })();
        visited.pop();
        result
    }

    fn block_names(&self, id: &str, suffix: &str) -> Result<Vec<String>> {
        let prefix = format!("{}/", self.path(id)?);
        let mut names: BTreeMap<u64, String> = BTreeMap::new();
        for name in self.archive.file_names() {
            let Some(tail) = name.strip_prefix(&prefix) else {
                continue;
            };
            let ordinal = tail
                .strip_suffix(&format!(".blockHash.{suffix}"))
                .or_else(|| tail.strip_suffix(&format!(".{suffix}")));
            if let Some(number) =
                ordinal.filter(|n| n.len() == 8 && n.bytes().all(|b| b.is_ascii_digit()))
            {
                let number = number
                    .parse()
                    .map_err(|_| malformed("invalid block bevy"))?;
                if names.insert(number, name.into()).is_some() {
                    return Err(malformed("ambiguous block-hash segment aliases"));
                }
            }
        }
        Ok(names.into_values().collect())
    }

    fn join_members(&mut self, names: &[String]) -> Result<Vec<u8>> {
        let mut result = Vec::new();
        for name in names {
            let limit = self.limits.member_bytes.saturating_sub(result.len() as u64);
            result.extend_from_slice(&member(&mut self.archive, name, limit)?);
        }
        Ok(result)
    }

    fn structural_input(&mut self, id: &str, p: &Property) -> Result<(Vec<u8>, String)> {
        let alg = algorithm(p);
        if self.has_type(id, "BlockHashes") {
            let (stream, suffix) = id
                .rsplit_once("/blockhash.")
                .ok_or_else(|| malformed("invalid BlockHashes resource"))?;
            let names = self.block_names(stream, suffix)?;
            if names.is_empty() {
                return Err(malformed("missing block hash members"));
            }
            return Ok((self.join_members(&names)?, alg.into()));
        }
        if is_property(&p.predicate, "imageStreamIndexHash") {
            let size = self.number(id, "size")?;
            let chunk = self.number(id, "chunkSize")?;
            let per = self.number(id, "chunksInSegment")?;
            if chunk == 0 || per == 0 {
                return Err(malformed("invalid chunk geometry"));
            }
            let count = size.div_ceil(chunk).div_ceil(per);
            if count > self.archive.len() as u64 {
                return Err(malformed("missing bevies"));
            }
            let path = self.path(id)?;
            let names: Vec<_> = (0..count).map(|n| format!("{path}/{n:08}.index")).collect();
            return Ok((self.join_members(&names)?, alg.into()));
        }
        if is_property(&p.predicate, "imageStreamHash") {
            return Err(Error::Unsupported(
                "imageStreamHash semantics require an independent vector".into(),
            ));
        }
        let map = if self.has_type(id, "Map") {
            id.to_owned()
        } else {
            self.value(id, "dataStream")?
                .ok_or_else(|| malformed("missing image map"))?
        };
        if !self.has_type(&map, "Map") {
            return Err(Error::Unsupported("composite digest requires a Map".into()));
        }
        let path = self.path(&map)?;
        let points = member(
            &mut self.archive,
            &format!("{path}/map"),
            self.limits.member_bytes,
        )?;
        let index = member(
            &mut self.archive,
            &format!("{path}/idx"),
            self.limits.member_bytes,
        )?;
        let path_name = format!("{path}/mapPath");
        let order = if self.archive.index_for_name(&path_name).is_some() {
            member(&mut self.archive, &path_name, self.limits.member_bytes)?
        } else {
            Vec::new()
        };
        for (key, bytes) in [
            ("mapPointHash", &points),
            ("mapIdxHash", &index),
            ("mapPathHash", &order),
        ] {
            if is_property(&p.predicate, key) {
                return Ok((bytes.clone(), alg.into()));
            }
        }
        if is_property(&p.predicate, "mapHash") {
            return Ok(([points, index, order].concat(), alg.into()));
        }
        let outer = match alg {
            "blockMapHashSHA512" | "SHA512" => "SHA512",
            "blockMapHashSHA256" | "SHA256" => "SHA256",
            _ => return Err(Error::Unsupported("block map digest algorithm".into())),
        };
        let digest = |bytes: &[u8]| -> Vec<u8> {
            if outer == "SHA512" {
                Sha512::digest(bytes).to_vec()
            } else {
                Sha256::digest(bytes).to_vec()
            }
        };
        let targets = std::str::from_utf8(&index).map_err(|_| malformed("map index UTF8"))?;
        let mut input = Vec::new();
        let mut seen = BTreeSet::new();
        for target in targets.lines() {
            if !self.has_type(target, "ImageStream") {
                if target == format!("{NS}Zero")
                    || target.starts_with(&format!("{NS}SymbolicStream"))
                {
                    continue;
                }
                return Err(Error::Unsupported(
                    "nested or external block map target".into(),
                ));
            }
            if self
                .value(target, "stored")?
                .is_some_and(|owner| owner != self.volume)
            {
                return Err(Error::Unsupported(
                    "cross-volume composite block map digest".into(),
                ));
            }
            if !seen.insert(target) {
                continue;
            }
            for suffix in ["md5", "sha1", "sha256", "sha512", "blake2b"] {
                let names = self.block_names(target, suffix)?;
                if !names.is_empty() {
                    input.extend(digest(&self.join_members(&names)?));
                }
            }
        }
        input.extend(digest(&points));
        input.extend(digest(&index));
        // Canonical v1 images include H(empty) when mapPath is absent.
        input.extend(digest(&order));
        Ok((input, outer.into()))
    }

    fn check_blocks(
        &mut self,
        id: &str,
        remaining: &mut u64,
        progress: &mut impl FnMut(&str, u64, u64) -> ControlFlow<()>,
    ) -> Result<Vec<IntegrityCheck>> {
        let mut checks = Vec::new();
        let size = self.number(id, "size")?;
        let chunk = self.number(id, "chunkSize")?;
        let per = self.number(id, "chunksInSegment")?;
        if chunk == 0 || chunk > self.limits.chunk_bytes || per == 0 {
            return Err(malformed("invalid block hash geometry"));
        }
        let count = size.div_ceil(chunk);
        let mut buffer = vec![0; chunk as usize];
        for (suffix, algorithm, width) in [
            ("md5", "MD5", 16),
            ("sha1", "SHA1", 20),
            ("sha256", "SHA256", 32),
            ("sha512", "SHA512", 64),
            ("blake2b", "Blake2b", 64),
        ] {
            let names = self.block_names(id, suffix)?;
            if names.is_empty() {
                continue;
            }
            if names.len() as u64 != count.div_ceil(per) {
                return Err(malformed("incomplete block hash bevy set"));
            }
            for (bevy, name) in names.iter().enumerate() {
                let ordinal = name.rsplit('/').next().unwrap()[..8]
                    .parse::<u64>()
                    .map_err(|_| malformed("block bevy number"))?;
                if ordinal != bevy as u64 {
                    return Err(malformed("block hash bevy gap"));
                }
                let recorded = member(&mut self.archive, name, self.limits.member_bytes)?;
                let first = bevy as u64 * per;
                let blocks = (count - first).min(per);
                if recorded.len() as u64 != blocks * width {
                    return Err(malformed("block hash count mismatch"));
                }
                let mut check = absent(id, name);
                check.algorithm = algorithm.into();
                check.outcome = CheckOutcome::Match;
                check.detail = None;
                check.expected = Some(format!("{blocks} block digests"));
                check.computed = check.expected.clone();
                for n in 0..blocks {
                    let offset = (first + n) * chunk;
                    if progress(id, offset, size).is_break() {
                        return Err(Error::Aborted);
                    }
                    let take = (size - offset).min(chunk) as usize;
                    *remaining = remaining
                        .checked_sub(take as u64)
                        .ok_or_else(|| malformed("verification byte limit exceeded"))?;
                    self.read_at(id, &mut buffer[..take], offset)?;
                    let start = (n * width) as usize;
                    let expected = hex(&recorded[start..start + width as usize]);
                    let value = compare(id, name, algorithm, &expected, &buffer[..take]);
                    if value.outcome != CheckOutcome::Match {
                        check.outcome = value.outcome;
                        if check.detail.is_none() {
                            check.detail = Some(format!(
                                "first mismatch at chunk {} (offset {offset})",
                                first + n
                            ));
                        }
                    }
                }
                checks.push(check);
            }
        }
        Ok(checks)
    }
}
