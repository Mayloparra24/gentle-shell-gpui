//! Render: the search field, category chips, grouped rows and key hints.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::ui;

use super::{CATEGORIES, Category, Line, PaletteAction, PaletteEvent, PaletteView, Row};

impl PaletteView {
    fn render_categories(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .px_4()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(ui::border())
            .children(CATEGORIES.iter().map(|(category, label)| {
                let category = *category;
                let active = category == self.category;
                div()
                    .id(SharedString::from(format!("palette-cat-{label}")))
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(ui::accent().opacity(0.14))
                            .border_color(ui::accent().opacity(0.45))
                            .text_color(ui::accent())
                    })
                    .when(!active, |d| {
                        d.border_color(gpui_kit::transparent_black())
                            .text_color(ui::muted())
                            .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_category(category, window, cx)
                    }))
                    .child(*label)
            }))
    }

    fn render_row(&self, i: usize, row: &Row, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = i == self.selected;
        let action = row.action.clone();
        let renaming = self
            .renaming
            .as_ref()
            .filter(|(index, _)| Some(*index) == row.workspace)
            .map(|(_, state)| state.clone());
        h_flex()
            .id(("palette-row", i))
            .group("palette-row")
            .relative()
            .h(px(44.))
            .flex_none()
            .pl_3()
            .pr_2()
            .gap_3()
            .rounded_lg()
            .cursor_pointer()
            .when(selected, |d| d.bg(ui::raised()))
            .when(!selected, |d| d.hover(|s| s.bg(ui::surface_hover())))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.renaming.is_none() {
                    cx.emit(PaletteEvent::Run(action.clone()));
                }
            }))
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(11.))
                        .h(px(22.))
                        .w(px(3.))
                        .rounded_full()
                        .bg(ui::accent()),
                )
            })
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .border_1()
                    .border_color(ui::border_strong())
                    .bg(ui::well())
                    .text_color(if selected || row.current {
                        ui::accent()
                    } else {
                        ui::muted()
                    })
                    .child(Icon::new(row.icon).small()),
            )
            .map(|d| match renaming {
                Some(state) => {
                    d.child(
                        ui::field()
                            .h(px(30.))
                            .w(px(280.))
                            .child(div().flex_1().child(Input::new(&state).appearance(false)))
                            .child(ui::key_button("palette-rename-enter", "↵").on_click(
                                cx.listener(|this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.press_enter(window, cx);
                                }),
                            )),
                    )
                }
                None => d.child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(360.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(14.))
                                .text_color(ui::fg())
                                .child(row.title.clone()),
                        )
                        .when(!row.meta.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(ui::faint())
                                    .child(row.meta.clone()),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(row.detail.clone()),
                        ),
                ),
            })
            .when_some(row.workspace, |d, index| {
                let deletable = self.workspaces.len() > 1;
                d.child(
                    h_flex()
                        .gap_0p5()
                        .flex_none()
                        .invisible()
                        .when(selected, |d| d.visible())
                        .group_hover("palette-row", |s| s.visible())
                        .child(
                            Button::new(("ws-rename", index))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Pencil))
                                .tooltip("Rename")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.start_rename(index, window, cx);
                                })),
                        )
                        .when(deletable, |d| {
                            d.child(
                                Button::new(("ws-delete", index))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::X))
                                    .tooltip("Delete workspace")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.emit(PaletteEvent::Run(PaletteAction::DeleteWorkspace(
                                            index,
                                        )));
                                    })),
                            )
                        }),
                )
            })
    }

    fn render_empty(&self) -> AnyElement {
        let loading = match self.category {
            Category::Files => self.files.is_none(),
            Category::Branches => self.branches.is_none(),
            _ => false,
        };
        let text = if loading {
            if self.category == Category::Files {
                "Indexing files…"
            } else {
                "Reading branches…"
            }
        } else if self.category == Category::Branches && self.needle.text.is_empty() {
            "No local branches here"
        } else {
            "No matches"
        };
        h_flex()
            .justify_center()
            .gap_2()
            .py_8()
            .text_sm()
            .text_color(ui::muted())
            .when(loading, |d| d.child(Spinner::new().small()))
            .child(text)
            .into_any_element()
    }
}

fn hint(keys: impl IntoIterator<Item = impl IntoElement>, label: &'static str) -> Div {
    h_flex().gap_1().children(keys).child(
        div()
            .pl_0p5()
            .text_xs()
            .text_color(ui::muted())
            .child(label),
    )
}

impl Render for PaletteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.rows.is_empty();
        let mut list = v_flex()
            .id("palette-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px_2()
            .pb_2();
        for line in &self.lines {
            list = match line {
                Line::Header(group) => list.child(
                    div()
                        .flex_none()
                        .px_3()
                        .pt_3()
                        .pb_1p5()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui::muted())
                        .child(*group),
                ),
                Line::Row(i) => list.child(self.render_row(*i, &self.rows[*i], cx)),
            };
        }
        if empty {
            list = list.child(self.render_empty());
        }
        let switch_keys: &[&'static str] = if cfg!(target_os = "macos") {
            &["⌃", "⌘", "K"]
        } else {
            &["Ctrl", "Alt", "K"]
        };

        let panel = v_flex()
            .id("palette")
            .w(px(680.))
            .h(px(560.))
            .max_h_full()
            .rounded_2xl()
            .border_1()
            .border_color(ui::border_strong())
            .bg(ui::screen_bg())
            .shadow_2xl()
            .overflow_hidden()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_none()
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(ui::border())
                    .child(Icon::new(IconName::Search).text_color(ui::muted()))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(16.))
                            .child(Input::new(&self.query).appearance(false)),
                    )
                    .child(
                        Button::new("palette-close")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::X))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.press_escape(window, cx)),
                            ),
                    ),
            )
            .child(self.render_categories(cx))
            .child(list)
            .child(
                h_flex()
                    .h(px(44.))
                    .flex_none()
                    .px_4()
                    .gap_4()
                    .border_t_1()
                    .border_color(ui::border())
                    .child(hint(["↑", "↓"].map(ui::kbd), "navigate"))
                    .child(hint(
                        [ui::key_button("palette-run", "↵").on_click(
                            cx.listener(|this, _, window, cx| this.press_enter(window, cx)),
                        )],
                        "run",
                    ))
                    .child(hint(["←", "→"].map(ui::kbd), "category"))
                    .child(hint(
                        switch_keys.iter().map(|k| ui::kbd(*k)),
                        "switch workspaces",
                    ))
                    .child(div().flex_1())
                    .child(hint(
                        [ui::key_button("palette-close", "esc").on_click(
                            cx.listener(|this, _, window, cx| this.press_escape(window, cx)),
                        )],
                        "close",
                    )),
            );

        div()
            .id("palette-backdrop")
            .absolute()
            .inset_0()
            .flex()
            // Centered like every other screen (`ui::screen`); the fixed
            // height keeps it from jumping as results change.
            .items_center()
            .justify_center()
            .p(px(32.))
            .occlude()
            .capture_key_down(cx.listener(Self::on_key_down))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Close)))
            .child(ui::scrim("palette-scrim"))
            .child(ui::pop_in(
                "palette-in",
                div().flex().max_h_full().child(panel),
            ))
    }
}
