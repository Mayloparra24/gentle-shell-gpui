//! The window's notification center: extensions' informational `notify`
//! messages (plus a record of warnings and errors, which also toast) collected
//! from every tab behind a bell in the title bar instead of toasts.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui;
use gentle_core::transcript::NoticeLevel;

/// Most notices kept; the oldest drop first.
pub const CAP: usize = 200;

pub struct Notice {
    pub id: u64,
    pub level: NoticeLevel,
    pub message: String,
    /// The tab it came from, as titled when it arrived.
    pub source: SharedString,
    /// The pane it came from, to jump back to it.
    pub origin: Option<usize>,
    /// When it last arrived.
    pub at: Instant,
    pub read: bool,
    /// Further arrivals of the same message folded into this one.
    pub repeats: u32,
}

/// Notices, newest first.
#[derive(Default)]
pub struct Notices {
    items: VecDeque<Notice>,
    next_id: u64,
}

impl Notices {
    /// Record a notice. A message already listed at the same level is moved to
    /// the top and counted instead of listed twice; it keeps its read state so
    /// startup chatter repeated by every new tab does not light the bell again.
    pub fn push(
        &mut self,
        level: NoticeLevel,
        message: String,
        source: SharedString,
        origin: Option<usize>,
        at: Instant,
    ) {
        let notice = match self
            .items
            .iter()
            .position(|n| n.level == level && n.message == message)
            .and_then(|ix| self.items.remove(ix))
        {
            Some(mut old) => {
                old.repeats += 1;
                old.at = at;
                old.source = source;
                old.origin = origin;
                old
            }
            None => {
                self.next_id += 1;
                Notice {
                    id: self.next_id,
                    level,
                    message,
                    source,
                    origin,
                    at,
                    read: false,
                    repeats: 0,
                }
            }
        };
        self.items.push_front(notice);
        self.items.truncate(CAP);
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    pub fn mark_read(&mut self, id: u64) {
        if let Some(notice) = self.items.iter_mut().find(|n| n.id == id) {
            notice.read = true;
        }
    }

    pub fn mark_all_read(&mut self) {
        for notice in &mut self.items {
            notice.read = true;
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &Notice> {
        self.items.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// "now", "42s", "5m", "3h", "2d".
pub fn ago(elapsed: Duration) -> String {
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

/// Jumps to the pane a notice came from.
pub type OnSelect = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The bell: an icon button with an unread count that opens the list.
/// `on_select` jumps to a notice's pane.
pub fn bell(store: &Entity<Notices>, on_select: OnSelect, cx: &App) -> impl IntoElement {
    let store = store.clone();
    let unread = store.read(cx).unread();
    Popover::new("notices")
        .anchor(Anchor::TopRight)
        .trigger(BellTrigger {
            unread,
            open: false,
        })
        .w(px(380.))
        .p_0()
        .content(move |_, _, cx| {
            ui::popover_in(
                ElementId::Name("notices-in".into()),
                panel(&store, on_select.clone(), cx),
            )
            .into_any_element()
        })
}

fn panel(store: &Entity<Notices>, on_select: OnSelect, cx: &mut Context<PopoverState>) -> Div {
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

/// The bell button; a dot badge with the unread count.
#[derive(IntoElement)]
struct BellTrigger {
    unread: usize,
    open: bool,
}

impl Selectable for BellTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for BellTrigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let unread = self.unread;
        div()
            .id("notices-bell")
            .relative()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .text_color(if unread > 0 { ui::fg() } else { ui::muted() })
            .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
            .when(self.open, |d| {
                d.bg(ui::surface_hover()).text_color(ui::fg())
            })
            // The open popover sits right under the bell; a tooltip would cover its header.
            .when(!self.open, |d| {
                d.tooltip(|window, cx| Tooltip::new("Notifications").build(window, cx))
            })
            .child(Icon::new(IconName::Bell).small())
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .right(px(1.))
                        .h(px(13.))
                        .min_w(px(13.))
                        .px(px(3.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(ui::petal())
                        .text_color(ui::well().opacity(1.))
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .line_height(relative(1.))
                        .child(if unread > 99 {
                            "99+".to_string()
                        } else {
                            unread.to_string()
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::notices::{CAP, Notices, ago};
    use gentle_core::transcript::NoticeLevel;

    fn push(store: &mut Notices, message: &str, at: Instant) {
        store.push(
            NoticeLevel::Info,
            message.to_string(),
            "tab".into(),
            Some(1),
            at,
        );
    }

    #[test]
    fn dedupes_repeats_to_the_top() {
        let mut store = Notices::default();
        let t0 = Instant::now();
        push(&mut store, "a", t0);
        push(&mut store, "b", t0);
        push(&mut store, "a", t0 + Duration::from_secs(5));
        assert_eq!(store.iter().count(), 2);
        let first = store.iter().next().unwrap();
        assert_eq!(first.message, "a");
        assert_eq!(first.repeats, 1);
        assert_eq!(first.at, t0 + Duration::from_secs(5));
        // Same text at another level is a different notice.
        store.push(NoticeLevel::Error, "a".into(), "tab".into(), None, t0);
        assert_eq!(store.iter().count(), 3);
    }

    #[test]
    fn counts_unread() {
        let mut store = Notices::default();
        let t0 = Instant::now();
        push(&mut store, "a", t0);
        push(&mut store, "b", t0);
        assert_eq!(store.unread(), 2);
        let id = store.iter().next().unwrap().id;
        store.mark_read(id);
        assert_eq!(store.unread(), 1);
        store.mark_all_read();
        assert_eq!(store.unread(), 0);
        // A repeat of a read notice stays read.
        push(&mut store, "a", t0);
        assert_eq!(store.unread(), 0);
        push(&mut store, "c", t0);
        assert_eq!(store.unread(), 1);
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.unread(), 0);
    }

    #[test]
    fn caps_at_200_dropping_oldest() {
        let mut store = Notices::default();
        let t0 = Instant::now();
        for i in 0..CAP + 25 {
            push(&mut store, &format!("n{i}"), t0);
        }
        assert_eq!(CAP, 200);
        assert_eq!(store.iter().count(), CAP);
        assert_eq!(
            store.iter().next().unwrap().message,
            format!("n{}", CAP + 24)
        );
        assert_eq!(store.iter().last().unwrap().message, "n25");
        assert_eq!(store.unread(), CAP);
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(Duration::from_secs(3)), "now");
        assert_eq!(ago(Duration::from_secs(42)), "42s");
        assert_eq!(ago(Duration::from_secs(300)), "5m");
        assert_eq!(ago(Duration::from_secs(7200)), "2h");
        assert_eq!(ago(Duration::from_secs(200_000)), "2d");
    }
}
