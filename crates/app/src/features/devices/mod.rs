//! Devices: launching Android emulators and iOS simulators and showing each
//! one in its own floating card. The panel discovers what the machine has,
//! boots with the cheapest flags (`-no-window`, no audio, no cameras), and
//! streams downscaled frames only while the card is visible and the window is
//! active. When an agent starts a device from a shell tool, the window follows
//! it in the title bar's instance tray (see `SessionEvent::Device`) and only
//! opens the card when the user asks; different devices get different cards.

mod capture;
mod tools;
mod view;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gentle_core::devices::{Device, LaunchIntent, Platform, State};
use gpui_kit::*;

use crate::shared::floating::{self, CardState};
use crate::shared::ui;
pub(crate) use tools::simctl_path;
use tools::{Launched, Tools};

/// The card's place when it opens: a phone-shaped column at the bottom-right.
/// Docking scales it.
pub const DEFAULT_FRAME: floating::Frame = floating::Frame {
    right: 24.,
    bottom: 24.,
    w: 400.,
    h: 760.,
};

/// How long a discovery result stays fresh enough to skip another one.
const FRESH_FOR: Duration = Duration::from_secs(5);
/// How often to look for a device an agent was about to start.
const WATCH_EVERY: Duration = Duration::from_secs(2);
/// Stop waiting for an agent-started device after this long.
const WATCH_FOR: Duration = Duration::from_secs(180);
/// Preview cadence while a device is visible and the window is active. One
/// frame every 1.5 s is enough to follow a test, and the capture only runs
/// while the card is shown and the app has focus.
const PREVIEW_EVERY: Duration = Duration::from_millis(1500);
/// Screenshots during boot are mostly discarded by the emulator; slower.
const BOOT_PREVIEW_EVERY: Duration = Duration::from_millis(2000);
/// The texture width band: enough for a 2× card without ever uploading a
/// full 3× maximized screen.
const MIN_PREVIEW_WIDTH: usize = 360;
const MAX_PREVIEW_WIDTH: usize = 1440;
/// Until the card has been laid out once, assume a 2× phone-sized preview.
const FIRST_PREVIEW_WIDTH: usize = 800;

pub enum DevicesEvent {
    /// The card's button: send it to the instance tray.
    Hide,
}

/// A device an agent's shell command is about to start. The panel waits for
/// it to appear instead of launching a second copy.
struct Watch {
    platform: Platform,
    id: Option<String>,
    until: Instant,
}

pub struct DevicesPanel {
    card: CardState,
    tools: Tools,
    devices: Vec<Device>,
    /// Missing-tool and no-device hints, shown under the list.
    notes: Vec<String>,
    /// AVD name (Android) or UDID (iOS) of the device the card follows.
    selected: Option<String>,
    /// The name being launched, shown while the emulator starts.
    launching: Option<String>,
    /// A device whose boot is still being watched; shutdown must stop it even
    /// before it reaches `launched`.
    pending: Option<Device>,
    frame: Option<Arc<RenderImage>>,
    /// The preview's box in points and the window scale factor, recorded by
    /// the renderer so each capture matches the display's real pixel width.
    preview: Rc<Cell<(f32, f32, f32)>>,
    /// Width / height of the frames, once one has been captured.
    aspect: Option<f32>,
    /// Frames replaced while no window was available to release their atlas
    /// tiles; the next render drops them.
    pending_drops: Vec<Arc<RenderImage>>,
    error: Option<String>,
    capture_error: Option<String>,
    shown: bool,
    /// A screen covers the window; stop taking screenshots.
    covered: bool,
    window_active: bool,
    discovering: bool,
    /// The list is open instead of the preview.
    picking: bool,
    launched: Vec<Launched>,
    cancel: Arc<AtomicBool>,
    last_discovery: Option<Instant>,
    watch: Option<Watch>,
    /// The watch task is alive; a finished one must not block a new watch.
    watching: bool,
    _discover: Option<Task<()>>,
    _launch: Option<Task<()>>,
    _watch: Option<Task<()>>,
    /// The capture loop is alive. It ends itself whenever there is nothing
    /// to capture, and the next render that wants frames starts it again,
    /// so a hidden card costs no wake-ups at all.
    capturing: bool,
    _activation: Option<Subscription>,
}

impl EventEmitter<DevicesEvent> for DevicesPanel {}

impl DevicesPanel {
    /// The AVD name (Android) or UDID (iOS) this card follows.
    pub fn selected_key(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Put the card in a stage slot; see [`crate::shared::floating::CardState`].
    pub fn set_frame(&mut self, frame: floating::Frame, cx: &mut Context<Self>) {
        self.card.set_frame(frame);
        cx.notify();
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let active = window.is_window_active();
        let mut this = Self {
            card: CardState::new(DEFAULT_FRAME),
            tools: Tools::default(),
            devices: Vec::new(),
            notes: Vec::new(),
            selected: None,
            launching: None,
            pending: None,
            frame: None,
            preview: Rc::new(Cell::new((0., 0., 1.))),
            aspect: None,
            pending_drops: Vec::new(),
            error: None,
            capture_error: None,
            shown: false,
            covered: false,
            window_active: active,
            discovering: false,
            picking: false,
            launched: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            last_discovery: None,
            watch: None,
            watching: false,
            _discover: None,
            _launch: None,
            _watch: None,
            capturing: false,
            _activation: None,
        };
        this._activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            let active = window.is_window_active();
            if this.window_active != active {
                this.window_active = active;
                cx.notify();
            }
        }));
        this
    }

    /// Follow a device an agent is starting, without showing the card: poll
    /// discovery until it appears, so the tray shows its state and the card
    /// is already on it when the user opens it.
    pub fn follow(&mut self, intent: LaunchIntent, cx: &mut Context<Self>) {
        self.selected = intent.id.clone();
        self.watch = Some(Watch {
            platform: intent.platform,
            id: intent.id,
            until: Instant::now() + WATCH_FOR,
        });
        self.error = None;
        self.capture_error = None;
        self.clear_frame();
        self.refresh_if_stale(cx);
        // A new intent replaces any watch already running; cancel it so the
        // fresh one cannot race with a loop that is about to stop.
        self.watching = false;
        self._watch = None;
        self.start_watch(cx);
        cx.notify();
    }

    /// Show the card (the palette's Devices, or a row of the tray).
    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.shown = true;
        self.picking = false;
        self.refresh_if_stale(cx);
        if self.watch.is_none() {
            self.settle();
        }
        cx.notify();
    }

    /// Hide the card. The device keeps running and a pending agent launch is
    /// still followed; only the screenshots stop.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.shown = false;
        self.picking = false;
        cx.notify();
    }

    pub fn set_covered(&mut self, covered: bool, cx: &mut Context<Self>) {
        if self.covered != covered {
            self.covered = covered;
            cx.notify();
        }
    }

    /// Stop every device this panel started. Called when the app quits and
    /// when the panel is dropped.
    pub fn shutdown_all(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(device) = self.pending.take() {
            self.tools.request_stop(&device);
        }
        for launched in &mut self.launched {
            self.tools.request_stop(&launched.device);
            if let Some(child) = &mut launched.child {
                let _ = child.kill();
            }
        }
        self.launched.clear();
    }

    /// The tray's Stop: the device this card follows (an agent may have
    /// started it outside the card) and everything the card launched. The
    /// card is dropped right after, so nothing here waits.
    pub fn stop_all(&mut self) {
        if let Some(device) = self.running().cloned() {
            self.tools.request_stop(&device);
        }
        self.shutdown_all();
    }

    /// The platform of the device this card follows or waits for.
    pub fn platform(&self) -> Option<Platform> {
        self.resolved()
            .map(|device| device.platform)
            .or_else(|| self.watch.as_ref().map(|watch| watch.platform))
    }

    fn toggle_maximized(&mut self, cx: &mut Context<Self>) {
        self.card.toggle_maximized();
        cx.notify();
    }

    fn start_drag(
        &mut self,
        kind: floating::DragKind,
        start: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.card.start(kind, start);
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.card.drag_to(position) {
            cx.notify();
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.card.end_drag() {
            cx.notify();
        }
    }

    /// Forget the preview (the device stopped or another was chosen). The
    /// texture is released on the next render, which has the window.
    fn clear_frame(&mut self) {
        if let Some(frame) = self.frame.take() {
            self.pending_drops.push(frame);
        }
    }

    fn resolved(&self) -> Option<&Device> {
        let key = self.selected.as_deref()?;
        self.devices
            .iter()
            .find(|device| device.key() == key && device.state.is_running())
            .or_else(|| self.devices.iter().find(|device| device.key() == key))
    }

    /// The device whose screen the card shows, if it is running.
    fn running(&self) -> Option<&Device> {
        self.resolved().filter(|device| device.state.is_running())
    }

    fn busy(&self) -> bool {
        self.discovering
            || self.launching.is_some()
            || self
                .resolved()
                .is_some_and(|device| device.state == State::Booting)
    }

    pub fn status(&self) -> String {
        if let Some(error) = self.error.clone().or_else(|| self.capture_error.clone()) {
            return error;
        }
        if let Some(name) = &self.launching {
            return format!("Starting {name}…");
        }
        match self.resolved() {
            Some(device) => match device.state {
                State::Ready if device.platform == Platform::Android => {
                    format!("Ready · {}", device.id)
                }
                State::Ready => "Ready".to_string(),
                State::Booting => "Booting…".to_string(),
                State::Shutdown if self.watch.is_some() => {
                    "Waiting for the agent to start it…".to_string()
                }
                State::Shutdown => "Not running".to_string(),
            },
            None if self.watch.is_some() => "Waiting for the agent to start it…".to_string(),
            None if self.discovering || self.tools.has_android() || self.tools.has_ios() => {
                "Choose a device".to_string()
            }
            None => "No emulator tools found".to_string(),
        }
    }

    pub fn status_color(&self) -> Hsla {
        if self.error.is_some() || self.capture_error.is_some() {
            return ui::danger();
        }
        match self.resolved() {
            Some(device) if device.state == State::Ready => ui::success(),
            Some(device) if device.state == State::Booting => ui::champagne(),
            _ if self.watch.is_some() => ui::champagne(),
            _ => ui::faint(),
        }
    }

    /// The device's name; before discovery finds it, the name the agent
    /// launched it with.
    pub fn title(&self) -> String {
        self.resolved()
            .map(|device| device.name.clone())
            .or_else(|| self.selected.clone())
            .unwrap_or_else(|| "Devices".to_string())
    }

    /// Discover again unless the last result is still fresh (the tray asks
    /// when it opens, so a hidden card's state is not stale there).
    pub fn refresh_if_stale(&mut self, cx: &mut Context<Self>) {
        let stale = self
            .last_discovery
            .is_none_or(|at| at.elapsed() >= FRESH_FOR);
        if stale {
            self.refresh(cx);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.discovering {
            return;
        }
        self.discovering = true;
        self._discover = Some(cx.spawn(async move |this, cx| {
            let (tools, discovery) = cx
                .background_executor()
                .spawn(async move {
                    let tools = Tools::detect();
                    let discovery = tools.discover();
                    (tools, discovery)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.discovering = false;
                this.tools = tools;
                this.devices = discovery.devices;
                this.notes = discovery.notes;
                this.last_discovery = Some(Instant::now());
                this.settle();
                this.start_watch_if_needed(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// After a discovery: follow a match, pick a sensible default, and end
    /// the launch state once the device is ready.
    fn settle(&mut self) {
        if let Some(watch) = &self.watch {
            let found = self.devices.iter().find(|device| {
                device.platform == watch.platform
                    && device.state.is_running()
                    && watch
                        .id
                        .as_deref()
                        .is_none_or(|id| device.key() == id || device.id == id || device.name == id)
            });
            if let Some(device) = found {
                self.selected = Some(device.key());
                self.watch = None;
            } else if Instant::now() >= watch.until {
                self.watch = None;
            }
        }
        let resolved = self
            .resolved()
            .map(|device| (device.state, device.name.clone()));
        if let Some((state, name)) = resolved {
            if state == State::Ready
                && self
                    .launching
                    .as_deref()
                    .is_some_and(|launching| launching == name)
            {
                self.launching = None;
            }
            if state.is_running() {
                self.error = None;
            } else {
                self.capture_error = None;
                self.clear_frame();
            }
        }
        if self.selected.is_none()
            && self.watch.is_none()
            && let Some(device) = self
                .devices
                .iter()
                .find(|device| device.state == State::Ready)
        {
            self.selected = Some(device.key());
        }
    }

    fn pick(&mut self, device: Device, cx: &mut Context<Self>) {
        // The user chose a device: stop following whatever an agent was
        // about to start.
        self.watch = None;
        self.watching = false;
        if device.state == State::Shutdown {
            self.launch(device, cx);
            return;
        }
        let key = device.key();
        if self.selected.as_deref() != Some(key.as_str()) {
            self.clear_frame();
        }
        self.selected = Some(key);
        self.picking = false;
        self.error = None;
        self.capture_error = None;
        cx.notify();
    }

    fn launch(&mut self, target: Device, cx: &mut Context<Self>) {
        if self.launching.is_some() {
            return;
        }
        self.selected = Some(target.key());
        self.launching = Some(target.name.clone());
        self.picking = false;
        self.clear_frame();
        self.error = None;
        self.capture_error = None;
        self.pending = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let tools = self.tools.clone();
        let boot_tools = tools.clone();
        let used = tools::used_ports(&self.devices);
        let launch_cancel = cancel.clone();
        self._launch = Some(cx.spawn(async move |this, cx| {
            let launched = cx
                .background_executor()
                .spawn(async move { tools.launch(&target, &used, &launch_cancel) })
                .await;
            let launched = match launched {
                Ok(launched) => launched,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.launching = None;
                        this.error = Some(error);
                        cx.notify();
                    });
                    return;
                }
            };
            let device = launched.device.clone();
            let child = launched.child;
            // Watch the device boot before reporting it as ready. The panel
            // already shows its serial and previews whatever is on screen.
            let _ = this.update(cx, |this, cx| {
                this.selected = Some(device.key());
                this.launching = Some(device.name.clone());
                this.pending = Some(device.clone());
                this.frame = None;
                // The discovery list still says "Stopped" for this one;
                // mark it booting so the card shows the spinner and hides
                // the Launch action while the system comes up.
                if let Some(row) = this
                    .devices
                    .iter_mut()
                    .find(|row| row.key() == device.key())
                {
                    row.state = State::Booting;
                }
                cx.notify();
            });
            let pending = device.clone();
            let booted = cx
                .background_executor()
                .spawn(async move {
                    match pending.platform {
                        Platform::Android => {
                            let Some(child) = child else {
                                return Err("The emulator process was lost.".to_string());
                            };
                            boot_tools
                                .wait_android(&pending.id, child, &cancel)
                                .map(Some)
                        }
                        Platform::Ios => boot_tools.wait_ios(&pending.id).map(|_| None),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.launching = None;
                this.pending = None;
                match booted {
                    Ok(child) => {
                        this.launched.retain(|launched| {
                            launched.device.id != device.id && launched.device.key() != device.key()
                        });
                        this.launched.push(Launched {
                            device: device.clone(),
                            child,
                        });
                        this.error = None;
                        this.refresh(cx);
                    }
                    Err(error) => {
                        if let Some(row) = this
                            .devices
                            .iter_mut()
                            .find(|row| row.key() == device.key())
                        {
                            row.state = State::Shutdown;
                        }
                        this.error = Some(error);
                        this.refresh(cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        let Some(device) = self.resolved().cloned() else {
            return;
        };
        self.cancel.store(true, Ordering::Relaxed);
        self.launching = None;
        self.pending = None;
        self.watch = None;
        self.clear_frame();
        self.error = None;
        if let Some(row) = self
            .devices
            .iter_mut()
            .find(|row| row.key() == device.key())
        {
            row.state = State::Shutdown;
        }
        // A process we own is killed directly; `adb emu kill` is the clean
        // path for one the agent started.
        let mut owned = false;
        for launched in &mut self.launched {
            if (launched.device.id == device.id || launched.device.key() == device.key())
                && let Some(child) = &mut launched.child
            {
                let _ = child.kill();
                owned = true;
            }
        }
        self.launched.retain(|launched| {
            launched.device.id != device.id && launched.device.key() != device.key()
        });
        let tools = self.tools.clone();
        self._launch = Some(cx.spawn(async move |this, cx| {
            if owned {
                // Give adb a moment to notice the process is gone.
                cx.background_executor()
                    .timer(Duration::from_millis(700))
                    .await;
            } else {
                let result = cx
                    .background_executor()
                    .spawn(async move { tools.stop(&device) })
                    .await;
                if let Err(error) = result {
                    let _ = this.update(cx, |this, cx| {
                        this.error = Some(error);
                        cx.notify();
                    });
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.refresh(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn start_watch_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.watch.is_some() {
            self.start_watch(cx);
        }
    }

    /// Poll discovery while waiting for an agent-started device.
    fn start_watch(&mut self, cx: &mut Context<Self>) {
        if self.watching {
            return;
        }
        self.watching = true;
        self._watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_EVERY).await;
                let keep = this
                    .update(cx, |this, cx| {
                        if this.watch.is_none() {
                            this.watching = false;
                            return false;
                        }
                        // Keep following an agent while the card is hidden
                        // or the app is in the background, so the tray shows
                        // its state and the device is already selected when
                        // the user looks. Only a screen over the card makes
                        // the probe useless.
                        if this.covered {
                            return true;
                        }
                        this.refresh(cx);
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    /// Take frames while [`Self::next_capture`] wants them, off the UI thread,
    /// and put them back only while the card is visible. The loop ends as
    /// soon as there is nothing to capture; [`Render`] starts it again.
    fn start_capture(&mut self, cx: &mut Context<Self>) {
        self.capturing = true;
        cx.spawn(async move |this, cx| {
            loop {
                let shot = this
                    .update(cx, |this, _| {
                        let shot = this.next_capture();
                        this.capturing = shot.is_some();
                        shot
                    })
                    .ok()
                    .flatten();
                let Some(Shot {
                    tools,
                    device,
                    every,
                    target,
                }) = shot
                else {
                    break;
                };
                let expected = device.id.clone();
                let shot = cx
                    .background_executor()
                    .spawn(async move { capture::capture(&tools, &device, target) })
                    .await;
                let keep = this
                    .update_in(cx, |this, window, cx| {
                        if !this.shown
                            || this.covered
                            || !this.running().is_some_and(|device| device.id == expected)
                        {
                            return true;
                        }
                        let booting = this
                            .running()
                            .is_some_and(|device| device.state == State::Booting);
                        match shot {
                            Ok(image) => {
                                let size = image.size(0);
                                if size.height.0 > 0 {
                                    this.aspect = Some(size.width.0 as f32 / size.height.0 as f32);
                                }
                                let old = this.frame.replace(image);
                                if let Some(old) = old {
                                    let _ = window.drop_image(old);
                                }
                                this.capture_error = None;
                            }
                            Err(error) if booting => {
                                // adb often answers before the screen exists;
                                // keep the previous frame and stay quiet.
                                this.capture_error = None;
                                let _ = error;
                            }
                            Err(error) => {
                                this.capture_error = Some(error);
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
                cx.background_executor().timer(every).await;
            }
        })
        .detach();
    }

    /// The next frame to take, or `None` when nothing would show it.
    fn next_capture(&self) -> Option<Shot> {
        if !self.shown || self.covered || !self.window_active {
            return None;
        }
        let device = self.running()?;
        let every = if device.state == State::Booting {
            BOOT_PREVIEW_EVERY
        } else {
            PREVIEW_EVERY
        };
        Some(Shot {
            tools: self.tools.clone(),
            device: device.clone(),
            every,
            target: self.target_width(),
        })
    }

    /// The texture width that matches the preview's real pixels. The image is
    /// fit inside the card with `Contain`, so the limiting dimension depends
    /// on the phone's aspect ratio.
    fn target_width(&self) -> usize {
        let (w, h, scale) = self.preview.get();
        let aspect = self.aspect.unwrap_or(0.5);
        let draw_width = if w > 0. && h > 0. {
            w.min(h * aspect)
        } else {
            FIRST_PREVIEW_WIDTH as f32
        };
        let scale = if scale > 0. { scale } else { 1. };
        ((draw_width * scale).round() as usize).clamp(MIN_PREVIEW_WIDTH, MAX_PREVIEW_WIDTH)
    }
}

impl Drop for DevicesPanel {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

impl Render for DevicesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Every change that can make frames worth taking again (shown,
        // uncovered, window activated, device booted) notifies, so the next
        // render restarts the loop.
        if !self.capturing && self.next_capture().is_some() {
            self.start_capture(cx);
        }
        view::render(self, window, cx)
    }
}

/// One frame the capture loop takes next.
struct Shot {
    tools: Tools,
    device: Device,
    every: Duration,
    target: usize,
}
