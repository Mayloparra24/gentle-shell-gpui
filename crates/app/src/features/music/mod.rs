//! Music: gentle-stream (the generative player behind Gentle Shell's
//! `/gentle:music`) in the title bar. A ♪ button appears once the player is
//! installed (Configuration → Music); it opens a small player with the piece
//! you hear, a live spectrum, play/pause, next, volume and a mood field
//! ("rainy night in a small café") that recomposes the music at the next bar.
//!
//! Play, pause, next, volume and the state go straight to the background
//! player over its socket ([`client`]). Starting the player and turning a mood
//! into a style run a tiny Python helper inside gentle-stream's own
//! environment (its `client.ensure` and `GentleShell.interpret`), so the app
//! never re-implements the player's startup or its model prompt.
//!
//! Cheap when idle: the state is polled every few seconds (to follow a player
//! started elsewhere), and the spectrum only while the player is open.

mod client;
mod view;

pub use view::title_button;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gentle_core::music::{MoodResult, MusicState, parse_mood_result};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::*;
use serde_json::json;

use crate::features::setup::music::{self as setup, MusicStatus};
use client::{Connection, Error};

/// Spectrum bands drawn in the player.
const BANDS: usize = 40;
/// How fast a band falls back after a peak (per poll).
const DECAY: f32 = 0.8;

/// Runs inside gentle-stream's Python: `start` attaches to the player or
/// starts it (`client.ensure`); `mood <text> <gentle-shell>` asks the model
/// for a style the way gentle-stream's own TUI does and hands it to the player.
const HELPER: &str = r#"
import json, sys
def out(**fields):
    print(json.dumps(fields, ensure_ascii=False), flush=True)
try:
    from gentle_stream import client
    player = client.ensure()
    if sys.argv[1] == "mood":
        from gentle_stream.bridge import GentleShell
        style, usage = GentleShell(binary=sys.argv[3]).interpret_with_usage(sys.argv[2], player.style)
        player.set_style(style)
        out(ok=True, name=style.name, usage=usage.format() if usage else None)
    else:
        out(ok=True, name=player.now.style.name)
    player.detach()
except Exception as error:
    out(ok=False, error=str(error) or type(error).__name__)
"#;

/// What the player is busy with.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    Starting,
    Composing(String),
}

/// The title-bar button's look changed (installed, playing, paused).
pub struct MusicChanged;

pub struct Music {
    status: Option<MusicStatus>,
    /// `None`: nothing is playing.
    state: Option<MusicState>,
    /// The spectrum as drawn: peaks fall back slowly.
    shown: Vec<f32>,
    job: Option<Job>,
    /// The last mood's result or the last error: text and whether it went well.
    note: Option<(String, bool)>,
    /// The player is open: poll fast, with the spectrum.
    watching: bool,
    conn: Arc<Mutex<Option<Connection>>>,
    mood: Entity<InputState>,
    volume: Entity<SliderState>,
    /// The user is dragging the volume: the player's value doesn't move it.
    sliding: bool,
    /// This app started the player, so quitting the app stops it.
    started_here: bool,
    _poll: Task<()>,
    _job: Task<()>,
    _status: Task<()>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<MusicChanged> for Music {}

impl Music {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mood = cx.new(|cx| InputState::new(window, cx).placeholder("Describe a mood…"));
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(80.)
        });
        let subscriptions = [
            cx.subscribe_in(&mood, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.submit_mood(window, cx);
                }
            }),
            cx.subscribe(&volume, |this, _, event: &SliderEvent, cx| {
                let (value, done) = match event {
                    SliderEvent::Change(value) => (value.start(), false),
                    SliderEvent::Release(value) => (value.start(), true),
                };
                this.sliding = !done;
                this.send(json!({"cmd": "set_volume", "volume": value / 100.}), cx);
            }),
        ];
        let mut this = Self {
            status: None,
            state: None,
            shown: Vec::new(),
            job: None,
            note: None,
            watching: false,
            conn: Arc::default(),
            mood,
            volume,
            sliding: false,
            started_here: false,
            _poll: Task::ready(()),
            _job: Task::ready(()),
            _status: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this._poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok((wait, request)) = this.read_with(cx, |this, _| this.next_poll()) else {
                    return;
                };
                cx.background_executor().timer(wait).await;
                let Some(request) = request else { continue };
                let Ok(conn) = this.read_with(cx, |this, _| this.conn.clone()) else {
                    return;
                };
                let result = cx
                    .background_executor()
                    .spawn(async move { request_on(&conn, &request) })
                    .await;
                if this
                    .update_in(cx, |this, window, cx| this.apply(result, window, cx))
                    .is_err()
                {
                    return;
                }
            }
        });
        this.refresh(cx);
        this
    }

    /// Look for the player again (after Configuration installs or removes it).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self._status = cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { setup::status() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !status.usable() {
                    this.state = None;
                }
                this.status = Some(status);
                cx.emit(MusicChanged);
                cx.notify();
            });
        });
    }

    /// The button shows: gentle-stream is installed and runs here.
    pub fn available(&self) -> bool {
        self.status.as_ref().is_some_and(MusicStatus::usable)
    }

    pub fn playing(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.playing)
    }

    /// The player popover opened or closed.
    pub fn set_watching(&mut self, watching: bool, cx: &mut Context<Self>) {
        self.watching = watching;
        if !watching {
            self.shown.clear();
        }
        cx.notify();
    }

    fn next_poll(&self) -> (Duration, Option<serde_json::Value>) {
        if !self.available() {
            return (Duration::from_secs(5), None);
        }
        if self.watching {
            (
                Duration::from_millis(70),
                Some(json!({"cmd": "state", "bands": BANDS})),
            )
        } else if self.state.is_some() {
            (Duration::from_millis(1500), Some(json!({"cmd": "state"})))
        } else {
            // A player started elsewhere (`/gentle:music` in a terminal) shows up.
            (Duration::from_secs(4), Some(json!({"cmd": "state"})))
        }
    }

    fn apply(
        &mut self,
        result: Result<Option<MusicState>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = (self.state.is_some(), self.playing());
        match result {
            Ok(Some(mut state)) => {
                if !state.spectrum.is_empty() {
                    self.shown.resize(state.spectrum.len(), 0.);
                    for (shown, level) in self.shown.iter_mut().zip(&state.spectrum) {
                        *shown = level.max(*shown * DECAY);
                    }
                }
                let volume = (state.volume * 100.).round();
                let slider = self.volume.read(cx).value().start();
                if !self.sliding && (slider - volume).abs() >= 1. {
                    self.volume
                        .update(cx, |s, cx| s.set_value(volume, window, cx));
                }
                state.spectrum.clear();
                self.state = Some(state);
            }
            Ok(None) => {
                self.state = None;
                self.shown.clear();
            }
            Err(error) => self.note = Some((error, false)),
        }
        if before != (self.state.is_some(), self.playing()) {
            cx.emit(MusicChanged);
        }
        cx.notify();
    }

    /// A control (`toggle`, `next`, `set_volume`, `stop`), answered with the
    /// new state.
    fn send(&mut self, request: serde_json::Value, cx: &mut Context<Self>) {
        let conn = self.conn.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { request_on(&conn, &request) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| this.apply(result, window, cx));
        })
        .detach();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.send(json!({"cmd": "toggle"}), cx);
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        self.send(json!({"cmd": "next"}), cx);
    }

    /// The app is quitting: stop the music it started. A player started
    /// elsewhere (`/gentle:music` in a terminal) keeps playing. Blocking, but
    /// one local request.
    pub fn shutdown(&self) {
        if self.started_here && self.state.is_some() {
            let _ = request_on(&self.conn, &json!({"cmd": "stop"}));
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.note = None;
        self.send(json!({"cmd": "stop"}), cx);
    }

    /// Send what the mood field says, and clear it.
    fn submit_mood(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.mood.read(cx).value().trim().to_string();
        if text.is_empty() || self.job.is_some() {
            return;
        }
        self.mood.update(cx, |s, cx| s.set_value("", window, cx));
        self.set_mood(text, cx);
    }

    /// Start the player (the default lo-fi piece).
    pub fn start(&mut self, cx: &mut Context<Self>) {
        self.run_helper(Job::Starting, vec!["start".into()], cx);
    }

    /// Recompose to `mood` (starting the player when nothing plays).
    pub fn set_mood(&mut self, mood: String, cx: &mut Context<Self>) {
        let shell = pi_rpc::resolve_backend(&std::env::temp_dir())
            .map(|spec| spec.program.to_string_lossy().into_owned());
        let Ok(shell) = shell else {
            self.note = Some((
                "Install Gentle Shell first: it reads the mood.".into(),
                false,
            ));
            cx.notify();
            return;
        };
        self.run_helper(
            Job::Composing(mood.clone()),
            vec!["mood".into(), mood, shell],
            cx,
        );
    }

    fn run_helper(&mut self, job: Job, args: Vec<String>, cx: &mut Context<Self>) {
        let Some(python) = self.status.as_ref().and_then(|s| s.python.clone()) else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let starting = job == Job::Starting;
        // A mood starts the player too when nothing plays.
        if self.state.is_none() {
            self.started_here = true;
        }
        self.job = Some(job);
        self.note = None;
        cx.notify();
        self._job = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { helper(&python, &args) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.job = None;
                this.note = match result {
                    Ok(MoodResult { name, usage }) if !starting => Some((
                        match usage {
                            Some(usage) => format!("Now playing {name} · {usage}"),
                            None => format!("Now playing {name}"),
                        },
                        true,
                    )),
                    Ok(_) => None,
                    Err(error) => Some((error, false)),
                };
                // Show the new piece now rather than at the next slow poll.
                this.send(json!({"cmd": "state"}), cx);
                cx.notify();
            });
        });
    }
}

/// One request on the shared connection, connecting (and reconnecting once
/// after a dropped one) as needed. `Ok(None)`: nothing is playing.
fn request_on(
    conn: &Mutex<Option<Connection>>,
    request: &serde_json::Value,
) -> Result<Option<MusicState>, String> {
    let mut conn = conn.lock().unwrap_or_else(|e| e.into_inner());
    for attempt in 0..2 {
        if conn.is_none() {
            match Connection::open(&client::runtime_dir()) {
                Ok(open) => *conn = Some(open),
                Err(Error::NoPlayer) => return Ok(None),
                Err(Error::Failed(error)) => return Err(error),
            }
        }
        let answer = conn.as_mut().expect("connected above").request(request);
        match answer {
            Ok(state) => {
                if state.is_none() {
                    // `stop`: the player exits and closes every connection.
                    *conn = None;
                }
                return Ok(state);
            }
            Err(error) => {
                *conn = None;
                match error {
                    Error::NoPlayer if attempt == 0 => continue,
                    Error::NoPlayer => return Ok(None),
                    Error::Failed(error) => return Err(error),
                }
            }
        }
    }
    Ok(None)
}

/// Run [`HELPER`] in gentle-stream's Python.
fn helper(python: &PathBuf, args: &[String]) -> Result<MoodResult, String> {
    let output = pi_rpc::command(python)
        .arg("-c")
        .arg(HELPER)
        .args(args)
        .env("PATH", pi_rpc::child_path())
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("gentle-stream's Python didn't start: {e}"))?;
    parse_mood_result(&String::from_utf8_lossy(&output.stdout)).map_err(|error| {
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .map(str::to_string)
            .unwrap_or(error)
    })
}
