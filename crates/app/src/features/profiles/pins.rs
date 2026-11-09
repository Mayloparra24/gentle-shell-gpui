//! Repository pin files: reading and writing the clone pin and the
//! committed declaration, the Git worktree they belong to, and following a
//! renamed profile.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use gentle_core::profiles::{PinRead, PinStatus, is_valid_profile_name, parse_pin, serialize_pin};

use super::files::{restore_bytes, write_atomic};

fn read_pin(path: &Path) -> PinRead {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => PinRead::Missing,
        Err(_) => PinRead::Invalid,
        Ok(text) => parse_pin(&text).map_or(PinRead::Invalid, PinRead::Valid),
    }
}

pub(super) fn write_pin(path: &Path, profile: &str) -> Result<(), String> {
    if !is_valid_profile_name(profile) {
        return Err(format!("Invalid profile name: {profile:?}."));
    }
    write_atomic(path, &serialize_pin(profile)).map_err(|e| e.to_string())
}

pub(super) fn clear_pin(path: &Path) -> io::Result<()> {
    restore_bytes(path, None)
}

/// `resolveSessionWorktreeWithGit`: worktree root and Git common directory.
fn worktree_identity(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    let git = |arg: &str| -> Option<PathBuf> {
        let output = pi_rpc::command("git")
            .args(["--no-optional-locks", "-C"])
            .arg(cwd)
            .args(["rev-parse", "--path-format=absolute", arg])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        dunce::canonicalize(text.trim_end_matches(['\n', '\r'])).ok()
    };
    Some((git("--show-toplevel")?, git("--git-common-dir")?))
}

pub fn read_pin_status(cwd: &Path) -> Option<PinStatus> {
    let (root, common) = worktree_identity(cwd)?;
    Some(pin_status_at(root, &common))
}

pub(super) fn pin_status_at(root: PathBuf, common: &Path) -> PinStatus {
    let local_path = common.join("gentle-ai").join("profile-pin.json");
    let repo_path = root.join(".pi").join("gentle-ai").join("profile.json");
    PinStatus {
        local: read_pin(&local_path),
        repo: read_pin(&repo_path),
        local_path,
        repo_path,
    }
}

/// Rename follows the clone pin; a committed declaration is only reported.
pub(super) fn follow_renamed_pin(cwd: &Path, from: &str, to: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Some(status) = read_pin_status(cwd) else {
        return (Vec::new(), Vec::new());
    };
    let mut followed = Vec::new();
    let mut declared = Vec::new();
    if status.local == PinRead::Valid(from.to_string()) && write_pin(&status.local_path, to).is_ok()
    {
        followed.push(status.local_path.clone());
    }
    if status.repo == PinRead::Valid(from.to_string()) {
        declared.push(status.repo_path.clone());
    }
    (followed, declared)
}
