//! What each page says and shows. Illustrations are drawn with the app's own
//! pieces (cards, chips, key caps, the pixel rose), so they follow the theme.

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::features::session::mode_style;
use crate::shared::banner::pixel_rose;
use crate::shared::ui;
use gentle_core::gentle_state::PermissionMode;

/// ⌘ on macOS, Ctrl elsewhere, for key caps.
fn cmd() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    }
}

/// A page's headline and the line under it.
pub(super) fn heading(title: impl IntoElement, line: &'static str) -> Div {
    v_flex().items_center().gap_3().child(title).child(
        div()
            .max_w(px(540.))
            .text_base()
            .text_center()
            .line_height(px(24.))
            .text_color(ui::muted())
            .child(line),
    )
}

/// A plain headline.
pub(super) fn title(text: &'static str) -> Div {
    div()
        .text_size(px(40.))
        .line_height(px(48.))
        .font_weight(FontWeight::BOLD)
        .text_color(ui::fg())
        .child(text)
}

/// "Hi, I'm Gentle Shell." with the name in petal, underlined by a wave.
pub(super) fn hello_title() -> Div {
    h_flex()
        .text_size(px(40.))
        .line_height(px(48.))
        .font_weight(FontWeight::BOLD)
        .text_color(ui::fg())
        .child("Hi, I'm\u{a0}")
        .child(
            div()
                .relative()
                .text_color(ui::petal())
                .child("Gentle Shell")
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(-8.))
                        .h(px(8.))
                        .child(wave(ui::petal())),
                ),
        )
        .child(".")
}

/// A hand-drawn-looking wave across the element's width.
fn wave(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let mid = bounds.origin.y + bounds.size.height / 2.;
            let amplitude = px(2.5);
            let half = px(5.);
            let mut path = PathBuilder::stroke(px(2.));
            let mut x = bounds.origin.x;
            path.move_to(point(x, mid));
            let mut up = true;
            while x + half <= bounds.origin.x + bounds.size.width {
                let ctrl_y = if up {
                    mid - amplitude * 2.
                } else {
                    mid + amplitude * 2.
                };
                path.curve_to(point(x + half, mid), point(x + half / 2., ctrl_y));
                x += half;
                up = !up;
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size_full()
}

/// A colored square with an icon and a caption under it.
pub(super) fn tile(icon: IconName, color: Hsla, label: &'static str) -> Div {
    v_flex()
        .w(px(156.))
        .items_center()
        .gap_2p5()
        .child(
            div()
                .size(px(44.))
                .rounded_xl()
                .flex()
                .items_center()
                .justify_center()
                .bg(color)
                .shadow_md()
                .child(Icon::new(icon).text_color(ui::screen_bg())),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui::fg().opacity(0.85))
                .child(label),
        )
}

pub(super) fn tiles(items: [(IconName, Hsla, &'static str); 3]) -> Div {
    h_flex()
        .pt_3()
        .gap_4()
        .justify_center()
        .children(items.map(|(icon, color, label)| tile(icon, color, label)))
}

/// Page 1: the rose, big, on a soft halo.
pub(super) fn rose_mark() -> Div {
    div()
        .relative()
        .size(px(196.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .bg(ui::petal().opacity(0.07)),
        )
        .child(
            div()
                .absolute()
                .inset(px(34.))
                .rounded_full()
                .bg(ui::petal().opacity(0.06)),
        )
        .child(pixel_rose(132., ui::petal()))
}

/// A muted bar standing for a line of text.
fn line(width: f32, color: Hsla) -> Div {
    div().h(px(6.)).w(px(width)).rounded_full().bg(color)
}

fn pane() -> Div {
    v_flex()
        .p_2p5()
        .gap_1p5()
        .rounded_lg()
        .bg(ui::panel())
        .border_1()
        .border_color(ui::border())
}

/// Page 2: a window with tabs, an agent, a split and a terminal.
pub(super) fn window_mock() -> Div {
    let tab = |name: &'static str, active: bool| {
        h_flex()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded_md()
            .text_xs()
            .when(active, |d| d.bg(ui::surface()))
            .text_color(if active { ui::fg() } else { ui::faint() })
            .child(div().size(px(6.)).rounded_full().bg(if active {
                ui::petal()
            } else {
                ui::faint()
            }))
            .child(name)
    };
    let agent = pane()
        .flex_1()
        .child(
            h_flex().justify_end().child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(ui::petal().opacity(0.16))
                    .child(line(70., ui::petal().opacity(0.6))),
            ),
        )
        .child(line(150., ui::muted().opacity(0.35)))
        .child(line(120., ui::muted().opacity(0.35)))
        .child(line(136., ui::muted().opacity(0.35)))
        .child(
            h_flex()
                .mt_1()
                .gap_1p5()
                .child(ui::chip("bash", ui::info()))
                .child(line(64., ui::muted().opacity(0.25))),
        );
    let helper = pane()
        .child(
            h_flex()
                .gap_1p5()
                .child(
                    Icon::new(IconName::Bot)
                        .xsmall()
                        .text_color(ui::champagne()),
                )
                .child(line(60., ui::muted().opacity(0.35))),
        )
        .child(line(90., ui::muted().opacity(0.25)));
    let terminal = pane()
        .font_family(ui::MONO)
        .text_size(px(10.5))
        .child(div().text_color(ui::muted()).child("$ cargo test"))
        .child(div().text_color(ui::success()).child("ok. 245 passed"));
    v_flex()
        .w(px(440.))
        .p_2()
        .gap_2()
        .rounded_xl()
        .bg(ui::popover_bg())
        .border_1()
        .border_color(ui::border_strong())
        .shadow_2xl()
        .child(
            h_flex()
                .gap_1()
                .child(tab("api", true))
                .child(tab("web", false))
                .child(div().px_1().text_xs().text_color(ui::faint()).child("+")),
        )
        .child(
            h_flex()
                .gap_2()
                .items_stretch()
                .child(agent)
                .child(v_flex().w(px(150.)).gap_2().child(helper).child(terminal)),
        )
}

/// Key caps with what they do.
pub(super) fn keys(items: &[(&str, &'static str)]) -> Div {
    h_flex()
        .pt_3()
        .gap_5()
        .justify_center()
        .children(items.iter().map(|(key, what)| {
            h_flex()
                .gap_2()
                .child(ui::kbd(key.to_string()))
                .child(div().text_sm().text_color(ui::muted()).child(*what))
        }))
}

pub(super) fn window_keys() -> Div {
    let cmd = cmd();
    keys(&[
        (&format!("{cmd}T"), "Agent tab"),
        (&format!("{cmd}D"), "Split"),
        (&format!("{cmd}K"), "Everything else"),
    ])
}

/// Page 3: profiles routing each role to a model.
pub(super) fn profiles_mock() -> Div {
    let profile = |name: &'static str, active: bool| {
        h_flex()
            .gap_1()
            .px_2p5()
            .py_1()
            .rounded_md()
            .text_xs()
            .text_color(if active { ui::petal() } else { ui::muted() })
            .when(active, |d| {
                d.bg(ui::petal().opacity(0.12))
                    .border_1()
                    .border_color(ui::petal().opacity(0.25))
            })
            .child(name)
            .when(active, |d| d.child("★"))
    };
    let route = |role: &'static str, model: &'static str, color: Hsla| {
        h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .rounded_lg()
            .bg(ui::panel())
            .child(Icon::new(IconName::Bot).xsmall().text_color(ui::faint()))
            .child(div().flex_1().text_sm().text_color(ui::fg()).child(role))
            .child(ui::chip(model, color))
    };
    v_flex()
        .w(px(400.))
        .p_3()
        .gap_2()
        .rounded_xl()
        .bg(ui::popover_bg())
        .border_1()
        .border_color(ui::border_strong())
        .shadow_2xl()
        .child(
            h_flex()
                .pb_1()
                .gap_1()
                .child(profile("claude", true))
                .child(profile("gpt", false))
                .child(profile("fast", false)),
        )
        .child(route("orchestrator", "claude-opus", ui::petal()))
        .child(route("explore", "deepseek-flash", ui::info()))
        .child(route("review", "gpt-5", ui::champagne()))
        .child(route("verify", "inherit", ui::faint()))
}

/// Page 4: Gentle Shell's permission modes, as the session's picker shows
/// them.
pub(super) fn control_mock() -> Div {
    let mode = |mode: PermissionMode, on: bool| {
        let (icon, color) = mode_style(Some(mode));
        let color = if mode == PermissionMode::Normal {
            ui::petal()
        } else {
            color
        };
        v_flex()
            .w(px(168.))
            .p_3()
            .gap_1p5()
            .rounded_xl()
            .bg(if on { color.opacity(0.10) } else { ui::panel() })
            .border_1()
            .border_color(if on {
                color.opacity(0.35)
            } else {
                ui::border()
            })
            .child(
                h_flex()
                    .gap_1p5()
                    .child(Icon::new(icon).small().text_color(color))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::fg())
                            .child(mode.label()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .line_height(px(16.))
                    .text_color(ui::muted())
                    .child(mode.description()),
            )
    };
    h_flex()
        .gap_3()
        .items_stretch()
        .child(mode(PermissionMode::Normal, true))
        .child(mode(PermissionMode::Auto, false))
        .child(mode(PermissionMode::Yolo, false))
        .child(mode(PermissionMode::SuperYolo, false))
}

pub(super) fn control_keys() -> Div {
    keys(&[
        ("↵", "Steer mid-turn"),
        (&format!("{}.", cmd()), "Stop the turn"),
        ("esc", "Close panels"),
    ])
}
