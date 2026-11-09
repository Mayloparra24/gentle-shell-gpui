//! The selected profile: its scope card, the notes the screen must show and
//! the route table.

use gentle_core::profiles::{
    ORCHESTRATOR, PinRead, format_selection, is_review_role, pin_notes, unmatched_notes,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::features::profiles::{ALL_ROW, ProfilesView};
use crate::shared::ui;

use super::{EFFORT_W, TIER_W, route_cells, route_columns, route_name_column};

impl ProfilesView {
    fn render_row(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let (label, hint) = match key {
            ORCHESTRATOR => ("orchestrator".to_string(), Some("settings.json")),
            ALL_ROW => ("every agent".to_string(), Some("sets all rows")),
            k if is_review_role(k) => (k.to_string(), Some("review role")),
            k => (k.to_string(), None),
        };
        let custom = self.custom_target.as_deref() == Some(key);
        let row = v_flex()
            .rounded_lg()
            .hover(|s| s.bg(ui::surface_hover()))
            .when(custom, |d| d.bg(ui::raised()))
            .child(
                route_columns()
                    .px_2()
                    .py_1p5()
                    .child(
                        // The hint sits under the name so the column stays
                        // narrow enough for the name itself.
                        route_name_column()
                            .child(
                                div()
                                    .truncate()
                                    .font_family(ui::MONO)
                                    .text_xs()
                                    .text_color(if key == ALL_ROW {
                                        ui::muted()
                                    } else {
                                        ui::fg()
                                    })
                                    .child(label),
                            )
                            .when_some(hint, |d, hint| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(ui::muted())
                                        .child(hint),
                                )
                            }),
                    )
                    .child(
                        route_cells()
                            .child(self.render_model_cell(key, cx))
                            .child(self.render_effort_cell(key, cx))
                            .child(self.render_tier_cell(key, cx)),
                    ),
            )
            .when(custom, |d| {
                d.child(
                    h_flex()
                        .px_2()
                        .pb_2()
                        .gap_2()
                        .child(
                            ui::field()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.custom_input).appearance(false).small()),
                        )
                        .child(
                            Button::new("custom-apply")
                                .small()
                                .outline()
                                .icon(Icon::new(IconName::Check))
                                .label("Set")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_custom(window, cx)
                                })),
                        )
                        .child(
                            Button::new("custom-cancel")
                                .small()
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.custom_target = None;
                                    cx.notify();
                                })),
                        ),
                )
            });
        ui::appear(SharedString::from(format!("route-{key}")), row).into_any_element()
    }

    fn render_scope(&self) -> impl IntoElement {
        let name = self.selected.clone().unwrap_or_default();
        let mut parts: Vec<(&str, Hsla)> = Vec::new();
        if self.file.active.as_deref() == Some(name.as_str()) {
            parts.push(("Default for new sessions", ui::accent()));
        }
        if self.pin_winner().as_deref() == Some(name.as_str()) {
            parts.push(("Used in this repo", ui::warning()));
        }
        let default_line = match &self.file.active {
            Some(active) => format!("Global default: {active}"),
            None => "No global default profile".into(),
        };
        let pin_line = match &self.pin {
            None => "Repo pin: none (not inside a Git worktree)".to_string(),
            Some(status) => {
                let layer = |read: &PinRead| match read {
                    PinRead::Missing => "none".to_string(),
                    PinRead::Invalid => "invalid file".to_string(),
                    PinRead::Valid(p) => p.clone(),
                };
                format!(
                    "Repo pin: local {} · declaration {}",
                    layer(&status.local),
                    layer(&status.repo)
                )
            }
        };
        ui::card()
            .p_3()
            .gap_1()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::fg())
                            .child(name),
                    )
                    .when(parts.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(ui::muted())
                                .child("Not in use. Pick a button below to use it."),
                        )
                    })
                    .children(parts.into_iter().map(|(part, color)| ui::chip(part, color))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_0p5()
                    .text_xs()
                    .text_color(ui::muted())
                    .child(default_line)
                    .child(pin_line),
            )
    }

    pub(super) fn render_table(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let notes: Vec<String> = self
            .notices
            .iter()
            .cloned()
            .chain(pin_notes(self.pin.as_ref(), &self.file))
            .chain(unmatched_notes(&self.draft, &self.names))
            .collect();
        let orchestrator_now = self.pi_default.clone();
        v_flex()
            .gap_3()
            .child(self.render_scope())
            .children(notes.into_iter().map(|note| {
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(ui::warning().opacity(0.22))
                    .bg(ui::warning().opacity(0.08))
                    .text_xs()
                    .text_color(ui::warning())
                    .child(Icon::new(IconName::TriangleAlert).xsmall().flex_shrink_0())
                    .child(div().flex_1().min_w_0().child(note))
            }))
            .child(
                h_flex()
                    .px_1()
                    .gap_2()
                    .child(ui::section_label("Routes"))
                    .child(div().flex_1())
                    .child(
                        Button::new("all-fast")
                            .small()
                            .ghost()
                            .icon(Icon::new(IconName::Zap))
                            .label("All Fast")
                            .tooltip("Set every compatible OpenAI route to Fast (draft only)")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.all_fast(cx))),
                    ),
            )
            .child(
                ui::card()
                    .p_1()
                    .gap_0p5()
                    .child(
                        route_columns()
                            .px_2()
                            .pt_1p5()
                            .pb_1()
                            .child(route_name_column().child(ui::section_label("Route")))
                            .child(
                                route_cells()
                                    .child(div().flex_1().min_w_0().px_1().child(ui::section_label("Model")))
                                    .child(
                                        div()
                                            .w(px(EFFORT_W))
                                            .flex_shrink_0()
                                            .px_1()
                                            .child(ui::section_label("Effort")),
                                    )
                                    .child(
                                        div()
                                            .w(px(TIER_W))
                                            .flex_shrink_0()
                                            .px_1()
                                            .child(ui::section_label("Tier")),
                                    ),
                            ),
                    )
                    .child(self.render_row(ORCHESTRATOR, cx))
                    .child(self.render_row(ALL_ROW, cx))
                    .when(!self.names.is_empty(), |d| {
                        d.child(div().mx_2().my_1().h(px(1.)).bg(ui::border()))
                            .child(
                                h_flex()
                                    .px_2()
                                    .pt_0p5()
                                    .gap_2()
                                    .child(ui::section_label("Agents"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(ui::faint())
                                            .child(self.names.len().to_string()),
                                    ),
                            )
                    })
                    .children(self.names.iter().map(|key| self.render_row(key, cx))),
            )
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(ui::faint())
                    .child(format!(
                        "Pi default orchestrator now: {}. Fast applies only to openai / openai-codex GPT-5.6 / 6 / 6.1 models; the selector is a request, not a billing guarantee.",
                        orchestrator_now.map_or_else(|| "not set".to_string(), |e| format_selection(Some(&e)))
                    )),
            )
    }
}
