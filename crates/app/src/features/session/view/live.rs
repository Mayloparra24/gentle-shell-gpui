//! Estado exclusivamente visual y temporizadores vivos de una sesión.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;

/// What the view tracks beside the transcript: when running things started
/// (live timers), the prompt Pi has not picked up yet, picker and dialog
/// highlights. Reset with the conversation.
#[derive(Default)]
pub(in crate::features::session) struct Live {
    pub(super) turn: Option<Instant>,
    /// Tool and shell items by transcript index.
    pub(super) tools: HashMap<usize, Span>,
    /// Thinking blocks by (item, block).
    pub(super) thinking: HashMap<(usize, usize), Span>,
    /// Spans in `tools` and `thinking` still running (their labels show tenths).
    pub(super) open_spans: usize,
    pub(super) pending: Option<Pending>,
    /// Keyboard highlight of the front dialog, by dialog id.
    pub(super) dialog_cursor: Option<(String, usize)>,
    pub(super) model_picker: bool,
    pub(super) model_cursor: usize,
    pub(super) model_query: String,
    pub(super) model_scroll: ScrollHandle,
    /// The `/` menu's keyboard highlight, for the composer text it was moved
    /// on (other text starts at the top), its scroll, and the text Esc
    /// dismissed it for.
    pub(super) slash_cursor: usize,
    pub(super) slash_query: String,
    pub(super) slash_scroll: ScrollHandle,
    pub(super) slash_dismissed: Option<String>,
    /// Where the effort knob springs from (0..=1 along the slider), when the
    /// level last changed, and a counter that restarts its animation on
    /// every change.
    pub(super) effort_from: f32,
    pub(super) effort_moved_at: Option<Instant>,
    pub(super) effort_moves: u64,
    /// Re-renders running timers while something is live.
    pub(super) ticker: Option<Task<()>>,
    /// The ticker's loop is still going (it also ends by itself once idle).
    pub(super) ticking: bool,
    /// Ticks so far, and the timer text they last painted.
    pub(super) ticks: u64,
    pub(super) stamp: Option<LabelStamp>,
    /// A click on the rail (or the jump-down button) gliding the list.
    pub(super) glide: Option<super::rail::Glide>,
    /// The rail's last `(offset, thumb top)`, so the thumb only moves the
    /// way the pane did.
    pub(super) rail_thumb: Option<(f32, f32)>,
    /// Scrolled just now: the rail shows its thumb.
    pub(super) rail_scrolling: bool,
    /// Hides the thumb a moment after the last scroll.
    pub(super) rail_hide: Option<Task<()>>,
    /// The thumb is being dragged.
    pub(super) rail_drag: Option<super::rail::RailDrag>,
    /// The rail mark under the mouse, by its position among your messages.
    pub(super) rail_hover: Option<usize>,
    /// When each answer finished streaming: its petal line and dot draw
    /// back and fade out over `ANSWER_FADE` from then (dropped once faded).
    pub(in crate::features::session) answer_done: HashMap<usize, Instant>,
}

/// How often live timers are re-checked.
pub(super) const TICK: Duration = Duration::from_millis(200);

/// What the live timer labels read: a repaint is due only when it changes.
/// `fine` is set while a tenths-of-a-second label (running tool or
/// thinking block) shows, and changes every tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LabelStamp {
    fine: Option<u64>,
    turn_secs: Option<u64>,
    pending_secs: Option<u64>,
}

pub(super) fn label_stamp(
    fine_tick: Option<u64>,
    turn: Option<Duration>,
    pending: Option<Duration>,
) -> LabelStamp {
    LabelStamp {
        fine: fine_tick,
        turn_secs: turn.map(|d| d.as_secs()),
        pending_secs: pending.map(|d| d.as_secs()),
    }
}

/// Whether live timers need ticking: a turn, a `!command` or a prompt in flight.
pub(super) fn ticker_needed(running: bool, shells: bool, pending: bool) -> bool {
    running || shells || pending
}

#[derive(Clone, Copy)]
pub(super) struct Span {
    start: Instant,
    pub(super) took: Option<Duration>,
}

impl Span {
    pub(super) fn elapsed(&self) -> Duration {
        self.took.unwrap_or_else(|| self.start.elapsed())
    }
}

/// A prompt sent while idle, shown right away until Pi's own events arrive.
pub(super) struct Pending {
    pub(super) text: String,
    /// The attachments it carries, shown until Pi echoes them.
    pub(super) images: Vec<Arc<Image>>,
    /// Transcript length when it was sent: Pi's user message lands here.
    pub(super) base: usize,
    pub(super) since: Instant,
    /// Pi answered the `prompt` request.
    pub(super) acked: Option<Instant>,
    /// Pi reported the agent running.
    pub(super) started: bool,
}

impl Live {
    pub(in crate::features::session) fn expect_reply(
        &mut self,
        text: &str,
        images: Vec<Arc<Image>>,
        base: usize,
    ) {
        self.pending = Some(Pending {
            text: text.trim().to_string(),
            images,
            base,
            since: Instant::now(),
            acked: None,
            started: false,
        });
    }

    pub(in crate::features::session) fn reply_acknowledged(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.acked.get_or_insert_with(Instant::now);
        }
    }

    pub(in crate::features::session) fn reply_failed(&mut self) {
        self.pending = None;
    }
}

/// Rate-limit usage an extension reported (`51% used (seven_day)`).
pub(in crate::features::session) struct RateLimit {
    pub(in crate::features::session) percent: u32,
    pub(in crate::features::session) text: String,
}

/// Open or close `key`'s span; returns the change in open spans.
pub(super) fn track<K: Eq + Hash>(
    map: &mut HashMap<K, Span>,
    key: K,
    live: bool,
    now: Instant,
) -> isize {
    match map.get_mut(&key) {
        Some(span) if live && span.took.is_some() => {
            *span = Span {
                start: now,
                took: None,
            };
            1
        }
        Some(span) if !live && span.took.is_none() => {
            span.took = Some(now - span.start);
            -1
        }
        Some(_) => 0,
        None if live => {
            map.insert(
                key,
                Span {
                    start: now,
                    took: None,
                },
            );
            1
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use super::{label_stamp, ticker_needed, track};

    #[test]
    fn timers_stop_once_nothing_is_live() {
        assert!(!ticker_needed(false, false, false));
        assert!(ticker_needed(true, false, false));
        assert!(ticker_needed(false, true, false));
        assert!(ticker_needed(false, false, true));
    }

    #[test]
    fn timer_repaints_only_when_labels_change() {
        let at = |ms| Some(Duration::from_millis(ms));
        assert_eq!(
            label_stamp(None, at(1_100), None),
            label_stamp(None, at(1_900), None)
        );
        assert_ne!(
            label_stamp(None, at(1_900), None),
            label_stamp(None, at(2_100), None)
        );
        assert_ne!(
            label_stamp(None, None, at(900)),
            label_stamp(None, None, at(1_000))
        );
        assert_ne!(
            label_stamp(Some(1), at(1_100), None),
            label_stamp(Some(2), at(1_300), None)
        );
    }

    #[test]
    fn live_spans_open_close_and_restart_without_double_counting() {
        let now = Instant::now();
        let mut spans = HashMap::new();

        assert_eq!(track(&mut spans, 7, true, now), 1);
        assert_eq!(track(&mut spans, 7, true, now), 0);
        assert_eq!(
            track(&mut spans, 7, false, now + Duration::from_millis(40)),
            -1
        );
        assert_eq!(
            track(&mut spans, 7, false, now + Duration::from_millis(50)),
            0
        );
        let restarted = now + Duration::from_millis(60);
        assert_eq!(track(&mut spans, 7, true, restarted), 1);
        assert_eq!(spans[&7].start, restarted);
        assert_eq!(spans[&7].took, None);
    }
}
