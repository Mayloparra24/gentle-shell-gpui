//! The todo island: the task list floating over the conversation.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::session::SessionView;
use crate::ui;

impl SessionView {
    /// The todo island: a Dynamic-Island-style pill at the top of the
    /// conversation that unfolds downward into the task list. It only exists
    /// while the session has tasks.
    pub(super) fn render_todo_island(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.todo.tasks.is_empty() {
            return None;
        }
        let t = self.todo_morph();
        let total = self.todo.tasks.len();
        let done = self
            .todo
            .tasks
            .iter()
            .filter(|task| task.status == "done")
            .count();
        let all_done = done == total;
        let color = if all_done {
            ui::success()
        } else {
            ui::accent()
        };

        let content_h = todo_content_height(&self.todo.tasks);
        let body_h = content_h.min(TODO_MAX_BODY);
        let h = TODO_FRAME + TODO_PILL + body_h * t;
        let body_area = (h - TODO_FRAME - TODO_PILL).max(0.);
        // Rows fade in while the pill alone is still showing.
        let content_opacity = (t / 0.55).clamp(0., 1.);
        // Soften the row the footer of the card clips mid-unfold, then let the
        // fade melt away as the card settles; once open the list scrolls.
        let fade =
            ((content_h - body_area) / 24.).clamp(0., 1.) * (1. - ((t - 0.8) / 0.2).clamp(0., 1.));
        let pane = self.pane_width.get();
        let full_w = (pane - 32.).clamp(TODO_PILL_W, TODO_CARD_W);
        let w = TODO_PILL_W + (full_w - TODO_PILL_W) * t;
        let left = todo_left(pane, w);

        let badge = div()
            .size(px(22.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .bg(color.opacity(0.14))
            .child(Icon::new(IconName::ListTodo).xsmall().text_color(color));

        let pill = h_flex()
            .id("todo-island-pill")
            .absolute()
            .left_0()
            .right_0()
            .top_0()
            .h(px(TODO_PILL))
            .px(px(12.))
            .gap_1p5()
            .items_center()
            .cursor_pointer()
            // GPUI clips children to the card's rectangle, not its rounded
            // corners: match the card's inner radius (19 − 1 border) so the
            // hover fill doesn't poke out at the corners.
            .rounded_t(px(18.))
            .when(t <= 0.02, |d| d.rounded_b(px(18.)))
            .hover(|d| d.bg(ui::surface_hover().opacity(0.45)))
            .when(t > 0.02, |d| d.border_b_1().border_color(ui::border()))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_todo(cx)))
            .tooltip({
                let tip = if self.todo.open {
                    "Hide tasks"
                } else {
                    "Show tasks"
                };
                move |window, cx| Tooltip::new(tip).build(window, cx)
            })
            .child(badge)
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui::fg())
                    .child("Todos"),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_family(ui::MONO)
                    .text_size(px(10.5))
                    .text_color(if all_done { ui::success() } else { ui::muted() })
                    .child(format!("{done}/{total}")),
            )
            .child(div().flex_1().min_w_0())
            .child(todo_bars(done, total, color))
            .child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .text_color(ui::faint())
                    // Folded the card opens down; open, it collapses back up.
                    .rotate(gpui_kit::radians(std::f32::consts::PI * t)),
            );

        let body = div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(TODO_PILL))
            .h(px(body_area))
            .overflow_hidden()
            .opacity(content_opacity)
            .child(
                v_flex()
                    .id("todo-island-list")
                    .size_full()
                    .overflow_y_scroll()
                    .p(px(TODO_LIST_PAD))
                    .gap(px(TODO_ROW_GAP))
                    .children(todo_rows(&self.todo.tasks)),
            )
            .when(fade > 0.01, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(24.))
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(ui::popover_bg().opacity(0.), 0.),
                            linear_color_stop(ui::popover_bg(), 1.),
                        ))
                        .opacity(fade),
                )
            });

        let card = div()
            .absolute()
            .top(px(TODO_TOP))
            .left(px(left))
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .rounded(px(19.))
            .bg(ui::popover_bg())
            .border_1()
            .border_color(ui::border_strong().opacity(0.8))
            .occlude()
            .shadow(vec![
                BoxShadow {
                    color: ui::backdrop().opacity(0.34),
                    offset: point(px(0.), px(10.)),
                    blur_radius: px(30.),
                    spread_radius: px(2.),
                    inset: false,
                },
                BoxShadow {
                    color: color.opacity(0.08),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(40.),
                    spread_radius: px(2.),
                    inset: false,
                },
            ])
            .child(body)
            .child(pill);

        // Slip in the first time tasks arrive, never on every task update.
        Some(
            card.with_animation(
                "todo-island-appear",
                Animation::new(Duration::from_millis(320)),
                |card, t| {
                    let s = ui::spring(t);
                    card.opacity(s.min(1.)).mt(px(-8. * (1. - s)))
                },
            )
            .into_any_element(),
        )
    }

    /// Where the island's unfold is right now (0 folded, 1 open), eased.
    fn todo_morph(&self) -> f32 {
        let seconds = self.todo.motion_at.elapsed().as_secs_f32();
        let t = (seconds / TODO_MOTION.as_secs_f32()).clamp(0., 1.);
        // Opening springs a touch past the target, like the iOS island; closing
        // just eases out so the card never shrinks below the pill.
        let eased = if self.todo.motion_to > self.todo.motion_from {
            ui::spring(t)
        } else {
            1. - (1. - t).powi(3)
        };
        self.todo.motion_from + (self.todo.motion_to - self.todo.motion_from) * eased
    }

    /// Unfold or fold the card, starting from wherever the morph is now so
    /// a click mid-animation reverses smoothly. A background task repaints
    /// for the duration of the motion.
    fn toggle_todo(&mut self, cx: &mut Context<Self>) {
        let from = self.todo_morph();
        self.todo.open = !self.todo.open;
        self.todo.motion_from = from;
        self.todo.motion_to = if self.todo.open { 1. } else { 0. };
        self.todo.motion_at = Instant::now();
        cx.notify();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let moving = this
                    .update(cx, |this, cx| {
                        let moving = this.todo.motion_at.elapsed() < TODO_MOTION;
                        if moving {
                            cx.notify();
                        }
                        moving
                    })
                    .unwrap_or(false);
                if !moving {
                    break;
                }
            }
            // Paint the exact end state: folded at 0 removes the card.
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
}

/// Height of the collapsed island, and of its header once unfolded.
const TODO_PILL: f32 = 36.;
/// The island's 1 px border on both sides, inside its height.
const TODO_FRAME: f32 = 2.;
/// Air between the island and the top of the conversation.
const TODO_TOP: f32 = 10.;
/// Conversations wider than this dock the island to the top-right; narrower
/// ones keep it centered over the transcript.
const TODO_WIDE: f32 = 800.;
/// Right inset when the island is docked.
const TODO_EDGE: f32 = 24.;
/// Folded width; it widens toward its card width as it unfolds.
const TODO_PILL_W: f32 = 200.;
const TODO_CARD_W: f32 = 420.;
/// The list never gets taller than this; past it, it scrolls.
const TODO_MAX_BODY: f32 = 288.;
const TODO_LIST_PAD: f32 = 10.;
const TODO_ROW: f32 = 24.;
const TODO_ROW_NOTE: f32 = 38.;
const TODO_ROW_GAP: f32 = 2.;
/// Unfold/fold duration; the morph is eased out and interruptible.
const TODO_MOTION: Duration = Duration::from_millis(360);

/// The island's left edge: centered while the conversation is narrow, docked
/// to the top-right once there is room beside the messages.
fn todo_left(pane: f32, width: f32) -> f32 {
    if pane >= TODO_WIDE {
        (pane - width - TODO_EDGE).max(0.)
    } else {
        (pane - width) / 2.
    }
}

/// Exact height of the list's content, so the island can size its unfold.
fn todo_content_height(todo: &[gentle_core::tools::TodoTask]) -> f32 {
    if todo.is_empty() {
        return 0.;
    }
    let note = |task: &gentle_core::tools::TodoTask| {
        task.note
            .as_deref()
            .is_some_and(|note| !note.trim().is_empty())
    };
    let rows: f32 = todo
        .iter()
        .map(|task| if note(task) { TODO_ROW_NOTE } else { TODO_ROW })
        .sum();
    let gaps = todo.len().saturating_sub(1) as f32 * TODO_ROW_GAP;
    TODO_LIST_PAD * 2. + rows + gaps
}

/// A small equalizer for the island: one bar per task (up to six), filled as
/// tasks complete. It gives the pill a live, Dynamic-Island-like feel.
fn todo_bars(done: usize, total: usize, color: Hsla) -> Div {
    const HEIGHTS: [f32; 6] = [8., 12., 9., 14., 11., 6.];
    let count = total.clamp(1, HEIGHTS.len());
    let filled = if total == 0 {
        0
    } else {
        ((done as f32 / total as f32) * count as f32).round() as usize
    };
    h_flex()
        .gap(px(2.))
        .items_center()
        .children(HEIGHTS[..count].iter().enumerate().map(|(i, height)| {
            div()
                .w(px(3.))
                .h(px(*height))
                .rounded_full()
                .bg(if i < filled { color } else { ui::border() })
        }))
}

/// One row per todo task (status badge, one-line title, optional note) for
/// the task island. Rows have a fixed height so the unfold can size itself.
fn todo_rows(todo: &[gentle_core::tools::TodoTask]) -> Vec<AnyElement> {
    todo.iter()
        .enumerate()
        .map(|(index, task)| {
            let done = task.status == "done";
            let active = task.status == "in_progress";
            let note = task.note.as_deref().filter(|note| !note.trim().is_empty());
            let (title_color, note_color) = match task.status.as_str() {
                "done" => (ui::muted(), ui::faint()),
                "in_progress" => (ui::fg(), ui::accent()),
                "blocked" => (ui::fg(), ui::warning()),
                "dropped" => (ui::muted(), ui::faint()),
                _ => (ui::fg(), ui::faint()),
            };
            h_flex()
                .id(("todo-row", index))
                .h(px(if note.is_some() {
                    TODO_ROW_NOTE
                } else {
                    TODO_ROW
                }))
                .w_full()
                .items_center()
                .gap_2p5()
                .px_2()
                .rounded_lg()
                .when(active, |d| d.bg(ui::accent().opacity(0.07)))
                .child(todo_badge(&task.status))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(title_color)
                                .when(done, |d| d.line_through())
                                .child(task.title.clone()),
                        )
                        .when_some(note, |this, note| {
                            this.child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(10.5))
                                    .text_color(note_color)
                                    .child(note.to_string()),
                            )
                        }),
                )
                .into_any_element()
        })
        .collect()
}

/// The task's status as a soft little disc: check, live dot, `!`, dash, ring.
fn todo_badge(status: &str) -> Div {
    let base = div()
        .size(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1();
    match status {
        "done" => base
            .bg(ui::success().opacity(0.14))
            .border_color(ui::success().opacity(0.3))
            .child(
                Icon::new(IconName::Check)
                    .xsmall()
                    .text_color(ui::success()),
            ),
        "in_progress" => base
            .bg(ui::accent().opacity(0.08))
            .border_color(ui::accent().opacity(0.55))
            .child(div().size(px(6.)).rounded_full().bg(ui::accent())),
        "blocked" => base
            .bg(ui::warning().opacity(0.12))
            .border_color(ui::warning().opacity(0.32))
            .child(
                div()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::warning())
                    .child("!"),
            ),
        "dropped" => base
            .border_color(ui::faint().opacity(0.3))
            .child(div().w(px(6.)).h(px(1.5)).rounded_full().bg(ui::faint())),
        _ => base.border_color(ui::border_strong()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TODO_LIST_PAD, TODO_ROW, TODO_ROW_GAP, TODO_ROW_NOTE, todo_content_height, todo_left,
    };

    #[test]
    fn todo_island_centers_narrow_and_docks_right_wide() {
        // Narrow: centered over the conversation.
        assert_eq!(todo_left(500., 200.), 150.);
        // Wide: right edge pinned 24 px from the conversation's edge.
        assert_eq!(todo_left(1200., 420.), 756.);
        assert_eq!(todo_left(1200., 200.), 976.);
        // Just below the breakpoint it is still centered.
        assert_eq!(todo_left(799., 420.), (799. - 420.) / 2.);
    }

    #[test]
    fn todo_island_height_counts_rows_notes_and_gaps() {
        let task = |status: &str, note: Option<&str>| gentle_core::tools::TodoTask {
            title: "Task".into(),
            status: status.into(),
            note: note.map(str::to_string),
        };
        assert_eq!(
            todo_content_height(&[task("done", None), task("in_progress", Some("working"))]),
            2. * TODO_LIST_PAD + TODO_ROW + TODO_ROW_NOTE + TODO_ROW_GAP
        );
        // A blank note is one line, not two.
        assert_eq!(
            todo_content_height(&[task("pending", Some("  "))]),
            2. * TODO_LIST_PAD + TODO_ROW
        );
        assert_eq!(todo_content_height(&[]), 0.);
    }
}
