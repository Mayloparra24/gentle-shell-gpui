//! Provider list: grouping, filtering and one row per provider.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::ProviderInfo;

use super::ProvidersView;
use super::render::badge;
use crate::shared::ui;

impl ProvidersView {
    pub(super) fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.filter.read(cx).value().to_lowercase();
        let matching = self.providers.iter().filter(|p| {
            query.is_empty() || p.name.to_lowercase().contains(&query) || p.id.contains(&query)
        });
        // Signed in first, then subscriptions (what most people sign in
        // with), then everything that only takes an API key.
        let mut groups: [(&str, Vec<&ProviderInfo>); 3] = [
            ("Signed in", Vec::new()),
            ("Subscriptions", Vec::new()),
            ("API keys", Vec::new()),
        ];
        for provider in matching {
            let group = if provider.configured {
                0
            } else if provider.oauth.is_some() {
                1
            } else {
                2
            };
            groups[group].1.push(provider);
        }
        groups[1]
            .1
            .sort_by_key(|p| !p.oauth.as_ref().is_some_and(|o| o.subscription));
        if groups.iter().all(|(_, list)| list.is_empty()) {
            return v_flex()
                .py_8()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(ui::muted())
                        .child("No providers match"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(ui::faint())
                        .child("Try another name or id."),
                );
        }
        let mut column = v_flex().gap_4();
        for (title, list) in groups {
            if list.is_empty() {
                continue;
            }
            let count = list.len();
            let mut rows = Vec::with_capacity(count);
            for (i, provider) in list.into_iter().enumerate() {
                rows.push(self.render_row(provider, i, cx).into_any_element());
            }
            column = column.child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .px_1()
                            .pb_1()
                            .gap_2()
                            .child(ui::section_label(title))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(ui::faint())
                                    .child(count.to_string()),
                            ),
                    )
                    .child(ui::card().p_1().gap_0p5().children(rows)),
            );
        }
        column
    }

    fn render_row(
        &self,
        provider: &ProviderInfo,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = provider.clone();
        let p2 = provider.clone();
        let p3 = provider.clone();
        let recommended =
            !provider.configured && provider.oauth.as_ref().is_some_and(|o| o.subscription);
        let status = provider
            .configured
            .then(|| match provider.source.as_deref() {
                Some("environment") => ("env", ui::info()),
                _ => ("signed in", ui::success()),
            });
        let detail = provider
            .oauth
            .as_ref()
            .map(|o| o.label.clone())
            .or_else(|| {
                provider
                    .source
                    .as_deref()
                    .filter(|s| provider.configured && *s != "stored" && *s != "environment")
                    .map(|s| format!("via {s}"))
            });
        let row = h_flex()
            .px_2()
            .py_1p5()
            .gap_3()
            .rounded_lg()
            .hover(|s| s.bg(ui::surface_hover()))
            .child(badge(&provider.name, provider.configured))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(ui::fg())
                                    .child(provider.name.clone()),
                            )
                            .when_some(status, |d, (label, color)| {
                                d.child(ui::chip(label, color).flex_shrink_0())
                            }),
                    )
                    .when_some(detail, |d, detail| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(detail),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .when(provider.oauth.is_some(), |d| {
                        d.child(
                            Button::new(SharedString::from(format!("oauth-{}", provider.id)))
                                .small()
                                .when(recommended, |b| b.primary())
                                .when(!recommended, |b| b.ghost())
                                .icon(Icon::new(IconName::LogIn))
                                .label("Sign in")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin(&p, true, window, cx)
                                })),
                        )
                    })
                    .when(
                        provider.api_key.as_ref().is_some_and(|k| k.interactive),
                        |d| {
                            d.child(
                                Button::new(SharedString::from(format!("key-{}", provider.id)))
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::KeyRound))
                                    .label("API key")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.begin(&p2, false, window, cx)
                                    })),
                            )
                        },
                    )
                    .when(
                        provider.configured && provider.source.as_deref() == Some("stored"),
                        |d| {
                            d.child(
                                Button::new(SharedString::from(format!("logout-{}", provider.id)))
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::LogOut))
                                    .tooltip("Sign out")
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.logout(&p3, cx)),
                                    ),
                            )
                        },
                    ),
            );
        ui::appear(
            SharedString::from(format!("provider-{}-{index}", provider.id)),
            row,
        )
    }
}
