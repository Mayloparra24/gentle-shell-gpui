//! Sounds and window attention for agent moments, on Gentle Shell's own
//! notification policy (`<gentle-ai config home>/notifications.json`; parsed
//! and validated by `gentle_core::notifications`). This module reads, caches
//! and writes the file and plays the sounds.
//!
//! Gentle Shell never plays these over RPC (`notificationContextAllowed`: only
//! the TUI is an audio host), so the app is the only player for its tabs.
//! Mirrored rules:
//! - Focus is not part of the policy: the terminal plays whether or not it is
//!   frontmost. The app adds a Dock bounce / taskbar flash when inactive.
//! - Context high is latched once per episode ([`ContextLatch`]).
//! - One playback at a time; identical alerts within the 2 s occurrence TTL
//!   and lower-or-equal priority alerts inside `minimumIntervalMs` are dropped.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gentle_core::notifications::{Alert, CONTEXT_HIGH_PERCENT, Change, Policy};
use pi_rpc::file_cache::Stamp;
use serde_json::Value;

/// `NOTIFICATION_TTL_MS`: an occurrence older than this never plays.
const TTL: Duration = Duration::from_millis(2000);
/// Gentle Shell's bounded playback deadline.
const PLAYBACK_TIMEOUT: Duration = Duration::from_millis(11_000);

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

/// Context-usage threshold (percent). Gentle Shell fixes it in
/// `lib/context-threshold.ts`; notifications.json has no key for it.
pub fn context_threshold(_cx: &gpui_kit::App) -> f64 {
    CONTEXT_HIGH_PERCENT
}

/// `ContextHighLatch`: one alert per high episode. A finite reading below the
/// threshold rearms it; an unknown reading (`None`, right after a compaction)
/// keeps it. Call [`ContextLatch::rearm`] after a successful compaction or a
/// session switch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextLatch {
    high: bool,
}

impl ContextLatch {
    /// `true` exactly once while `percent` stays at or above `threshold`.
    pub fn observe(&mut self, percent: Option<f64>, threshold: f64) -> bool {
        let Some(percent) = percent.filter(|p| p.is_finite()) else {
            return false;
        };
        if percent < threshold {
            self.high = false;
            return false;
        }
        !std::mem::replace(&mut self.high, true)
    }

    pub fn rearm(&mut self) {
        self.high = false;
    }
}

/// Drops repeats: the same alert within the occurrence TTL, and anything not
/// louder than the last played alert within `minimumIntervalMs`.
#[derive(Debug, Default)]
struct Gate {
    last: Option<(Alert, Instant)>,
}

impl Gate {
    fn admit(&mut self, alert: Alert, now: Instant, minimum_interval: Duration) -> bool {
        if let Some((last, at)) = self.last {
            let since = now.saturating_duration_since(at);
            if (last == alert && since < TTL)
                || (since < minimum_interval && alert.priority() <= last.priority())
            {
                return false;
            }
        }
        self.last = Some((alert, now));
        true
    }
}

static GATE: Mutex<Gate> = Mutex::new(Gate { last: None });
/// One playback at a time, like Gentle Shell's shared playback reservation.
static PLAYING: AtomicBool = AtomicBool::new(false);

/// Play/attend per the user's Gentle Shell notifications.json policy.
/// `window_active` = the app window is frontmost and focused.
pub fn fire(alert: Alert, window_active: bool, cx: &mut gpui_kit::App) {
    let policy = policy();
    if !policy.plays(alert) {
        return;
    }
    let admitted = GATE.lock().unwrap_or_else(|e| e.into_inner()).admit(
        alert,
        Instant::now(),
        policy.minimum_interval(),
    );
    if !admitted {
        return;
    }
    if let Some(sound) = policy.sound(alert) {
        play(sound.to_string());
    }
    if !window_active {
        // Deferred: `fire` may run inside a window update. Each platform
        // skips its active window (Dock bounce, taskbar flash, X11 urgency).
        cx.defer(|cx| {
            for window in cx.windows() {
                let _ = window.update(cx, |_, window, _| window.request_attention());
            }
        });
    }
}

fn play(sound: String) {
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

/// Gentle Shell's `assets/sounds/<id>.wav` (MIT, Gentle AI contributors),
/// rebuilt from its published recipe: sine tones, mono 16-bit 22 050 Hz.
fn builtin_wav(id: &str) -> Option<Vec<u8>> {
    const RATE: u32 = 22_050;
    let tones: &[f64] = match id {
        "success" => &[660., 880.],
        "error" => &[330., 220.],
        "attention" => &[880., 880., 880.],
        _ => return None,
    };
    let count = 2646usize;
    let mut samples: Vec<i16> = Vec::with_capacity(tones.len() * (count + 661));
    for &frequency in tones {
        for i in 0..count {
            let envelope = 1f64.min(i as f64 / 220.).min((count - 1 - i) as f64 / 440.);
            let t = 2. * std::f64::consts::PI * frequency * i as f64 / RATE as f64;
            samples.push((4000. * envelope * t.sin()).round() as i16);
        }
        samples.extend(std::iter::repeat_n(0, 661));
    }
    let data = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    Some(wav)
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

    #[test]
    fn gate_drops_repeats_and_quieter_alerts() {
        let mut gate = Gate::default();
        let t = Instant::now();
        let interval = Duration::from_millis(1000);
        assert!(gate.admit(Alert::TurnEnded, t, interval));
        // Same alert inside the 2 s TTL.
        assert!(!gate.admit(Alert::TurnEnded, t + Duration::from_millis(1500), interval));
        // Louder alert inside the minimum interval still plays.
        assert!(gate.admit(Alert::NeedsYou, t + Duration::from_millis(1600), interval));
        // Quieter or equal priority inside the interval does not.
        assert!(!gate.admit(Alert::TurnEnded, t + Duration::from_millis(2000), interval));
        assert!(!gate.admit(
            Alert::ContextHigh,
            t + Duration::from_millis(2100),
            interval
        ));
        // After both windows anything plays again.
        assert!(gate.admit(Alert::TurnEnded, t + Duration::from_millis(3700), interval));
        assert!(gate.admit(
            Alert::NeedsYou,
            t + Duration::from_millis(5800),
            Duration::ZERO
        ));
    }

    #[test]
    fn context_latch_fires_once_per_episode() {
        let mut latch = ContextLatch::default();
        assert!(!latch.observe(Some(89.9), 90.));
        assert!(latch.observe(Some(90.), 90.));
        assert!(!latch.observe(Some(95.), 90.));
        // Unknown readings (after a compaction) keep the episode.
        assert!(!latch.observe(None, 90.));
        assert!(!latch.observe(Some(f64::NAN), 90.));
        assert!(!latch.observe(Some(96.), 90.));
        assert!(!latch.observe(Some(40.), 90.));
        assert!(latch.observe(Some(91.), 90.));
        latch.rearm();
        assert!(latch.observe(Some(91.), 90.));
    }

    #[test]
    fn builtin_tones_match_gentle_shell_sizes() {
        assert_eq!(builtin_wav("success").unwrap().len(), 13_272);
        assert_eq!(builtin_wav("error").unwrap().len(), 13_272);
        assert_eq!(builtin_wav("attention").unwrap().len(), 19_886);
        assert!(builtin_wav("other").is_none());
        assert_eq!(&builtin_wav("success").unwrap()[..4], b"RIFF");
    }
}
