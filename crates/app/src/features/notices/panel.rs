//! The notification list inside the bell's popover: header actions (mark all
//! read, clear) and one row per notice, which jumps to its pane.

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::popover::PopoverState;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::ui;
use gentle_core::transcript::NoticeLevel;

use super::{Notices, OnSelect};

/// "now", "42s", "5m", "3h", "2d".
fn ago(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    match s {
        0..10 => "now".into(),
        10..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn level_style(level: NoticeLevel) -> (IconName, Hsla) {
    match level {
        NoticeLevel::Info => (IconName::Info, ui::info()),
        NoticeLevel::Warning => (IconName::TriangleAlert, ui::warning()),
        NoticeLevel::Error => (IconName::CircleX, ui::danger()),
    }
}

pub(super) fn panel(
    store: &Entity<Notices>,
    on_select: OnSelect,
    cx: &mut Context<PopoverState>,
) -> Div {
    let notices = store.read(cx);
    let unread = notices.unread();
    let now = Instant::now();
    let action = |id: &'static str, label: &'static str| {
        div()
            .id(id)
            .px_2()
            .h(px(24.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .text_size(px(11.5))
            .text_color(ui::muted())
            .cursor_pointer()
            .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
            .child(label)
    };
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
                .child("Notifications"),
        )
        .when(unread > 0, |d| {
            d.child(ui::chip(format!("{unread} new"), ui::petal()))
        })
        .child(div().flex_1())
        .when(unread > 0, |d| {
            let store = store.clone();
            d.child(
                action("notices-read", "Mark all read").on_click(cx.listener(
                    move |_, _, _, cx| {
                        store.update(cx, |s, cx| {
                            s.mark_all_read();
                            cx.notify();
                        });
                        cx.notify();
                    },
                )),
            )
        })
        .when(!notices.is_empty(), |d| {
            let store = store.clone();
            d.child(
                action("notices-clear", "Clear").on_click(cx.listener(move |_, _, _, cx| {
                    store.update(cx, |s, cx| {
                        s.clear();
                        cx.notify();
                    });
                    cx.notify();
                })),
            )
        });

    let body = if notices.is_empty() {
        v_flex()
            .py_8()
            .gap_2()
            .items_center()
            .text_color(ui::faint())
            .child(Icon::new(IconName::BellOff).small())
            .child(div().text_size(px(12.)).child("No notifications"))
            .into_any_element()
    } else {
        let rows = notices.iter().map(|notice| {
            let (icon, color) = level_style(notice.level);
            let id = notice.id;
            let origin = notice.origin;
            let store = store.clone();
            let on_select = on_select.clone();
            let mut meta = notice.source.to_string();
            if !meta.is_empty() {
                meta.push_str("  ·  ");
            }
            meta.push_str(&ago(now.saturating_duration_since(notice.at)));
            if notice.repeats > 0 {
                meta.push_str(&format!("  ·  ×{}", notice.repeats + 1));
            }
            h_flex()
                .id(("notice", id as usize))
                .w_full()
                .items_start()
                .gap_2p5()
                .px_2()
                .py_2()
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(ui::surface_hover()))
                .on_click(cx.listener(move |state: &mut PopoverState, _, window, cx| {
                    store.update(cx, |s, cx| {
                        s.mark_read(id);
                        cx.notify();
                    });
                    if let Some(origin) = origin {
                        state.dismiss(window, cx);
                        on_select(origin, window, cx);
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .mt(px(1.))
                        .size(px(22.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .bg(color.opacity(0.12))
                        .child(Icon::new(icon).xsmall().text_color(color)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .line_height(relative(1.4))
                                .text_color(if notice.read { ui::muted() } else { ui::fg() })
                                .child(notice.message.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(ui::faint())
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(meta),
                        ),
                )
                .child(
                    div()
                        .mt(px(7.))
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .when(!notice.read, |d| d.bg(ui::petal())),
                )
        });
        v_flex()
            .id("notices-list")
            .max_h(px(420.))
            .overflow_y_scroll()
            .px_1()
            .pb_1()
            .children(rows)
            .into_any_element()
    };

    v_flex()
        .w_full()
        .font_family(cx.theme().font_family.clone())
        .child(header)
        .child(div().h(px(1.)).bg(ui::border()))
        .child(div().pt_1().child(body))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::ago;

    #[test]
    fn relative_times() {
        assert_eq!(ago(Duration::from_secs(3)), "now");
        assert_eq!(ago(Duration::from_secs(42)), "42s");
        assert_eq!(ago(Duration::from_secs(300)), "5m");
        assert_eq!(ago(Duration::from_secs(7200)), "2h");
        assert_eq!(ago(Duration::from_secs(200_000)), "2d");
    }
}
