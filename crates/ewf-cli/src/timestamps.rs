use std::{
    fs::{File, FileTimes},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{Result, invalid};

pub(crate) fn seconds(value: i64) -> Result<SystemTime> {
    let duration = Duration::from_secs(value.unsigned_abs());
    if value < 0 {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    }
    .ok_or_else(|| invalid("recorded file time is outside the host range"))
}

pub(crate) fn rfc3339(value: &str) -> Result<SystemTime> {
    let nanos = time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)?
        .unix_timestamp_nanos();
    let magnitude = nanos.unsigned_abs();
    let whole = u64::try_from(magnitude / 1_000_000_000)
        .map_err(|_| invalid("recorded file time is outside the host range"))?;
    let fractional =
        u32::try_from(magnitude % 1_000_000_000).expect("nanosecond remainder fits u32");
    let duration = Duration::new(whole, fractional);
    if nanos < 0 {
        UNIX_EPOCH.checked_sub(duration)
    } else {
        UNIX_EPOCH.checked_add(duration)
    }
    .ok_or_else(|| invalid("recorded file time is outside the host range"))
}

pub(crate) fn apply(
    file: &File,
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> Result<Vec<&'static str>> {
    let mut fields = Vec::new();
    let mut times = FileTimes::new();
    if let Some(value) = accessed {
        times = times.set_accessed(value);
        fields.push("accessed");
    }
    if let Some(value) = modified {
        times = times.set_modified(value);
        fields.push("modified");
    }
    if !fields.is_empty() {
        file.set_times(times)?;
    }
    Ok(fields)
}
