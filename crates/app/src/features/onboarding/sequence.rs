//! The welcome screen's install sequence: an overall progress bar, then the
//! required checks as numbered steps on a rail (waiting, installing with a
//! sliding bar and the log's tail, done, failed) and the extras below. Steps
//! come in one by one after the welcome's intro (`entrance`).

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::OnboardingView;
use super::checks::{Tone, blocks, is_ok, status_text, tone};
use super::view::status_label;
use crate::features::setup::{Check, Need, Status};
use crate::shared::ui;

/// Between one step coming in and the next.
const STEP_GAP: Duration = Duration::from_millis(110);
/// Install output lines a running step shows.
const LOG_LINES: usize = 4;
/// The step markers on the rail.
const NODE: f32 = 30.;

/// Where a step is in the sequence.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Done,
    Installing,
    Failed,
    /// Not done; `next` is the first step still to do.
    Waiting {
        next: bool,
    },
}

impl OnboardingView {
    pub(super) fn render_sequence(&self, cx: &mut Context<Self>) -> Div {
        let Some(checks) = self.checks.as_deref() else {
            return v_flex().w_full().items_center().child(ui::appear_after(
                "sequence-looking",
                self.entrance,
                8.,
                h_flex()
                    .py_8()
                    .gap_2()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(Spinner::new().small().color(ui::petal()))
                    .child("Looking at what this machine already has…"),
            ));
        };
        let required: Vec<&Check> = checks.iter().filter(|c| c.need == Need::Required).collect();
        let extras: Vec<&Check> = checks.iter().filter(|c| c.need != Need::Required).collect();
        let done = required.iter().filter(|c| is_ok(c)).count();
        // The next step to do: signing in can wait, so it never holds the
        // highlight.
        let next = required.iter().position(|c| blocks(c) && !is_ok(c));
        let steps = required.len();
        v_flex()
            .w_full()
            .gap_6()
            .child(ui::appear_after(
                "sequence-progress",
                self.entrance,
                8.,
                self.render_progress(done, steps),
            ))
            .child(
                v_flex().children(
                    required
                        .iter()
                        .enumerate()
                        .map(|(i, check)| {
                            let step = self.step_of(check, next == Some(i));
                            self.render_step(check, i, step, i + 1 == steps, cx)
                        })
                        .collect::<Vec<_>>(),
                ),
            )
            .when(!extras.is_empty(), |d| {
                d.child(self.render_extras(&extras, steps, cx))
            })
    }

    fn step_of(&self, check: &Check, next: bool) -> Step {
        if self.installing == Some(check.id) {
            Step::Installing
        } else if is_ok(check) {
            Step::Done
        } else if matches!(check.status, Status::Failed(_))
            || self.failure.as_ref().is_some_and(|(id, _)| *id == check.id)
        {
            Step::Failed
        } else {
            Step::Waiting { next }
        }
    }

    /// "2 of 4 ready", the bar growing from the last count, the platform.
    fn render_progress(&self, done: usize, steps: usize) -> Div {
        let total = steps.max(1) as f32;
        let to = done as f32 / total;
        let from = done.saturating_sub(1) as f32 / total;
        let complete = done == steps;
        h_flex()
            .gap_3()
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .font_family(ui::MONO)
                    .text_color(if complete { ui::success() } else { ui::muted() })
                    .child(format!("{done} of {steps} ready")),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(5.))
                    .rounded_full()
                    .bg(ui::well())
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(linear_gradient(
                                90.,
                                linear_color_stop(ui::petal(), 0.),
                                linear_color_stop(
                                    if complete {
                                        ui::success()
                                    } else {
                                        ui::petal_active()
                                    },
                                    1.,
                                ),
                            ))
                            .with_animation(
                                SharedString::from(format!("sequence-progress-{done}")),
                                Animation::new(Duration::from_millis(700))
                                    .with_easing(ease_out_quint()),
                                move |el, t| el.w(relative(from + (to - from) * t)),
                            ),
                    ),
            )
            .when_some(self.platform.clone(), |d, platform| {
                d.child(ui::chip(platform, ui::petal()))
            })
    }

    fn render_step(
        &self,
        check: &Check,
        index: usize,
        step: Step,
        last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = step_color(check, step);
        let rail = v_flex()
            .flex_none()
            .w(px(NODE))
            .items_center()
            .child(node(index, step))
            .when(!last, |d| {
                d.child(
                    div()
                        .mt_1()
                        .w(px(2.))
                        .flex_1()
                        .min_h(px(18.))
                        .rounded_full()
                        .bg(if step == Step::Done {
                            ui::success().opacity(0.55)
                        } else {
                            ui::border()
                        }),
                )
            });
        // Signing in can wait: say so instead of "Missing".
        let optional = !blocks(check) && !is_ok(check) && step != Step::Installing;
        let dim = matches!(step, Step::Waiting { next: false }) && !optional;
        let content = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .pt(px(4.))
            .pb(px(if last { 0. } else { 22. }))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if dim { ui::muted() } else { ui::fg() })
                            .child(check.title),
                    )
                    .child(if optional {
                        ui::chip("Optional", ui::info())
                    } else {
                        ui::chip(status_label(&check.status, step == Step::Installing), color)
                    })
                    .child(div().flex_1())
                    .children(self.render_action(check, index, cx)),
            )
            .child(div().text_sm().text_color(ui::muted()).child(check.summary))
            .child(
                div()
                    .text_xs()
                    .font_family(ui::MONO)
                    .text_color(ui::faint())
                    .truncate()
                    .child(status_text(&check.status).to_string()),
            )
            .when(step == Step::Installing, |d| {
                d.child(div().pt_1().child(shimmer(index)))
            })
            .children(self.render_log(check, LOG_LINES).map(|log| log.mt_1()));
        let row = h_flex().items_start().gap_4().child(rail).child(content);
        ui::appear_after(
            ("sequence-step", index),
            self.entrance + STEP_GAP * (index as u32 + 1),
            12.,
            row,
        )
        .into_any_element()
    }

    /// Recommended and optional checks: compact rows under the steps.
    fn render_extras(&self, extras: &[&Check], after: usize, cx: &mut Context<Self>) -> AnyElement {
        let rows = extras.iter().enumerate().map(|(i, check)| {
            let index = after + i;
            let step = self.step_of(check, false);
            let icon = match step {
                Step::Installing => Spinner::new()
                    .xsmall()
                    .color(ui::petal())
                    .into_any_element(),
                Step::Done => Icon::new(IconName::CircleCheck)
                    .xsmall()
                    .text_color(ui::success())
                    .into_any_element(),
                Step::Failed => Icon::new(IconName::CircleX)
                    .xsmall()
                    .text_color(ui::danger())
                    .into_any_element(),
                Step::Waiting { .. } => Icon::new(IconName::Plus)
                    .xsmall()
                    .text_color(ui::faint())
                    .into_any_element(),
            };
            v_flex()
                .gap_1p5()
                .px_3()
                .py_2()
                .rounded_lg()
                .bg(ui::panel())
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            div()
                                .flex_none()
                                .size_4()
                                .flex()
                                .justify_center()
                                .child(icon),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui::fg())
                                .child(check.title),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(check.summary),
                        )
                        .children(self.render_action(check, index, cx)),
                )
                .when(step == Step::Installing, |d| d.child(shimmer(index)))
                .children(self.render_log(check, LOG_LINES))
        });
        ui::appear_after(
            "sequence-extras",
            self.entrance + STEP_GAP * (after as u32 + 1),
            12.,
            v_flex()
                .gap_2()
                .child(div().px_1().child(ui::section_label("Also good to have")))
                .child(v_flex().gap_1().children(rows)),
        )
        .into_any_element()
    }
}

/// The chip's color for `check` at `step`.
fn step_color(check: &Check, step: Step) -> Hsla {
    match step {
        Step::Installing => ui::petal(),
        Step::Waiting { next: false } => ui::faint(),
        _ => match tone(check) {
            Tone::Good => ui::success(),
            Tone::Warn => ui::warning(),
            Tone::Bad => ui::danger(),
        },
    }
}

/// The marker on the rail: the step's number, a spinner with a pulsing halo,
/// a check that pops in, or a cross.
fn node(index: usize, step: Step) -> AnyElement {
    let circle = || {
        div()
            .size(px(NODE))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
    };
    match step {
        Step::Done => ui::pop_in(
            ("sequence-done", index),
            circle().bg(ui::success()).child(
                Icon::new(IconName::Check)
                    .small()
                    .text_color(ui::screen_bg()),
            ),
        )
        .into_any_element(),
        Step::Installing => div()
            .relative()
            .size(px(NODE))
            .child(ui::pulse(
                ("sequence-halo", index),
                div()
                    .absolute()
                    .inset(px(-6.))
                    .rounded_full()
                    .bg(ui::petal().opacity(0.22)),
            ))
            .child(
                circle()
                    .absolute()
                    .inset_0()
                    .bg(ui::screen_bg())
                    .border_2()
                    .border_color(ui::petal())
                    .child(Spinner::new().xsmall().color(ui::petal())),
            )
            .into_any_element(),
        Step::Failed => circle()
            .bg(ui::danger().opacity(0.12))
            .border_1()
            .border_color(ui::danger().opacity(0.6))
            .child(Icon::new(IconName::X).small().text_color(ui::danger()))
            .into_any_element(),
        Step::Waiting { next } => circle()
            .border_1()
            .border_color(if next {
                ui::petal().opacity(0.7)
            } else {
                ui::border_strong()
            })
            .when(next, |d| d.bg(ui::petal().opacity(0.08)))
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if next { ui::petal() } else { ui::faint() })
            .child((index + 1).to_string())
            .into_any_element(),
    }
}

/// An indeterminate bar: a petal glint sliding across while an install runs.
fn shimmer(index: usize) -> impl IntoElement {
    div()
        .relative()
        .h(px(3.))
        .w_full()
        .rounded_full()
        .bg(ui::petal().opacity(0.12))
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(relative(0.35))
                .rounded_full()
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(ui::petal().opacity(0.), 0.),
                    linear_color_stop(ui::petal(), 1.),
                ))
                .with_animation(
                    ("sequence-shimmer", index),
                    Animation::new(Duration::from_millis(1300))
                        .repeat()
                        .with_easing(ease_in_out),
                    |el, t| el.left(relative(-0.35 + t * 1.35)),
                ),
        )
}
