//! Terminal summaries; the JSON report remains the automation contract.
use std::io::{self, Write};

use serde_json::Value;

pub(super) fn print(report: &Value, json: bool) -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    if json {
        serde_json::to_writer(&mut out, report)?;
        writeln!(out)?;
    } else {
        summary(&mut out, report)?;
    }
    out.flush()
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), |s| s.escape_debug().to_string())
}

fn field(out: &mut impl Write, label: &str, value: &Value) -> io::Result<()> {
    if !value.is_null() && value.as_str() != Some("") {
        writeln!(out, "{label}: {}", text(value))?;
    }
    Ok(())
}

fn reference(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(
        out,
        "Reference hashes: {}",
        match value.as_bool() {
            Some(true) => "match",
            Some(false) => "mismatch",
            None => "none available",
        }
    )
}

fn summary(out: &mut impl Write, report: &Value) -> io::Result<()> {
    let status = report["status"]
        .as_str()
        .unwrap_or("failed")
        .replace('_', " ");
    writeln!(out, "Status: {status}")?;
    for (label, key) in [
        ("Error", "error"),
        ("History error", "history_error"),
        ("Report error", "report_error"),
        ("Image", "image"),
        ("Output", "output"),
        ("Directory", "output_directory"),
        ("Format", "format"),
    ] {
        field(out, label, &report[key])?;
    }
    if report["source"].is_string() {
        field(out, "Source", &report["source"])?;
    } else {
        field(out, "Source", &report["source"]["path"])?;
    }
    if !report["output"].is_null() || !report["output_directory"].is_null() {
        writeln!(
            out,
            "Published: {}",
            match report["published"].as_bool() {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unresolved",
            }
        )?;
    }
    field(out, "Media bytes", &report["media"]["logical_bytes"])?;
    field(out, "Segments", &report["segments"]["count"])?;
    for (label, key) in [
        ("Case", "case_number"),
        ("Evidence", "evidence_number"),
        ("Examiner", "examiner"),
        ("Description", "description"),
        ("Notes", "notes"),
    ] {
        field(out, label, &report["metadata"][key])?;
    }
    if let Some(entries) = report["entries"].as_array() {
        writeln!(out, "{:>7}  {:>12}  {:<10}  Name", "Entry", "Bytes", "Type")?;
        for entry in entries {
            writeln!(
                out,
                "{:>7}  {:>12}  {:<10}  {}",
                entry["index"],
                entry["size"],
                text(&entry["type"]),
                text(&entry["name"])
            )?;
        }
        if let Some(next) = report["next_offset"].as_u64() {
            writeln!(out, "Next page: --offset {next}")?;
        }
    }
    field(out, "File", &report["entry"]["name"])?;
    let verification = &report["verification"];
    if verification.is_object() {
        field(out, "Scope", &verification["scope"])?;
        field(out, "Bytes checked", &verification["bytes_verified"])?;
        reference(out, &verification["references_match"])?;
        field(out, "SHA256", &verification["sha256"])?;
    }
    field(out, "Files collected", &report["collection"]["files"])?;
    field(out, "Files verified", &report["verified_files"])?;
    if let Some(count) = report["substituted_sectors"].as_u64().filter(|n| *n != 0) {
        writeln!(out, "Substituted sectors: {count}")?;
    }
    let analysis = &report["analysis"];
    if analysis.is_object() {
        field(out, "Coverage", &analysis["media_status"])?;
        field(out, "Errors", &analysis["error_count"])?;
        field(out, "Warnings", &analysis["warning_count"])?;
        reference(out, &analysis["references_match"])?;
        field(out, "SHA256", &analysis["hashes"]["sha256"])?;
        if let Some(findings) = analysis["findings"].as_array() {
            for finding in findings.iter().take(5) {
                field(out, "Finding", &finding["message"])?;
            }
            if findings.len() > 5 {
                writeln!(out, "Use --json for detailed findings.")?;
            }
        }
    }
    let checkpoint = &report["checkpoint"];
    if checkpoint.is_object() {
        field(out, "Saved bytes", &checkpoint["checkpoint_bytes"])?;
        field(out, "Sealed segments", &checkpoint["sealed_segments"])?;
        field(
            out,
            "Segment hashes validated",
            &checkpoint["segment_hashes_validated"],
        )?;
    } else if report["status"] == "cancelled" {
        field(out, "Saved bytes", &report["checkpoint_bytes"])?;
    }
    let recovery = &report["recovery"];
    if recovery.is_object() {
        for (label, key) in [
            ("Zero-filled chunks", "chunks_zero_filled"),
            ("Checksum-suspect chunks", "chunks_checksum_suspect"),
            ("Alternate chunks", "chunks_redundant"),
        ] {
            field(out, label, &recovery[key])?;
        }
    }
    if let Some(command) = report["recovery_command"].as_array() {
        // Quoted arguments preserve spaces and escape terminal control characters.
        writeln!(
            out,
            "Resolve publication: {}",
            command
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        )?;
    }
    let history = &report["history"];
    if let Some(runs) = history["runs"].as_array() {
        writeln!(out, "Saved history (not a new verification):")?;
        for run in runs {
            writeln!(out, "  Run {}: {}", run["run"], text(&run["status"]))?;
        }
        field(out, "Counters complete", &history["counters_complete"])?;
    }
    field(out, "Report", &report["report_path"])?;
    Ok(())
}
