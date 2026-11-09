//! Render: the hero, the grouped checklist rows with their install log, the
//! footer (or the embedded bar in Configuration) and the pixel rose.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::checks::{Tone, all_set, install_plan, is_ok, required_ok, status_text, tone};
use super::{OnboardingEvent, OnboardingView};
use crate::features::setup::{Check, Fix, Need, Status};
use crate::shared::ui;

/// Install output lines a running row shows.
const LOG_LINES: usize = 8;

impl OnboardingView {
    fn render_hero(&self) -> Div {
        let checks = self.checks.as_deref();
        let done = checks.is_some_and(all_set);
        let (title, line) = match checks {
            None => (
                "Welcome to Gentle Shell",
                "Looking at what this machine already has…",
            ),
            Some(checks) if !required_ok(checks) => (
                "Let's get you set up",
                "A few things Gentle Shell needs are missing. Install them here; anything \
                 that needs your password opens in a terminal for you to run.",
            ),
            Some(_) if done => (
                "You're all set",
                "Everything Gentle Shell needs is here. Optional extras are below.",
            ),
            Some(_) if self.first_run => (
                "Welcome to Gentle Shell",
                "Everything required is ready. The recommended extras below make it better.",
            ),
            Some(_) => (
                "Setup & requirements",
                "Everything required is ready. The recommended extras below make it better.",
            ),
        };
        let rose_size = if done { 88. } else { 56. };
        let mark = div().flex().justify_center().child(rose(
            rose_size,
            if done { ui::success() } else { ui::petal() },
        ));
        v_flex()
            .items_center()
            .gap_2()
            .px_8()
            .pt_8()
            .pb_4()
            .child(if done {
                // A new id when the state flips, so the rose blooms again.
                ui::pop_in("onboarding-rose-done", mark).into_any_element()
            } else {
                ui::pop_in("onboarding-rose", mark).into_any_element()
            })
            .child(
                div()
                    .pt_2()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::fg())
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(520.))
                    .text_sm()
                    .text_center()
                    .text_color(ui::muted())
                    .child(line),
            )
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .when_some(self.platform.clone(), |d, platform| {
                        d.child(ui::chip(platform, ui::petal()))
                    })
                    .when(self.checking && self.checks.is_some(), |d| {
                        d.child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(ui::faint())
                                .child(Spinner::new().xsmall().color(ui::faint()))
                                .child("Checking again…"),
                        )
                    }),
            )
    }

    fn render_row(&self, check: &Check, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.installing.is_some();
        let mine = self.installing == Some(check.id);
        let tone = tone(check);
        let color = match tone {
            Tone::Good => ui::success(),
            Tone::Warn => ui::warning(),
            Tone::Bad => ui::danger(),
        };
        let icon = if mine {
            Spinner::new().small().color(ui::petal()).into_any_element()
        } else {
            Icon::new(match tone {
                Tone::Good => IconName::CircleCheck,
                Tone::Warn => IconName::TriangleAlert,
                Tone::Bad => IconName::CircleX,
            })
            .small()
            .text_color(color)
            .into_any_element()
        };
        let label = match (&check.status, mine) {
            (_, true) => "Installing",
            (Status::Ok(_), _) => "Ready",
            (Status::Missing(_), _) => "Missing",
            (Status::Outdated(_), _) => "Outdated",
            (Status::Failed(_), _) => "Failed",
        };
        let id = check.id;
        let action = (!is_ok(check))
            .then(|| match &check.fix {
                Fix::Auto => {
                    let button = Button::new(("onboarding-install", index))
                        .small()
                        .icon(Icon::new(match check.status {
                            Status::Failed(_) => IconName::RefreshCw,
                            _ => IconName::ArrowDown,
                        }))
                        .label(match check.status {
                            Status::Failed(_) => "Retry",
                            Status::Outdated(_) => "Update",
                            _ => "Install",
                        })
                        .loading(mine)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.install(id, cx)));
                    Some(if check.need == Need::Required {
                        button.primary()
                    } else {
                        button.ghost()
                    })
                }
                Fix::Terminal(command) => {
                    let command = command.clone();
                    Some(
                        Button::new(("onboarding-terminal", index))
                            .small()
                            .ghost()
                            .icon(Icon::new(IconName::SquareTerminal))
                            .label("Open terminal")
                            .tooltip(command.clone())
                            .disabled(busy)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(OnboardingEvent::Terminal(command.clone()))
                            })),
                    )
                }
                Fix::SignIn => Some(
                    Button::new(("onboarding-sign-in", index))
                        .small()
                        .primary()
                        .icon(Icon::new(IconName::KeyRound))
                        .label("Sign in")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::SignIn))),
                ),
                Fix::None => None,
            })
            .flatten();
        let failure = self
            .failure
            .as_ref()
            .filter(|(failed, _)| *failed == check.id)
            .map(|(_, message)| message.clone());
        let show_log = self.log_for == Some(check.id) && (mine || failure.is_some());
        let log = show_log.then(|| {
            let start = self.log.len().saturating_sub(LOG_LINES);
            v_flex()
                .ml(px(32.))
                .px_3()
                .py_2()
                .gap_0p5()
                .rounded_lg()
                .bg(ui::well())
                .font_family(ui::MONO)
                .text_xs()
                .when(self.log.is_empty() && mine, |d| {
                    d.child(div().text_color(ui::faint()).child("Starting…"))
                })
                .children(
                    self.log[start..]
                        .iter()
                        .map(|line| div().text_color(ui::muted()).truncate().child(line.clone())),
                )
                .when_some(failure, |d, message| {
                    d.child(div().text_color(ui::danger()).child(message))
                })
        });
        let row = v_flex()
            .gap_2()
            .px_3()
            .py_2p5()
            .rounded(px(ui::ISLAND_ROW))
            .when(tone == Tone::Good, |d| d.bg(ui::success().opacity(0.08)))
            .when(mine, |d| d.bg(ui::petal().opacity(0.06)))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .size_5()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ui::fg())
                                            .child(check.title),
                                    )
                                    .child(ui::chip(label, if mine { ui::petal() } else { color })),
                            )
                            .child(div().text_xs().text_color(ui::muted()).child(check.summary))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(ui::MONO)
                                    .text_color(ui::faint())
                                    .truncate()
                                    .child(status_text(&check.status).to_string()),
                            ),
                    )
                    .children(action),
            )
            .children(log);
        // Keyed by tone too, so a row that turns green settles in again.
        ui::appear(
            ElementId::Name(format!("onboarding-row-{index}-{tone:?}").into()),
            row,
        )
        .into_any_element()
    }

    fn render_checks(&self, checks: &[Check], cx: &mut Context<Self>) -> Div {
        let mut column = v_flex()
            .gap_5()
            .when(!self.embedded, |column| column.px_6().pb_2());
        let mut index = 0;
        for (need, label) in [
            (Need::Required, "Required"),
            (Need::Recommended, "Recommended"),
            (Need::Optional, "Optional"),
        ] {
            let group: Vec<&Check> = checks.iter().filter(|c| c.need == need).collect();
            if group.is_empty() {
                continue;
            }
            let rows: Vec<AnyElement> = group
                .into_iter()
                .map(|check| {
                    index += 1;
                    self.render_row(check, index, cx)
                })
                .collect();
            column = column.child(
                v_flex()
                    .gap_2()
                    .child(div().px_1().child(ui::section_label(label)))
                    .child(
                        ui::island(cx)
                            .p(px(6.))
                            .child(v_flex().gap_1().children(rows)),
                    ),
            );
        }
        column
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.installing.is_some();
        let checks = self.checks.as_deref();
        let ready = checks.is_some_and(required_ok);
        let plan = checks.map(install_plan).unwrap_or_default();
        h_flex()
            .px_6()
            .py_4()
            .gap_2()
            .child(
                Button::new("onboarding-recheck")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::RefreshCw))
                    .label("Check again")
                    .loading(self.checking)
                    .disabled(busy || self.checking)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(div().flex_1())
            .when(!ready, |d| {
                d.child(
                    Button::new("onboarding-skip")
                        .small()
                        .ghost()
                        .label("Skip for now")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::Close))),
                )
            })
            .when(!plan.is_empty() || busy, |d| {
                d.child(
                    Button::new("onboarding-install-all")
                        .small()
                        .primary()
                        .icon(Icon::new(IconName::ArrowDown))
                        .label(if busy {
                            "Installing…"
                        } else {
                            "Install everything"
                        })
                        .loading(busy)
                        .disabled(busy || self.checking)
                        .on_click(cx.listener(|this, _, _, cx| this.install_all(cx))),
                )
            })
            .child({
                let button = Button::new("onboarding-continue")
                    .small()
                    .icon(Icon::new(IconName::ArrowRight))
                    .label("Continue")
                    .disabled(busy || !ready)
                    .on_click(cx.listener(|this, _, _, cx| this.finish(cx)));
                if plan.is_empty() && !busy {
                    button.primary()
                } else {
                    button.ghost()
                }
            })
    }

    /// Configuration's header for the embedded checklist: the platform, then
    /// Check again and Install everything.
    fn render_embedded_bar(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.installing.is_some();
        let plan = self.checks.as_deref().map(install_plan).unwrap_or_default();
        h_flex()
            .pb_3()
            .gap_2()
            .items_center()
            .when_some(self.platform.clone(), |d, platform| {
                d.child(ui::chip(platform, ui::muted()).border_0().rounded_full())
            })
            .child(div().flex_1())
            .child(
                Button::new("setup-recheck")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::RefreshCw))
                    .label("Check again")
                    .loading(self.checking)
                    .disabled(busy || self.checking)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .when(!plan.is_empty() || busy, |d| {
                d.child(
                    Button::new("setup-install-all")
                        .small()
                        .primary()
                        .icon(Icon::new(IconName::ArrowDown))
                        .label(if busy {
                            "Installing…"
                        } else {
                            "Install everything"
                        })
                        .loading(busy)
                        .disabled(busy || self.checking)
                        .on_click(cx.listener(|this, _, _, cx| this.install_all(cx))),
                )
            })
    }
}

/// Gentle Shell's pixel rose, `height` px tall, in `color`.
fn rose(height: f32, color: Hsla) -> impl IntoElement {
    let (dots, (width, rows)) = crate::shared::banner::rose();
    let dot = height / rows.max(1) as f32;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let side = px((dot * 0.55).max(1.0));
            for &(x, y) in dots {
                let origin = point(
                    bounds.origin.x + px(x as f32 * dot),
                    bounds.origin.y + px(y as f32 * dot),
                );
                window.paint_quad(fill(Bounds::new(origin, size(side, side)), color));
            }
        },
    )
    .w(px(width as f32 * dot))
    .h(px(height))
}

impl Render for OnboardingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.checks {
            None => h_flex()
                .py_8()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::petal()))
                .child("Checking Node, Gentle Shell and the rest…")
                .into_any_element(),
            Some(checks) => self.render_checks(checks, cx).into_any_element(),
        };
        let busy = self.busy();
        if self.embedded {
            return v_flex()
                .w_full()
                .child(self.render_embedded_bar(cx))
                .child(body)
                .into_any_element();
        }
        let (backdrop, panel) = ui::screen(Some(720.));
        backdrop
            .occlude()
            .child(ui::pop_in(
                "onboarding-screen",
                panel
                    .w_full()
                    .max_w(px(720.))
                    .relative()
                    .child(
                        div()
                            .id("onboarding-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(self.render_hero())
                            .child(body),
                    )
                    .child(self.render_footer(cx))
                    .child(
                        h_flex()
                            .absolute()
                            .top_3()
                            .right_3()
                            .gap_2()
                            .child(ui::kbd("esc"))
                            .child(
                                Button::new("onboarding-close")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::X))
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::Close)),
                                    ),
                            ),
                    ),
            ))
            .into_any_element()
    }
}
