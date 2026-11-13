//! The water reminder: whether it is on, how often it shows, and when the
//! next one is due. The clock starts when the app opens; reminders fall on
//! whole multiples of the interval from then, so changing the interval
//! keeps counting from the same start instead of restarting the wait.

use std::time::Duration;

use serde_json::{Value, json};

/// The intervals the settings offer, in minutes.
pub const INTERVALS: [u32; 6] = [20, 30, 45, 60, 90, 120];
/// The interval a fresh install uses, in minutes.
pub const DEFAULT_MINUTES: u32 = 60;

/// The `hydration` key of the app's `settings.json`:
/// `{ "enabled": true, "everyMinutes": 60 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reminder {
    pub enabled: bool,
    pub every_minutes: u32,
}

impl Default for Reminder {
    fn default() -> Self {
        Self {
            enabled: true,
            every_minutes: DEFAULT_MINUTES,
        }
    }
}

impl Reminder {
    /// Read the `hydration` value; anything missing or out of the offered
    /// intervals falls back to the default for that field.
    pub fn from_json(value: &Value) -> Self {
        let default = Self::default();
        Self {
            enabled: value["enabled"].as_bool().unwrap_or(default.enabled),
            every_minutes: value["everyMinutes"]
                .as_u64()
                .and_then(|m| u32::try_from(m).ok())
                .filter(|m| INTERVALS.contains(m))
                .unwrap_or(default.every_minutes),
        }
    }

    pub fn to_json(self) -> Value {
        json!({ "enabled": self.enabled, "everyMinutes": self.every_minutes })
    }

    pub fn every(self) -> Duration {
        Duration::from_secs(u64::from(self.every_minutes.max(1)) * 60)
    }

    /// How long from `elapsed` (time since the app opened) to the next
    /// reminder; `None` while it is off. Never zero: a reminder due right
    /// now has just shown, so the next one is a whole interval away.
    pub fn until_next(self, elapsed: Duration) -> Option<Duration> {
        if !self.enabled {
            return None;
        }
        let every = self.every().as_millis();
        let into = elapsed.as_millis() % every;
        Some(Duration::from_millis((every - into) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn reminders_fall_on_multiples_of_the_interval_since_launch() {
        let r = Reminder {
            enabled: true,
            every_minutes: 30,
        };
        assert_eq!(r.until_next(Duration::ZERO), Some(30 * MIN));
        assert_eq!(r.until_next(10 * MIN), Some(20 * MIN));
        // Right on a reminder: the next one is a whole interval away.
        assert_eq!(r.until_next(60 * MIN), Some(30 * MIN));
        // A longer interval chosen 50 min in still counts from launch.
        let longer = Reminder {
            every_minutes: 45,
            ..r
        };
        assert_eq!(longer.until_next(50 * MIN), Some(40 * MIN));
        assert_eq!(
            Reminder {
                enabled: false,
                ..r
            }
            .until_next(10 * MIN),
            None
        );
    }

    #[test]
    fn settings_round_trip_and_fall_back_field_by_field() {
        let r = Reminder {
            enabled: false,
            every_minutes: 90,
        };
        assert_eq!(Reminder::from_json(&r.to_json()), r);
        assert_eq!(Reminder::from_json(&Value::Null), Reminder::default());
        // An interval the settings don't offer keeps the switch, not the value.
        let odd = Reminder::from_json(&json!({ "enabled": false, "everyMinutes": 7 }));
        assert_eq!(
            odd,
            Reminder {
                enabled: false,
                every_minutes: DEFAULT_MINUTES
            }
        );
    }
}
