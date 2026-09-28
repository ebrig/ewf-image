//! Structured integrity inspection with explicit media coverage.

use std::path::Path;
use std::sync::atomic::Ordering;

use ewf_image::{EwfError, EwfPassword, VerifyOptions, analyze_path_with_progress};
use serde_json::{Value, json};

use super::{Progress, Result, hex};

pub fn run(
    path: &Path,
    limit: usize,
    password: Option<&EwfPassword>,
    progress: &mut Progress<'_>,
    report: &mut Value,
) -> Result<()> {
    report["phase"] = json!("analysis");
    report["image"] = json!(path);
    report["media_verified"] = json!(false);
    if progress.stop.load(Ordering::Relaxed) {
        return Err(EwfError::Aborted.into());
    }
    let options = VerifyOptions::default().with_maximum_findings(limit);
    let mut callback = |p: ewf_image::VerifyProgress| {
        report["bytes_processed"] = json!(p.bytes_processed);
        report["bytes_verified"] = json!(p.bytes_verified);
        report["media_bytes"] = json!(p.bytes_total);
        progress.event("analysis", p.bytes_processed, p.bytes_total)
    };
    let analysis = match password {
        Some(password) => ewf_image::analyze_path_with_progress_and_password(
            path,
            &options,
            password,
            &mut callback,
        )?,
        None => analyze_path_with_progress(path, &options, &mut callback)?,
    };
    let references_match =
        (!analysis.comparisons.is_empty()).then(|| analysis.comparisons.iter().all(|c| c.matches));
    report["media_verified"] = json!(references_match == Some(true) && analysis.error_count == 0);
    report["status"] = json!(if analysis.error_count != 0 {
        "analysis_findings"
    } else if analysis.warning_count != 0 {
        "analysis_warnings"
    } else {
        "analyzed"
    });
    report["analysis"] = serde_json::to_value(&analysis)?;
    report["analysis"]["hashes"] = analysis.hashes.as_ref().map_or(
        Value::Null,
        |h| json!({"md5": hex(&h.md5), "sha1": hex(&h.sha1), "sha256": hex(&h.sha256)}),
    );
    report["analysis"]["references_match"] = json!(references_match);
    Ok(())
}
