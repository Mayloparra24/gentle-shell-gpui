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
//!
//! Layout: `policy` (notifications.json cache and write), `sound` (player
//! process), `tones` (built-in WAVs); timing lives in
//! `gentle_core::alert_timing`.

use std::sync::Mutex;
use std::time::Instant;

use gentle_core::alert_timing::Gate;
use gentle_core::notifications::{Alert, CONTEXT_HIGH_PERCENT};

mod policy;
mod sound;
mod tones;

pub use gentle_core::alert_timing::ContextLatch;
pub use policy::{apply, policy};

/// Context-usage threshold (percent). Gentle Shell fixes it in
/// `lib/context-threshold.ts`; notifications.json has no key for it.
pub fn context_threshold(_cx: &gpui_kit::App) -> f64 {
    CONTEXT_HIGH_PERCENT
}

static GATE: Mutex<Gate> = Mutex::new(Gate::new());

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
        sound::play(sound.to_string());
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
