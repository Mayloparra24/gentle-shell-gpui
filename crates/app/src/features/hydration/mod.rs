//! The water reminder: from the moment the app opens, every so often a
//! shower of pixel drops falls over the window and fills a glass. On or
//! off and how often live in Configuration → Water reminder and in the
//! app's `settings.json` (`hydration`); the timing rules are
//! `gentle_core::hydration`.

use std::time::{Duration, Instant};

use gentle_core::hydration::{INTERVALS, Reminder};
use gpui_kit::*;

use crate::shared::themes;

mod drops;

pub use drops::DURATION;

/// Key of the app's `settings.json`.
const KEY: &str = "hydration";
/// After a reminder shows, the next wait is measured from this much later,
/// so a timer that wakes a hair early can't count the same one twice.
const SETTLE: Duration = Duration::from_secs(1);

/// What Configuration and the card call each of [`INTERVALS`].
pub const LABELS: [&str; INTERVALS.len()] =
    ["20 min", "30 min", "45 min", "1 hour", "90 min", "2 hours"];

pub fn label(minutes: u32) -> &'static str {
    INTERVALS
        .iter()
        .position(|m| *m == minutes)
        .map_or("a while", |i| LABELS[i])
}

pub struct Hydration {
    /// When the app opened: reminders count from here.
    start: Instant,
    reminder: Reminder,
    runs: u64,
    /// The run showing now.
    playing: Option<u64>,
    _timer: Task<()>,
    _end: Task<()>,
}

impl Global for Hydration {}

/// Start the clock with the saved settings.
pub fn init(cx: &mut App) {
    let reminder = themes::store_path()
        .and_then(|path| themes::read_key(&path, KEY))
        .map(|value| Reminder::from_json(&value))
        .unwrap_or_default();
    cx.set_global(Hydration {
        start: Instant::now(),
        reminder,
        runs: 0,
        playing: None,
        _timer: Task::ready(()),
        _end: Task::ready(()),
    });
    schedule(cx);
}

pub fn reminder(cx: &App) -> Reminder {
    cx.global::<Hydration>().reminder
}

/// The run on screen now, to paint ([`overlay`]).
pub fn playing(cx: &App) -> Option<u64> {
    cx.global::<Hydration>().playing
}

/// Save new settings and re-time the next reminder. The clock keeps its
/// start: a new interval counts from when the app opened.
pub fn set(reminder: Reminder, cx: &mut App) {
    cx.update_global::<Hydration, _>(|h, _| h.reminder = reminder);
    schedule(cx);
    if let Some(path) = themes::store_path() {
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = themes::write_key(&path, KEY, reminder.to_json()) {
                    eprintln!("gentle-shell-gpui: could not save the water reminder: {err}");
                }
            })
            .detach();
    }
}

/// Show the reminder now (Configuration's "Show it now").
pub fn show(cx: &mut App) {
    let run = cx.update_global::<Hydration, _>(|h, _| {
        h.runs += 1;
        h.playing = Some(h.runs);
        h.runs
    });
    let end = cx.spawn(async move |cx| {
        cx.background_executor().timer(DURATION).await;
        cx.update(|cx| {
            if playing(cx) == Some(run) {
                cx.update_global::<Hydration, _>(|h, _| h.playing = None);
            }
        });
    });
    cx.global_mut::<Hydration>()._end = end;
}

/// Wait for each reminder in turn; replacing the task (new settings)
/// cancels the old wait.
fn schedule(cx: &mut App) {
    let timer = cx.spawn(async move |cx| {
        loop {
            let Some(wait) = cx.update(|cx| {
                let h = cx.global::<Hydration>();
                h.reminder.until_next(h.start.elapsed())
            }) else {
                return;
            };
            cx.background_executor().timer(wait).await;
            cx.update(show);
            cx.background_executor().timer(SETTLE).await;
        }
    });
    cx.global_mut::<Hydration>()._timer = timer;
}

/// The reminder's paint for run `run`, over the whole window.
pub fn overlay(run: u64, cx: &App) -> impl IntoElement {
    drops::overlay(run, reminder(cx).every_minutes, cx.reduce_motion())
}
