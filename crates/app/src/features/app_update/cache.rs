//! `app-update.json`: the six-hour cache of the latest release and the
//! "Skip this version" choice, and the blocking check and skip built on it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::network::fetch_release;
use super::release::{Outcome, Release, decide};
use super::{CURRENT, Problem};

/// A fresh answer is trusted for six hours.
const TTL_MS: u64 = 6 * 60 * 60 * 1000;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Store {
    #[serde(default)]
    checked_at: u64,
    #[serde(default)]
    latest: Option<Release>,
    #[serde(default)]
    skipped: Option<String>,
}

fn store_path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("gentle-shell-gpui")
            .join("app-update.json"),
    )
}

fn read_store(path: &Path) -> Store {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_store(path: &Path, store: &Store) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(store)? + "\n")?;
    fs::rename(&tmp, path)
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn cached(store: &Store, now: u64) -> Option<&Release> {
    store
        .latest
        .as_ref()
        .filter(|_| now.saturating_sub(store.checked_at) < TTL_MS)
}

/// Blocking: the startup (cached) or manual (fresh) answer.
pub(super) fn check_blocking(manual: bool) -> Result<Outcome, Problem> {
    let path = store_path();
    let mut store = path.as_deref().map(read_store).unwrap_or_default();
    let now = now_ms();
    let release = match cached(&store, now).filter(|_| !manual) {
        Some(release) => release.clone(),
        None => {
            let release = fetch_release()?;
            store.latest = Some(release.clone());
            store.checked_at = now;
            if let Some(path) = &path {
                let _ = write_store(path, &store);
            }
            release
        }
    };
    Ok(decide(CURRENT, release, store.skipped.as_deref(), manual))
}

pub(super) fn skip_blocking(version: &str) -> io::Result<()> {
    let path = store_path().ok_or_else(|| io::Error::other("no data directory"))?;
    let mut store = read_store(&path);
    store.skipped = Some(version.to_string());
    write_store(&path, &store)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{release, temp_dir};
    use super::*;

    #[test]
    fn caches_for_six_hours() {
        let store = Store {
            checked_at: 1_000,
            latest: Some(release("0.2.0")),
            skipped: None,
        };
        assert!(cached(&store, 1_000 + TTL_MS - 1).is_some());
        assert!(cached(&store, 1_000 + TTL_MS).is_none());
        let dir = temp_dir("store");
        let path = dir.join("app-update.json");
        write_store(&path, &store).unwrap();
        let read = read_store(&path);
        assert_eq!(read.latest, store.latest);
        assert_eq!(read.checked_at, 1_000);
        fs::remove_dir_all(&dir).unwrap();
    }
}
