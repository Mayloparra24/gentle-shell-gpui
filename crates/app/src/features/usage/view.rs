//! The usage screen: header with the tab toggle, scrolling body, refresh
//! footer, and the pieces both tabs share.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Selectable as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::credentials::CardState;
use super::{StatsState, Tab, UsageEvent, UsageState, UsageView};
use crate::shared::ui;

mod stats;
mod subscription;

/// Segmented toggle frame; children are small ghost buttons, the selected one
/// `.selected(true)`.
fn segmented() -> Div {
    h_flex()
        .flex_shrink_0()
        .p(px(2.))
        .gap(px(2.))
        .rounded_lg()
        .bg(ui::well())
        .border_1()
        .border_color(ui::border())
}

/// A colored note line with an optional leading icon.
fn status_line(color: Hsla, icon: Option<IconName>, text: impl Into<SharedString>) -> Div {
    h_flex()
        .gap_2()
        .min_w_0()
        .text_xs()
        .text_color(color)
        .when_some(icon, |d, icon| {
            d.child(Icon::new(icon).xsmall().flex_shrink_0())
        })
        .child(div().min_w_0().child(text.into()))
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tab {
            Tab::Usage => self.render_usage(),
            Tab::Stats => self.render_stats(cx),
        };
        let busy = match self.tab {
            Tab::Usage => {
                matches!(self.usage, UsageState::Loading)
                    || matches!(&self.usage, UsageState::Loaded { cards, .. } if cards.iter().any(|c| c.state == CardState::Loading))
            }
            Tab::Stats => matches!(self.stats, StatsState::Loading),
        };
        let (subtitle, status) = match self.tab {
            Tab::Usage => (
                "Subscription windows of signed-in providers",
                match &self.usage {
                    UsageState::Loaded { cards, .. } => format!(
                        "{} provider{}",
                        cards.len(),
                        if cards.len() == 1 { "" } else { "s" }
                    ),
                    _ => String::new(),
                },
            ),
            Tab::Stats => (
                "Token and cost totals from local sessions",
                match &self.stats {
                    StatsState::Loaded { data, .. } => format!("{} session files", data.files),
                    _ => String::new(),
                },
            ),
        };
        let tab_button = |id: &'static str, label: &'static str, icon: IconName, tab: Tab| {
            Button::new(id)
                .xsmall()
                .ghost()
                .selected(self.tab == tab)
                .icon(Icon::new(icon))
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
        };
        let (backdrop, panel) = ui::screen(Some(760.));
        backdrop.occlude().child(ui::pop_in(
            "usage-screen",
            panel
                .w_full()
                .max_w(px(760.))
                .child(
                    ui::screen_header()
                        .child(Icon::new(IconName::Gauge).small().text_color(ui::muted()))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Usage"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(subtitle),
                        )
                        .child(
                            segmented()
                                .child(tab_button(
                                    "usage-tab-usage",
                                    "Subscription",
                                    IconName::Zap,
                                    Tab::Usage,
                                ))
                                .child(tab_button(
                                    "usage-tab-stats",
                                    "Local stats",
                                    IconName::ChartPie,
                                    Tab::Stats,
                                )),
                        )
                        .child(
                            ui::key_button("usage-esc", "esc")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(UsageEvent::Close))),
                        )
                        .child(
                            Button::new("usage-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(UsageEvent::Close))),
                        ),
                )
                .child(
                    div()
                        .id("usage-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_4()
                        .child(body),
                )
                .child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap_2()
                        .border_t_1()
                        .border_color(ui::border())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::faint())
                                .child(status),
                        )
                        .child(
                            Button::new("usage-refresh")
                                .primary()
                                .small()
                                .icon(Icon::new(IconName::RefreshCw))
                                .label("Refresh")
                                .loading(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        ),
                ),
        ))
    }
}
