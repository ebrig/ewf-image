//! Concise terminal output, without duplicating large JSON reports in memory.
use aff4_image::{
    CheckOutcome, ContainerVerification, IntegrityCheck, SetVerification, Verification,
};
use std::io::{self, Write};

pub(super) struct Output {
    json: bool,
}

pub(super) fn clean(value: &str) -> String {
    value.escape_debug().to_string()
}

impl Output {
    pub(super) const fn new(json: bool) -> Self {
        Self { json }
    }

    pub(super) fn emit(
        &self,
        value: &impl serde::Serialize,
        summary: impl FnOnce() -> String,
    ) -> io::Result<()> {
        let mut out = io::BufWriter::new(io::stdout().lock());
        if self.json {
            serde_json::to_writer(&mut out, value)?;
            writeln!(out)?;
        } else {
            writeln!(out, "{}", summary().trim_end())?;
        }
        out.flush()
    }
}

pub(super) fn checks<'a>(checks: impl Iterator<Item = &'a IntegrityCheck>) -> String {
    let mut matched = 0;
    let mut mismatch = 0;
    let mut missing = 0;
    let mut unsupported = 0;
    let mut unreadable = 0;
    let mut details = Vec::new();
    for check in checks {
        match check.outcome {
            CheckOutcome::Match => matched += 1,
            CheckOutcome::Mismatch => mismatch += 1,
            CheckOutcome::Missing => missing += 1,
            CheckOutcome::Unsupported => unsupported += 1,
            CheckOutcome::Unreadable => unreadable += 1,
        }
        if check.outcome != CheckOutcome::Match && details.len() < 5 {
            details.push(format!(
                "  {:?}: {} ({}){}",
                check.outcome,
                clean(&check.resource),
                clean(&check.algorithm),
                check
                    .detail
                    .as_ref()
                    .map(|d| format!(": {}", clean(d)))
                    .unwrap_or_default()
            ));
        }
    }
    let mut result = format!("Checks: {matched} matched");
    for (count, label) in [
        (mismatch, "mismatched"),
        (missing, "missing"),
        (unsupported, "unsupported"),
        (unreadable, "unreadable"),
    ] {
        if count != 0 {
            result.push_str(&format!(", {count} {label}"));
        }
    }
    for detail in details {
        result.push('\n');
        result.push_str(&detail);
    }
    if mismatch + missing + unsupported + unreadable > 5 {
        result.push_str("\nUse --json for all check details.");
    }
    result
}

pub(super) fn container(report: &ContainerVerification) -> String {
    let mut result = format!(
        "Verification: {}\nResources: {}\n{}",
        if report.all_match() {
            "passed"
        } else {
            "failed or incomplete"
        },
        report.resources.len(),
        checks(
            report
                .metadata
                .iter()
                .flat_map(|m| m.checks.iter())
                .chain(report.checks.iter())
        )
    );
    if let Some(error) = &report.metadata_error {
        result.push_str(&format!("\nMetadata error: {}", clean(error)));
    }
    let mut errors = report
        .resources
        .iter()
        .filter_map(|r| r.error.as_ref().map(|e| (&r.resource, e)));
    for (resource, error) in errors.by_ref().take(5) {
        result.push_str(&format!(
            "\nUnreadable: {}: {}",
            clean(resource),
            clean(error)
        ));
    }
    if errors.next().is_some() {
        result.push_str("\nUse --json for all resource errors.");
    }
    result
}

pub(super) fn linear(report: &Verification) -> String {
    let mut result = format!(
        "Bytes checked: {}\nReference hashes: {}\nSHA256: {}",
        report.bytes_verified,
        match report.references_match {
            Some(true) => "match",
            Some(false) => "mismatch",
            None => "none available",
        },
        report.sha256
    );
    if !report.unsupported_hashes.is_empty() {
        result.push_str(&format!(
            "\nUnsupported hashes: {}",
            report.unsupported_hashes.len()
        ));
    }
    result
}

pub(super) fn set(report: &SetVerification) -> String {
    let mut text = format!(
        "Verification: {}\nScope: selected image and supplied volumes\n{}",
        if report.all_match() {
            "passed"
        } else {
            "failed or incomplete"
        },
        checks(report.volumes.iter().flat_map(|v| {
            v.metadata
                .iter()
                .flat_map(|m| m.checks.iter())
                .chain(v.checks.iter())
        }))
    );
    if let Some(digest) = &report.assembled {
        text.push_str(&format!(
            "\nBytes checked: {}\nExternal SHA256: {}\nSHA256: {}",
            digest.bytes,
            match digest.external_match {
                Some(true) => "match",
                Some(false) => "mismatch",
                None => "not supplied",
            },
            digest.sha256
        ));
    }
    let mut errors = report
        .image_error
        .iter()
        .map(|e| format!("Image error: {}", clean(e)))
        .chain(
            report
                .volumes
                .iter()
                .filter_map(|v| v.metadata_error.as_ref())
                .map(|e| format!("Metadata error: {}", clean(e))),
        )
        .chain(report.streams.iter().filter_map(|s| {
            s.error
                .as_ref()
                .map(|e| format!("Unreadable: {}: {}", clean(&s.resource), clean(e)))
        }));
    for error in errors.by_ref().take(5) {
        text.push('\n');
        text.push_str(&error);
    }
    if errors.next().is_some() {
        text.push_str("\nUse --json for all errors.");
    }
    text
}
