//! The window's device panel: one tab per simulator or emulator, each the
//! hub's live viewer pinned to that device. Docked as a column at the right
//! by default, or floating as a card; a button switches. Only the active
//! tab has a web view, so background tabs cost nothing.

use std::time::{Duration, Instant};

use gentle_core::devices::{Device, DeviceCommand, LaunchIntent, Platform, State};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{Hub, HubStatus, hub};
use crate::features::browser::WebSurface;
use crate::shared::floating::{self, CardState, DragKind};
use crate::shared::ui;

/// The docked column's width; a phone fits with its controls.
pub const DOCK_WIDTH: f32 = 400.;
/// Stop waiting for a device an agent started itself after this long.
const FOLLOW_FOR: Duration = Duration::from_secs(180);

/// The floating card's place when it opens.
const FLOAT_FRAME: floating::Frame = floating::Frame {
    right: 24.,
    bottom: 24.,
    w: 400.,
    h: 760.,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Docked,
    Floating,
}

pub enum DevicePanelEvent {
    /// Shown, hidden or moved between docked and floating: the window lays
    /// itself out again.
    Layout,
    /// "Use devices" is off: open Configuration → Devices.
    OpenSettings,
}

struct Tab {
    device: Device,
    /// The live viewer and the hub port it was built for; only the active
    /// tab has one.
    surface: Option<(u16, Entity<WebSurface>)>,
    /// The last command an agent ran on this device.
    activity: Option<String>,
}

pub struct DevicePanel {
    hub: Entity<Hub>,
    tabs: Vec<Tab>,
    active: usize,
    visible: bool,
    mode: Mode,
    card: CardState,
    covered: bool,
    /// The device list replaces the viewer (the + button, or no tabs).
    picking: bool,
    /// Devices agents start themselves (`emulator -avd`, `simctl boot`): a
    /// tab opens when each shows up running.
    following: Vec<(LaunchIntent, Instant)>,
    /// Names or ids `gentle-device open` asked for before the list arrived.
    pending: Vec<String>,
    _hub: Subscription,
}

impl EventEmitter<DevicePanelEvent> for DevicePanel {}

impl DevicePanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let hub = hub(cx);
        let subscription = cx.observe(&hub, |this, _, cx| this.sync(cx));
        Self {
            hub,
            tabs: Vec::new(),
            active: 0,
            visible: false,
            mode: Mode::Docked,
            card: CardState::new(FLOAT_FRAME),
            covered: false,
            picking: false,
            following: Vec::new(),
            pending: Vec::new(),
            _hub: subscription,
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            return;
        }
        self.visible = true;
        self.picking |= self.tabs.is_empty();
        self.hub.update(cx, |hub, cx| hub.watch(true, cx));
        self.sync(cx);
        cx.emit(DevicePanelEvent::Layout);
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        self.visible = false;
        // The web views (and their streams) go; the tabs stay.
        for tab in &mut self.tabs {
            tab.surface = None;
        }
        self.hub.update(cx, |hub, cx| hub.watch(false, cx));
        cx.emit(DevicePanelEvent::Layout);
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            self.hide(cx)
        } else {
            self.show(cx)
        }
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.emit(DevicePanelEvent::Layout);
        cx.notify();
    }

    /// A screen opened or closed over the window.
    pub fn set_covered(&mut self, covered: bool, cx: &mut Context<Self>) {
        self.covered = covered;
        if let Some((_, surface)) = self.tabs.get(self.active).and_then(|t| t.surface.as_ref()) {
            surface.update(cx, |surface, cx| surface.set_covered(covered, cx));
        }
    }

    /// Show `device` in its tab (a new one, or the one it has), booting it
    /// when it's off.
    pub fn open(&mut self, device: Device, cx: &mut Context<Self>) {
        let index = match self
            .tabs
            .iter()
            .position(|t| t.device.key() == device.key())
        {
            Some(index) => index,
            None => {
                self.tabs.push(Tab {
                    device: device.clone(),
                    surface: None,
                    activity: None,
                });
                self.tabs.len() - 1
            }
        };
        if device.state == State::Shutdown {
            self.hub.update(cx, |hub, cx| hub.boot(device, cx));
        }
        self.picking = false;
        self.activate(index, cx);
        self.show(cx);
    }

    /// `gentle-device open <query>`: the device with that name or id.
    pub fn open_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.hub.update(cx, |hub, cx| hub.ensure(cx));
        match self.hub.read(cx).find(&query).cloned() {
            Some(device) => self.open(device, cx),
            None => {
                self.pending.push(query);
                self.show(cx);
            }
        }
    }

    /// `gentle-device close <query>`: close its tab, and shut it down.
    pub fn close_query(&mut self, query: &str, shutdown: bool, cx: &mut Context<Self>) {
        let device = self.hub.read(cx).find(query).cloned();
        if let Some(device) = device {
            if let Some(index) = self
                .tabs
                .iter()
                .position(|t| t.device.key() == device.key())
            {
                self.close_tab(index, cx);
            }
            if shutdown {
                self.hub.update(cx, |hub, cx| hub.shutdown(device, cx));
            }
        }
    }

    /// An agent started a device with its own shell command: open its tab
    /// once it runs.
    pub fn follow(&mut self, intent: LaunchIntent, cx: &mut Context<Self>) {
        self.hub.update(cx, |hub, cx| hub.ensure(cx));
        self.following.push((intent, Instant::now() + FOLLOW_FOR));
        self.show(cx);
    }

    /// An agent drove a device: the matching tab shows the command.
    pub fn activity(&mut self, command: DeviceCommand, cx: &mut Context<Self>) {
        let tab = self
            .tabs
            .iter_mut()
            .enumerate()
            .filter(|(_, t)| command.platform.is_none_or(|p| p == t.device.platform))
            .min_by_key(|(i, _)| usize::from(*i != self.active))
            .map(|(_, t)| t);
        if let Some(tab) = tab {
            tab.activity = Some(command.summary);
            cx.notify();
        }
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active = index.min(self.tabs.len().saturating_sub(1));
        // Only the active tab keeps a web view.
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if i != self.active {
                tab.surface = None;
            }
        }
        self.sync(cx);
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.picking = true;
        }
        let active = if self.active > index {
            self.active - 1
        } else {
            self.active
        };
        self.activate(active, cx);
    }

    /// Follow the hub: device states, the viewers for running devices, and
    /// the devices agents asked for.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let hub = self.hub.read(cx);
        let port = hub.port();
        let devices = hub.devices().to_vec();
        for tab in &mut self.tabs {
            if let Some(device) = devices.iter().find(|d| d.key() == tab.device.key()) {
                tab.device = device.clone();
            }
        }
        // Devices agents asked for by name, now that the list has them.
        let pending = std::mem::take(&mut self.pending);
        for query in pending {
            match self.hub.read(cx).find(&query).cloned() {
                Some(device) => self.open(device, cx),
                None => self.pending.push(query),
            }
        }
        // Devices agents started themselves, once they run.
        let now = Instant::now();
        self.following.retain(|(_, until)| *until > now);
        let mut found = Vec::new();
        self.following.retain(|(intent, _)| {
            let hit = devices.iter().find(|d| {
                d.platform == intent.platform
                    && d.state == State::Ready
                    && intent.id.as_deref().is_none_or(|id| {
                        d.id.eq_ignore_ascii_case(id) || d.name.eq_ignore_ascii_case(id)
                    })
            });
            match hit {
                Some(device) => {
                    found.push(device.clone());
                    false
                }
                None => true,
            }
        });
        for device in found {
            self.open(device, cx);
        }
        // The active tab's viewer, once its device runs and the panel shows.
        let visible = self.visible;
        let covered = self.covered;
        if let (Some(port), Some(tab)) = (port, self.tabs.get_mut(self.active)) {
            let ready = tab.device.state == State::Ready;
            let current = tab.surface.as_ref().map(|(p, _)| *p);
            if visible && ready && current != Some(port) {
                let surface = cx.new(|_| {
                    WebSurface::new(
                        super::hub::viewer_url(port),
                        Some(super::hub::viewer_script(&tab.device)),
                    )
                });
                surface.update(cx, |surface, cx| surface.set_covered(covered, cx));
                tab.surface = Some((port, surface));
            } else if !ready {
                tab.surface = None;
            }
        }
        cx.notify();
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    fn render_tabs(&self, cx: &mut Context<Self>) -> Div {
        h_flex()
            .gap_1()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                let active = i == self.active && !self.picking;
                let booting =
                    self.hub.read(cx).is_booting(&tab.device) || tab.device.state == State::Booting;
                h_flex()
                    .id(("device-tab", i))
                    .group("device-tab")
                    .h(px(26.))
                    .min_w_0()
                    .pl_2()
                    .pr_0p5()
                    .gap_1p5()
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(ui::surface_hover()).text_color(ui::fg()))
                    .when(!active, |d| {
                        d.text_color(ui::muted())
                            .hover(|s| s.bg(ui::surface_hover().opacity(0.6)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picking = false;
                        this.activate(i, cx);
                    }))
                    .child(platform_icon(
                        tab.device.platform,
                        if booting {
                            ui::champagne()
                        } else if active {
                            ui::petal()
                        } else {
                            ui::faint()
                        },
                    ))
                    .child(div().min_w_0().truncate().child(tab.device.name.clone()))
                    .child(
                        div()
                            .id(("device-tab-close", i))
                            .size(px(18.))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .text_color(ui::faint())
                            .invisible()
                            .group_hover("device-tab", |s| s.visible())
                            .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, cx);
                            }))
                            .child(Icon::new(IconName::X).xsmall()),
                    )
            }))
            .child(
                Button::new("device-add")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Plus))
                    .tooltip("Open a simulator or emulator")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.picking = !this.picking || this.tabs.is_empty();
                        this.hub.update(cx, |hub, cx| hub.refresh(cx));
                        cx.notify();
                    })),
            )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let docked = self.mode == Mode::Docked;
        h_flex()
            .id("device-header")
            .h(px(floating::HEADER))
            .flex_none()
            .px_1p5()
            .gap_1()
            .when(!docked, |d| {
                d.cursor(CursorStyle::OpenHand).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.card.start(DragKind::Move, event.position);
                        cx.notify();
                    }),
                )
            })
            .child(self.render_tabs(cx))
            .child(
                h_flex()
                    .gap_0p5()
                    // Buttons keep their mouse-down from starting a drag.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("device-mode")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(if docked {
                                IconName::PictureInPicture2
                            } else {
                                IconName::PanelRight
                            }))
                            .tooltip(if docked { "Float" } else { "Dock to the right" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_mode(
                                    if docked { Mode::Floating } else { Mode::Docked },
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new("device-hide")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Minus))
                            .tooltip("Hide · devices keep running")
                            .on_click(cx.listener(|this, _, _, cx| this.hide(cx))),
                    ),
            )
    }

    /// The device list: running first, each opens (and boots) on click.
    fn render_picker(&self, cx: &mut Context<Self>) -> Div {
        let hub = self.hub.read(cx);
        let mut list = v_flex().gap_0p5().p_2();
        if let Some(note) = hub_note(hub.status()) {
            return list.child(note.render(cx));
        }
        let devices = hub.devices().to_vec();
        if devices.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_xs()
                    .text_color(ui::muted())
                    .child("No simulators or emulators yet: install Xcode (iOS) or Android Studio and create a virtual device."),
            );
        }
        for (i, device) in devices.into_iter().enumerate() {
            let running = device.state.is_running();
            let booting = hub.is_booting(&device);
            let open = device.clone();
            list = list.child(
                h_flex()
                    .id(("device-row", i))
                    .gap_2p5()
                    .px_2()
                    .py_1p5()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|s| s.bg(ui::surface_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| this.open(open.clone(), cx)))
                    .child(platform_icon(device.platform, ui::petal()))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(ui::fg())
                                    .truncate()
                                    .child(device.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(ui::faint())
                                    .child(device.subtitle()),
                            ),
                    )
                    .child(if booting {
                        ui::chip("Booting", ui::champagne())
                    } else if running {
                        ui::chip("Running", ui::success())
                    } else {
                        ui::chip("Off", ui::faint())
                    }),
            );
        }
        for error in hub
            .errors()
            .iter()
            .map(String::as_str)
            .chain(hub.last_error())
        {
            list = list.child(
                div()
                    .px_2()
                    .pt_1()
                    .text_xs()
                    .text_color(ui::warning())
                    .child(error.to_string()),
            );
        }
        list
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.picking || self.tabs.is_empty() {
            return div()
                .id("device-picker")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(self.render_picker(cx))
                .into_any_element();
        }
        let tab = &self.tabs[self.active];
        let hub = self.hub.read(cx);
        let waiting = |text: String| {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::accent()))
                .child(text)
                .into_any_element()
        };
        if let Some(note) = hub_note(hub.status()) {
            return div().p_2().child(note.render(cx)).into_any_element();
        }
        match &tab.surface {
            Some((_, surface)) => div()
                .flex_1()
                .min_h_0()
                .mx(px(floating::GRIP))
                .mb(px(floating::GRIP))
                .rounded(px(floating::BODY_RADIUS))
                .bg(ui::well())
                .overflow_hidden()
                .child(surface.clone())
                .into_any_element(),
            None if tab.device.state == State::Shutdown && !hub.is_booting(&tab.device) => {
                let device = tab.device.clone();
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui::muted())
                            .child(format!("{} is off.", tab.device.name)),
                    )
                    .child(
                        Button::new("device-boot")
                            .small()
                            .primary()
                            .icon(Icon::new(IconName::Play))
                            .label("Start")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let device = device.clone();
                                this.hub.update(cx, |hub, cx| hub.boot(device, cx));
                            })),
                    )
                    .into_any_element()
            }
            None => waiting(format!("Starting {}…", tab.device.name)),
        }
    }

    fn render_panel(&self, cx: &mut Context<Self>) -> Div {
        let activity = self
            .tabs
            .get(self.active)
            .filter(|_| !self.picking)
            .and_then(|t| t.activity.clone());
        v_flex()
            .size_full()
            .child(self.render_header(cx))
            .when_some(activity, |d, activity| {
                d.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_size(px(10.5))
                        .font_family(ui::MONO)
                        .text_color(ui::faint())
                        .truncate()
                        .child(activity),
                )
            })
            .child(self.render_body(cx))
    }
}

impl Render for DevicePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        match self.mode {
            Mode::Docked => div()
                .w(px(DOCK_WIDTH))
                .flex_none()
                .h_full()
                .rounded_xl()
                .bg(ui::panel())
                .overflow_hidden()
                .child(self.render_panel(cx))
                .into_any_element(),
            Mode::Floating => {
                self.card.fit();
                let card = ui::island(cx)
                    .absolute()
                    .flex()
                    .flex_col()
                    .occlude()
                    .shadow(floating::shadow(self.card.dragging()))
                    .map(|card| floating::position(&self.card, card))
                    .child(self.render_panel(cx))
                    .children(floating::grips(
                        &self.card,
                        cx,
                        "device-grip",
                        |this, kind, start, cx| {
                            this.card.start(kind, start);
                            cx.notify();
                        },
                    ));
                let drag_layer = floating::drag_layer(
                    &self.card,
                    cx,
                    "device-drag",
                    |this, position, cx| {
                        if this.card.drag_to(position) {
                            cx.notify();
                        }
                    },
                    |this, cx| {
                        if this.card.end_drag() {
                            cx.notify();
                        }
                    },
                );
                div()
                    .absolute()
                    .inset_0()
                    .child(floating::area_canvas(&self.card))
                    .child(card)
                    .when_some(drag_layer, |root, layer| root.child(layer))
                    .into_any_element()
            }
        }
    }
}

fn platform_icon(platform: Platform, color: Hsla) -> Icon {
    Icon::new(match platform {
        Platform::Ios => IconName::Apple,
        Platform::Android => IconName::Smartphone,
    })
    .small()
    .text_color(color)
}

/// Why the hub can't show devices yet, with what to do about it.
enum HubNote {
    Off,
    Starting,
    Failed(String),
}

fn hub_note(status: &HubStatus) -> Option<HubNote> {
    match status {
        HubStatus::Running(_) => None,
        HubStatus::Starting => Some(HubNote::Starting),
        HubStatus::Off => Some(HubNote::Off),
        HubStatus::Failed(error) => Some(HubNote::Failed(error.clone())),
    }
}

impl HubNote {
    fn render(self, cx: &mut Context<DevicePanel>) -> AnyElement {
        let text = |t: String| div().text_xs().text_color(ui::muted()).child(t);
        match self {
            Self::Starting => h_flex()
                .p_3()
                .gap_2()
                .child(Spinner::new().xsmall().color(ui::accent()))
                .child(text("Starting the device hub…".into()))
                .into_any_element(),
            Self::Off => v_flex()
                .p_3()
                .gap_2()
                .child(text(
                    "Turn on Use devices to see simulators and emulators here, live and touchable."
                        .into(),
                ))
                .child(
                    Button::new("device-settings")
                        .small()
                        .primary()
                        .label("Configuration → Devices")
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(DevicePanelEvent::OpenSettings)),
                        ),
                )
                .into_any_element(),
            Self::Failed(error) => v_flex()
                .p_3()
                .gap_2()
                .child(div().text_xs().text_color(ui::danger()).child(error))
                .child(
                    Button::new("device-retry")
                        .small()
                        .ghost()
                        .icon(Icon::new(IconName::RefreshCw))
                        .label("Try again")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hub.update(cx, |hub, cx| hub.ensure(cx));
                        })),
                )
                .into_any_element(),
        }
    }
}
