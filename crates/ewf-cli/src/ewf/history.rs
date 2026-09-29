//! Immutable CLI acquisition records and a replaceable derived report.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ewf_image::AcquisitionProgress;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{ReadArgs, Result, hex, invalid, session::Session, sidecar};

const MAX_RECORD: u64 = 16 * 1024 * 1024;
const MAX_HISTORY: u64 = 64 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;

#[cfg(test)]
mod tests;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    session_id: String,
    sequence: u64,
    run: u64,
    event: String,
    unix_time_ms: u64,
    data: Value,
}

pub(super) struct History {
    directory: PathBuf,
    output: PathBuf,
    session_id: String,
    sequence: u64,
    bytes: u64,
    run: u64,
    checkpoint: Option<u64>,
    last_error_attempt: Option<u64>,
    substituted: Option<u64>,
    sector_size: u32,
    phase: String,
    pub failure: Option<String>,
    // Keep CLI serialization through the closing record and report publication.
    _lock: File,
}

pub(super) fn read_policy(args: &ReadArgs) -> Value {
    json!({"retries": args.retries, "zero_fill": args.zero_fill,
        "checkpoint_interval": args.checkpoint_interval,
        "bulk_read_bytes": args.bulk_read_bytes,
        "read_timeout_ms": args.read_timeout_ms, "stop_after": args.stop_after})
}

fn session_id(session: &Session) -> Result<String> {
    Ok(hex(&Sha256::digest(serde_json::to_vec(session)?)))
}

fn regular(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(invalid(
            "history paths must be ordinary files and directories",
        ));
    }
    Ok(())
}

#[cfg_attr(not(unix), allow(clippy::unnecessary_wraps))] // Unix fsync can fail.
fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn publish(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    if bytes.len() as u64 > MAX_RECORD {
        return Err(invalid("history record/report exceeds 16 MiB"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("missing history parent"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".pending-history-")
        .tempfile_in(parent)?;
    #[cfg(test)]
    {
        let split = bytes.len() / 2;
        temporary.write_all(&bytes[..split])?;
        tests::crash_at("partial", path);
        temporary.write_all(&bytes[split..])?;
    }
    #[cfg(not(test))]
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    #[cfg(test)]
    tests::crash_at("synced", path);
    if replace {
        temporary.persist(path)?;
    } else {
        temporary.persist_noclobber(path)?;
    }
    #[cfg(test)]
    tests::crash_at("installed", path);
    sync_directory(parent)?;
    Ok(())
}

impl History {
    pub fn open(session: &Session, lock: File, fresh: bool) -> Result<Self> {
        let directory = sidecar(&session.output, "ewf-history");
        let id = session_id(session)?;
        let mut history = Self {
            directory,
            output: session.output.clone(),
            session_id: id,
            sequence: 0,
            bytes: 0,
            run: 0,
            checkpoint: None,
            last_error_attempt: None,
            substituted: None,
            sector_size: session.source.sector_size,
            phase: String::new(),
            failure: None,
            _lock: lock,
        };
        if history.directory.try_exists()? {
            if fresh {
                return Err(invalid("acquisition history already exists"));
            }
            let loaded = load(session)?;
            history.sequence = loaded.records;
            history.bytes = loaded.bytes;
        } else {
            fs::create_dir(&history.directory)?;
            sync_directory(
                history
                    .directory
                    .parent()
                    .ok_or_else(|| invalid("missing history parent"))?,
            )?;
        }
        if history.sequence == 0 {
            history.event(
                "session",
                json!({"session": session, "prior_history_unavailable": !fresh}),
            )?;
        }
        Ok(history)
    }

    pub fn start(&mut self, command: &str, args: &ReadArgs) -> Result<()> {
        self.run = self.sequence + 1;
        self.event(
            "run_start",
            json!({"command": command,
            "tool_version": env!("CARGO_PKG_VERSION"), "read_policy": read_policy(args)}),
        )
    }

    pub fn event(&mut self, event: &str, data: Value) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(invalid(error));
        }
        let result = self.append(event, data);
        if let Err(error) = &result {
            self.failure = Some(error.to_string());
        }
        result
    }

    fn append(&mut self, event: &str, data: Value) -> Result<()> {
        if self.sequence >= MAX_RECORDS as u64 {
            return Err(invalid("history exceeds record limit"));
        }
        let sequence = self.sequence + 1;
        let record = Record {
            schema_version: 1,
            session_id: self.session_id.clone(),
            sequence,
            run: self.run,
            event: event.into(),
            unix_time_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .try_into()?,
            data,
        };
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        if self.bytes + bytes.len() as u64 > MAX_HISTORY {
            return Err(invalid("history exceeds 64 MiB"));
        }
        publish(
            &self.directory.join(format!("{sequence:020}.json")),
            &bytes,
            false,
        )?;
        self.sequence = sequence;
        self.bytes += bytes.len() as u64;
        Ok(())
    }

    pub fn phase(&mut self, phase: &str) -> Result<()> {
        if self.phase != phase {
            self.event("phase", json!({"phase": phase}))?;
            self.phase = phase.into();
        }
        Ok(())
    }

    pub fn progress(&mut self, p: AcquisitionProgress) -> Result<()> {
        let data = json!({"accepted_bytes": p.bytes_written, "checkpoint_bytes": p.checkpoint_bytes,
            "substituted_sectors": p.substituted_sectors, "read_attempts": p.read_attempts,
            "retry_attempts": p.retry_attempts, "read_offset": p.read_offset,
            "read_error": p.read_error.map(|kind| format!("{kind:?}"))});
        if p.read_error.is_some() && self.last_error_attempt != Some(p.read_attempts) {
            self.event("read_error", data.clone())?;
            self.last_error_attempt = Some(p.read_attempts);
        }
        if let Some(previous) = self.substituted
            && p.substituted_sectors > previous
        {
            self.event(
                "substitution",
                json!({"first_sector": p.read_offset / u64::from(self.sector_size),
                "sector_count": p.substituted_sectors - previous, "progress": data}),
            )?;
        }
        self.substituted = Some(p.substituted_sectors);
        if self.checkpoint != Some(p.checkpoint_bytes) {
            self.event("checkpoint", data)?;
            self.checkpoint = Some(p.checkpoint_bytes);
        }
        Ok(())
    }

    pub fn finish(&mut self, report: &Value) -> Result<()> {
        self.event("run_end", report.clone())?;
        let session = Session::load(&self.output)?;
        save_report(&session, &load(&session)?.report)
    }
}

struct Loaded {
    records: u64,
    bytes: u64,
    report: Value,
}

pub(super) fn report(session: &Session, write: bool) -> Result<Value> {
    let loaded = load(session)?;
    if write {
        save_report(session, &loaded.report)?;
    }
    Ok(loaded.report)
}

fn save_report(session: &Session, report: &Value) -> Result<()> {
    let target = sidecar(&session.output, "ewf-report.json");
    if target.try_exists()? {
        regular(&target, false)?;
        let mut bytes = Vec::new();
        File::open(&target)?
            .take(MAX_RECORD + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(invalid("existing report exceeds size limit"));
        }
        let old: Value = serde_json::from_slice(&bytes)?;
        if old["report_kind"] != "ewf_acquisition_history"
            || old["session_id"] != session_id(session)?
        {
            return Err(invalid(
                "refusing to replace an unrelated acquisition report",
            ));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    publish(&target, &bytes, true)
}

fn load(session: &Session) -> Result<Loaded> {
    let directory = sidecar(&session.output, "ewf-history");
    regular(&directory, true)?;
    let id = session_id(session)?;
    let mut paths = Vec::new();
    let mut pending = 0_u64;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        regular(&entry.path(), false)?;
        if name.starts_with(".pending-history-") {
            pending += 1;
        } else if name.len() == 25
            && Path::new(&name)
                .extension()
                .is_some_and(|extension| extension == "json")
            && name.as_bytes()[..20].iter().all(u8::is_ascii_digit)
        {
            paths.push(entry.path());
        } else {
            return Err(invalid("unexpected acquisition history entry"));
        }
        if paths.len() as u64 + pending > MAX_RECORDS as u64 {
            return Err(invalid("history exceeds record limit"));
        }
    }
    paths.sort();
    let mut runs: Vec<Value> = Vec::new();
    let mut total_bytes = 0;
    let mut prior_unavailable = true;
    for (index, path) in paths.iter().enumerate() {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_RECORD + 1)
            .read_to_end(&mut bytes)?;
        total_bytes += bytes.len() as u64;
        if bytes.len() as u64 > MAX_RECORD || total_bytes > MAX_HISTORY {
            return Err(invalid("history exceeds size limit"));
        }
        if bytes.last() != Some(&b'\n') {
            return Err(invalid("truncated committed history record"));
        }
        let record: Record = serde_json::from_slice(&bytes)?;
        let sequence = index as u64 + 1;
        if record.schema_version != 1
            || record.session_id != id
            || record.sequence != sequence
            || path.file_name().and_then(|n| n.to_str())
                != Some(format!("{sequence:020}.json").as_str())
        {
            return Err(invalid("history schema, session, or sequence mismatch"));
        }
        if index == 0 {
            if record.event != "session"
                || record.run != 0
                || record.data["session"] != serde_json::to_value(session)?
            {
                return Err(invalid("invalid history session record"));
            }
            prior_unavailable = record.data["prior_history_unavailable"]
                .as_bool()
                .ok_or_else(|| invalid("missing history provenance"))?;
            continue;
        }
        if record.event == "run_start" {
            if record.run != sequence {
                return Err(invalid("invalid history run identifier"));
            }
            runs.push(json!({"run": record.run, "started_unix_ms": record.unix_time_ms,
                "start": record.data, "status": "interrupted", "last_progress": null, "result": null,
                "publication": null}));
            continue;
        }
        let run = runs
            .last_mut()
            .ok_or_else(|| invalid("history event without a run"))?;
        if run["run"] != record.run || !run["result"].is_null() {
            return Err(invalid("history event outside its active run"));
        }
        match record.event.as_str() {
            "checkpoint" | "read_error" => run["last_progress"] = record.data,
            "substitution" => run["last_progress"] = record.data["progress"].clone(),
            "phase" => run["last_phase"] = record.data["phase"].clone(),
            "published" => run["publication"] = record.data,
            "run_end" => {
                if !record.data["status"].is_string() || !record.data["exit_code"].is_u64() {
                    return Err(invalid("invalid history closing result"));
                }
                run["status"] = record.data["status"].clone();
                run["ended_unix_ms"] = json!(record.unix_time_ms);
                run["result"] = record.data;
            }
            _ => return Err(invalid("unknown acquisition history event")),
        }
    }
    let incomplete =
        prior_unavailable || pending != 0 || runs.iter().any(|run| run["result"].is_null());
    let mut reads = 0_u64;
    let mut retries = 0_u64;
    for run in &runs {
        let stats = if run["result"].is_null() {
            &run["last_progress"]
        } else {
            &run["result"]
        };
        reads = reads
            .checked_add(stats["read_attempts"].as_u64().unwrap_or(0))
            .ok_or_else(|| invalid("history counter overflow"))?;
        retries = retries
            .checked_add(stats["retry_attempts"].as_u64().unwrap_or(0))
            .ok_or_else(|| invalid("history counter overflow"))?;
    }
    let latest = runs.last().cloned().unwrap_or(Value::Null);
    Ok(Loaded {
        records: paths.len() as u64,
        bytes: total_bytes,
        report: json!({
            "schema_version": 1, "report_kind": "ewf_acquisition_history", "session_id": id,
            "session": session, "recorded_only": true, "record_count": paths.len(),
            "pending_records": pending, "prior_history_unavailable": prior_unavailable,
            "counters_complete": !incomplete, "read_attempts": reads, "retry_attempts": retries,
            "latest_run": latest, "runs": runs,
        }),
    })
}
