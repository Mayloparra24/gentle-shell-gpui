//! Summarizing one session file: a light JSONL scan mirroring Pi's
//! `buildSessionInfo`, capped at `MAX_SCAN_BYTES` and cached by mtime and
//! length.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use gentle_core::sessions::{MAX_SCAN_BYTES, SessionSummary, parse_iso_ms};
use pi_rpc::file_cache::{FileCache, Stamp};
use serde::Deserialize;
use serde_json::Value;

/// One JSONL line, keeping only the small fields we need. Unknown fields
/// (including message content) are skipped by serde without being built.
#[derive(Deserialize)]
struct LightEntry {
    #[serde(rename = "type")]
    kind: Option<Value>,
    id: Option<Value>,
    timestamp: Option<Value>,
    cwd: Option<Value>,
    name: Option<Value>,
    #[serde(rename = "modelId")]
    model_id: Option<Value>,
    message: Option<LightMessage>,
}

#[derive(Deserialize)]
struct LightMessage {
    role: Option<Value>,
    timestamp: Option<Value>,
    model: Option<Value>,
}

#[derive(Deserialize)]
struct ContentEntry {
    message: ContentMessage,
}

#[derive(Deserialize)]
struct ContentMessage {
    content: Value,
}

fn str_of(v: &Option<Value>) -> Option<&str> {
    v.as_ref().and_then(Value::as_str)
}

/// Text blocks of a message's `content` (string or block array), space-joined.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Summaries by file, so reopening the browser only parses new or changed
/// sessions. Holds non-session files too, as `None`.
static SUMMARIES: FileCache<Option<SessionSummary>> = FileCache::new(8_192);

pub(super) fn cached_summary(path: &Path) -> Option<SessionSummary> {
    cached_summary_in(&SUMMARIES, path)
}

pub(super) fn cached_summary_in(
    cache: &FileCache<Option<SessionSummary>>,
    path: &Path,
) -> Option<SessionSummary> {
    let file = File::open(path).ok()?;
    let Ok(meta) = file.metadata() else {
        return summarize(file, path).ok().flatten();
    };
    let stamp = Stamp::from_metadata(&meta);
    if let Some(summary) = cache.get(path, stamp) {
        return summary;
    }
    // A read error is not cached: the same bytes may read fine next time.
    let summary = summarize(file, path).ok()?;
    cache.insert(path, stamp, summary.clone());
    summary
}

/// Summarize one session file the way Pi's `buildSessionInfo` does.
/// `Ok(None)` for a file that is not a session.
pub(super) fn summarize(file: File, path: &Path) -> std::io::Result<Option<SessionSummary>> {
    let mtime_ms = file
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut scanned = 0u64;
    let mut header: Option<(String, PathBuf, Option<i64>)> = None;
    let mut name = None;
    let mut first_message: Option<String> = None;
    let mut message_count = 0;
    let mut last_activity: Option<i64> = None;
    let mut model = None;
    let mut truncated = false;
    loop {
        if scanned >= MAX_SCAN_BYTES {
            truncated = !reader.fill_buf().map_or(true, |b| b.is_empty());
            break;
        }
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        scanned += read as u64;
        let Ok(entry) = serde_json::from_slice::<LightEntry>(&line) else {
            continue;
        };
        let kind = str_of(&entry.kind).unwrap_or_default();
        let Some(_) = &header else {
            // The first parsed entry must be the header.
            let Some(id) = str_of(&entry.id) else {
                return Ok(None);
            };
            if kind != "session" {
                return Ok(None);
            }
            let cwd = PathBuf::from(str_of(&entry.cwd).unwrap_or_default());
            header = Some((
                id.to_string(),
                cwd,
                str_of(&entry.timestamp).and_then(parse_iso_ms),
            ));
            continue;
        };
        match kind {
            "session_info" => {
                name = str_of(&entry.name)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string);
            }
            "model_change" => {
                if let Some(id) = str_of(&entry.model_id) {
                    model = Some(id.to_string());
                }
            }
            "message" => {
                message_count += 1;
                let Some(message) = &entry.message else {
                    continue;
                };
                let role = str_of(&message.role).unwrap_or_default();
                if role != "user" && role != "assistant" {
                    continue;
                }
                let at = message
                    .timestamp
                    .as_ref()
                    .and_then(Value::as_f64)
                    .map(|t| t as i64)
                    .or_else(|| str_of(&entry.timestamp).and_then(parse_iso_ms));
                if let Some(at) = at {
                    last_activity = Some(last_activity.map_or(at, |l| l.max(at)));
                }
                if role == "assistant" {
                    if let Some(m) = str_of(&message.model) {
                        model = Some(m.to_string());
                    }
                } else if first_message.is_none()
                    && let Ok(full) = serde_json::from_slice::<ContentEntry>(&line)
                {
                    let text = content_text(&full.message.content);
                    if !text.is_empty() {
                        first_message = Some(text);
                    }
                }
            }
            _ => {}
        }
    }
    let Some((id, cwd, created_ms)) = header else {
        return Ok(None);
    };
    let modified_ms = if truncated {
        mtime_ms.max(last_activity.unwrap_or(0))
    } else {
        last_activity
            .filter(|t| *t > 0)
            .or(created_ms)
            .unwrap_or(mtime_ms)
    };
    Ok(Some(SessionSummary {
        path: path.to_path_buf(),
        id,
        cwd,
        name,
        first_message,
        modified_ms,
        message_count,
        truncated,
        model,
    }))
}
