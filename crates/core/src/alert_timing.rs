//! When an alert may play: the context-high episode latch and the repeat
//! gate, mirroring Gentle Shell's notification timing. Pure: callers pass the
//! clock reading; playing and the shared state are the app's (`alerts`).

use std::time::{Duration, Instant};

use crate::notifications::Alert;

/// `NOTIFICATION_TTL_MS`: an occurrence older than this never plays.
const TTL: Duration = Duration::from_millis(2000);

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
pub struct Gate {
    last: Option<(Alert, Instant)>,
}

impl Gate {
    /// An empty gate, usable in a `static`.
    pub const fn new() -> Self {
        Gate { last: None }
    }

    pub fn admit(&mut self, alert: Alert, now: Instant, minimum_interval: Duration) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
