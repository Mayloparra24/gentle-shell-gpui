//! Restarting into the installed update: a detached per-OS helper that waits
//! for this process to exit, and cleanup of what a Windows update left.

use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use super::install::old_exe;

/// A detached helper that waits for `pid` to exit, then opens `target`. It
/// starts after this app released the single-instance guard, so the new app
/// becomes the primary instead of forwarding to the dying one.
fn relaunch_command(target: &Path, pid: u32) -> Command {
    if cfg!(windows) {
        let quoted = target.display().to_string().replace('\'', "''");
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &format!(
                "Wait-Process -Id {pid} -Timeout 60 -ErrorAction SilentlyContinue; Start-Process -FilePath '{quoted}'"
            ),
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }
        command
    } else {
        let launch = if cfg!(target_os = "macos") {
            "exec open -n \"$1\""
        } else {
            "exec \"$1\""
        };
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                "i=0; while kill -0 {pid} 2>/dev/null && [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done; {launch}"
            ))
            .arg("sh")
            .arg(target);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        command
    }
}

pub(super) fn spawn_relaunch(target: &Path) -> io::Result<()> {
    relaunch_command(target, std::process::id())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// The previous exe a Windows update left behind.
pub(super) fn remove_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(old_exe(&exe));
    }
}
