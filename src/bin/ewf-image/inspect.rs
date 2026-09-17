//! Metadata-only image inspection. Never implies media verification.

use std::path::Path;

use ewf_image::{Image, OpenOptions, check_file_encryption};
use serde_json::{Value, json};

use super::{Result, error_ranges, hex, substituted_sectors};

pub fn open(path: &Path, report: &mut Value) -> Result<Image> {
    report["image"] = json!(path);
    report["media_verified"] = json!(false);
    report["encryption_detected"] = json!(check_file_encryption(path)?);
    Ok(Image::open_with_options(
        path,
        OpenOptions::default()
            .with_chunk_cache_size(0)
            .with_maximum_open_handles(Some(16)),
    )?)
}

pub fn info(path: &Path, report: &mut Value) -> Result<()> {
    report["phase"] = json!("inspection");
    let image = open(path, report)?;
    let info = image.info();
    let media = &info.media;
    let metadata = &info.metadata;
    report["format"] = json!(format!("{:?}", info.format));
    report["format_profile"] = json!(format!("{:?}", info.format_profile));
    report["segments"] = json!({"count": info.segment_count,
        "paths": info.segment_paths, "stored_bytes": image.segment_set_size()?});
    report["media"] = json!({"logical_bytes": info.logical_size,
        "chunk_bytes": info.chunk_size, "sectors_per_chunk": media.sectors_per_chunk,
        "bytes_per_sector": media.bytes_per_sector, "sector_count": media.sector_count,
        "chunk_count": media.chunk_count, "error_granularity": media.error_granularity,
        "media_type": media.media_type.map(|v| format!("{v:?}")),
        "compression": media.compression_method.map(|v| format!("{v:?}")),
        "set_identifier": media.set_identifier.map(|v| hex(&v)),
        "acquisition_complete": info.acquisition_complete});
    // Deliberately omit the legacy password header and arbitrary raw sections.
    report["metadata"] = json!({"case_number": metadata.case_number,
        "evidence_number": metadata.evidence_number, "examiner": metadata.examiner,
        "description": metadata.description, "notes": metadata.notes,
        "acquisition_software": metadata.acquisition_software,
        "acquisition_software_version": metadata.acquisition_software_version,
        "os_version": metadata.os_version, "acquisition_date": metadata.acquisition_date,
        "system_date": metadata.system_date});
    report["stored_hashes"] = json!({"md5": info.stored_hashes.md5.map(|v| hex(&v)),
        "sha1": info.stored_hashes.sha1.map(|v| hex(&v)),
        "values": info.stored_hashes.hash_values});
    report["acquisition_errors"] = error_ranges(&info.acquisition_errors);
    report["substituted_sectors"] = json!(substituted_sectors(&info.acquisition_errors)?);
    report["has_logical_file_catalog"] = json!(info.single_files.is_some());
    report["status"] = json!("inspected");
    Ok(())
}
