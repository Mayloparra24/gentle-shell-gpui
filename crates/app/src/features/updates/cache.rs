//! The launcher config as the update check's store: the day-old
//! `startupReleaseCheck` record (ISO or numeric `checkedAt`) and key writes
//! that keep every other launcher key.

use std::path::Path;

use gentle_core::version::stable_parts;
use serde_json::Value;

/// Same budget as the launcher: a real answer is trusted for a day.
const TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// `2026-11-06T17:08:31.806Z` → epoch ms (UTC `Z` only, as `toISOString` writes).
fn parse_iso_ms(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (date.next()??, date.next()??, date.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut hms = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (h, mi, s) = (hms.next()??, hms.next()??, hms.next()??);
    let ms: i64 = format!("{frac:0<3}").get(..3)?.parse().ok()?;
    let days = days_from_civil(y, m, d);
    let total = ((days * 24 + h) * 60 + mi) * 60 + s;
    u64::try_from(total * 1000 + ms).ok()
}

pub(super) fn format_iso_ms(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

pub(super) fn read_config(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()))
}

/// A fresh `startupReleaseCheck` (number or ISO `checkedAt`), as the launcher reads it.
pub(super) fn cached_latest(config: &Value, now: u64) -> Option<String> {
    let check = config.get("startupReleaseCheck")?;
    let checked = match check.get("checkedAt")? {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => parse_iso_ms(s)?,
        _ => return None,
    };
    let latest = check.get("latest")?.as_str()?;
    (stable_parts(latest).is_some() && now.saturating_sub(checked) < TTL_MS)
        .then(|| latest.to_string())
}

/// Set `key` in the launcher config, keeping every other key; refuses to
/// overwrite a config that is not a JSON object (the launcher refuses too).
pub(super) fn write_key(path: &Path, key: &str, value: Value) -> std::io::Result<()> {
    let mut config = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| std::io::Error::other("the launcher config is not a JSON object"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(e) => return Err(e),
    };
    config[key] = value;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&config)? + "\n")?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips_like_to_iso_string() {
        let ms = parse_iso_ms("2026-11-06T17:08:31.806Z").unwrap();
        assert_eq!(format_iso_ms(ms), "2026-11-06T17:08:31.806Z");
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_ms("2024-02-29T12:00:00Z")
                .map(format_iso_ms)
                .as_deref(),
            Some("2024-02-29T12:00:00.000Z")
        );
        assert_eq!(parse_iso_ms("2026-11-06 17:08:31"), None);
    }

    #[test]
    fn cache_is_trusted_for_a_day_only() {
        let at = parse_iso_ms("2026-11-06T00:00:00.000Z").unwrap();
        let config = serde_json::json!({"startupReleaseCheck": {"checkedAt": "2026-11-06T00:00:00.000Z", "latest": "4.7.0"}});
        assert_eq!(
            cached_latest(&config, at + TTL_MS - 1).as_deref(),
            Some("4.7.0")
        );
        assert_eq!(cached_latest(&config, at + TTL_MS), None);
        let numeric =
            serde_json::json!({"startupReleaseCheck": {"checkedAt": at, "latest": "4.7.0"}});
        assert_eq!(cached_latest(&numeric, at).as_deref(), Some("4.7.0"));
        let bad = serde_json::json!({"startupReleaseCheck": {"checkedAt": at, "latest": "next"}});
        assert_eq!(
            cached_latest(&bad, at),
            None,
            "a malformed latest never suppresses a check"
        );
    }

    #[test]
    fn writes_keep_the_launcher_keys() {
        let dir = std::env::temp_dir().join(format!("gs-updates-{}", std::process::id()));
        let path = dir.join("config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"provisioned":{"x":1},"home":"isolated"}"#).unwrap();
        write_key(&path, "skippedStartupRelease", Value::from("4.7.0")).unwrap();
        let config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["provisioned"]["x"], 1);
        assert_eq!(config["home"], "isolated");
        assert_eq!(config["skippedStartupRelease"], "4.7.0");
        std::fs::write(&path, "[1]").unwrap();
        assert!(
            write_key(&path, "k", Value::from(1)).is_err(),
            "never clobbers a foreign config"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1]");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
