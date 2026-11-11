//! `gentle-open`: the small executable the agents (and the user's terminal)
//! get on PATH to open a page in the running app's in-app browser.
//!
//! It is a generated shim next to the app, not a second binary: a launch of
//! `gentle-shell-gpui --open-url <url>` goes through the single-instance
//! server and returns once the app accepted it.

use std::path::{Path, PathBuf};

/// Write (or refresh) the shim in the app-owned toolchain folder. That folder
/// is already first on the PATH every agent child receives, so `gentle-open`
/// is available in the agent's shell tool.
pub fn install_open_tool() {
    let Some(dir) = shim_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some((name, body)) = shim(&exe) else {
        return;
    };
    let path = dir.join(name);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(body.as_str()) {
        return;
    }
    if std::fs::write(&path, &body).is_ok() {
        make_executable(&path);
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
fn shim(exe: &Path) -> Option<(&'static str, String)> {
    let exe = exe.to_string_lossy().replace('\'', "'\\''");
    Some((
        "gentle-open",
        format!("#!/bin/sh\nexec '{exe}' --open-url \"$@\"\n"),
    ))
}

#[cfg(windows)]
fn shim(exe: &Path) -> Option<(&'static str, String)> {
    // `%` would be expanded by cmd.exe; the exe path is the only place it can
    // appear here.
    let exe = exe.to_string_lossy().replace('%', "%%");
    Some((
        "gentle-open.cmd",
        format!("@echo off\r\n\"{exe}\" --open-url %*\r\n"),
    ))
}
