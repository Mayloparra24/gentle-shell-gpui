//! Screen chrome (`Render`) plus the badge and notice shared by the views.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{ProvidersEvent, ProvidersView};
use crate::shared::ui;

/// Rounded initial badge for a provider.
pub(super) fn badge(name: &str, signed_in: bool) -> Div {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let color = if signed_in {
        ui::success()
    } else {
        ui::petal()
    };
    div()
        .flex_shrink_0()
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .bg(color.opacity(0.12))
        .border_1()
        .border_color(color.opacity(0.25))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(initial)
}

/// Inline status message with a leading icon.
pub(super) fn notice(icon: IconName, color: Hsla, text: String) -> Div {
    h_flex()
        .items_start()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(color.opacity(0.08))
        .border_1()
        .border_color(color.opacity(0.22))
        .child(Icon::new(icon).small().text_color(color).mt_0p5())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .text_color(color)
                .child(text),
        )
}

impl Render for ProvidersView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let listing = self.flow.is_none() && !(self.error.is_some() && self.helper.is_none());
        let body: AnyElement = if let Some(error) = &self.error
            && self.helper.is_none()
        {
            notice(IconName::CircleAlert, ui::danger(), error.clone()).into_any_element()
        } else if let Some(flow) = &self.flow {
            self.render_flow(flow, cx).into_any_element()
        } else if self.providers.is_empty() {
            h_flex()
                .py_8()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::accent()))
                .child("Loading providers…")
                .into_any_element()
        } else {
            self.render_list(cx).into_any_element()
        };
        let flow_finished = self.flow.as_ref().map(|f| f.finished.is_some());
        let (backdrop, panel) = ui::screen(Some(640.));
        backdrop.occlude().child(ui::pop_in(
            "providers-screen",
            panel
                .w_full()
                .max_w(px(640.))
                .child(
                    ui::screen_header()
                        .child(
                            Icon::new(IconName::KeyRound)
                                .small()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Providers"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child("Credentials go to Gentle Shell's auth.json, like /login."),
                        )
                        .child(
                            Button::new("providers-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(ProvidersEvent::Close)),
                                ),
                        ),
                )
                .when(listing && !self.providers.is_empty(), |d| {
                    d.child(
                        div().px_4().pt_4().pb_1().child(
                            ui::field()
                                .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
                                .child(Input::new(&self.filter).appearance(false).small()),
                        ),
                    )
                })
                .child(
                    div()
                        .id("providers-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_4()
                        .child(body),
                )
                .when_some(flow_finished, |d, finished| {
                    d.child(
                        h_flex()
                            .px_4()
                            .py_3()
                            .gap_2()
                            .border_t_1()
                            .border_color(ui::border())
                            .child(div().flex_1())
                            .child(
                                Button::new("flow-back")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(if finished {
                                        IconName::ArrowLeft
                                    } else {
                                        IconName::X
                                    }))
                                    .label(if finished { "Back" } else { "Cancel" })
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_flow(cx))),
                            ),
                    )
                }),
        ))
    }
}
