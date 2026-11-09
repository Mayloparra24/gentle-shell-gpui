//! The screen's render: the profile list, the selected profile's scope and
//! route table, the confirmation card and the footer. Each lives in a child
//! module; the route table's column layout is shared here.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::ui;

use super::{Leave, ProfilesView};

mod cells;
mod confirm;
mod footer;
mod profile_list;
mod table;

/// Route table columns. The name column keeps its width; the model, effort
/// and tier cells wrap under it together when the window is too narrow.
const NAME_W: f32 = 168.;
const EFFORT_W: f32 = 118.;
const TIER_W: f32 = 92.;
const CELLS_MIN_W: f32 = 340.;

/// One route row's (or the column header's) two wrapping halves.
fn route_columns() -> Div {
    h_flex().flex_wrap().gap_x_2().gap_y_1()
}

fn route_name_column() -> Div {
    v_flex().w(px(NAME_W)).flex_shrink_0().min_w_0().gap_0p5()
}

fn route_cells() -> Div {
    h_flex().flex_1().min_w(px(CELLS_MIN_W)).gap_2()
}

impl Render for ProfilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.loading {
            h_flex()
                .p_4()
                .gap_2()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::muted()))
                .child("Loading profiles…")
                .into_any_element()
        } else if let Some(error) = &self.error {
            v_flex()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(ui::danger())
                        .child(error.clone()),
                )
                .child(
                    Button::new("profiles-retry")
                        .small()
                        .ghost()
                        .icon(Icon::new(IconName::RefreshCw))
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                )
                .into_any_element()
        } else {
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(self.render_profile_list(cx))
                .child(
                    div()
                        .id("profiles-table")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .p_4()
                        .child(if self.selected.is_some() {
                            self.render_table(cx).into_any_element()
                        } else {
                            v_flex()
                                .items_center()
                                .py_8()
                                .gap_2()
                                .child(Icon::new(IconName::Layers).text_color(ui::faint()))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(ui::muted())
                                        .child("No profiles left. Create one with New."),
                                )
                                .into_any_element()
                        }),
                )
                .into_any_element()
        };
        let ready = !self.loading && self.error.is_none();
        let (backdrop, panel) = ui::screen(None);
        backdrop.occlude().child(ui::pop_in(
            "profiles-screen",
            panel
                .max_w(px(1100.))
                .max_h(px(760.))
                .child(
                    ui::screen_header()
                        .child(
                            Icon::new(IconName::Layers)
                                .small()
                                .flex_shrink_0()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Profiles"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child("Agent-model routing snapshots, like /gentle:profiles."),
                        )
                        .when(self.busy, |d| {
                            d.child(Spinner::new().small().color(ui::muted()))
                        })
                        .child(
                            Button::new("profiles-refresh")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::RefreshCw))
                                .tooltip("Re-read profiles, agents and pins")
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        )
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("profiles-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.leave(Leave::Close, window, cx)
                                })),
                        ),
                )
                .child(div().flex().flex_col().flex_1().min_h_0().child(body))
                .children(self.render_confirm(cx))
                .when(ready, |d| d.child(self.render_footer(cx))),
        ))
    }
}
