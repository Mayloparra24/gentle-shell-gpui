//! Floating web pages and the title bar's instance tray. What an agent opens
//! (a dev server's URL, the design canvas, `gentle-open`) starts hidden in the
//! tray, so the window never rearranges on its own; the user looks at it from
//! the tray's circle. What the user opens (`/web`) shows at once. A hidden
//! page costs nothing: it has no native view. Simulators and emulators have
//! their own panel (`features::devices`).

use super::*;
use crate::features::browser::{BrowserEvent, BrowserPanel, CanvasNotice};
use crate::shared::floating;
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::{ActiveTheme as _, Selectable};

/// One web page, shown floating or kept in the tray.
pub(super) struct Stage {
    id: u64,
    panel: Entity<BrowserPanel>,
    shown: bool,
    /// The agent pane that opened it, named in the tray.
    origin: Option<PaneId>,
    /// What the tray last drew for it: title and address. The page's own
    /// redraws only redraw the window when this changes.
    summary: (String, String),
    _subscriptions: [Subscription; 2],
}

impl Stage {
    /// The floating card, while shown.
    pub(super) fn element(&self) -> Option<AnyElement> {
        self.shown.then(|| self.panel.clone().into_any_element())
    }

    fn set_shown(&mut self, shown: bool, cx: &mut App) {
        self.shown = shown;
        self.panel.update(cx, |panel, cx| {
            if shown {
                panel.show(cx)
            } else {
                panel.hide(cx)
            }
        });
    }

    /// What the tray shows for it: title and address.
    fn summary(panel: &Entity<BrowserPanel>, cx: &App) -> (String, String) {
        let panel = panel.read(cx);
        (panel.title(), address(panel.url()))
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
        let existing = self.stages.iter().find_map(|stage| {
            let panel = stage.panel.read(cx);
            (!panel.is_design() && panel.url() == url).then(|| (stage.id, stage.panel.clone()))
        });
        if let Some((id, entity)) = existing {
            entity.update(cx, |panel, cx| panel.navigate(url, None, cx));
            if reveal {
                self.set_stage_shown(id, true, window, cx);
            }
            return;
        }
        let panel = cx.new(|cx| BrowserPanel::new(url, None, cx));
        self.add_stage(panel, origin, reveal, window, cx);
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
        let design = |stage: &Stage, cx: &App| {
            stage
                .panel
                .read(cx)
                .is_design()
                .then(|| stage.panel.clone())
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
                    self.remove_stage(id, window, cx);
                }
            }
            CanvasNotice::Open { url, .. } => {
                let label = Some("Design canvas".to_string());
                match self.stages.iter().find_map(|stage| design(stage, cx)) {
                    Some(entity) => entity.update(cx, |panel, cx| panel.navigate(url, label, cx)),
                    None => {
                        let panel = cx.new(|cx| BrowserPanel::new(url, label, cx));
                        self.add_stage(panel, Some(origin), false, window, cx);
                    }
                }
            }
        }
    }

    fn add_stage(
        &mut self,
        panel: Entity<BrowserPanel>,
        origin: Option<PaneId>,
        shown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_stage;
        self.next_stage += 1;
        // The card's own button only hides it; the tray closes it.
        let subscriptions = [
            cx.subscribe_in(
                &panel,
                window,
                move |this, _, _: &BrowserEvent, window, cx| {
                    this.set_stage_shown(id, false, window, cx)
                },
            ),
            cx.observe(&panel, move |this, _, cx| this.stage_changed(id, cx)),
        ];
        let summary = Stage::summary(&panel, cx);
        let mut stage = Stage {
            id,
            panel,
            shown: false,
            origin,
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
        let summary = Stage::summary(&stage.panel, cx);
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

    /// Close a page.
    fn remove_stage(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.stages.iter().position(|stage| stage.id == id) else {
            return;
        };
        let stage = self.stages.remove(index);
        if stage.shown {
            self.tile_stages(window, cx);
        }
        cx.notify();
    }

    /// A screen opened or closed over the window: web views hide under it.
    pub(super) fn set_stages_covered(&self, covered: bool, cx: &mut App) {
        for stage in self.stages.iter().filter(|stage| stage.shown) {
            stage
                .panel
                .update(cx, |panel, cx| panel.set_covered(covered, cx));
        }
        self.devices
            .update(cx, |panel, cx| panel.set_covered(covered, cx));
    }

    pub(super) fn shown_stages(&self) -> usize {
        self.stages.iter().filter(|stage| stage.shown).count()
    }

    /// Dock the shown stages at the bottom-right ([`floating::docked`]): side
    /// by side at their natural size, shrunk together when they don't fit in
    /// the part of the window they may take, wrapped into rows past that.
    /// Cards stay draggable and resizable afterwards.
    pub(super) fn tile_stages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown: Vec<&Stage> = self.stages.iter().filter(|stage| stage.shown).collect();
        let frame = crate::features::browser::DEFAULT_FRAME;
        let sizes: Vec<(f32, f32)> = shown.iter().map(|_| (frame.w, frame.h)).collect();
        let size = window.viewport_size();
        let frames = floating::docked(&sizes, (size.width.as_f32(), size.height.as_f32()));
        for (stage, frame) in shown.iter().zip(frames) {
            stage
                .panel
                .update(cx, |panel, cx| panel.set_frame(frame, cx));
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
                TrayRow {
                    id: stage.id,
                    design: stage.panel.read(cx).is_design(),
                    title,
                    status,
                    color: if stage.shown {
                        ui::accent()
                    } else {
                        ui::faint()
                    },
                    source: stage
                        .origin
                        .map(|origin| self.pane_title(origin))
                        .unwrap_or_default(),
                    shown: stage.shown,
                }
            })
            .collect();
        let count = rows.len();
        let weak = cx.entity().downgrade();
        Some(
            div()
                // Clicks here are not title-bar drags.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    Popover::new("instances")
                        .anchor(Anchor::TopRight)
                        .trigger(TrayTrigger { count, open: false })
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
    design: bool,
    title: String,
    status: String,
    color: Hsla,
    /// The agent tab that opened it.
    source: SharedString,
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
                .child("Pages"),
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
        let icon = if row.design {
            IconName::Palette
        } else {
            IconName::Globe
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
                    ),
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
                            .icon(Icon::new(IconName::X))
                            .tooltip("Close")
                            .on_click(move |_, window, cx| {
                                let _ =
                                    close.update(cx, |this, cx| this.remove_stage(id, window, cx));
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

/// The circle in the title bar, with a badge counting the pages.
#[derive(IntoElement)]
struct TrayTrigger {
    count: usize,
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
                d.tooltip(|window, cx| Tooltip::new("Pages agents opened").build(window, cx))
            })
            .child(Icon::new(IconName::Globe).small())
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
                    .bg(ui::petal())
                    .text_color(ui::well().opacity(1.))
                    .text_size(px(9.))
                    .font_weight(FontWeight::BOLD)
                    .line_height(relative(1.))
                    .child(self.count.to_string()),
            )
    }
}
