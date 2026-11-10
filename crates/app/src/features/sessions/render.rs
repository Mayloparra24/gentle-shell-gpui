//! Rendering: the sessions screen (toolbar, scope toggle, footer) and the
//! capped, filtered list of session rows.

use std::time::{SystemTime, UNIX_EPOCH};

use gentle_core::sessions::{SessionSummary, relative_time};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::ui;

use super::{Scope, SessionsEvent, SessionsView};

/// Most rows rendered at once; the search box narrows the rest.
const MAX_ROWS: usize = 200;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl SessionsView {
    fn render_row(
        &self,
        ix: usize,
        session: &SessionSummary,
        now: i64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.is_current(session);
        let other_project = !session.cwd.as_os_str().is_empty() && session.cwd != self.cwd;
        let mut subtitle = Vec::new();
        if other_project || self.scope == Scope::All {
            let (prefix, leaf) = ui::split_path(&session.cwd);
            subtitle.push(format!("{prefix}{leaf}"));
        }
        if session.name.is_some()
            && let Some(first) = &session.first_message
        {
            subtitle.push(gentle_core::text::first_line(first.trim(), 90));
        }
        let mut meta = Vec::new();
        if let Some(model) = &session.model {
            meta.push(model.clone());
        }
        meta.push(format!(
            "{}{} msg{}",
            session.message_count,
            if session.truncated { "+" } else { "" },
            if session.message_count == 1 { "" } else { "s" }
        ));
        meta.push(relative_time(now, session.modified_ms));
        let target = session.clone();
        let row = h_flex()
            .id(("session-row", ix))
            .relative()
            .px_3()
            .py_2()
            .gap_3()
            .rounded_lg()
            .cursor_pointer()
            .when(current, |d| d.bg(ui::raised()))
            .when(!current, |d| d.hover(|d| d.bg(ui::surface_hover())))
            .on_click(cx.listener(move |this, _, _, cx| this.open(&target, cx)))
            .when(current, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(10.))
                        .bottom(px(10.))
                        .w(px(2.))
                        .rounded_full()
                        .bg(ui::accent()),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(ui::well())
                    .border_1()
                    .border_color(ui::border_strong())
                    .child(
                        Icon::new(if current {
                            IconName::Check
                        } else {
                            IconName::FileText
                        })
                        .small()
                        .text_color(if current {
                            ui::accent()
                        } else {
                            ui::muted()
                        }),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(
                                        if session.name.is_some() || session.first_message.is_some()
                                        {
                                            ui::fg()
                                        } else {
                                            ui::faint()
                                        },
                                    )
                                    .child(session.title()),
                            )
                            .when(current, |d| {
                                d.child(ui::chip("current", ui::accent()).flex_shrink_0())
                            })
                            .when(other_project && self.scope == Scope::All, |d| {
                                d.child(ui::chip("new tab", ui::warning()).flex_shrink_0())
                            }),
                    )
                    .when(!subtitle.is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(subtitle.join(" · ")),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(240.))
                    .truncate()
                    .text_xs()
                    .text_color(ui::faint())
                    .child(meta.join(" · ")),
            );
        ui::appear(("session-row-in", ix), div().child(row))
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let empty = |text: &'static str| {
            v_flex()
                .py_8()
                .items_center()
                .gap_2()
                .child(Icon::new(IconName::Inbox).text_color(ui::faint()))
                .child(div().text_sm().text_color(ui::muted()).child(text))
                .into_any_element()
        };
        if self.sessions.is_empty() {
            if self.loading {
                return h_flex()
                    .py_8()
                    .justify_center()
                    .gap_2()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(Spinner::new().small().color(ui::accent()))
                    .child("Reading sessions…")
                    .into_any_element();
            }
            return empty(match self.scope {
                Scope::Project => "No sessions for this project yet.",
                Scope::All => "No sessions found.",
            });
        }
        let visible = self.visible(cx);
        if visible.is_empty() {
            return empty("No sessions match.");
        }
        let hidden = visible.len().saturating_sub(MAX_ROWS);
        let now = now_ms();
        let rows: Vec<AnyElement> = visible
            .into_iter()
            .take(MAX_ROWS)
            .enumerate()
            .map(|(ix, s)| self.render_row(ix, s, now, cx).into_any_element())
            .collect();
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .child(ui::section_label(match self.scope {
                        Scope::Project => "This project",
                        Scope::All => "All projects",
                    })),
            )
            .children(rows)
            .when(hidden > 0, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(ui::faint())
                        .child(format!("{hidden} more — refine the search")),
                )
            })
            .into_any_element()
    }
}

impl Render for SessionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all = self.scope == Scope::All;
        let count = self.sessions.len();
        let toolbar = h_flex()
            .gap_2()
            .child(
                ui::field()
                    .flex_1()
                    .min_w_0()
                    .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
                    .child(Input::new(&self.filter).appearance(false).small()),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded_lg()
                    .bg(ui::well())
                    .border_1()
                    .border_color(ui::border())
                    .children(
                        [
                            (Scope::Project, IconName::Folder, "This project"),
                            (Scope::All, IconName::Globe, "All"),
                        ]
                        .map(|(scope, icon, label)| {
                            let active = self.scope == scope;
                            h_flex()
                                .id(SharedString::from(format!("sessions-scope-{label}")))
                                .gap_1p5()
                                .px_2p5()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_xs()
                                .whitespace_nowrap()
                                .when(active, |d| d.bg(ui::raised()).text_color(ui::fg()))
                                .when(!active, |d| {
                                    d.text_color(ui::muted()).hover(|d| d.text_color(ui::fg()))
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.set_scope(scope, cx)),
                                )
                                .child(Icon::new(icon).xsmall())
                                .child(label)
                        }),
                    ),
            )
            .child(
                Button::new("sessions-refresh")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::RefreshCw))
                    .tooltip("Reload")
                    .loading(self.loading && count > 0)
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            );
        let (prefix, leaf) = ui::split_path(&self.cwd);
        let hint = |key: Stateful<Div>, label: &'static str| {
            h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(ui::muted())
                .child(key)
                .child(label)
        };
        let (backdrop, panel) = ui::screen(Some(760.));
        backdrop.occlude().child(ui::pop_in(
            "sessions-screen",
            panel
                .w_full()
                .max_w(px(760.))
                .child(
                    ui::screen_header()
                        .child(
                            Icon::new(IconName::FolderOpen)
                                .small()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Sessions"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(if all {
                                    "Every project".to_string()
                                } else {
                                    format!("{prefix}{leaf}")
                                }),
                        )
                        .when(count > 0, |d| {
                            d.child(ui::chip(format!("{count}"), ui::muted()).flex_shrink_0())
                        })
                        .child(
                            Button::new("sessions-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionsEvent::Close))),
                        ),
                )
                .child(div().px_4().pt_4().pb_3().child(toolbar))
                .child(
                    div()
                        .id("sessions-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_2()
                        .pb_3()
                        .child(self.render_list(cx)),
                )
                .child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap_4()
                        .border_t_1()
                        .border_color(ui::border())
                        .child(hint(
                            ui::key_button("sessions-open-first", "↵")
                                .on_click(cx.listener(|this, _, _, cx| this.open_first(cx))),
                            "open first match",
                        ))
                        .child(hint(
                            ui::key_button("sessions-close-hint", "esc")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionsEvent::Close))),
                            "close",
                        ))
                        .child(div().flex_1())
                        .child(
                            Button::new("sessions-new")
                                .small()
                                .primary()
                                .icon(Icon::new(IconName::Plus))
                                .label("New session")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(SessionsEvent::NewSession)),
                                ),
                        ),
                ),
        ))
    }
}
