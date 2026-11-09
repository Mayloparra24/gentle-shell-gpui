//! Subscription tab: one card per signed-in provider, a meter per window.

use gentle_core::usage::subscription::{UsageLimit, format_reset};
use gpui_kit::assets::IconName;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::status_line;
use crate::features::usage::credentials::{CardState, ProviderCard};
use crate::features::usage::{UsageState, UsageView, now_ms};
use crate::shared::ui;

/// Meter fill by how much of the window is spent.
fn bar_color(percent: f64) -> Hsla {
    if percent >= 95.0 {
        ui::danger()
    } else if percent >= 80.0 {
        ui::warning()
    } else if percent >= 50.0 {
        ui::accent()
    } else {
        ui::success()
    }
}

fn updated_ago(fetched_at: i64, now: i64) -> String {
    let minutes = (now - fetched_at) / 60_000;
    if minutes < 1 {
        "updated just now".into()
    } else {
        format!("updated {minutes}m ago")
    }
}

impl UsageView {
    pub(super) fn render_usage(&self) -> AnyElement {
        let (cards, others) = match &self.usage {
            UsageState::Loading => {
                return h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(Spinner::new().small().color(ui::muted()))
                    .child("Reading credentials…")
                    .into_any_element();
            }
            UsageState::Failed(error) => {
                return ui::card()
                    .p_4()
                    .gap_2()
                    .border_color(ui::danger().opacity(0.35))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .small()
                                    .text_color(ui::danger()),
                            )
                            .child(ui::section_label("Could not read credentials")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui::danger())
                            .child(error.clone()),
                    )
                    .into_any_element();
            }
            UsageState::Loaded { cards, others } => (cards, others),
        };
        let now = now_ms();
        v_flex()
            .gap_3()
            .child(ui::section_label("Subscriptions"))
            .when(cards.is_empty(), |d| {
                d.child(
                    ui::card().p_4().child(
                        div()
                            .text_sm()
                            .text_color(ui::muted())
                            .child("No subscription provider is signed in. Sign in to ChatGPT (Codex), Claude or NaN from Providers."),
                    ),
                )
            })
            .children(
                cards
                    .iter()
                    .enumerate()
                    .map(|(i, card)| ui::appear(("usage-card", i), Self::render_card(card, now))),
            )
            .when(!others.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(ui::faint())
                        .child(format!("No subscription usage for: {}", others.join(", "))),
                )
            })
            .into_any_element()
    }

    fn render_card(card: &ProviderCard, now: i64) -> Div {
        let (plan, fetched_at) = match &card.state {
            CardState::Ready(usage) => (usage.plan.clone(), Some(usage.fetched_at)),
            _ => (None, None),
        };
        let status = match &card.state {
            CardState::Loading => Some(ui::chip("fetching", ui::info())),
            CardState::SignIn(_) => Some(ui::chip("signed out", ui::warning())),
            CardState::Failed(_) => Some(ui::chip("error", ui::danger())),
            CardState::Note(_) | CardState::Ready(_) => None,
        };
        let body: AnyElement = match &card.state {
            CardState::Loading => h_flex()
                .gap_2()
                .text_xs()
                .text_color(ui::muted())
                .child(Spinner::new().xsmall().color(ui::muted()))
                .child("Fetching usage…")
                .into_any_element(),
            CardState::Note(note) => {
                status_line(ui::muted(), None, note.clone()).into_any_element()
            }
            CardState::SignIn(note) => {
                status_line(ui::warning(), Some(IconName::LogIn), note.clone()).into_any_element()
            }
            CardState::Failed(error) => {
                status_line(ui::danger(), Some(IconName::CircleAlert), error.clone())
                    .into_any_element()
            }
            CardState::Ready(usage) => v_flex()
                .gap_3()
                .children(
                    usage
                        .limits
                        .iter()
                        .map(|limit| Self::render_limit(limit, now)),
                )
                .into_any_element(),
        };
        ui::card()
            .gap_3()
            .p_4()
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::champagne())
                            .child(card.title),
                    )
                    .when_some(plan, |d, plan| {
                        d.child(ui::chip(plan, ui::accent()).flex_shrink_0())
                    })
                    .children(status)
                    .child(div().flex_1())
                    .when_some(fetched_at, |d, at| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::faint())
                                .child(updated_ago(at, now)),
                        )
                    }),
            )
            .child(body)
    }

    fn render_limit(limit: &UsageLimit, now: i64) -> AnyElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui::muted())
                            .child(limit.name.clone()),
                    )
                    .when(limit.limit_reached, |d| {
                        d.child(ui::chip("limit reached", ui::danger()).flex_shrink_0())
                    }),
            )
            .children(limit.windows.iter().map(|window| {
                let percent = window.used_percent.max(0.0);
                let color = bar_color(percent);
                let label = if window.label.is_empty() {
                    "period".to_string()
                } else {
                    window.label.clone()
                };
                h_flex()
                    .gap_3()
                    .min_w_0()
                    .child(
                        div()
                            .w(px(52.))
                            .flex_shrink_0()
                            .truncate()
                            .text_xs()
                            .text_color(ui::muted())
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(60.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(ui::well())
                            .border_1()
                            .border_color(ui::border())
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((percent / 100.0).min(1.0) as f32))
                                    .rounded_full()
                                    .bg(color),
                            ),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .flex_shrink_0()
                            .text_xs()
                            .font_family(ui::MONO)
                            .text_color(color)
                            .text_right()
                            .child(format!("{}%", percent.round() as i64)),
                    )
                    .child(
                        div()
                            .w(px(128.))
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(ui::faint())
                            .child(format_reset(window.reset_at, now)),
                    )
            }))
            .into_any_element()
    }
}
