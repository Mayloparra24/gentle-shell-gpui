//! The small commands agents (and the user's terminal) get on PATH to reach
//! the running app: `gentle-open <url>` shows a page in the in-app browser,
//! `gentle-device …` opens and lists simulators and emulators in the device
//! panel.
//!
//! They are generated shims next to the app, not more binaries: each runs
//! `gentle-shell-gpui` with a flag (`--open-url`, `--device`), which hands
//! the request to the running app through the single-instance server.

use std::path::{Path, PathBuf};

/// Write (or refresh) the shims in the app-owned toolchain folder. That
/// folder is already first on the PATH every agent child receives.
pub fn install() {
    let Some(dir) = shim_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    for (name, flag) in [("gentle-open", "--open-url"), ("gentle-device", "--device")] {
        let (file, body) = shim(&exe, name, flag);
        let path = dir.join(file);
        if std::fs::read_to_string(&path).ok().as_deref() == Some(body.as_str()) {
            continue;
        }
        if std::fs::write(&path, &body).is_ok() {
            make_executable(&path);
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
}

/// Windows runs the `.cmd` shim as is.
#[cfg(not(unix))]
fn make_executable(_: &Path) {}

fn shim_dir() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("gentle-shell-gpui")
            .join("toolchain")
            .join("bin"),
    )
}

#[cfg(unix)]
fn shim(exe: &Path, name: &str, flag: &str) -> (String, String) {
    let exe = exe.to_string_lossy().replace('\'', "'\\''");
    (
        name.to_string(),
        format!("#!/bin/sh\nexec '{exe}' {flag} \"$@\"\n"),
    )
}

#[cfg(windows)]
fn shim(exe: &Path, name: &str, flag: &str) -> (String, String) {
    // `%` would be expanded by cmd.exe; the exe path is the only place it can
    // appear here.
    let exe = exe.to_string_lossy().replace('%', "%%");
    (
        format!("{name}.cmd"),
        format!("@echo off\r\n\"{exe}\" {flag} %*\r\n"),
    )
}
