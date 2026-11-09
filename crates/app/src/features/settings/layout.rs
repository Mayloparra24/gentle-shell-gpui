//! The Configuration screen's building blocks: a titled card and its rows.

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable as _, h_flex};
use gpui_kit::*;

use crate::shared::ui;

pub(super) fn row(label: &'static str, value: impl Into<SharedString>) -> impl IntoElement {
    h_flex()
        .gap_3()
        .py_1()
        .items_start()
        .border_b_1()
        .border_color(ui::border().opacity(0.5))
        .child(
            div()
                .w(relative(0.32))
                .max_w(px(180.))
                .flex_shrink_0()
                .truncate()
                .text_xs()
                .text_color(ui::muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .font_family(ui::MONO)
                .text_color(ui::fg())
                .child(value.into()),
        )
}

pub(super) fn section(icon: IconName, title: &'static str) -> Div {
    ui::card().gap_0p5().p_4().child(
        h_flex()
            .pb_2()
            .gap_2()
            .child(Icon::new(icon).xsmall().text_color(ui::champagne()))
            .child(ui::section_label(title)),
    )
}

pub(super) fn or_dash(value: &Option<String>, fallback: &str) -> String {
    value.clone().unwrap_or_else(|| fallback.to_string())
}
