//! notifications.json IO: the cached policy, re-read only when the file
//! changes, and the atomic write behind [`apply`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gentle_core::notifications::{Change, Policy};
use pi_rpc::file_cache::Stamp;
use serde_json::Value;

fn policy_file() -> PathBuf {
    pi_rpc::resolve_homes().config.join("notifications.json")
}

static POLICY: Mutex<Option<(PathBuf, Option<Stamp>, Policy)>> = Mutex::new(None);

/// The current policy, re-read only when the file changes.
pub fn policy() -> Policy {
    let file = policy_file();
    let stamp = Stamp::of(&file);
    let mut cached = POLICY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((path, at, policy)) = cached.as_ref()
        && *path == file
        && *at == stamp
        && stamp.is_some()
    {
        return policy.clone();
    }
    let policy = Policy::from_read(file.clone(), std::fs::read_to_string(&file));
    *cached = Some((file, stamp, policy.clone()));
    policy
}

/// Apply `change` and write the file the way `writeNotificationSettings`
/// does: one JSON line, private temp file, atomic rename.
pub fn apply(change: Change) -> std::io::Result<PathBuf> {
    let current = policy();
    let value = current.changed(change);
    write_file(&current.file, &value)?;
    *POLICY.lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(current.file)
}

fn write_file(path: &Path, value: &Value) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let temp = dir.join(format!(
        "notifications.json.{}-{nanos}.tmp",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = (|| {
        use std::io::Write as _;
        let mut file = options.open(&temp)?;
        file.write_all(format!("{value}\n").as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use gentle_core::notifications::{default_file, parse};

    #[test]
    fn written_files_parse_back() {
        let dir = std::env::temp_dir().join(format!("alerts-test-{}", std::process::id()));
        let path = dir.join("notifications.json");
        let mut value = default_file();
        value["enabled"] = true.into();
        write_file(&path, &value).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with('\n') && !raw.trim_end().contains('\n'));
        assert!(raw.starts_with(r#"{"schema":"gentle-shell.notifications/v1","enabled":true"#));
        assert_eq!(parse(&raw, false), Some(value));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
