//! "Is there a newer Gentle Shell?", answered the way the launcher's own
//! interactive startup check answers it (`lib/gentle-shell-startup-update.ts`):
//! the npm registry's `gentle-pi/latest`, stable versions only, a one-day
//! cache and the "Skip this version" choice, both kept in the launcher's
//! `~/.gentle-shell/config.json` so the app and the terminal agree.
//!
//! Version parsing, comparison and the offer are `gentle_core::version`.
//! Here, `registry` asks npm and `cache` reads and writes the launcher config.

mod cache;
mod registry;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gentle_core::version::offer_for;
use serde_json::Value;

pub use gentle_core::version::{UpdateOffer, current_from_versions};

/// The launcher config: `GENTLE_SHELL_CONFIG`, else `~/.gentle-shell/config.json`.
pub fn config_path() -> Option<PathBuf> {
    match std::env::var_os("GENTLE_SHELL_CONFIG") {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => dirs::home_dir().map(|home| home.join(".gentle-shell").join("config.json")),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Blocking: the offer for `current`, from the day-old cache or the registry.
/// Offline or unreadable answers mean "no offer", never an error.
pub fn check(current: &str) -> Option<UpdateOffer> {
    let path = config_path()?;
    let config = cache::read_config(&path);
    let skipped = config
        .get("skippedStartupRelease")
        .and_then(Value::as_str)
        .map(str::to_string);
    let now = now_ms();
    let latest = match cache::cached_latest(&config, now) {
        Some(latest) => latest,
        None => {
            let latest = registry::fetch_latest()?;
            // Like the launcher: only a real answer is worth a day's trust.
            let record =
                serde_json::json!({"checkedAt": cache::format_iso_ms(now), "latest": latest});
            let _ = cache::write_key(&path, "startupReleaseCheck", record);
            latest
        }
    };
    offer_for(current, &latest, skipped.as_deref())
}

/// "Skip this version": the key the terminal's startup prompt writes too.
pub fn skip(version: &str) -> std::io::Result<()> {
    let path = config_path().ok_or_else(|| std::io::Error::other("no home directory"))?;
    cache::write_key(&path, "skippedStartupRelease", Value::from(version))
}
