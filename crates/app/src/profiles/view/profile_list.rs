//! The profile list: one row per profile with its default and pin marks,
//! the name entry and the New, Duplicate, Rename and Delete buttons.

use gentle_core::profiles::PinRead;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::profiles::{Leave, NameMode, ProfilesView};
use crate::ui;

impl ProfilesView {
    pub(super) fn render_profile_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pin_winner = self.pin_winner();
        let declared = match self.pin.as_ref().map(|p| &p.repo) {
            Some(PinRead::Valid(name)) => Some(name.clone()),
            _ => None,
        };
        let has_selection = self.selected.is_some();
        let count = self.file.profiles.len();
        v_flex()
            .w(px(210.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(ui::border())
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .gap_2()
                    .child(ui::section_label("Profiles"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui::faint())
                            .child(count.to_string()),
                    ),
            )
            .child(
                div()
                    .id("profiles-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .children(self.file.profiles.iter().enumerate().map(
                                |(ix, (name, config))| {
                                    let selected = self.selected.as_deref() == Some(name.as_str());
                                    let active = self.file.active.as_deref() == Some(name.as_str());
                                    let pinned = pin_winner.as_deref() == Some(name.as_str());
                                    let is_declared = declared.as_deref() == Some(name.as_str());
                                    let roles = config.roles().count();
                                    let target = name.clone();
                                    let row = v_flex()
                                        .id(("profile", ix))
                                        .px_2()
                                        .py_1p5()
                                        .gap_1()
                                        .rounded_lg()
                                        .cursor_pointer()
                                        .when(selected, |d| d.bg(ui::raised()))
                                        .when(!selected, |d| d.hover(|s| s.bg(ui::surface_hover())))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if this.selected.as_deref() != Some(target.as_str()) {
                                                this.leave(
                                                    Leave::Switch(target.clone()),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_sm()
                                                        .text_color(if selected {
                                                            ui::fg()
                                                        } else {
                                                            ui::muted()
                                                        })
                                                        .child(name.clone()),
                                                )
                                                .when(active, |d| {
                                                    d.child(
                                                        Icon::new(IconName::Star)
                                                            .xsmall()
                                                            .text_color(ui::accent()),
                                                    )
                                                })
                                                .when(pinned, |d| {
                                                    d.child(
                                                        Icon::new(IconName::GitBranch)
                                                            .xsmall()
                                                            .text_color(ui::warning()),
                                                    )
                                                }),
                                        )
                                        .child(
                                            h_flex()
                                                .flex_wrap()
                                                .gap_1()
                                                .child(
                                                    div().text_xs().text_color(ui::faint()).child(
                                                        format!(
                                                            "{roles} {}",
                                                            if roles == 1 {
                                                                "role"
                                                            } else {
                                                                "roles"
                                                            }
                                                        ),
                                                    ),
                                                )
                                                .when(active, |d| {
                                                    d.child(ui::chip("default", ui::accent()))
                                                })
                                                .when(pinned, |d| {
                                                    d.child(ui::chip("this repo", ui::warning()))
                                                })
                                                .when(is_declared && !pinned, |d| {
                                                    d.child(ui::chip("declared", ui::info()))
                                                }),
                                        );
                                    ui::appear(("profile-in", ix), div().child(row))
                                },
                            )),
                    ),
            )
            .child(
                v_flex()
                    .p_2()
                    .gap_2()
                    .border_t_1()
                    .border_color(ui::border())
                    .when_some(self.name_mode, |d, mode| {
                        let title = match mode {
                            NameMode::New => "New empty profile".to_string(),
                            NameMode::Duplicate => format!(
                                "Duplicate as (empty = {}-copy)",
                                self.selected.as_deref().unwrap_or_default()
                            ),
                            NameMode::Rename => format!(
                                "Rename \"{}\" to",
                                self.selected.as_deref().unwrap_or_default()
                            ),
                        };
                        d.child(
                            v_flex()
                                .gap_1p5()
                                .child(div().text_xs().text_color(ui::muted()).child(title))
                                .child(
                                    ui::field().child(
                                        Input::new(&self.name_input).appearance(false).small(),
                                    ),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Button::new("name-ok")
                                                .small()
                                                .outline()
                                                .icon(Icon::new(IconName::Check))
                                                .label("OK")
                                                .disabled(self.busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.submit_name(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("name-cancel")
                                                .small()
                                                .ghost()
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.name_mode = None;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("profile-new")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Plus))
                                    .label("New")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.leave(Leave::New, window, cx)
                                    })),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("profile-duplicate")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Copy))
                                    .tooltip("Duplicate")
                                    .disabled(self.busy || !has_selection)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.leave(Leave::Duplicate, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("profile-rename")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Replace))
                                    .tooltip("Rename")
                                    .disabled(self.busy || !has_selection)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_name(NameMode::Rename, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("profile-delete")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Trash))
                                    .tooltip("Delete")
                                    .disabled(self.busy || !has_selection)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.request_delete(cx)),
                                    ),
                            ),
                    ),
            )
    }
}
