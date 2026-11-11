//! The conversation's rail: a mark on the right edge for every message you
//! sent, the one you are reading highlighted, and a click that glides there;
//! plus a button back to the latest output. Both show only while the
//! conversation is longer than the pane.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::SessionView;
use crate::shared::ui;
use gentle_core::scroll;
use gentle_core::transcript::Item;

/// Where a glide goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// A list row, brought to the top of the pane.
    Row(usize),
    /// The latest output.
    End,
}

/// A smooth scroll in progress, one step per frame.
pub(super) struct Glide {
    target: Target,
    started: Instant,
}

/// A glide gives up and lands after this long.
const GLIDE_FOR: Duration = Duration::from_millis(900);
/// The share of the remaining distance each frame covers (ease-out).
const EASE: f32 = 0.24;
/// The most a frame moves. Steps stay inside the list's overdraw, where rows
/// are measured, so `scroll_by` never lands on a guessed height.
const CRUISE: f32 = 420.;
/// How long the scroll thumb stays after the last scroll.
const THUMB_LINGER: Duration = Duration::from_millis(1200);

/// A thumb drag: where the mouse and the thumb were when it began.
#[derive(Clone, Copy)]
pub(super) struct RailDrag {
    mouse_y: f32,
    thumb_top: f32,
}

/// The rail's layout for one frame.
pub(super) struct RailGeometry {
    estimate: scroll::Estimate,
    viewport: f32,
    rail: f32,
    /// Rail pixels per content pixel.
    scale: f32,
    thumb_top: f32,
    thumb_size: f32,
}

/// The message to show in a mark's tooltip: its first line, shortened.
fn preview(text: &str) -> String {
    gentle_core::text::first_line(text.trim(), 90)
}

impl SessionView {
    /// Glide to transcript item `ix` (one of your messages).
    fn glide_to_item(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.start_glide(Target::Row(ix + 1), cx);
    }

    fn start_glide(&mut self, target: Target, cx: &mut Context<Self>) {
        self.live.glide = Some(Glide {
            target,
            started: Instant::now(),
        });
        cx.notify();
    }

    /// A wheel or trackpad scroll: it takes over from a glide, and the thumb
    /// shows until shortly after the last one.
    pub(super) fn on_user_scroll(&mut self, cx: &mut Context<Self>) {
        self.live.glide = None;
        self.live.rail_scrolling = true;
        self.live.rail_hide = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(THUMB_LINGER).await;
            let _ = this.update(cx, |this, cx| {
                this.live.rail_scrolling = false;
                cx.notify();
            });
        }));
    }

    /// One frame of the glide in progress; asks for the next frame until it
    /// lands. A target the list has measured eases in; one it hasn't is
    /// approached at cruise speed (rows get measured as they near the pane).
    pub(super) fn step_glide(&mut self, window: &mut Window) {
        let Some(glide) = self.live.glide.as_ref() else {
            return;
        };
        let state = self.list.state();
        let viewport = state.viewport_bounds();
        let late = glide.started.elapsed() >= GLIDE_FOR;
        let step = |distance: f32| {
            let eased = distance * EASE;
            let eased = if eased.abs() < 1.5 {
                distance.signum() * distance.abs().min(1.5)
            } else {
                eased
            };
            px(eased.clamp(-CRUISE, CRUISE))
        };
        let offset = -f32::from(state.scroll_px_offset_for_scrollbar().y);
        let remaining = f32::from(state.max_offset_for_scrollbar().y) - offset;
        let done = match glide.target {
            Target::End => {
                if late || remaining <= 1. {
                    state.scroll_to_end();
                    true
                } else {
                    state.scroll_by(step(remaining));
                    false
                }
            }
            Target::Row(row) => {
                let delta = state
                    .bounds_for_item(row)
                    .map(|bounds| f32::from(bounds.top() - viewport.top()));
                match delta {
                    // A row near the end can't reach the top: the bottom is
                    // as close as it gets.
                    Some(delta) if delta > 0. && remaining <= 1. => true,
                    Some(delta) if !late && delta.abs() > 1. => {
                        state.scroll_by(step(delta.min(remaining)));
                        false
                    }
                    None if !late => {
                        let above = row < state.logical_scroll_top().item_ix;
                        state.scroll_by(px(if above { -CRUISE } else { CRUISE }));
                        false
                    }
                    // Arrived (or out of time): land exactly on the row.
                    _ => {
                        state.scroll_to(ListOffset {
                            item_ix: row,
                            offset_in_item: px(0.),
                        });
                        true
                    }
                }
            }
        };
        if done {
            self.live.glide = None;
        }
        window.request_animation_frame();
    }

    /// The thumb was grabbed at `mouse_y`.
    fn start_rail_drag(&mut self, mouse_y: f32, thumb_top: f32, cx: &mut Context<Self>) {
        self.live.glide = None;
        self.live.rail_hide = None;
        self.live.rail_scrolling = true;
        self.live.rail_drag = Some(RailDrag { mouse_y, thumb_top });
        cx.notify();
    }

    /// The mouse moved while the thumb is held: put the pane where the thumb
    /// is along the track, landing on the row the estimate places there (so
    /// rows the list hasn't laid out yet are reached too).
    fn drag_rail(&mut self, mouse_y: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.live.rail_drag else {
            return;
        };
        let geometry = self.scroll_estimate();
        let track = (geometry.rail - geometry.thumb_size).max(1.);
        let top = (drag.thumb_top + mouse_y - drag.mouse_y).clamp(0., track);
        let max_offset = (geometry.estimate.content - geometry.viewport).max(0.);
        let target = top / track * max_offset;
        let tops = &geometry.estimate.tops;
        let row = tops.partition_point(|t| *t <= target).saturating_sub(1);
        let within = target - tops.get(row).copied().unwrap_or(0.);
        self.list.state().scroll_to(ListOffset {
            item_ix: row,
            offset_in_item: px(within.max(0.)),
        });
        // The thumb stays under the mouse.
        self.live.rail_thumb = Some((target, top));
        cx.notify();
    }

    fn end_rail_drag(&mut self, cx: &mut Context<Self>) {
        if self.live.rail_drag.take().is_some() {
            // The thumb lingers like after a wheel scroll.
            self.on_user_scroll(cx);
            cx.notify();
        }
    }

    /// Note the heights the pane shows, estimate where it is along the whole
    /// conversation and place the thumb (see `gentle_core::scroll`).
    pub(super) fn scroll_estimate(&mut self) -> RailGeometry {
        self.list.record_heights();
        let (estimate, viewport) = self.list.estimate();
        // The rail is the pane less its 10 px top and 16 px bottom insets.
        let rail = (viewport - 26.).max(1.);
        let scale = rail / estimate.content.max(1.);
        let thumb_size = (viewport * scale).clamp(24_f32.min(rail), rail);
        let thumb_top = scroll::steady_thumb(
            self.live.rail_thumb,
            estimate.offset,
            estimate.offset * scale,
            scale,
            (estimate.content - viewport).max(0.),
            rail - thumb_size,
        );
        self.live.rail_thumb = Some((estimate.offset, thumb_top));
        RailGeometry {
            estimate,
            viewport,
            rail,
            scale,
            thumb_top,
            thumb_size,
        }
    }

    /// The marks for your messages and, while you scroll (or point at the
    /// rail), the scroll thumb. Both come from the same estimate, so a mark
    /// sits where the thumb is when that message is at the top. Nothing
    /// while everything fits in the pane.
    pub(super) fn render_rail(
        &self,
        geometry: &RailGeometry,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let RailGeometry {
            estimate,
            viewport,
            rail,
            scale,
            thumb_top,
            thumb_size: size,
        } = geometry;
        let (tops, content, offset) = (&estimate.tops, estimate.content, estimate.offset);
        let (viewport, rail, scale, thumb_top, size) =
            (*viewport, *rail, *scale, *thumb_top, *size);
        if content <= viewport + 1. {
            return None;
        }
        let dragging = self.live.rail_drag.is_some();
        let thumb = div()
            .id("rail-thumb")
            .absolute()
            .right(px(2.))
            .w(px(10.))
            .top(px(thumb_top))
            .h(px(size))
            .rounded_full()
            .cursor_grab()
            .bg(ui::fg().opacity(if dragging { 0.2 } else { 0.08 }))
            .hover(|s| s.bg(ui::fg().opacity(0.16)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.start_rail_drag(f32::from(event.position.y), thumb_top, cx);
                }),
            )
            .when(
                !self.live.rail_scrolling && self.live.glide.is_none() && !dragging,
                |d| d.invisible().group_hover("rail", |s| s.visible()),
            );
        // While the thumb is held, follow the mouse anywhere in the window.
        let follow = dragging.then(|| {
            let entity = cx.entity();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let moved = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            let y = f32::from(event.position.y);
                            moved.update(cx, |this, cx| this.drag_rail(y, cx));
                        }
                    });
                    let released = entity.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble {
                            released.update(cx, |this, cx| this.end_rail_drag(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });
        let marks: Vec<(usize, String)> = self
            .transcript
            .items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| match item {
                Item::User { text, .. } if !text.trim().is_empty() => Some((ix, preview(text))),
                _ => None,
            })
            .collect();
        // The message you are reading: the last one that starts above the
        // pane's upper third.
        let reading = offset + viewport / 3.;
        let current = marks
            .iter()
            .rev()
            .find(|(ix, _)| tops.get(ix + 1).is_some_and(|top| *top <= reading))
            .or(marks.first())
            .map(|(ix, _)| *ix);
        Some(
            div()
                .group("rail")
                .absolute()
                .top(px(10.))
                .bottom(px(16.))
                .right(px(2.))
                .w(px(16.))
                .child(thumb)
                .children(follow)
                .children(marks.into_iter().map(|(ix, text)| {
                    let at = tops.get(ix + 1).copied().unwrap_or(content) * scale;
                    let active = Some(ix) == current;
                    div()
                        .id(("rail-mark", ix))
                        .absolute()
                        .top(px(at.clamp(0., rail)))
                        .right(px(2.))
                        .w(px(16.))
                        .h(px(9.))
                        .mt(px(-4.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .group("rail-mark")
                        .cursor_pointer()
                        .tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
                        .on_click(cx.listener(move |this, _, _, cx| this.glide_to_item(ix, cx)))
                        .child(
                            div()
                                .h(px(3.))
                                .w(px(if active { 12. } else { 9. }))
                                .rounded_full()
                                .bg(if active { ui::accent() } else { ui::faint() })
                                .when(!active, |d| d.group_hover("rail-mark", |s| s.bg(ui::fg()))),
                        )
                }))
                .into_any_element(),
        )
    }

    /// Back to the latest output, while it is out of view.
    pub(super) fn render_jump_down(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.list.state();
        if state.max_offset_for_scrollbar().y <= px(0.) || self.list.at_bottom() {
            return None;
        }
        Some(
            h_flex()
                .absolute()
                .bottom(px(12.))
                .left_0()
                .right_0()
                .justify_center()
                .child(
                    div()
                        .id("jump-down")
                        .size(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(ui::popover_bg())
                        .border_1()
                        .border_color(ui::border())
                        .text_color(ui::muted())
                        .cursor_pointer()
                        .hover(|s| s.text_color(ui::fg()).bg(ui::surface_hover()))
                        .tooltip(|window, cx| Tooltip::new("Latest output").build(window, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.start_glide(Target::End, cx)))
                        .child(Icon::new(IconName::ArrowDown).small()),
                )
                .into_any_element(),
        )
    }
}
