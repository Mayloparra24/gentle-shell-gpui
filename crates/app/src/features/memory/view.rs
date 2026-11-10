//! The memory screen: observation list, detail pane, search and scope.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{MemoryEvent, MemoryView};
use crate::shared::ui;

impl Render for MemoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .selected
            .and_then(|id| self.items.iter().find(|o| o.id == id))
            .cloned();
        let list =
            v_flex()
                .gap_0p5()
                .children(self.items.iter().enumerate().map(|(i, o)| {
                    let id = o.id;
                    let active = self.selected == Some(id);
                    let row =
                        v_flex()
                            .id(("memory", id as u64))
                            .px_3()
                            .py_2()
                            .gap_1()
                            .rounded_lg()
                            .cursor_pointer()
                            .border_1()
                            .border_color(transparent_black())
                            .when(active, |d| {
                                d.bg(ui::raised()).border_color(ui::border_strong())
                            })
                            .when(!active, |d| d.hover(|d| d.bg(ui::surface_hover())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(id);
                                cx.notify();
                            }))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .child(ui::chip(o.kind.clone(), ui::petal()).flex_shrink_0())
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .text_color(ui::fg())
                                            .child(o.title.clone()),
                                    ),
                            )
                            .child(div().truncate().text_xs().text_color(ui::faint()).child(
                                format!(
                        "{}{}",
                        o.updated_at
                            .get(..16)
                            .unwrap_or(&o.updated_at)
                            .replace('T', " "),
                        o.project
                            .as_ref()
                            .filter(|_| self.all_projects)
                            .map(|p| format!(" · {p}"))
                            .unwrap_or_default()
                    ),
                            ));
                    ui::appear(("memory-row", i), div().child(row))
                }));

        let body: AnyElement = if let Some(error) = &self.error {
            div()
                .p_4()
                .child(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(ui::danger().opacity(0.08))
                        .border_1()
                        .border_color(ui::danger().opacity(0.22))
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .small()
                                .text_color(ui::danger())
                                .mt_0p5(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(ui::danger())
                                .child(error.clone()),
                        ),
                )
                .into_any_element()
        } else if self.items.is_empty() && !self.loading {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(Icon::new(IconName::Brain).text_color(ui::faint()))
                .child(
                    div()
                        .max_w(px(420.))
                        .text_center()
                        .text_sm()
                        .text_color(ui::muted())
                        .child(if self.query.read(cx).value().is_empty() {
                            "Nothing remembered for this project yet. el Gentleman saves decisions, bugs and discoveries here as you work."
                        } else {
                            "No memories match."
                        }),
                )
                .into_any_element()
        } else {
            h_flex()
                .size_full()
                .items_start()
                .child(
                    div()
                        .id("memory-list")
                        .w(relative(0.4))
                        .min_w(px(220.))
                        .max_w(px(340.))
                        .h_full()
                        .flex_shrink_0()
                        .overflow_y_scroll()
                        .p_2()
                        .child(list),
                )
                .child(
                    v_flex()
                        .id("memory-detail")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .p_5()
                        .gap_3()
                        .border_l_1()
                        .border_color(ui::border())
                        .when_some(selected, |d, o| {
                            d.child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui::fg())
                                    .child(o.title.clone()),
                            )
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .child(ui::chip(format!("#{}", o.id), ui::faint()))
                                    .child(ui::chip(o.kind.clone(), ui::petal()))
                                    .when(!o.scope.is_empty(), |d| {
                                        d.child(ui::chip(o.scope.clone(), ui::info()))
                                    })
                                    .when_some(o.topic_key.clone(), |d, k| {
                                        d.child(ui::chip(k, ui::champagne()))
                                    })
                                    .when(o.revision_count > 0, |d| {
                                        d.child(ui::chip(
                                            format!("{} revisions", o.revision_count),
                                            ui::muted(),
                                        ))
                                    }),
                            )
                            .child(
                                ui::card().p_4().child(
                                    div()
                                        .text_sm()
                                        .text_color(ui::fg())
                                        .whitespace_normal()
                                        .child(o.content.clone()),
                                ),
                            )
                        }),
                )
                .into_any_element()
        };

        let all = self.all_projects;
        let (backdrop, panel) = ui::screen(None);
        backdrop.occlude().child(ui::pop_in(
            "memory-screen",
            panel
                .max_w(px(1100.))
                .child(
                    ui::screen_header()
                        .child(Icon::new(IconName::Brain).small().text_color(ui::muted()))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Memory"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(match (&self.project, all) {
                                    (_, true) => "Engram · all projects".to_string(),
                                    (Some(p), false) => format!("Engram · {p}"),
                                    (None, false) => "Engram".to_string(),
                                }),
                        )
                        .when(self.loading, |d| {
                            d.child(Spinner::new().small().color(ui::accent()))
                        })
                        .child(
                            ui::key_button("memory-esc", "esc")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(MemoryEvent::Close))),
                        )
                        .child(
                            Button::new("memory-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(MemoryEvent::Close))),
                        ),
                )
                .child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(ui::border())
                        .child(
                            ui::field()
                                .flex_1()
                                .min_w_0()
                                .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
                                .child(Input::new(&self.query).appearance(false).small()),
                        )
                        .child(ui::segmented(
                            "memory-scope",
                            &["This project", "All projects"],
                            Some(usize::from(all)),
                            cx.listener(|this, index: &usize, _, cx| {
                                let all = *index == 1;
                                if this.all_projects != all {
                                    this.all_projects = all;
                                    this.reload(cx);
                                }
                            }),
                        ))
                        .child(
                            Button::new("memory-refresh")
                                .ghost()
                                .small()
                                .icon(Icon::new(IconName::RefreshCw))
                                .tooltip("Reload")
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        ),
                )
                .child(div().flex_1().min_h_0().child(body)),
        ))
    }
}
