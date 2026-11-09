//! Local stats input: the session files under `sessions/` (streamed once,
//! then cached by mtime and length) and the local UTC offset.

use std::collections::HashMap;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};

use gentle_core::usage::stats::{SessionRecord, UNKNOWN, text_or, usage_record};
use gentle_core::usage::time::parse_iso_ms;
use pi_rpc::file_cache::{FileCache, Stamp};
use serde_json::Value;

/// Session files above this size are skipped rather than streamed.
pub(super) const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct StatsData {
    pub sessions: Vec<SessionRecord>,
    pub files: usize,
    pub skipped_large: Vec<(PathBuf, u64)>,
    pub unreadable: usize,
}

/// Streams one session file: the header line, then only lines that mention
/// `"assistant"`. `None` for a file without a session header.
fn read_session_file(path: &Path, dir: &str) -> std::io::Result<Option<SessionRecord>> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut line = Vec::new();
    let mut record: Option<SessionRecord> = None;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let Some(session) = record.as_mut() else {
            if text.trim().is_empty() {
                continue;
            }
            let header: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if header.get("type").and_then(Value::as_str) != Some("session") {
                return Ok(None);
            }
            record = Some(SessionRecord {
                id: text_or(header.get("id"), UNKNOWN),
                cwd: text_or(header.get("cwd"), ""),
                dir: dir.to_string(),
                started_at: header
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_ms),
                messages: Vec::new(),
            });
            continue;
        };
        if !text.contains("\"assistant\"") {
            continue;
        }
        if let Some(usage) = serde_json::from_str::<Value>(&text)
            .ok()
            .as_ref()
            .and_then(usage_record)
        {
            session.messages.push(usage);
        }
    }
    if let Some(record) = record.as_mut()
        && record.started_at.is_none()
    {
        record.started_at = record.messages.first().map(|m| m.timestamp);
    }
    Ok(record)
}

fn last_usage_at(record: &SessionRecord) -> i64 {
    record
        .messages
        .iter()
        .map(|m| m.timestamp)
        .max()
        .unwrap_or(i64::MIN)
}

/// Parsed session files, so reopening Stats only reads new or changed ones.
static SESSION_RECORDS: FileCache<Option<SessionRecord>> = FileCache::new(16_384);

/// Top-level session files only (`sessions/<dir>/*.jsonl`; nested subagent
/// runs live deeper and are excluded, like Pi's `listSessionFiles`). Copies
/// of one session id keep the richer one.
pub fn load_sessions(root: &Path, max_bytes: u64) -> StatsData {
    load_sessions_in(&SESSION_RECORDS, root, max_bytes)
}

fn load_sessions_in(
    cache: &FileCache<Option<SessionRecord>>,
    root: &Path,
    max_bytes: u64,
) -> StatsData {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.file_name().to_string_lossy().into_owned();
        for child in std::fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = child.path();
            if child.file_type().is_ok_and(|t| t.is_file())
                && path.extension().is_some_and(|e| e == "jsonl")
            {
                files.push((path, dir.clone()));
            }
        }
    }
    files.sort();
    let mut data = StatsData {
        files: files.len(),
        ..StatsData::default()
    };
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (path, dir) in files {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                data.unreadable += 1;
                continue;
            }
        };
        let size = meta.len();
        if size > max_bytes {
            data.skipped_large.push((path, size));
            continue;
        }
        let stamp = Stamp::from_metadata(&meta);
        let read = match cache.get(&path, stamp) {
            Some(cached) => Ok(cached),
            // Read errors are not cached; the next load tries again.
            None => read_session_file(&path, &dir)
                .inspect(|record| cache.insert(&path, stamp, record.clone())),
        };
        let record = match read {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(_) => {
                data.unreadable += 1;
                continue;
            }
        };
        if record.id == UNKNOWN {
            data.sessions.push(record);
            continue;
        }
        match by_id.get(&record.id) {
            Some(&index) => {
                let kept = &data.sessions[index];
                let replace = if record.messages.len() != kept.messages.len() {
                    record.messages.len() > kept.messages.len()
                } else {
                    last_usage_at(&record) > last_usage_at(kept)
                };
                if replace {
                    data.sessions[index] = record;
                }
            }
            None => {
                by_id.insert(record.id.clone(), data.sessions.len());
                data.sessions.push(record);
            }
        }
    }
    data
}

/// The local UTC offset in seconds right now (DST drift across a 30-day
/// chart is ignored). Falls back to UTC when it cannot be determined.
pub(super) fn local_offset_secs() -> i64 {
    #[cfg(not(windows))]
    {
        let output = pi_rpc::command("date").arg("+%z").output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout);
            let t = text.trim().as_bytes();
            if t.len() == 5 && matches!(t[0], b'+' | b'-') && t[1..].iter().all(u8::is_ascii_digit)
            {
                let n = |i: usize| (t[i] - b'0') as i64;
                let secs = ((n(1) * 10 + n(2)) * 60 + n(3) * 10 + n(4)) * 60;
                return if t[0] == b'-' { -secs } else { secs };
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::{MAX_SESSION_BYTES, load_sessions};
    use crate::usage::now_ms;
    use gentle_core::usage::stats::{StatsRange, aggregate_stats};
    use gentle_core::usage::time::{DAY, civil_from_days};
    use serde_json::json;

    const NOW: i64 = 1_762_400_000_000;

    fn iso(ms: i64) -> String {
        let days = ms.div_euclid(DAY * 1000);
        let rem = ms.rem_euclid(DAY * 1000) / 1000;
        let (y, m, d) = civil_from_days(days);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }

    fn assistant(
        ms: i64,
        provider: &str,
        model: &str,
        input: u64,
        output: u64,
        cost: f64,
    ) -> String {
        json!({
            "type": "message",
            "timestamp": iso(ms),
            "message": { "role": "assistant", "provider": provider, "model": model,
                         "usage": { "input": input, "output": output, "cacheRead": 10, "cacheWrite": 0, "cost": { "total": cost } } }
        })
        .to_string()
    }

    #[test]
    fn stats_aggregate_over_synthetic_session_dir() {
        let root = std::env::temp_dir().join(format!(
            "gentle-usage-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let day = DAY * 1000;
        let write = |dir: &str, name: &str, lines: &[String]| {
            let path = root.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(name), lines.join("\n")).unwrap();
        };
        let header = |id: &str, cwd: &str, ms: i64| {
            json!({ "type": "session", "id": id, "cwd": cwd, "timestamp": iso(ms) }).to_string()
        };
        write(
            "--Users-me-alpha--",
            "a.jsonl",
            &[
                header("s1", "/Users/me/alpha", NOW - 2 * day),
                json!({ "type": "message", "timestamp": iso(NOW - 2 * day), "message": { "role": "user", "content": "hi" } }).to_string(),
                assistant(NOW - 2 * day, "anthropic", "claude", 100, 50, 0.5),
                assistant(NOW - day, "openai-codex", "gpt", 200, 100, 0.25),
                "{ not json \"assistant\"".into(),
            ],
        );
        // A second copy of s1 with fewer records loses.
        write(
            "--Users-me-alpha--",
            "b.jsonl",
            &[
                header("s1", "/Users/me/alpha", NOW - 2 * day),
                assistant(NOW, "x", "x", 1, 1, 9.0),
            ],
        );
        write(
            "--Users-me-beta--",
            "c.jsonl",
            &[
                header("s2", "", NOW - 40 * day),
                assistant(NOW - 40 * day, "anthropic", "claude", 1000, 0, 1.0),
                assistant(NOW, "anthropic", "claude", 10, 0, 0.0),
            ],
        );
        write("--Users-me-beta--", "notes.txt", &["ignored".into()]);
        write(
            "--Users-me-beta--",
            "no-header.jsonl",
            &[assistant(NOW, "anthropic", "claude", 1, 1, 1.0)],
        );
        write(
            "--Users-me-beta--/nested",
            "sub.jsonl",
            &[
                header("s3", "", NOW),
                assistant(NOW, "a", "nested", 5, 5, 5.0),
            ],
        );

        let data = load_sessions(&root, MAX_SESSION_BYTES);
        assert_eq!(data.files, 4, "only sessions/<dir>/*.jsonl");
        assert_eq!(data.sessions.len(), 2);
        assert!(data.skipped_large.is_empty());

        let all = aggregate_stats(&data.sessions, StatsRange::All, NOW, 0);
        assert_eq!(all.sessions, 2);
        assert_eq!(all.messages, 4);
        assert_eq!(all.totals.input, 1310);
        assert_eq!(all.totals.output, 150);
        assert_eq!(all.totals.cache_read, 40);
        assert_eq!(all.totals.total, 1310 + 150 + 40);
        assert!((all.totals.cost - 1.75).abs() < 1e-9);
        assert_eq!(all.models[0].model, "claude");
        assert_eq!(all.models[0].messages, 3);
        assert_eq!(all.models[0].tokens, 160 + 1010 + 20);
        assert_eq!(all.models[1].model, "gpt");
        assert_eq!(all.projects.len(), 2);
        assert_eq!(
            all.projects[0].project, "--Users-me-beta--",
            "a header without cwd falls back to the session dir"
        );
        assert_eq!(all.projects[0].sessions, 1);
        assert_eq!(all.projects[1].project, "/Users/me/alpha");
        assert_eq!(all.projects[1].messages, 2);
        assert_eq!(all.daily.len(), 30);
        assert_eq!(all.daily.last().unwrap().tokens, 20);
        assert_eq!(all.daily[29 - 2].tokens, 160);
        assert!((all.daily[29 - 1].cost - 0.25).abs() < 1e-9);
        assert_eq!(all.active_days, 4);

        let week = aggregate_stats(&data.sessions, StatsRange::Week, NOW, 0);
        assert_eq!(week.messages, 3, "the 40-day-old message is outside 7d");
        assert_eq!(week.sessions, 2);
        assert_eq!(week.daily, all.daily, "the chart ignores the range");

        let capped = load_sessions(&root, 10);
        assert_eq!(capped.skipped_large.len(), 4);
        assert!(capped.sessions.is_empty());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn session_records_are_cached_by_mtime_and_len() {
        use pi_rpc::file_cache::FileCache;
        use std::time::{Duration, UNIX_EPOCH};
        let root = std::env::temp_dir().join(format!(
            "gentle-usage-cache-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let dir = root.join("--w--");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jsonl");
        let write = |cost: f64| {
            let lines = [
                json!({ "type": "session", "id": "s1", "cwd": "/w", "timestamp": iso(NOW) })
                    .to_string(),
                assistant(NOW, "anthropic", "claude", 100, 50, cost),
            ];
            std::fs::write(&path, lines.join("\n")).unwrap();
        };
        let set_mtime = |secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        let cost = |data: &super::StatsData| data.sessions[0].messages[0].cost;
        let cache = FileCache::new(8);

        write(0.5);
        set_mtime(7_000_000);
        let first = super::load_sessions_in(&cache, &root, MAX_SESSION_BYTES);
        assert_eq!(cost(&first), 0.5);

        // Same mtime and length: the cached record, not the new cost.
        write(0.7);
        set_mtime(7_000_000);
        let cached = super::load_sessions_in(&cache, &root, MAX_SESSION_BYTES);
        assert_eq!(cached.sessions, first.sessions);

        set_mtime(7_000_001);
        let fresh = super::load_sessions_in(&cache, &root, MAX_SESSION_BYTES);
        assert_eq!(cost(&fresh), 0.7);
        assert_eq!(
            fresh.sessions,
            load_sessions(&root, MAX_SESSION_BYTES).sessions
        );

        std::fs::remove_dir_all(&root).unwrap();
    }
}
