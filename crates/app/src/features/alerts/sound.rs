//! Sound playback: one player process at a time on a background thread,
//! bounded by a deadline; built-in tones are cached as WAV files first.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::tones::builtin_wav;

/// Gentle Shell's bounded playback deadline.
const PLAYBACK_TIMEOUT: Duration = Duration::from_millis(11_000);

/// One playback at a time, like Gentle Shell's shared playback reservation.
static PLAYING: AtomicBool = AtomicBool::new(false);

pub(super) fn play(sound: String) {
    if PLAYING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("alert-sound".into())
        .spawn(move || {
            if let Some(path) = sound_path(&sound) {
                run_player(&path);
            }
            PLAYING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        PLAYING.store(false, Ordering::Release);
    }
}

/// The file to play: the user's, or the bundled tone written once to the cache.
fn sound_path(sound: &str) -> Option<PathBuf> {
    if let Some(path) = sound.strip_prefix("file:") {
        return Some(PathBuf::from(path));
    }
    let id = sound.strip_prefix("builtin:")?;
    let bytes = builtin_wav(id)?;
    let dir = dirs::cache_dir()?.join("gentle-shell-gpui").join("sounds");
    let path = dir.join(format!("{id}.wav"));
    let current = std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64);
    if !current {
        std::fs::create_dir_all(&dir).ok()?;
        let temp = dir.join(format!("{id}.{}.tmp", std::process::id()));
        std::fs::write(&temp, &bytes).ok()?;
        std::fs::rename(&temp, &path).ok()?;
    }
    Some(path)
}

/// Gentle Shell's players: `afplay` on macOS, the first of
/// `paplay`/`pw-play`/`aplay` on Linux (nothing found: silent), and on Windows
/// the fixed system PowerShell playing WAV through .NET `SoundPlayer`, hidden.
#[cfg(target_os = "macos")]
fn player_command(path: &Path) -> Option<std::process::Command> {
    let mut command = pi_rpc::command("/usr/bin/afplay");
    command.arg(path);
    Some(command)
}

#[cfg(target_os = "windows")]
fn player_command(path: &Path) -> Option<std::process::Command> {
    // Same trust anchor as Gentle Shell's adapter; the path travels in the
    // environment, so no byte of it becomes command text.
    let mut command =
        pi_rpc::command("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            "(New-Object System.Media.SoundPlayer $env:GENTLE_ALERT_SOUND).PlaySync()",
        ])
        .env("GENTLE_ALERT_SOUND", path);
    Some(command)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn player_command(path: &Path) -> Option<std::process::Command> {
    let player = ["/usr/bin/paplay", "/usr/bin/pw-play", "/usr/bin/aplay"]
        .into_iter()
        .find(|p| Path::new(p).is_file())?;
    let mut command = pi_rpc::command(player);
    command.arg(path);
    Some(command)
}

fn run_player(path: &Path) {
    use std::process::Stdio;
    let Some(mut command) = player_command(path) else {
        return;
    };
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let started = Instant::now();
    while let Ok(None) = child.try_wait() {
        if started.elapsed() >= PLAYBACK_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
