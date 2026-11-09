//! The composer's model picker: trigger, searchable list and its keys.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::NARROW;
use crate::features::session::SessionView;
use crate::features::session::state::ModelInfo;
use crate::shared::ui;
use gentle_core::transcript::format_tokens;

impl SessionView {
    fn filtered_models(&self, cx: &App) -> Vec<ModelInfo> {
        let query = self.model_search.read(cx).value().trim().to_lowercase();
        self.models
            .available
            .iter()
            .filter(|m| {
                query.is_empty()
                    || m.name.to_lowercase().contains(&query)
                    || m.id.to_lowercase().contains(&query)
                    || m.provider.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    fn set_model_picker(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.live.model_picker == open {
            return;
        }
        self.live.model_picker = open;
        if open {
            self.live.model_query.clear();
            self.model_search.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.focus(window, cx);
            });
            self.live.model_cursor = self
                .models
                .current
                .as_ref()
                .and_then(|current| self.models.available.iter().position(|m| m == current))
                .unwrap_or(0);
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    /// The search text changed: highlight the first match again.
    pub(in crate::features::session) fn model_query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.model_search.read(cx).value().to_string();
        if query != self.live.model_query {
            self.live.model_query = query;
            self.live.model_cursor = 0;
        }
        cx.notify();
    }

    /// ↑/↓/Enter/Esc while the model picker is open. Runs as a keystroke
    /// interceptor so it wins over the focused search field.
    pub(in crate::features::session) fn model_picker_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.live.model_picker {
            return false;
        }
        let m = &keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return false;
        }
        let models = self.filtered_models(cx);
        let n = models.len();
        let cursor = self.live.model_cursor.min(n.saturating_sub(1));
        match keystroke.key.as_str() {
            "up" | "down" if n > 0 => {
                let next = if keystroke.key == "up" {
                    (cursor + n - 1) % n
                } else {
                    (cursor + 1) % n
                };
                self.live.model_cursor = next;
                // Rows sit between provider headers in the scrolled list.
                let headers = 1 + models[1..=next]
                    .iter()
                    .zip(&models[..next])
                    .filter(|(a, b)| a.provider != b.provider)
                    .count();
                self.live.model_scroll.scroll_to_item(next + headers);
            }
            "enter" => {
                if let Some(model) = models.get(cursor) {
                    self.pick_model(model.clone(), window, cx);
                }
            }
            "escape" => self.set_model_picker(false, window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    fn pick_model(&mut self, model: ModelInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.set_model_picker(false, window, cx);
        if self.models.current.as_ref() != Some(&model) {
            self.set_model(model, cx);
        }
    }

    pub(super) fn render_model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let busy = self.busy();
        let label = self
            .models
            .current
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "No model".into());
        let dot = self
            .models
            .current
            .as_ref()
            .map_or(ui::faint(), |m| provider_color(&m.provider));
        let trigger = Button::new("model")
            .ghost()
            .xsmall()
            .text_color(ui::info())
            .tooltip("Choose model")
            .child(if busy {
                Spinner::new().xsmall().color(ui::info()).into_any_element()
            } else {
                div().size(px(7.)).rounded_full().bg(dot).into_any_element()
            })
            .child(
                div()
                    .min_w_0()
                    .when(self.pane_width.get() < NARROW, |d| d.max_w(px(120.)))
                    .truncate()
                    .child(label),
            )
            .dropdown_caret(true);

        let models = self.filtered_models(cx);
        let cursor = self.live.model_cursor.min(models.len().saturating_sub(1));
        let current = self.models.current.clone();
        let search = self.model_search.clone();
        let has_any = !self.models.available.is_empty();
        let content_weak = weak.clone();
        let scroll = self.live.model_scroll.clone();

        Popover::new("model-picker")
            .anchor(Anchor::BottomLeft)
            .trigger(trigger)
            .open(self.live.model_picker)
            .track_focus(&self.model_search.focus_handle(cx))
            .on_open_change(move |open, window, cx| {
                let _ = weak.update(cx, |this, cx| this.set_model_picker(*open, window, cx));
            })
            .w(px(380.))
            .appearance(false)
            .content(move |_, _, cx| {
                let weak = content_weak.clone();
                let mut list = v_flex()
                    .id("model-list")
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .px(px(6.));
                let mut provider = "";
                for (i, model) in models.iter().enumerate() {
                    if model.provider != provider {
                        provider = &model.provider;
                        list = list.child(
                            h_flex()
                                .gap_1p5()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(provider_color(provider)),
                                )
                                .child(ui::section_label(provider.to_string())),
                        );
                    }
                    let is_current = current.as_ref() == Some(model);
                    let pick = model.clone();
                    let weak = weak.clone();
                    list = list.child(
                        h_flex()
                            .id(("model-row", i))
                            .relative()
                            .w_full()
                            .gap_2()
                            .px_3()
                            .py_1p5()
                            .rounded(px(ui::ISLAND_ROW))
                            .cursor_pointer()
                            .hover(|s| s.bg(ui::surface_hover()))
                            .when(i == cursor, |this| this.bg(ui::surface_hover()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(13.))
                                    .text_color(if is_current { ui::accent() } else { ui::fg() })
                                    .child(model.name.clone()),
                            )
                            .when(model.context_window > 0, |this| {
                                this.child(
                                    ui::chip(format_tokens(model.context_window), ui::muted())
                                        .border_0()
                                        .rounded_full(),
                                )
                            })
                            .when(model.reasoning, |this| {
                                this.child(
                                    ui::chip("reasoning", ui::petal()).border_0().rounded_full(),
                                )
                            })
                            .when(is_current, |this| {
                                this.child(
                                    Icon::new(IconName::Check).xsmall().text_color(ui::accent()),
                                )
                            })
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.pick_model(pick.clone(), window, cx)
                                });
                            }),
                    );
                }
                if models.is_empty() {
                    list = list.child(
                        div()
                            .px_3()
                            .py_4()
                            .text_size(px(12.))
                            .text_color(ui::faint())
                            .child(if has_any {
                                "No models match."
                            } else {
                                "No models yet. Sign in to a provider."
                            }),
                    );
                }
                let sign_in = weak.clone();
                let content = v_flex()
                    .w_full()
                    .font_family(cx.theme().font_family.clone())
                    .text_color(ui::fg())
                    .child(
                        div().p(px(6.)).child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .h(px(36.))
                                .px_3()
                                .rounded(px(ui::ISLAND_ROW))
                                .bg(ui::well())
                                .child(Icon::new(IconName::Search).xsmall().text_color(ui::faint()))
                                .child(div().flex_1().child(Input::new(&search).appearance(false))),
                        ),
                    )
                    .child(list)
                    .child(
                        div().p(px(6.)).child(
                            Button::new("picker-sign-in")
                                .ghost()
                                .small()
                                .w_full()
                                .icon(Icon::new(IconName::LogIn))
                                .label(if has_any {
                                    "Sign in to another provider…"
                                } else {
                                    "Sign in to a provider…"
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = sign_in.update(cx, |this, cx| {
                                        this.set_model_picker(false, window, cx);
                                        cx.emit(
                                            crate::features::session::SessionEvent::OpenProviders,
                                        );
                                    });
                                }),
                        ),
                    );
                ui::island(cx)
                    .w_full()
                    .child(ui::popover_in(
                        ElementId::Name("model-picker-in".into()),
                        content,
                    ))
                    .into_any_element()
            })
    }
}

fn provider_color(provider: &str) -> Hsla {
    let palette = [
        ui::petal(),
        ui::info(),
        ui::success(),
        ui::champagne(),
        ui::warning(),
    ];
    let hash = provider
        .bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    palette[hash % palette.len()]
}
