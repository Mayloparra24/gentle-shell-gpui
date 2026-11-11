//! Floating stages (web pages and device cards) and the title bar's instance
//! tray. What an agent opens (a dev server's URL, the design canvas, an
//! emulator it boots) starts hidden in the tray, so the window never
//! rearranges on its own; the user looks at it from the tray's circle. What
//! the user opens (`/web`, the palette's Devices) shows at once. A hidden
//! stage costs nothing on screen: a web page has no native view and a device
//! takes no screenshots.

use super::*;
use crate::features::browser::{BrowserEvent, BrowserPanel, CanvasNotice};
use crate::features::devices::{DevicesEvent, DevicesPanel};
use crate::shared::floating::{self, Frame};
use gentle_core::devices::{DeviceCommand, LaunchIntent};
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::{ActiveTheme as _, Selectable};

pub(super) enum StageKind {
    Web(Entity<BrowserPanel>),
    Device(Entity<DevicesPanel>),
}

impl StageKind {
    /// What the tray shows for it: title and status line.
    fn summary(&self, cx: &App) -> (String, String) {
        match self {
            Self::Web(entity) => {
                let panel = entity.read(cx);
                (panel.title(), address(panel.url()))
            }
            Self::Device(entity) => {
                let panel = entity.read(cx);
                (panel.title(), panel.status())
            }
        }
    }
}

/// One web page or device card, shown floating or kept in the tray.
pub(super) struct Stage {
    id: u64,
    kind: StageKind,
    shown: bool,
    /// The agent pane that opened it (or drives it), named in the tray.
    origin: Option<PaneId>,
    /// The last device command that agent ran on it.
    activity: Option<String>,
    /// What the tray last drew for it. A panel's own redraws (a new frame
    /// every 1.5 s) only redraw the window when this changes.
    summary: (String, String),
    _subscriptions: [Subscription; 2],
}

impl Stage {
    /// The floating card, while shown.
    pub(super) fn element(&self) -> Option<AnyElement> {
        self.shown.then(|| match &self.kind {
            StageKind::Web(entity) => entity.clone().into_any_element(),
            StageKind::Device(entity) => entity.clone().into_any_element(),
        })
    }

    fn set_shown(&mut self, shown: bool, cx: &mut App) {
        self.shown = shown;
        match &self.kind {
            StageKind::Web(entity) => entity.update(cx, |panel, cx| {
                if shown {
                    panel.show(cx)
                } else {
                    panel.hide(cx)
                }
            }),
            StageKind::Device(entity) => entity.update(cx, |panel, cx| {
                if shown {
                    panel.show(cx)
                } else {
                    panel.hide(cx)
                }
            }),
        }
    }

    /// The size a card opens with, before docking scales it.
    fn natural_size(&self) -> (f32, f32) {
        let frame = match self.kind {
            StageKind::Web(_) => crate::features::browser::DEFAULT_FRAME,
            StageKind::Device(_) => crate::features::devices::DEFAULT_FRAME,
        };
        (frame.w, frame.h)
    }

    fn place(&self, frame: Frame, cx: &mut App) {
        match &self.kind {
            StageKind::Web(entity) => entity.update(cx, |panel, cx| panel.set_frame(frame, cx)),
            StageKind::Device(entity) => entity.update(cx, |panel, cx| panel.set_frame(frame, cx)),
        }
    }
}

/// A URL without its scheme, for status lines.
fn address(url: &str) -> String {
    url.trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string()
}

impl Workspace {
    // ── Opening ─────────────────────────────────────────────────────────────

    /// Show a page in its own card. `reveal` is false for pages an agent
    /// opened (a dev server's URL, `gentle-open`): those wait in the tray. A
    /// page that is already open reuses its card.
    pub(super) fn open_web(
        &mut self,
        url: String,
        origin: Option<PaneId>,
        reveal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.stages.iter().find_map(|stage| match &stage.kind {
            StageKind::Web(entity) => {
                let panel = entity.read(cx);
                (!panel.is_design() && panel.url() == url).then(|| (stage.id, entity.clone()))
            }
            StageKind::Device(_) => None,
        });
        if let Some((id, entity)) = existing {
            entity.update(cx, |panel, cx| panel.navigate(url, None, cx));
            if reveal {
                self.set_stage_shown(id, true, window, cx);
            }
            return;
        }
        let panel = cx.new(|_| BrowserPanel::new(url, None));
        self.add_stage(StageKind::Web(panel), origin, reveal, window, cx);
    }

    /// Gentle Design's canvas: kept in the tray when it opens, reloaded in
    /// place when it refreshes, removed when it stops.
    pub(super) fn show_design_canvas(
        &mut self,
        canvas: CanvasNotice,
        origin: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let design = |stage: &Stage, cx: &App| match &stage.kind {
            StageKind::Web(entity) if entity.read(cx).is_design() => Some(entity.clone()),
            _ => None,
        };
        match canvas {
            CanvasNotice::Stopped => {
                let ids: Vec<u64> = self
                    .stages
                    .iter()
                    .filter(|stage| design(stage, cx).is_some())
                    .map(|stage| stage.id)
                    .collect();
                for id in ids {
                    self.remove_stage(id, false, window, cx);
                }
            }
            CanvasNotice::Open { url, .. } => {
                let label = Some("Design canvas".to_string());
                match self.stages.iter().find_map(|stage| design(stage, cx)) {
                    Some(entity) => entity.update(cx, |panel, cx| panel.navigate(url, label, cx)),
                    None => {
                        let panel = cx.new(|_| BrowserPanel::new(url, label));
                        self.add_stage(StageKind::Web(panel), Some(origin), false, window, cx);
                    }
                }
            }
        }
    }

    /// The palette's Devices: show the first device card, or a new one.
    pub(super) fn show_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self
            .stages
            .iter()
            .find(|stage| matches!(stage.kind, StageKind::Device(_)))
            .map(|stage| stage.id);
        match existing {
            Some(id) => self.set_stage_shown(id, true, window, cx),
            None => {
                let panel = cx.new(|cx| DevicesPanel::new(window, cx));
                self.add_stage(StageKind::Device(panel), None, true, window, cx);
            }
        }
    }

    /// An agent is starting a device: follow it from the tray, reusing the
    /// card already on that device. Each different device gets its own card,
    /// so Android and iOS (or two emulators) can be followed at once.
    pub(super) fn follow_device(
        &mut self,
        intent: LaunchIntent,
        origin: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.stages.iter_mut().find(|stage| {
            let StageKind::Device(entity) = &stage.kind else {
                return false;
            };
            let panel = entity.read(cx);
            match intent.id.as_deref() {
                Some(want) => panel.selected_key() == Some(want),
                // An unnamed device: reuse a card that is not on one yet.
                None => {
                    panel.selected_key().is_none()
                        && panel
                            .platform()
                            .is_none_or(|platform| platform == intent.platform)
                }
            }
        });
        if let Some(stage) = existing {
            stage.origin.get_or_insert(origin);
            if let StageKind::Device(entity) = &stage.kind {
                entity.update(cx, |panel, cx| panel.follow(intent, cx));
            }
            cx.notify();
            return;
        }
        let panel = cx.new(|cx| DevicesPanel::new(window, cx));
        panel.update(cx, |panel, cx| panel.follow(intent, cx));
        self.add_stage(StageKind::Device(panel), Some(origin), false, window, cx);
    }

    /// An agent drove a device: show the command on that agent's device
    /// card (a card opened from the palette is adopted by the first agent
    /// that drives its platform).
    pub(super) fn device_activity(
        &mut self,
        origin: PaneId,
        command: DeviceCommand,
        cx: &mut Context<Self>,
    ) {
        let fits = |stage: &Stage, cx: &App| match &stage.kind {
            StageKind::Device(entity) => match (command.platform, entity.read(cx).platform()) {
                (Some(wanted), Some(platform)) => wanted == platform,
                _ => true,
            },
            StageKind::Web(_) => false,
        };
        let own = self
            .stages
            .iter()
            .any(|stage| stage.origin == Some(origin) && fits(stage, cx));
        let mut changed = false;
        for stage in &mut self.stages {
            let candidate = if own {
                stage.origin == Some(origin)
            } else {
                stage.origin.is_none()
            };
            if candidate && fits(stage, cx) {
                stage.origin = Some(origin);
                stage.activity = Some(command.summary.clone());
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn add_stage(
        &mut self,
        kind: StageKind,
        origin: Option<PaneId>,
        shown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_stage;
        self.next_stage += 1;
        // The card's own button only hides it; the tray stops or closes it.
        let subscriptions = match &kind {
            StageKind::Web(entity) => [
                cx.subscribe_in(
                    entity,
                    window,
                    move |this, _, _: &BrowserEvent, window, cx| {
                        this.set_stage_shown(id, false, window, cx)
                    },
                ),
                cx.observe(entity, move |this, _, cx| this.stage_changed(id, cx)),
            ],
            StageKind::Device(entity) => [
                cx.subscribe_in(
                    entity,
                    window,
                    move |this, _, _: &DevicesEvent, window, cx| {
                        this.set_stage_shown(id, false, window, cx)
                    },
                ),
                cx.observe(entity, move |this, _, cx| this.stage_changed(id, cx)),
            ],
        };
        let summary = kind.summary(cx);
        let mut stage = Stage {
            id,
            kind,
            shown: false,
            origin,
            activity: None,
            summary,
            _subscriptions: subscriptions,
        };
        stage.set_shown(shown, cx);
        self.stages.push(stage);
        if shown {
            self.tile_stages(window, cx);
        }
        cx.notify();
    }

    // ── Showing, hiding, closing ────────────────────────────────────────────

    /// Redraw the tray only when what it shows for this stage changed.
    fn stage_changed(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(stage) = self.stages.iter_mut().find(|stage| stage.id == id) else {
            return;
        };
        let summary = stage.kind.summary(cx);
        if stage.summary != summary {
            stage.summary = summary;
            cx.notify();
        }
    }

    fn set_stage_shown(
        &mut self,
        id: u64,
        shown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(stage) = self.stages.iter_mut().find(|stage| stage.id == id) else {
            return;
        };
        if stage.shown == shown {
            return;
        }
        stage.set_shown(shown, cx);
        self.tile_stages(window, cx);
        cx.notify();
    }

    fn hide_all_stages(&mut self, cx: &mut Context<Self>) {
        for stage in self.stages.iter_mut().filter(|stage| stage.shown) {
            stage.set_shown(false, cx);
        }
        cx.notify();
    }

    /// Drop a stage. `stop` also stops the device it follows (the tray's
    /// Stop); dropping the panel already stops what the card launched.
    fn remove_stage(&mut self, id: u64, stop: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.stages.iter().position(|stage| stage.id == id) else {
            return;
        };
        let stage = self.stages.remove(index);
        if stop && let StageKind::Device(entity) = &stage.kind {
            entity.update(cx, |panel, _| panel.stop_all());
        }
        if stage.shown {
            self.tile_stages(window, cx);
        }
        cx.notify();
    }

    /// Stop every device the cards started; called on quit.
    pub(super) fn shutdown_stages(&self, cx: &mut App) {
        for stage in &self.stages {
            if let StageKind::Device(entity) = &stage.kind {
                entity.update(cx, |panel, _| panel.shutdown_all());
            }
        }
    }

    /// A screen opened or closed over the window: web views hide under it
    /// and device cards stop capturing.
    pub(super) fn set_stages_covered(&self, covered: bool, cx: &mut App) {
        for stage in self.stages.iter().filter(|stage| stage.shown) {
            match &stage.kind {
                StageKind::Web(entity) => {
                    entity.update(cx, |panel, cx| panel.set_covered(covered, cx))
                }
                StageKind::Device(entity) => {
                    entity.update(cx, |panel, cx| panel.set_covered(covered, cx))
                }
            }
        }
    }

    pub(super) fn shown_stages(&self) -> usize {
        self.stages.iter().filter(|stage| stage.shown).count()
    }

    /// The tray opened: discover again any device whose state may be stale
    /// (a hidden card stops polling once its device has booted).
    fn refresh_tray(&mut self, cx: &mut Context<Self>) {
        for stage in &self.stages {
            if let StageKind::Device(entity) = &stage.kind {
                entity.update(cx, |panel, cx| panel.refresh_if_stale(cx));
            }
        }
    }

    /// Dock the shown stages at the bottom-right ([`floating::docked`]): side
    /// by side at their natural size, shrunk together when they don't fit in
    /// the part of the window they may take, wrapped into rows past that.
    /// Cards stay draggable and resizable afterwards.
    pub(super) fn tile_stages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown: Vec<&Stage> = self.stages.iter().filter(|stage| stage.shown).collect();
        let sizes: Vec<(f32, f32)> = shown.iter().map(|stage| stage.natural_size()).collect();
        let size = window.viewport_size();
        let frames = floating::docked(&sizes, (size.width.as_f32(), size.height.as_f32()));
        for (stage, frame) in shown.iter().zip(frames) {
            stage.place(frame, cx);
        }
    }

    // ── The tray ────────────────────────────────────────────────────────────

    /// The title bar's circle: how many instances are open, colored by the
    /// one that most needs a look, and the list of them in a popover. Nothing
    /// while there are none.
    pub(super) fn render_tray(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.stages.is_empty() {
            return None;
        }
        let rows: Vec<TrayRow> = self
            .stages
            .iter()
            .map(|stage| {
                let (title, status) = stage.summary.clone();
                let (device, design, color) = match &stage.kind {
                    StageKind::Device(entity) => (true, false, entity.read(cx).status_color()),
                    StageKind::Web(entity) => (
                        false,
                        entity.read(cx).is_design(),
                        if stage.shown {
                            ui::accent()
                        } else {
                            ui::faint()
                        },
                    ),
                };
                TrayRow {
                    id: stage.id,
                    device,
                    design,
                    title,
                    status,
                    color,
                    source: stage
                        .origin
                        .map(|origin| self.pane_title(origin))
                        .unwrap_or_default(),
                    activity: stage.activity.clone(),
                    shown: stage.shown,
                }
            })
            .collect();
        let rank = |color: Hsla| {
            if color == ui::danger() {
                3
            } else if color == ui::champagne() {
                2
            } else if color == ui::success() {
                1
            } else {
                0
            }
        };
        let badge = rows
            .iter()
            .map(|row| row.color)
            .max_by_key(|color| rank(*color))
            .filter(|color| rank(*color) > 0)
            .unwrap_or_else(ui::petal);
        let count = rows.len();
        let weak = cx.entity().downgrade();
        let refresh = weak.clone();
        Some(
            div()
                // Clicks here are not title-bar drags.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Popover::new("instances")
                        .anchor(Anchor::TopRight)
                        .trigger(TrayTrigger {
                            count,
                            badge,
                            open: false,
                        })
                        .on_open_change(move |open, _, cx| {
                            if *open {
                                let _ = refresh.update(cx, |this, cx| this.refresh_tray(cx));
                            }
                        })
                        .w(px(380.))
                        .p_0()
                        .content(move |_, _, cx| {
                            ui::popover_in(
                                ElementId::Name("instances-in".into()),
                                tray_panel(rows.clone(), weak.clone(), cx),
                            )
                            .into_any_element()
                        }),
                )
                .into_any_element(),
        )
    }
}

/// What the tray shows for one stage, captured when the window draws.
#[derive(Clone)]
struct TrayRow {
    id: u64,
    device: bool,
    design: bool,
    title: String,
    status: String,
    color: Hsla,
    /// The agent tab that opened or drives it.
    source: SharedString,
    activity: Option<String>,
    shown: bool,
}

fn tray_panel(
    rows: Vec<TrayRow>,
    workspace: WeakEntity<Workspace>,
    cx: &mut Context<PopoverState>,
) -> Div {
    let any_shown = rows.iter().any(|row| row.shown);
    let header = h_flex()
        .px_3()
        .pt_2p5()
        .pb_2()
        .gap_2()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui::fg())
                .child("Instances"),
        )
        .child(div().flex_1())
        .when(any_shown, |d| {
            let workspace = workspace.clone();
            d.child(
                div()
                    .id("instances-hide-all")
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_size(px(11.5))
                    .text_color(ui::muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
                    .child("Hide all")
                    .on_click(move |_, _, cx| {
                        let _ = workspace.update(cx, |this, cx| this.hide_all_stages(cx));
                    }),
            )
        });
    let rows = rows.into_iter().map(|row| {
        let id = row.id;
        let icon = match (row.device, row.design) {
            (true, _) => IconName::Smartphone,
            (false, true) => IconName::Palette,
            (false, false) => IconName::Globe,
        };
        let mut meta = row.source.to_string();
        if !meta.is_empty() {
            meta.push_str("  ·  ");
        }
        meta.push_str(&row.status);
        let open = workspace.clone();
        let toggle = workspace.clone();
        let close = workspace.clone();
        let shown = row.shown;
        h_flex()
            .id(("instance", id as usize))
            .w_full()
            .items_start()
            .gap_2p5()
            .px_2()
            .py_2()
            .rounded(px(8.))
            .hover(|s| s.bg(ui::surface_hover()))
            .child(
                div()
                    .mt(px(1.))
                    .size(px(22.))
                    .flex_none()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .bg(row.color.opacity(0.12))
                    .child(Icon::new(icon).xsmall().text_color(row.color)),
            )
            .child(
                // The row's text opens the card and closes the list.
                v_flex()
                    .id(("instance-open", id as usize))
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .cursor_pointer()
                    .on_click(cx.listener(move |state: &mut PopoverState, _, window, cx| {
                        state.dismiss(window, cx);
                        let _ =
                            open.update(cx, |this, cx| this.set_stage_shown(id, true, window, cx));
                    }))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .truncate()
                            .text_color(if shown { ui::fg() } else { ui::muted() })
                            .child(row.title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ui::faint())
                            .truncate()
                            .child(meta),
                    )
                    .when_some(row.activity, |d, activity| {
                        d.child(
                            div()
                                .font_family(ui::MONO)
                                .text_size(px(10.5))
                                .text_color(ui::muted())
                                .truncate()
                                .child(format!("› {activity}")),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_0p5()
                    .child(
                        Button::new(("instance-toggle", id as usize))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(if shown {
                                IconName::EyeOff
                            } else {
                                IconName::Eye
                            }))
                            .tooltip(if shown { "Hide" } else { "Show" })
                            .on_click(move |_, window, cx| {
                                let _ = toggle.update(cx, |this, cx| {
                                    this.set_stage_shown(id, !shown, window, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new(("instance-close", id as usize))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(if row.device {
                                IconName::Power
                            } else {
                                IconName::X
                            }))
                            .tooltip(if row.device {
                                "Stop the device"
                            } else {
                                "Close"
                            })
                            .on_click(move |_, window, cx| {
                                let _ = close
                                    .update(cx, |this, cx| this.remove_stage(id, true, window, cx));
                            }),
                    ),
            )
    });
    v_flex()
        .w_full()
        .font_family(cx.theme().font_family.clone())
        .child(header)
        .child(div().h(px(1.)).bg(ui::border()))
        .child(
            v_flex()
                .id("instances-list")
                .max_h(px(420.))
                .overflow_y_scroll()
                .px_1()
                .py_1()
                .children(rows),
        )
}

/// The circle in the title bar; a badge with the count, colored by the
/// instance that most needs a look (error, booting, ready).
#[derive(IntoElement)]
struct TrayTrigger {
    count: usize,
    badge: Hsla,
    open: bool,
}

impl Selectable for TrayTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for TrayTrigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id("instances-tray")
            .relative()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .text_color(ui::fg())
            .hover(|s| s.bg(ui::surface_hover()))
            .when(self.open, |d| d.bg(ui::surface_hover()))
            .when(!self.open, |d| {
                d.tooltip(|window, cx| {
                    Tooltip::new("Instances: devices and pages agents opened").build(window, cx)
                })
            })
            .child(Icon::new(IconName::MonitorSmartphone).small())
            .child(
                div()
                    .absolute()
                    .top(px(2.))
                    .right(px(1.))
                    .h(px(13.))
                    .min_w(px(13.))
                    .px(px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(self.badge)
                    .text_color(ui::well().opacity(1.))
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .line_height(relative(1.))
                    .child(self.count.to_string()),
            )
    }
}
