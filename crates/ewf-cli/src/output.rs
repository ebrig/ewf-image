use serde_json::Value;
use std::io::{self, Write};

fn text(v: &Value) -> String {
    v.as_str()
        .map_or_else(|| v.to_string(), |s| s.escape_debug().to_string())
}

pub(crate) fn print(report: &Value, json: bool) -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    if json {
        serde_json::to_writer(&mut out, report)?;
        writeln!(out)?;
    } else {
        let status = report["status"]
            .as_str()
            .unwrap_or("failed")
            .replace('_', " ");
        writeln!(out, "Status: {status}")?;
        for (label, key) in [
            ("Error", "error"),
            ("Input", "input"),
            ("Image", "image"),
            ("Output", "output"),
            ("Format", "format"),
            ("Bytes", "bytes"),
            ("Sector bytes", "sector_size"),
            ("History error", "history_error"),
            ("Report error", "report_error"),
            ("Report", "report_path"),
            ("Files verified", "verified_files"),
        ] {
            if !report[key].is_null() {
                writeln!(out, "{label}: {}", text(&report[key]))?;
            }
        }
        field(&mut out, "Media bytes", &report["media"]["logical_bytes"])?;
        field(
            &mut out,
            "Sector bytes",
            &report["media"]["bytes_per_sector"],
        )?;
        field(&mut out, "Segments", &report["segments"]["count"])?;
        if report["source"].is_string() {
            field(&mut out, "Source", &report["source"])?;
        } else {
            field(&mut out, "Source", &report["source"]["path"])?;
        }
        for (label, key) in [
            ("Case", "case_number"),
            ("Evidence", "evidence_number"),
            ("Examiner", "examiner"),
            ("Description", "description"),
            ("Notes", "notes"),
            ("Acquisition software", "acquisition_software"),
            ("Acquisition date", "acquisition_date"),
            ("Software version", "acquisition_software_version"),
            ("Source OS", "os_version"),
            ("System date", "system_date"),
        ] {
            field(&mut out, label, &report["metadata"][key])?;
        }
        if report["output"].is_string() || report["output_directory"].is_string() {
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
        if let Some(lines) = report["summary"].as_array() {
            for line in lines {
                writeln!(out, "{}", text(line))?;
            }
        }
        let v = &report["verification"];
        if v["sha256"].is_null() {
            field(&mut out, "SHA256", &report["sha256"])?;
        }
        for (label, key) in [
            ("Scope", "scope"),
            ("Bytes checked", "bytes_verified"),
            ("SHA256", "sha256"),
        ] {
            if !v[key].is_null() {
                writeln!(out, "{label}: {}", text(&v[key]))?;
            }
        }
        if v.get("references_match").is_some() {
            writeln!(
                out,
                "Reference hashes: {}",
                match v["references_match"].as_bool() {
                    Some(true) => "match",
                    Some(false) => "mismatch",
                    None => "none available",
                }
            )?;
        }
        field(&mut out, "External SHA256 match", &report["external_match"])?;
        field(&mut out, "External SHA256 match", &v["external_match"])?;
        field(&mut out, "Bytes checked", &v["bytes"])?;
        field(&mut out, "Image error", &v["image_error"])?;
        field(&mut out, "SHA256", &v["assembled"]["sha256"])?;
        field(
            &mut out,
            "External SHA256 match",
            &v["assembled"]["external_match"],
        )?;
        for check in [v, &report["container_verification"]] {
            field(&mut out, "Metadata error", &check["metadata_error"])?;
            if let Some(hashes) = check["unsupported_hashes"].as_array() {
                // A whole-container report separately covers structural hashes.
                if report["container_verification"].is_null() {
                    for hash in hashes {
                        field(&mut out, "Unchecked reference", hash)?;
                    }
                }
            }
            if let Some(resources) = check["resources"].as_array() {
                for resource in resources.iter().filter(|r| !r["error"].is_null()).take(5) {
                    writeln!(
                        out,
                        "Unreadable: {}: {}",
                        text(&resource["resource"]),
                        text(&resource["error"])
                    )?;
                }
            }
            let failures: Vec<_> = check["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(check["metadata"]["checks"].as_array().into_iter().flatten())
                .chain(
                    check["volumes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|v| {
                            v["checks"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .chain(v["metadata"]["checks"].as_array().into_iter().flatten())
                        }),
                )
                .filter(|c| c["outcome"] != "Match")
                .collect();
            for c in failures.iter().take(5) {
                writeln!(
                    out,
                    "Check {}: {} ({})",
                    text(&c["outcome"]),
                    text(&c["resource"]),
                    text(&c["algorithm"])
                )?;
            }
            if failures.len() > 5 {
                writeln!(
                    out,
                    "{} additional incomplete checks; use --json for details.",
                    failures.len() - 5
                )?;
            }
        }
        if let Some(disks) = report["disks"].as_array() {
            for disk in disks {
                writeln!(
                    out,
                    "Disk: {}  {} bytes  sector {}",
                    text(&disk["resource_id"]),
                    disk["logical_size"],
                    text(&disk["block_size"])
                )?;
            }
        }
        for key in ["entries", "resources", "records"] {
            if key == "resources" && report["disks"].as_array().is_some_and(|d| !d.is_empty()) {
                continue;
            }
            if let Some(rows) = report[key].as_array() {
                let limit = if key == "resources" { 10 } else { usize::MAX };
                for row in rows.iter().take(limit) {
                    let id = row
                        .get("id")
                        .or_else(|| row.get("index"))
                        .or_else(|| row.get("subject"))
                        .unwrap_or(&Value::Null);
                    let name = row
                        .get("name")
                        .or_else(|| row.get("predicate"))
                        .or_else(|| row.get("types"))
                        .unwrap_or(&Value::Null);
                    let size = row
                        .get("size")
                        .or_else(|| row.get("value"))
                        .unwrap_or(&Value::Null);
                    let name = if key == "resources" {
                        row["types"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(|s| s.rsplit('#').next().unwrap_or(s))
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        text(name)
                    };
                    writeln!(out, "{}  {}  {}", text(id), name, text(size))?;
                }
                if rows.len() > limit {
                    writeln!(
                        out,
                        "{} more resources; use files or --json for the full list.",
                        rows.len() - limit
                    )?;
                }
            }
        }
        if let Some(next) = report["next_offset"].as_u64() {
            writeln!(out, "Next page: --offset {next}")?;
        }
        if let Some(losses) = report["metadata_not_preserved"].as_array() {
            for loss in losses {
                writeln!(out, "Not preserved: {}", text(loss))?;
            }
        }
        if let Some(warnings) = report["warnings"].as_array() {
            for warning in warnings {
                writeln!(out, "Note: {}", text(warning))?;
            }
        }
        for (label, key) in [
            ("Saved bytes", "checkpoint_bytes"),
            ("Sealed segments", "sealed_segments"),
            ("Segment hashes validated", "segment_hashes_validated"),
        ] {
            field(&mut out, label, &report["checkpoint"][key])?;
        }
        if report["status"] == "cancelled" && report["checkpoint"].is_null() {
            field(&mut out, "Saved bytes", &report["checkpoint_bytes"])?;
        }
        if let Some(command) = report["recovery_command"].as_array() {
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
        for (label, key) in [
            ("Zero-filled chunks", "chunks_zero_filled"),
            ("Checksum-suspect chunks", "chunks_checksum_suspect"),
            ("Alternate chunks", "chunks_redundant"),
        ] {
            field(&mut out, label, &report["recovery"][key])?;
        }
        if report["substituted_sectors"]
            .as_u64()
            .is_some_and(|n| n > 0)
        {
            field(
                &mut out,
                "Substituted sectors",
                &report["substituted_sectors"],
            )?;
        }
        field(&mut out, "Files collected", &report["collection"]["files"])?;
        if let Some(runs) = report["history"]["runs"].as_array() {
            writeln!(out, "Saved history (not a new verification):")?;
            for run in runs {
                writeln!(out, "Saved run {}: {}", run["run"], text(&run["status"]))?;
            }
        }
        if !report["analysis"].is_null() {
            writeln!(
                out,
                "Coverage: {}; errors: {}; warnings: {}",
                text(&report["analysis"]["media_status"]),
                report["analysis"]["error_count"],
                report["analysis"]["warning_count"]
            )?;
        }
    }
    out.flush()
}

fn field(out: &mut impl Write, label: &str, value: &Value) -> io::Result<()> {
    if !value.is_null() && value.as_str() != Some("") {
        writeln!(out, "{label}: {}", text(value))?;
    }
    Ok(())
}
