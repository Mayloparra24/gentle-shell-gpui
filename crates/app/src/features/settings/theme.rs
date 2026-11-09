//! The Theme section: the app's theme grid and window transparency.

use gpui_kit::assets::IconName;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::layout::section;
use crate::{shared::themes, shared::ui};

/// Tern-style theme grid: a card per theme drawn in its own colors; clicking
/// applies it to every window and remembers it. A slider above it sets how
/// much of the blurred desktop shows through.
pub(super) fn theme_section(transparency: &Entity<SliderState>, cx: &App) -> Div {
    let state = cx.global::<themes::Themes>();
    let groups = [
        (themes::Source::Gentle, "Gentle Shell"),
        (themes::Source::Pi, "Your Pi themes"),
        (themes::Source::Builtin, "Popular"),
    ];
    let percent = (state.transparency * 100.).round();
    let mut card = section(IconName::Sparkles, "Theme").child(
        h_flex()
            .pt_1()
            .pb_2()
            .gap_3()
            .child(
                div()
                    .w(px(120.))
                    .text_sm()
                    .text_color(ui::muted())
                    .child("Transparency"),
            )
            .child(
                div().flex_1().child(
                    Slider::new(transparency)
                        .horizontal()
                        .accessibility_label("Window transparency"),
                ),
            )
            .child(
                div()
                    .w(px(44.))
                    .text_right()
                    .text_sm()
                    .font_family(ui::MONO)
                    .text_color(ui::fg())
                    .child(format!("{percent:.0}%")),
            ),
    );
    for (source, label) in groups {
        let cards: Vec<_> = state
            .catalog
            .iter()
            .enumerate()
            .filter(|(_, t)| t.source == source)
            .map(|(i, t)| theme_card(i, t, i == state.active))
            .collect();
        if cards.is_empty() {
            continue;
        }
        card = card
            .child(div().pt_2().pb_1p5().child(ui::section_label(label)))
            .child(div().grid().grid_cols(4).gap_2().children(cards));
    }
    card
}

fn theme_card(index: usize, theme: &themes::Theme, selected: bool) -> Stateful<Div> {
    let solid = |hex: u32| -> Hsla { rgb(hex).into() };
    // Red, yellow, green, cyan, blue, magenta, then the accent.
    let swatches = [1usize, 3, 2, 6, 4, 5]
        .into_iter()
        .map(|i| theme.ansi[i])
        .chain([theme.accent]);
    div()
        .id(("theme-card", index))
        .flex()
        .flex_col()
        .gap_1p5()
        .p_1p5()
        .rounded_xl()
        .border_2()
        .border_color(if selected { ui::accent() } else { ui::border() })
        .when(selected, |d| d.bg(ui::accent().opacity(0.08)))
        .cursor_pointer()
        .hover(|d| d.bg(ui::surface_hover()))
        .on_click(move |_, _, cx| themes::select(index, cx))
        .child(
            v_flex()
                .gap_2()
                .px_2p5()
                .py_2()
                .rounded_lg()
                .bg(solid(theme.bg))
                .border_1()
                .border_color(solid(theme.border).opacity(0.7))
                .font_family(ui::MONO)
                .text_xs()
                .child(
                    h_flex()
                        .gap_1()
                        .overflow_hidden()
                        .child(div().text_color(solid(theme.accent)).child("❯"))
                        .child(div().text_color(solid(theme.fg)).child("Aa"))
                        .child(div().text_color(solid(theme.ansi[2])).child("ok"))
                        .child(div().text_color(solid(theme.muted)).child("~/src")),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .children(swatches.map(|c| div().size_2().rounded_full().bg(solid(c)))),
                ),
        )
        .child(
            h_flex()
                .gap_1()
                .px(px(2.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(if selected { ui::fg() } else { ui::muted() })
                        .child(theme.name.clone()),
                )
                .when(selected, |d| {
                    d.child(Icon::new(IconName::Check).xsmall().text_color(ui::accent()))
                }),
        )
}
