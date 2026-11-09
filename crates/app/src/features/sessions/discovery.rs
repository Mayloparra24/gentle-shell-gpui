//! Finding session files on disk (per-cwd folders or a flat `sessionDir`) and
//! summarizing them in parallel, newest first.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use gentle_core::sessions::{SessionSummary, session_dir_name, sort_newest_first};
use serde_json::Value;

use super::Scope;
use super::summary::cached_summary;

/// `settings.json`'s `sessionDir`, with `~` expanded like Pi's `normalizePath`.
fn configured_session_dir(agent_home: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(agent_home.join("settings.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let dir = value.get("sessionDir")?.as_str()?.trim();
    if dir.is_empty() {
        return None;
    }
    let home = dirs::home_dir();
    Some(match (dir, home) {
        ("~", Some(home)) => home,
        (d, Some(home)) if d.starts_with("~/") => home.join(&d[2..]),
        (d, _) => PathBuf::from(d),
    })
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .collect()
}

/// Session files to inspect for `scope`, plus whether headers must match `cwd`.
fn candidate_files(agent_home: &Path, cwd: &Path, scope: Scope) -> (Vec<PathBuf>, bool) {
    if let Some(custom) = configured_session_dir(agent_home) {
        return (jsonl_files(&custom), scope == Scope::Project);
    }
    let root = agent_home.join("sessions");
    match scope {
        Scope::Project => (jsonl_files(&root.join(session_dir_name(cwd))), false),
        Scope::All => {
            let Ok(entries) = std::fs::read_dir(&root) else {
                return (Vec::new(), false);
            };
            let files = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .flat_map(|d| jsonl_files(&d))
                .collect();
            (files, false)
        }
    }
}

/// Sessions for `scope`, newest first. Blocking: call off the UI thread.
pub(super) fn list_sessions(agent_home: &Path, cwd: &Path, scope: Scope) -> Vec<SessionSummary> {
    let (files, filter_cwd) = candidate_files(agent_home, cwd, scope);
    let mut sessions: Vec<SessionSummary> = read_summaries(&files)
        .into_iter()
        .filter(|s| !filter_cwd || s.cwd == cwd)
        .collect();
    sort_newest_first(&mut sessions);
    sessions
}

/// Parse `files` on a few threads; unreadable or non-session files are dropped.
fn read_summaries(files: &[PathBuf]) -> Vec<SessionSummary> {
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8)
        .min(files.len());
    if workers <= 1 {
        return files.iter().filter_map(|f| cached_summary(f)).collect();
    }
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(files.len()));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(i) else { break };
                    if let Some(summary) = cached_summary(file) {
                        out.lock().unwrap_or_else(|e| e.into_inner()).push(summary);
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}
