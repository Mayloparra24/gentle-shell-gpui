//! The title-bar bell: an icon button with the unread count that opens the
//! notification list in a popover.

use gpui_kit::assets::IconName;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Selectable, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::ui;

use super::panel::panel;
use super::{Notices, OnSelect};

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
