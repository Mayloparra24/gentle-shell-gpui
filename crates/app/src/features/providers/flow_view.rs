//! Sign-in flow screen: auth URL, device code, log, prompts and outcome.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::AuthPrompt;

use super::render::{badge, notice};
use super::{Flow, ProvidersView};
use crate::shared::ui;

impl ProvidersView {
    pub(super) fn render_flow(&self, flow: &Flow, cx: &mut Context<Self>) -> impl IntoElement {
        let mut column = v_flex().gap_3().child(
            h_flex()
                .gap_3()
                .child(badge(&flow.provider, flow.finished == Some(Ok(()))))
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().text_xs().text_color(ui::muted()).child("Sign in to"))
                        .child(
                            div()
                                .truncate()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child(flow.provider.clone()),
                        ),
                ),
        );
        if let Some((url, instructions)) = &flow.auth_url {
            let url = url.clone();
            column = column.child(
                ui::card()
                    .gap_3()
                    .p_4()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(Icon::new(IconName::Globe).small().text_color(ui::info()).mt_0p5())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(ui::muted())
                                    .child(instructions.clone().unwrap_or_else(|| {
                                        "Your browser opened the sign-in page. Finish there; if it does not return here, paste the code or redirect URL below.".into()
                                    })),
                            ),
                    )
                    .child(
                        h_flex().child(
                            Button::new("open-url")
                                .small()
                                .ghost()
                                .icon(Icon::new(IconName::ExternalLink))
                                .label("Open the sign-in page again")
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        ),
                    ),
            );
        }
        if let Some((code, uri)) = &flow.device {
            let uri = uri.clone();
            column = column.child(
                ui::card()
                    .gap_3()
                    .p_4()
                    .child(div().text_sm().text_color(ui::muted()).child(
                        "Enter this code on the verification page (copied to the clipboard):",
                    ))
                    .child(
                        ui::well_box().items_center().py_3().child(
                            div()
                                .text_2xl()
                                .font_family(ui::MONO)
                                .text_color(ui::champagne())
                                .child(code.clone()),
                        ),
                    )
                    .child(
                        h_flex().child(
                            Button::new("open-device")
                                .small()
                                .ghost()
                                .icon(Icon::new(IconName::ExternalLink))
                                .label("Open verification page")
                                .on_click(move |_, _, cx| cx.open_url(&uri)),
                        ),
                    ),
            );
        }
        let log: Vec<&String> = flow.log.iter().rev().take(3).rev().collect();
        if !log.is_empty() {
            column = column.child(
                ui::well_box().px_3().py_2().gap_0p5().children(
                    log.into_iter()
                        .map(|line| div().text_color(ui::muted()).child(line.clone())),
                ),
            );
        }
        if let Some((_, prompt)) = &flow.prompt {
            column = column.child(match prompt {
                AuthPrompt::Select { message, options } => ui::card()
                    .p_2()
                    .gap_0p5()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_sm()
                            .text_color(ui::fg())
                            .child(message.clone()),
                    )
                    .children(options.iter().enumerate().map(|(i, option)| {
                        let id = option.id.clone();
                        Button::new(("auth-option", i))
                            .small()
                            .ghost()
                            .w_full()
                            .justify_start()
                            .icon(Icon::new(IconName::ChevronRight))
                            .label(option.label.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.respond(id.clone(), window, cx)
                            }))
                    }))
                    .into_any_element(),
                AuthPrompt::Text {
                    message,
                    placeholder,
                }
                | AuthPrompt::Secret {
                    message,
                    placeholder,
                }
                | AuthPrompt::ManualCode {
                    message,
                    placeholder,
                } => v_flex()
                    .gap_2()
                    .child(div().text_sm().text_color(ui::fg()).child(message.clone()))
                    .when_some(placeholder.clone(), |d, p| {
                        d.child(div().text_xs().text_color(ui::faint()).child(p))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                ui::field()
                                    .flex_1()
                                    .min_w_0()
                                    .when(matches!(prompt, AuthPrompt::Secret { .. }), |d| {
                                        d.child(
                                            Icon::new(IconName::KeyRound)
                                                .small()
                                                .text_color(ui::faint()),
                                        )
                                    })
                                    .child(
                                        Input::new(self.active_input()).appearance(false).small(),
                                    ),
                            )
                            .child(
                                Button::new("auth-submit")
                                    .primary()
                                    .icon(Icon::new(IconName::ArrowRight))
                                    .label("Continue")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_answer(window, cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
            });
        }
        match &flow.finished {
            None if flow.prompt.is_none() => {
                column = column.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(ui::muted())
                        .child(Spinner::new().small().color(ui::accent()))
                        .child("Waiting for the provider…"),
                )
            }
            Some(Ok(())) => column = column.child(notice(
                IconName::CircleCheck,
                ui::success(),
                "Signed in. Tabs without a conversation reload their models now; open a new tab for the others.".into(),
            )),
            Some(Err(error)) => {
                column = column.child(notice(IconName::CircleAlert, ui::danger(), error.clone()))
            }
            None => {}
        }
        column
    }
}
