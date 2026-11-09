//! Gentle Shell's notification policy (`<gentle-ai config home>/notifications.json`,
//! owned by `lib/notification-policy.ts`), so the terminal and the app share
//! one file. Mirrored rules:
//! - Schema `gentle-shell.notifications/v1`, validated exactly (unknown keys,
//!   bad timings or sounds make the whole file invalid → the default preset,
//!   whose master switch is off: silence).
//! - A missing event key is silent; `null` is off; `builtin:<id>` plays the
//!   bundled tone; `file:<absolute path>` plays that file.
//! - Context high is a fixed product threshold (`CONTEXT_HIGH_PERCENT`, 90 %,
//!   `>=`).
//!
//! Reading, writing and playing the file is the app's (`alerts`).

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    TurnEnded,
    NeedsYou,
    ContextHigh,
}

impl Alert {
    pub const ALL: [Alert; 3] = [Alert::TurnEnded, Alert::NeedsYou, Alert::ContextHigh];

    /// Gentle Shell's event name for this moment.
    pub fn event(self) -> &'static str {
        match self {
            Alert::TurnEnded => "agent.completed",
            Alert::NeedsYou => "agent.attention",
            Alert::ContextHigh => "context.high",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Alert::TurnEnded => "Turn finished",
            Alert::NeedsYou => "Needs you",
            Alert::ContextHigh => "Context high",
        }
    }

    pub fn priority(self) -> u8 {
        priority(self.event())
    }
}

pub const SCHEMA: &str = "gentle-shell.notifications/v1";
/// `lib/context-threshold.ts`: fixed, not part of notifications.json.
pub const CONTEXT_HIGH_PERCENT: f64 = 90.;

const EVENTS: [&str; 15] = [
    "session.started",
    "session.shutdown",
    "agent.started",
    "agent.completed",
    "agent.failed",
    "agent.cancelled",
    "agent.attention",
    "subagent.queued",
    "subagent.running",
    "subagent.waiting",
    "subagent.completed",
    "subagent.failed",
    "subagent.cancelled",
    "subagent.timed_out",
    "context.high",
];
const BUILTINS: [&str; 3] = ["success", "error", "attention"];

/// `NOTIFICATION_PRIORITY`: belongs to the event, never the sound.
fn priority(event: &str) -> u8 {
    if event.ends_with(".failed") || event.ends_with(".timed_out") {
        3
    } else if event == "agent.attention" || event == "context.high" {
        2
    } else if event.ends_with(".completed") {
        1
    } else {
        0
    }
}

/// The preset sound for `event` (`DEFAULT_NOTIFICATION_SETTINGS`).
fn preset_sound(event: &str) -> Value {
    match priority(event) {
        3 => "builtin:error".into(),
        2 => "builtin:attention".into(),
        1 => "builtin:success".into(),
        _ => Value::Null,
    }
}

/// The whole default file, in Gentle Shell's key order.
pub fn default_file() -> Value {
    let events: Map<String, Value> = EVENTS
        .iter()
        .map(|e| ((*e).to_string(), preset_sound(e)))
        .collect();
    json!({
        "schema": SCHEMA,
        "enabled": false,
        "audio": {
            "backend": "auto",
            "minimumIntervalMs": 1000,
            "coalesceWindowMs": 300,
            "events": events,
        }
    })
}

fn keys_match(map: &Map<String, Value>, keys: &[&str]) -> bool {
    map.len() == keys.len() && keys.iter().all(|k| map.contains_key(*k))
}

fn integer_within(value: &Value, min: f64, max: f64) -> bool {
    value
        .as_f64()
        .is_some_and(|n| n.fract() == 0. && (min..=max).contains(&n))
}

/// `/[\x00-\x1f\x7f$`]|%[^%]+%/`: control characters, shell expansion, `%VAR%`.
fn unsafe_path(path: &str) -> bool {
    if path
        .chars()
        .any(|c| c.is_ascii_control() || c == '$' || c == '`')
    {
        return true;
    }
    let percents: Vec<usize> = path.match_indices('%').map(|(i, _)| i).collect();
    percents.windows(2).any(|w| w[1] > w[0] + 1)
}

/// `isNotificationSound`: syntax only, no stat.
fn valid_sound(value: &Value, windows: bool) -> bool {
    let Some(sound) = value.as_str() else {
        return value.is_null();
    };
    if let Some(id) = sound.strip_prefix("builtin:") {
        return BUILTINS.contains(&id);
    }
    let Some(path) = sound.strip_prefix("file:") else {
        return false;
    };
    if path.is_empty() || unsafe_path(path) || path.contains("://") {
        return false;
    }
    if windows {
        let b = path.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/')
            && !path[2..].contains(':')
    } else {
        path.starts_with('/') && !path.starts_with("//")
    }
}

/// `parseNotificationSettingsFile`: the whole file, or `None` when Gentle
/// Shell would reject it. The preset is never merged in.
pub fn parse(raw: &str, windows: bool) -> Option<Value> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let root = value.as_object()?;
    if root.get("schema").and_then(Value::as_str) != Some(SCHEMA)
        || !keys_match(root, &["schema", "enabled", "audio"])
        || !root["enabled"].is_boolean()
    {
        return None;
    }
    let audio = root["audio"].as_object()?;
    if !keys_match(
        audio,
        &["backend", "minimumIntervalMs", "coalesceWindowMs", "events"],
    ) || audio["backend"] != "auto"
        || !integer_within(&audio["minimumIntervalMs"], 0., 60_000.)
        || !integer_within(&audio["coalesceWindowMs"], 0., 2000.)
    {
        return None;
    }
    let events = audio["events"].as_object()?;
    events
        .iter()
        .all(|(event, sound)| EVENTS.contains(&event.as_str()) && valid_sound(sound, windows))
        .then_some(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileState {
    /// No file: the default preset.
    Missing,
    Valid,
    /// Present but rejected: the default preset, as Gentle Shell does.
    Invalid,
    Unreadable,
}

/// What the policy is right now (`resolveNotificationSettings`).
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub file: PathBuf,
    pub state: FileState,
    /// A validated file (or the default preset), `schema` included.
    pub value: Value,
}

impl Policy {
    pub fn from_read(file: PathBuf, read: std::io::Result<String>) -> Self {
        let (state, value) = match read {
            Ok(raw) => match parse(&raw, cfg!(windows)) {
                Some(value) => (FileState::Valid, value),
                None => (FileState::Invalid, default_file()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (FileState::Missing, default_file())
            }
            Err(_) => (FileState::Unreadable, default_file()),
        };
        Self { file, state, value }
    }

    pub fn enabled(&self) -> bool {
        self.value["enabled"].as_bool().unwrap_or(false)
    }

    /// The sound for `alert`, `None` when that event is off or missing.
    pub fn sound(&self, alert: Alert) -> Option<&str> {
        self.value["audio"]["events"][alert.event()].as_str()
    }

    /// Whether `alert` plays: the master switch and its event's sound.
    pub fn plays(&self, alert: Alert) -> bool {
        self.enabled() && self.sound(alert).is_some()
    }

    pub fn minimum_interval(&self) -> Duration {
        Duration::from_millis(
            self.value["audio"]["minimumIntervalMs"]
                .as_u64()
                .unwrap_or(1000),
        )
    }

    /// The file after `change`. An invalid or unreadable file is replaced by
    /// the preset (an explicit user action, as `/gentle:customize` confirms).
    pub fn changed(&self, change: Change) -> Value {
        let mut value = match self.state {
            FileState::Valid => self.value.clone(),
            _ => default_file(),
        };
        match change {
            Change::Enabled(on) => value["enabled"] = on.into(),
            Change::Event(alert, on) => {
                let event = alert.event();
                value["audio"]["events"][event] = if on {
                    // Keep a sound the user picked; otherwise the preset.
                    match self.sound(alert) {
                        Some(s) if self.state == FileState::Valid => s.into(),
                        _ => preset_sound(event),
                    }
                } else {
                    Value::Null
                };
            }
        }
        value
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Enabled(bool),
    Event(Alert, bool),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute `file:` sound Gentle Shell accepts on this OS (Windows
    /// needs a drive letter).
    fn sound_file(name: &str) -> String {
        if cfg!(windows) {
            format!("file:C:\\s\\{name}")
        } else {
            format!("file:/s/{name}")
        }
    }

    fn policy_of(raw: &str) -> Policy {
        Policy::from_read(PathBuf::from("n.json"), Ok(raw.to_string()))
    }

    fn file_with(enabled: bool, events: Value) -> String {
        json!({
            "schema": SCHEMA,
            "enabled": enabled,
            "audio": {"backend": "auto", "minimumIntervalMs": 1000, "coalesceWindowMs": 300, "events": events}
        })
        .to_string()
    }

    #[test]
    fn missing_file_is_the_silent_preset() {
        let missing = Policy::from_read(
            PathBuf::from("n.json"),
            Err(std::io::ErrorKind::NotFound.into()),
        );
        assert_eq!(missing.state, FileState::Missing);
        assert!(!missing.enabled());
        assert_eq!(missing.sound(Alert::TurnEnded), Some("builtin:success"));
        assert_eq!(missing.sound(Alert::NeedsYou), Some("builtin:attention"));
        assert_eq!(missing.sound(Alert::ContextHigh), Some("builtin:attention"));
        assert!(Alert::ALL.iter().all(|a| !missing.plays(*a)));
        let events = &missing.value["audio"]["events"];
        assert_eq!(events["agent.failed"], "builtin:error");
        assert_eq!(events["subagent.timed_out"], "builtin:error");
        assert_eq!(events["agent.started"], Value::Null);
        // Round-trips through Gentle Shell's own validation.
        assert!(parse(&missing.value.to_string(), false).is_some());
    }

    #[test]
    fn partial_events_are_silent_not_merged() {
        let p = policy_of(&file_with(
            true,
            json!({"agent.attention": "builtin:error"}),
        ));
        assert_eq!(p.state, FileState::Valid);
        assert!(p.plays(Alert::NeedsYou));
        assert_eq!(p.sound(Alert::NeedsYou), Some("builtin:error"));
        assert!(!p.plays(Alert::TurnEnded));
        assert!(!p.plays(Alert::ContextHigh));
    }

    #[test]
    fn per_event_enablement_follows_master_and_null() {
        let events = json!({"agent.completed": null, "context.high": sound_file("x.wav")});
        let on = policy_of(&file_with(true, events.clone()));
        assert!(!on.plays(Alert::TurnEnded));
        assert!(on.plays(Alert::ContextHigh));
        let off = policy_of(&file_with(false, events));
        assert!(!off.plays(Alert::ContextHigh));
    }

    #[test]
    fn malformed_files_fall_back_to_defaults() {
        let bad: [&str; 13] = [
            "not json",
            "[]",
            &file_with(true, json!({"agent.bogus": null})),
            &file_with(true, json!({"agent.completed": "builtin:nope"})),
            &file_with(true, json!({"agent.completed": "file:relative.wav"})),
            &file_with(true, json!({"agent.completed": "file:/a/$HOME.wav"})),
            &file_with(true, json!({"agent.completed": "file://host/x.wav"})),
            &file_with(true, json!({"agent.completed": "file:/%USER%/x.wav"})),
            r#"{"schema":"other","enabled":true,"audio":{"backend":"auto","minimumIntervalMs":1000,"coalesceWindowMs":300,"events":{}}}"#,
            r#"{"schema":"gentle-shell.notifications/v1","enabled":true,"extra":1,"audio":{"backend":"auto","minimumIntervalMs":1000,"coalesceWindowMs":300,"events":{}}}"#,
            r#"{"schema":"gentle-shell.notifications/v1","enabled":true,"audio":{"backend":"auto","minimumIntervalMs":1000.5,"coalesceWindowMs":300,"events":{}}}"#,
            r#"{"schema":"gentle-shell.notifications/v1","enabled":true,"audio":{"backend":"auto","minimumIntervalMs":1000,"coalesceWindowMs":2001,"events":{}}}"#,
            r#"{"schema":"gentle-shell.notifications/v1","enabled":"yes","audio":{"backend":"auto","minimumIntervalMs":1000,"coalesceWindowMs":300,"events":{}}}"#,
        ];
        for raw in bad {
            let p = policy_of(raw);
            assert_eq!(p.state, FileState::Invalid, "{raw}");
            assert_eq!(p.value, default_file());
            assert!(!p.enabled());
        }
        // Integral floats and `%` alone are still valid, as in JS.
        let ok = json!({
            "schema": SCHEMA,
            "enabled": true,
            "audio": {"backend": "auto", "minimumIntervalMs": 1000.0, "coalesceWindowMs": 0,
                "events": {"agent.completed": sound_file("100%.wav")}},
        });
        assert_eq!(policy_of(&ok.to_string()).state, FileState::Valid);
    }

    #[test]
    fn windows_paths_need_a_drive() {
        assert!(valid_sound(&json!("file:C:\\Sounds\\a.wav"), true));
        assert!(valid_sound(&json!("file:d:/a.wav"), true));
        assert!(!valid_sound(&json!("file:/a.wav"), true));
        assert!(!valid_sound(&json!("file:C:a.wav"), true));
        assert!(!valid_sound(&json!("file:C:\\a:b.wav"), true));
        assert!(!valid_sound(&json!("file://a.wav"), false));
    }

    #[test]
    fn changes_keep_the_rest_of_the_file() {
        let raw = file_with(
            true,
            json!({"agent.completed": sound_file("done.wav"), "subagent.failed": "builtin:error"}),
        );
        let p = policy_of(&raw);
        let off = p.changed(Change::Event(Alert::TurnEnded, false));
        assert_eq!(off["audio"]["events"]["agent.completed"], Value::Null);
        assert_eq!(off["audio"]["events"]["subagent.failed"], "builtin:error");
        let on = p.changed(Change::Event(Alert::TurnEnded, true));
        assert_eq!(
            on["audio"]["events"]["agent.completed"],
            sound_file("done.wav")
        );
        let attention = p.changed(Change::Event(Alert::NeedsYou, true));
        assert_eq!(
            attention["audio"]["events"]["agent.attention"],
            "builtin:attention"
        );
        let disabled = p.changed(Change::Enabled(false));
        assert_eq!(disabled["enabled"], false);
        assert_eq!(
            disabled["audio"]["events"]["agent.completed"],
            sound_file("done.wav")
        );
        for value in [off, on, attention, disabled] {
            assert!(parse(&value.to_string(), cfg!(windows)).is_some());
        }
        // An invalid file is replaced by the preset plus the change.
        let replaced = policy_of("{").changed(Change::Enabled(true));
        let mut expected = default_file();
        expected["enabled"] = true.into();
        assert_eq!(replaced, expected);
    }
}
