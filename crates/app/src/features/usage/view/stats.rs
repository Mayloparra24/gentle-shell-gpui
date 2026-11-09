//! Local stats tab: range toggle, totals, 30-day chart, model and project tables.

use std::path::Path;

use gentle_core::transcript::format_tokens;
use gentle_core::usage::stats::{DayStats, StatsRange, StatsSummary, format_cost};
use gentle_core::usage::time::day_label;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{segmented, status_line};
use crate::features::usage::{Metric, StatsState, UsageView};
use crate::shared::ui;

impl UsageView {
    pub(super) fn render_stats(&self, cx: &mut Context<Self>) -> AnyElement {
        let (data, summary) = match &self.stats {
            StatsState::Idle | StatsState::Loading => {
                return h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(Spinner::new().small().color(ui::muted()))
                    .child("Reading session files…")
                    .into_any_element();
            }
            StatsState::Loaded { data, summary, .. } => (data, summary),
        };
        let range_button = |id: &'static str, label: &'static str, range: StatsRange| {
            Button::new(id)
                .xsmall()
                .ghost()
                .selected(self.range == range)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_range(range, cx)))
        };
        let mut notes = Vec::new();
        if !data.skipped_large.is_empty() {
            notes.push(format!(
                "Skipped {} session file{} larger than 64 MiB.",
                data.skipped_large.len(),
                if data.skipped_large.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        if data.unreadable > 0 {
            notes.push(format!(
                "{} session file(s) could not be read.",
                data.unreadable
            ));
        }
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::section_label("Range"))
                    .child(div().flex_1())
                    .child(
                        segmented()
                            .child(range_button("usage-range-all", "All time", StatsRange::All))
                            .child(range_button(
                                "usage-range-30d",
                                "30 days",
                                StatsRange::Month,
                            ))
                            .child(range_button("usage-range-7d", "7 days", StatsRange::Week)),
                    ),
            )
            .child(Self::render_totals(summary))
            .child(self.render_chart(summary, cx))
            .child(Self::render_models(summary))
            .child(Self::render_projects(summary))
            .children(
                notes
                    .into_iter()
                    .map(|note| status_line(ui::warning(), Some(IconName::CircleAlert), note)),
            )
            .into_any_element()
    }

    fn render_totals(summary: &StatsSummary) -> AnyElement {
        let tile = |i: usize, label: &'static str, value: String, color: Hsla| {
            ui::appear(
                ("usage-tile", i),
                ui::card()
                    .flex_1()
                    .min_w(px(104.))
                    .gap_1()
                    .px_3()
                    .py_2p5()
                    .child(ui::section_label(label))
                    .child(
                        div()
                            .truncate()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .font_family(ui::MONO)
                            .text_color(color)
                            .child(value),
                    ),
            )
        };
        let totals = &summary.totals;
        let detail = |label: &'static str, value: u64| {
            h_flex()
                .gap_1p5()
                .text_xs()
                .child(div().text_color(ui::faint()).child(label))
                .child(
                    div()
                        .text_color(ui::muted())
                        .font_family(ui::MONO)
                        .child(format_tokens(value)),
                )
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(tile(0, "Tokens", format_tokens(totals.total), ui::fg()))
                    .child(tile(1, "Cost", format_cost(totals.cost), ui::champagne()))
                    .child(tile(2, "Sessions", summary.sessions.to_string(), ui::fg()))
                    .child(tile(3, "Messages", summary.messages.to_string(), ui::fg()))
                    .child(tile(
                        4,
                        "Active days",
                        summary.active_days.to_string(),
                        ui::fg(),
                    )),
            )
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .px_1()
                    .child(detail("input", totals.input))
                    .child(detail("output", totals.output))
                    .child(detail("cache read", totals.cache_read))
                    .child(detail("cache write", totals.cache_write)),
            )
            .into_any_element()
    }

    fn render_chart(&self, summary: &StatsSummary, cx: &mut Context<Self>) -> AnyElement {
        let metric = self.metric;
        let value = |day: &DayStats| match metric {
            Metric::Tokens => day.tokens as f64,
            Metric::Cost => day.cost,
        };
        let max = summary.daily.iter().map(value).fold(0.0, f64::max);
        let metric_button = |id: &'static str, label: &'static str, m: Metric| {
            Button::new(id)
                .xsmall()
                .ghost()
                .selected(metric == m)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.metric = m;
                    cx.notify();
                }))
        };
        let first = summary
            .daily
            .first()
            .map(|d| day_label(d.day))
            .unwrap_or_default();
        let last = summary
            .daily
            .last()
            .map(|d| day_label(d.day))
            .unwrap_or_default();
        let bar_color = match metric {
            Metric::Tokens => ui::accent(),
            Metric::Cost => ui::champagne(),
        };
        ui::card()
            .gap_3()
            .p_4()
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::section_label("Last 30 days"))
                    .child(div().flex_1())
                    .child(
                        segmented()
                            .child(metric_button(
                                "usage-metric-tokens",
                                "Tokens",
                                Metric::Tokens,
                            ))
                            .child(metric_button("usage-metric-cost", "Cost", Metric::Cost)),
                    ),
            )
            .child(
                h_flex()
                    .h(px(110.))
                    .items_end()
                    .gap(px(3.))
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(ui::border())
                    .bg(ui::well())
                    .children(summary.daily.iter().map(|day| {
                        let v = value(day);
                        let fraction = if max > 0.0 { (v / max) as f32 } else { 0.0 };
                        let tip = format!(
                            "{} · {} tokens · {}",
                            day_label(day.day),
                            format_tokens(day.tokens),
                            format_cost(day.cost)
                        );
                        div()
                            .id(SharedString::from(format!("usage-day-{}", day.day)))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .rounded_sm()
                            .hover(|s| s.bg(ui::surface_hover()))
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                            .child(
                                div()
                                    .w_full()
                                    .h(relative(fraction))
                                    .when(v > 0.0, |d| d.min_h(px(2.)))
                                    .rounded_sm()
                                    .bg(bar_color),
                            )
                    })),
            )
            .child(
                h_flex()
                    .text_xs()
                    .text_color(ui::faint())
                    .child(first)
                    .child(div().flex_1())
                    .child(div().font_family(ui::MONO).child(format!(
                        "peak {}",
                        match metric {
                            Metric::Tokens => format_tokens(max as u64),
                            Metric::Cost => format_cost(max),
                        }
                    )))
                    .child(div().flex_1())
                    .child(last),
            )
            .into_any_element()
    }

    fn table_header(columns: &[(&'static str, f32)]) -> Div {
        h_flex()
            .px_2()
            .pb_1p5()
            .gap_2()
            .border_b_1()
            .border_color(ui::border())
            .text_xs()
            .text_color(ui::faint())
            .child(div().flex_1().min_w_0().truncate().child(columns[0].0))
            .children(columns[1..].iter().map(|(label, width)| {
                div()
                    .w(px(*width))
                    .flex_shrink_0()
                    .text_right()
                    .child(*label)
            }))
    }

    fn numeric(width: f32, text: String) -> Div {
        div()
            .w(px(width))
            .flex_shrink_0()
            .text_right()
            .text_xs()
            .font_family(ui::MONO)
            .text_color(ui::muted())
            .child(text)
    }

    /// A table card: section heading, column header, rows.
    fn table(title: &'static str, columns: &[(&'static str, f32)], empty: bool) -> Div {
        ui::card()
            .gap_2()
            .p_3()
            .child(div().px_2().pt_1().child(ui::section_label(title)))
            .child(Self::table_header(columns))
            .when(empty, |d| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(ui::muted())
                        .child("No usage in this range."),
                )
            })
    }

    fn table_row() -> Div {
        h_flex()
            .px_2()
            .py_1p5()
            .gap_2()
            .min_w_0()
            .rounded_lg()
            .text_sm()
            .hover(|s| s.bg(ui::surface_hover()))
    }

    fn render_models(summary: &StatsSummary) -> AnyElement {
        Self::table(
            "Models",
            &[
                ("Model", 0.),
                ("Messages", 64.),
                ("Tokens", 64.),
                ("Cost", 72.),
                ("Share", 44.),
            ],
            summary.models.is_empty(),
        )
        .child(
            v_flex()
                .gap_0p5()
                .children(summary.models.iter().enumerate().map(|(i, model)| {
                    ui::appear(
                        ("usage-model", i),
                        Self::table_row()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_color(ui::fg())
                                            .truncate()
                                            .child(model.model.clone()),
                                    )
                                    .child(
                                        ui::chip(model.provider.clone(), ui::info())
                                            .flex_shrink_0(),
                                    ),
                            )
                            .child(Self::numeric(64., model.messages.to_string()))
                            .child(Self::numeric(64., format_tokens(model.tokens)))
                            .child(Self::numeric(72., format_cost(model.cost)))
                            .child(Self::numeric(
                                44.,
                                format!("{}%", (model.share * 100.0).round() as i64),
                            )),
                    )
                })),
        )
        .into_any_element()
    }

    fn render_projects(summary: &StatsSummary) -> AnyElement {
        Self::table(
            "Projects",
            &[
                ("Project", 0.),
                ("Sessions", 60.),
                ("Messages", 64.),
                ("Tokens", 64.),
                ("Cost", 72.),
            ],
            summary.projects.is_empty(),
        )
        .child(
            v_flex()
                .gap_0p5()
                .children(summary.projects.iter().enumerate().map(|(i, project)| {
                    let path = Path::new(&project.project);
                    let (prefix, leaf) = if path.is_absolute() {
                        ui::split_path(path)
                    } else {
                        (String::new(), project.project.clone())
                    };
                    ui::appear(
                        ("usage-project", i),
                        Self::table_row()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(ui::faint())
                                            .child(prefix),
                                    )
                                    .child(
                                        div().min_w_0().truncate().text_color(ui::fg()).child(leaf),
                                    ),
                            )
                            .child(Self::numeric(60., project.sessions.to_string()))
                            .child(Self::numeric(64., project.messages.to_string()))
                            .child(Self::numeric(64., format_tokens(project.tokens)))
                            .child(Self::numeric(72., format_cost(project.cost))),
                    )
                })),
        )
        .into_any_element()
    }
}
