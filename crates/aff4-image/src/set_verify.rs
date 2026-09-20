//! Verification retains the volume containing each reference as its context.
use super::super::integrity::{absent, compare};
use super::super::verify_all::{algorithm, declined};
use super::*;

/// Integrity checks recorded in one supplied volume's metadata context.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SetVolumeVerification {
    /// Declared volume identifier.
    pub volume: String,
    /// Caller-supplied path.
    pub path: PathBuf,
    /// Exact-byte metadata integrity; missing references remain explicit.
    pub metadata: Option<MetadataVerification>,
    /// Failure to interpret metadata integrity references.
    pub metadata_error: Option<String>,
    /// Linear, block, map and composite checks from this volume.
    pub checks: Vec<IntegrityCheck>,
}

/// Selected physical image plus all supplied-volume integrity references.
/// Internal matches do not establish independent authenticity.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SetVerification {
    /// Selected primary image.
    pub image: String,
    /// Complete assembled-image digest; absent on an incomplete read.
    pub assembled: Option<SetDigest>,
    /// Assembled-image read or geometry failure.
    pub image_error: Option<String>,
    /// Stored, explicitly described, and implicit gap bytes of the image.
    pub coverage: Option<ByteCoverage>,
    /// Contributor volume identifiers in caller-supplied stripe order.
    pub stripe_order: Vec<String>,
    /// Every owned ImageStream, verified once for its linear content.
    pub streams: Vec<ResourceVerification>,
    /// References stay scoped to the volume that records them.
    pub volumes: Vec<SetVolumeVerification>,
}

impl SetVerification {
    /// True only for complete bytes and matching metadata/content references.
    pub fn all_match(&self) -> bool {
        self.assembled
            .as_ref()
            .is_some_and(|v| v.external_match != Some(false))
            && self.image_error.is_none()
            && self.coverage.is_some()
            && self
                .streams
                .iter()
                .all(|r| r.error.is_none() && r.verification.is_some())
            && self.volumes.iter().all(|v| {
                v.metadata_error.is_none()
                    && v.metadata
                        .as_ref()
                        .is_some_and(MetadataVerification::all_match)
                    && v.checks.iter().all(|c| c.outcome == CheckOutcome::Match)
            })
    }
}

fn linear_check(id: &str, p: &Property, value: Option<&Verification>) -> IntegrityCheck {
    let mut check = declined(id, p, &Error::Unsupported("linear digest algorithm".into()));
    let Some(v) = value else {
        check.outcome = CheckOutcome::Unreadable;
        check.detail = Some("resource content was not completely verified".into());
        return check;
    };
    let computed = match algorithm(p) {
        "MD5" => &v.md5,
        "SHA1" => &v.sha1,
        "SHA256" => &v.sha256,
        "SHA512" => &v.sha512,
        "Blake2b" | "blake2b" => &v.blake2b,
        _ => return check,
    };
    check.computed = Some(computed.clone());
    check.detail = None;
    check.outcome =
        if p.value.len() != computed.len() || !p.value.bytes().all(|b| b.is_ascii_hexdigit()) {
            CheckOutcome::Unreadable
        } else if computed.eq_ignore_ascii_case(&p.value) {
            CheckOutcome::Match
        } else {
            CheckOutcome::Mismatch
        };
    check
}

impl VolumeSet {
    /// Verifies the selected image, every owned stream, and each volume's recorded
    /// integrity structures. Missing/unsupported references prevent all_match.
    /// Striped roots use the explicit supplied order; no ordering is guessed.
    /// Other images' linear/root hashes are reported as unsupported. Metadata
    /// references are internal; expected_image_sha256 is an external media digest.
    pub fn verify_full(
        &mut self,
        image: &str,
        expected_image_sha256: Option<&str>,
        mut progress: impl FnMut(&str, u64, u64) -> ControlFlow<()>,
    ) -> Result<SetVerification> {
        let mut remaining = self.volumes[0].limits.verification_bytes;
        let mut report = SetVerification {
            image: image.into(),
            assembled: None,
            image_error: None,
            coverage: None,
            stripe_order: self
                .volumes
                .iter()
                .filter(|v| v.has_type(image, "DiskImage"))
                .map(|v| v.volume.clone())
                .collect(),
            streams: Vec::new(),
            volumes: Vec::new(),
        };
        let result = self.size(image).and_then(|size| {
            remaining = remaining
                .checked_sub(size)
                .ok_or_else(|| malformed("verification byte limit exceeded"))?;
            self.scan_image(image, expected_image_sha256, true, |done, size| {
                progress(image, done, size)
            })
        });
        let image_hashes = match result {
            Err(Error::Aborted) => return Err(Error::Aborted),
            Err(e) => {
                report.image_error = Some(e.to_string());
                None
            }
            Ok((digest, hashes)) => {
                let (map, size) = self.image_map(image)?;
                let mut coverage = ByteCoverage::default();
                let mut end = 0;
                for range in &map.ranges {
                    coverage.gap_filled += range.start - end;
                    if is_symbolic(&range.target) {
                        coverage.described += range.end - range.start;
                    } else {
                        coverage.stored += range.end - range.start;
                    }
                    end = range.end;
                }
                coverage.gap_filled += size - end;
                report.coverage = Some(coverage);
                report.assembled = Some(digest);
                hashes
            }
        };
        let owners = self.owners.clone();
        let mut hashes = BTreeMap::new();
        for (id, &owner) in &owners {
            if progress(id, 0, 0).is_break() {
                return Err(Error::Aborted);
            }
            let value = self.volumes[owner].size(id).and_then(|size| {
                remaining = remaining
                    .checked_sub(size)
                    .ok_or_else(|| malformed("verification byte limit exceeded"))?;
                self.volumes[owner].verify(id, |done, size| progress(id, done, size))
            });
            let (verification, coverage, error) = match value {
                Err(Error::Aborted) => return Err(Error::Aborted),
                Err(e) => (None, None, Some(e.to_string())),
                Ok(v) => {
                    let coverage = ByteCoverage {
                        stored: v.bytes_verified,
                        ..Default::default()
                    };
                    hashes.insert(id.clone(), v.clone());
                    (Some(v), Some(coverage), None)
                }
            };
            report.streams.push(ResourceVerification {
                resource: id.clone(),
                verification,
                coverage,
                error,
            });
            self.volumes[owner].cache = None;
            self.volumes[owner].index_cache = None;
        }
        for context in 0..self.volumes.len() {
            let volume = &mut self.volumes[context];
            if progress(&volume.volume, 0, 0).is_break() {
                return Err(Error::Aborted);
            }
            let (metadata, metadata_error) = match volume.verify_metadata(None) {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let mut result = SetVolumeVerification {
                volume: volume.volume.clone(),
                path: self.paths[context].clone(),
                metadata,
                metadata_error,
                checks: Vec::new(),
            };
            let subjects: Vec<_> = volume.graph.keys().cloned().collect();
            let local: BTreeSet<_> = owners
                .iter()
                .filter(|(_, n)| **n == context)
                .map(|(id, _)| id.clone())
                .collect();
            for id in subjects {
                if progress(&id, 0, 0).is_break() {
                    return Err(Error::Aborted);
                }
                let properties = self.volumes[context].graph[&id].clone();
                for p in properties.iter().filter(|p| is_integrity_property(p)) {
                    let is_hash = is_property(&p.predicate, "hash");
                    let is_root = is_hash
                        && algorithm(p).starts_with("blockMapHash")
                        && !self.volumes[context].has_type(&id, "Map");
                    let check = if is_root {
                        if id != image {
                            declined(&id, p, &Error::Unsupported("unselected image root".into()))
                        } else {
                            match self.root_input(image, p) {
                                Ok((input, alg)) => {
                                    compare(&id, &p.predicate, &alg, &p.value, &input)
                                }
                                Err(e) => declined(&id, p, &e),
                            }
                        }
                    } else if is_hash
                        && !self.volumes[context].has_type(&id, "BlockHashes")
                        && !algorithm(p).starts_with("blockMapHash")
                    {
                        let value = if id == image {
                            image_hashes.as_ref()
                        } else {
                            hashes.get(&id)
                        };
                        linear_check(&id, p, value)
                    } else {
                        match self.volumes[context].structural_input_scoped(&id, p, Some(&local)) {
                            Ok((input, alg)) => compare(&id, &p.predicate, &alg, &p.value, &input),
                            Err(e) => declined(&id, p, &e),
                        }
                    };
                    result.checks.push(check);
                }
                if self.volumes[context].has_type(&id, "ImageStream") {
                    let blocks =
                        match owners.get(&id).copied() {
                            None => Err(malformed("stream has no owning volume")),
                            Some(owner) if owner == context => self.volumes[context]
                                .check_blocks_from(&id, None, &mut remaining, &mut progress),
                            Some(owner) => {
                                let (reference, data) =
                                    two_volumes(&mut self.volumes, context, owner);
                                reference.check_blocks_from(
                                    &id,
                                    Some(data),
                                    &mut remaining,
                                    &mut progress,
                                )
                            }
                        };
                    match blocks {
                        Err(Error::Aborted) => return Err(Error::Aborted),
                        Err(e) => {
                            result
                                .checks
                                .push(declined(&id, &placeholder("block hashes"), &e))
                        }
                        Ok(checks) => {
                            if checks.is_empty()
                                && local.contains(&id)
                                && !properties.iter().any(|p| is_property(&p.predicate, "hash"))
                            {
                                result
                                    .checks
                                    .push(absent(&id, "linear or block content digest"));
                            }
                            result.checks.extend(checks);
                        }
                    }
                    if let Some(&owner) = owners.get(&id) {
                        self.volumes[owner].cache = None;
                        self.volumes[owner].index_cache = None;
                    }
                }
                if self.volumes[context].has_type(&id, "Map") {
                    if let Err(e) = self.validate_context_map(context, &id) {
                        result
                            .checks
                            .push(declined(&id, &placeholder("map geometry"), &e));
                    }
                    if !properties.iter().any(|p| {
                        ["hash", "mapHash", "blockMapHash"]
                            .iter()
                            .any(|key| is_property(&p.predicate, key))
                    }) {
                        for key in ["mapPointHash", "mapIdxHash"] {
                            if !properties.iter().any(|p| is_property(&p.predicate, key)) {
                                result.checks.push(absent(&id, key));
                            }
                        }
                    }
                }
                if ["FileImage", "FileSubStream"]
                    .iter()
                    .any(|kind| self.volumes[context].has_type(&id, kind))
                {
                    result.checks.push(declined(
                        &id,
                        &placeholder("logical resource"),
                        &Error::Unsupported(
                            "logical resources in physical volume-set verification".into(),
                        ),
                    ));
                }
                if id == image && !properties.iter().any(|p| is_property(&p.predicate, "hash")) {
                    result.checks.push(absent(image, "image digest"));
                }
            }
            // Clear all payload caches before moving to the next context.
            for volume in &mut self.volumes {
                volume.cache = None;
                volume.index_cache = None;
                volume.maps.clear();
            }
            report.volumes.push(result);
        }
        Ok(report)
    }

    fn validate_context_map(&mut self, context: usize, id: &str) -> Result<()> {
        self.volumes[context].load_map(id)?;
        let map = self.volumes[context].maps[id].clone();
        let mut end = 0;
        for range in &map.ranges {
            if range.start > end {
                self.validate_set_range(&map.gap, end, range.start - end)?;
            }
            self.validate_set_range(&range.target, range.offset, range.end - range.start)?;
            end = range.end;
        }
        let size = self.volumes[context].size(id)?;
        if end < size {
            self.validate_set_range(&map.gap, end, size - end)?;
        }
        Ok(())
    }

    fn validate_set_range(&mut self, target: &str, offset: u64, length: u64) -> Result<()> {
        if is_symbolic(target) {
            return self.volumes[0].read_inner(target, &mut [], 0, &mut Vec::new());
        }
        let owner = self
            .owners
            .get(target)
            .copied()
            .ok_or_else(|| malformed("map target has no owner"))?;
        let size = self.volumes[owner].size(target)?;
        if offset.checked_add(length).is_none_or(|end| end > size) {
            return Err(malformed("map exceeds owned stream"));
        }
        Ok(())
    }

    fn root_input(&mut self, image: &str, p: &Property) -> Result<(Vec<u8>, String)> {
        let alg = match algorithm(p) {
            "blockMapHashSHA512" => "SHA512",
            "blockMapHashSHA256" => "SHA256",
            _ => return Err(Error::Unsupported("striped root algorithm".into())),
        };
        let contexts: Vec<_> = self
            .volumes
            .iter()
            .enumerate()
            .filter(|(_, v)| v.has_type(image, "DiskImage"))
            .map(|(n, _)| n)
            .collect();
        if contexts.is_empty() {
            return Err(malformed("image has no declaring volume"));
        }
        if contexts.len() > 1 && alg != "SHA512" {
            return Err(Error::Unsupported(
                "SHA256 striped root requires an independent vector".into(),
            ));
        }
        let mut result = Vec::new();
        for &context in &contexts {
            let map = self.volumes[context]
                .value(image, "dataStream")?
                .ok_or_else(|| malformed("stripe has no local map"))?;
            let local = self
                .owners
                .iter()
                .filter(|(_, n)| **n == context)
                .map(|(id, _)| id.clone())
                .collect();
            let property = Property {
                predicate: format!("{NS}blockMapHash"),
                value: String::new(),
                datatype: Some(format!("{NS}{alg}")),
                language: None,
            };
            let (input, _) =
                self.volumes[context].structural_input_scoped(&map, &property, Some(&local))?;
            if contexts.len() == 1 {
                return Ok((input, alg.into()));
            }
            if alg == "SHA512" {
                result.extend(sha2::Sha512::digest(&input));
            } else {
                result.extend(Sha256::digest(&input));
            }
        }
        Ok((result, alg.into()))
    }
}

fn is_integrity_property(p: &Property) -> bool {
    [
        "hash",
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
}
fn placeholder(name: &str) -> Property {
    Property {
        predicate: name.into(),
        value: String::new(),
        datatype: None,
        language: None,
    }
}
fn two_volumes(volumes: &mut [Container], a: usize, b: usize) -> (&mut Container, &mut Container) {
    if a < b {
        let (left, right) = volumes.split_at_mut(b);
        (&mut left[a], &mut right[0])
    } else {
        let (left, right) = volumes.split_at_mut(a);
        (&mut right[0], &mut left[b])
    }
}
