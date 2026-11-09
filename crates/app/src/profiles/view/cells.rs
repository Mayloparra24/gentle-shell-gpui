//! The route table's model, effort and tier cells and their menus.

use gentle_core::profiles::{
    ORCHESTRATOR, RouteEntry, THINKING_LEVELS, clamp_level, is_review_role, normalize_model_id,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Disableable as _, Icon, Selectable as _, Sizable as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::profiles::{ALL_ROW, ProfilesView};
use crate::ui;

use super::{EFFORT_W, TIER_W};

/// A two-line menu entry: the label over a muted detail.
fn menu_entry(label: &SharedString, detail: &SharedString) -> Div {
    v_flex()
        .min_w_0()
        .py_0p5()
        .child(div().text_sm().text_color(ui::fg()).child(label.clone()))
        .child(
            div()
                .text_xs()
                .font_family(ui::MONO)
                .text_color(ui::muted())
                .child(detail.clone()),
        )
}

impl ProfilesView {
    pub(in crate::profiles) fn inherit_label(&self, key: &str) -> &'static str {
        match key {
            ORCHESTRATOR => "leave untouched",
            ALL_ROW => "inherit",
            k if is_review_role(k) => "Pi persisted default model",
            _ => "inherit",
        }
    }

    /// The value one row shows; the every-agent row shows the shared value or
    /// "mixed".
    fn current_value(
        &self,
        key: &str,
        field: fn(&RouteEntry) -> &Option<String>,
    ) -> Result<Option<String>, ()> {
        if key == ALL_ROW {
            let values: Vec<Option<String>> = self
                .names
                .iter()
                .map(|n| self.draft.get(n).and_then(|e| field(e).clone()))
                .collect();
            return match values.first() {
                Some(first) if values.iter().all(|v| v == first) => Ok(first.clone()),
                Some(_) => Err(()),
                None => Ok(None),
            };
        }
        Ok(self.draft.get(key).and_then(|e| field(e).clone()))
    }

    fn is_edited(&self, key: &str, field: fn(&RouteEntry) -> &Option<String>) -> bool {
        if key == ALL_ROW {
            return false;
        }
        self.draft.get(key).and_then(|e| field(e).as_ref())
            != self.stored.get(key).and_then(|e| field(e).as_ref())
    }

    pub(super) fn render_model_cell(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let models = self.models.clone();
        let current = self.current_value(key, |e| &e.model);
        let inherit = self.inherit_label(key);
        let label = match &current {
            Ok(Some(model)) => model.clone(),
            Ok(None) if key != ORCHESTRATOR && is_review_role(key) => "Pi default".into(),
            Ok(None) => inherit.to_string(),
            Err(()) => "mixed".into(),
        };
        let edited = self.is_edited(key, |e| &e.model);
        let color = if edited {
            ui::warning()
        } else if matches!(current, Ok(Some(_))) {
            ui::fg()
        } else {
            ui::muted()
        };
        let key = key.to_string();
        let current = current.ok().flatten();
        // A stretched column gives the trigger a definite width, so a long
        // model id ellipsizes instead of spilling into the effort column.
        v_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(
                Button::new(SharedString::from(format!("model-{key}")))
                    .ghost()
                    .xsmall()
                    .w_full()
                    .font_family(ui::MONO)
                    .text_color(color)
                    .label(label)
                    .dropdown_caret(true)
                    .disabled(self.busy)
                    .dropdown_menu(move |mut menu, _, _| {
                        let pick = |model: Option<String>| {
                            let weak = weak.clone();
                            let key = key.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                let _ = weak
                                    .update(cx, |this, cx| this.set_model(&key, model.clone(), cx));
                            }
                        };
                        menu = menu.item(
                            PopupMenuItem::new(inherit)
                                .checked(current.is_none())
                                .on_click(pick(None)),
                        );
                        let mut provider = String::new();
                        for model in models.iter() {
                            if model.provider != provider {
                                provider = model.provider.clone();
                                menu = menu.label(provider.clone());
                            }
                            let id = model.route_id();
                            let Some(id) = normalize_model_id(&id) else {
                                continue;
                            };
                            let name = SharedString::from(model.name.clone());
                            let model_id = SharedString::from(model.id.clone());
                            menu = menu.item(
                                PopupMenuItem::element(move |_, _| menu_entry(&name, &model_id))
                                    .checked(current.as_deref() == Some(id.as_str()))
                                    .on_click(pick(Some(id))),
                            );
                        }
                        let weak = weak.clone();
                        let key = key.clone();
                        menu.separator()
                            .item(
                                PopupMenuItem::new("Custom model id…")
                                    .icon(Icon::new(IconName::Plus))
                                    .on_click(move |_, window, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.open_custom(&key, window, cx)
                                        });
                                    }),
                            )
                            .max_h(px(420.))
                            .scrollable(true)
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_effort_cell(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let model = if key == ALL_ROW {
            None
        } else {
            self.draft.get(key).and_then(|e| e.model.clone())
        };
        if key == ORCHESTRATOR && model.is_none() {
            return div()
                .w(px(EFFORT_W))
                .flex_shrink_0()
                .px_2()
                .text_xs()
                .text_color(ui::faint())
                .child("follows model")
                .into_any_element();
        }
        let supported = if key == ALL_ROW {
            let known: Vec<Vec<&str>> = self
                .names
                .iter()
                .filter_map(|n| {
                    self.supported_levels(self.draft.get(n).and_then(|e| e.model.as_deref()))
                })
                .collect();
            (!known.is_empty()).then(|| {
                THINKING_LEVELS
                    .iter()
                    .copied()
                    .filter(|l| known.iter().all(|k| k.contains(l)))
                    .collect::<Vec<_>>()
            })
        } else {
            self.supported_levels(model.as_deref())
        };
        let current = self.current_value(key, |e| &e.thinking);
        let label = match &current {
            Ok(Some(level)) => match &supported {
                Some(levels) if !levels.contains(&level.as_str()) => {
                    format!("{level} → {}", clamp_level(level, levels))
                }
                _ => level.clone(),
            },
            Ok(None) if is_review_role(key) => "Pi default".into(),
            Ok(None) => "inherit".into(),
            Err(()) => "mixed".into(),
        };
        let edited = self.is_edited(key, |e| &e.thinking);
        let color = if edited {
            ui::warning()
        } else if matches!(current, Ok(Some(_))) {
            ui::fg()
        } else {
            ui::muted()
        };
        let options: Vec<&'static str> = THINKING_LEVELS
            .iter()
            .copied()
            .filter(|l| supported.as_ref().is_none_or(|s| s.contains(l)))
            .collect();
        let inherit = if is_review_role(key) {
            "Pi persisted default effort"
        } else {
            "Inherit effort"
        };
        let weak = cx.entity().downgrade();
        let key = key.to_string();
        let current = current.ok().flatten();
        v_flex()
            .w(px(EFFORT_W))
            .flex_shrink_0()
            .overflow_hidden()
            .child(
                Button::new(SharedString::from(format!("effort-{key}")))
                    .ghost()
                    .xsmall()
                    .w_full()
                    .font_family(ui::MONO)
                    .text_color(color)
                    .label(label)
                    .dropdown_caret(true)
                    .disabled(self.busy)
                    .dropdown_menu(move |mut menu, _, _| {
                        let pick = |level: Option<String>| {
                            let weak = weak.clone();
                            let key = key.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.set_thinking(&key, level.clone(), cx)
                                });
                            }
                        };
                        menu = menu.item(
                            PopupMenuItem::new(inherit)
                                .checked(current.is_none())
                                .on_click(pick(None)),
                        );
                        for level in &options {
                            menu = menu.item(
                                PopupMenuItem::new(*level)
                                    .checked(current.as_deref() == Some(*level))
                                    .on_click(pick(Some(level.to_string()))),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_tier_cell(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let entry = self.draft.get(key);
        let model = entry.and_then(|e| e.model.as_deref());
        let tier = entry.and_then(|e| e.tier.as_deref());
        let edited = self.is_edited(key, |e| &e.tier);
        let cell = div().w(px(TIER_W)).flex_shrink_0();
        if key == ALL_ROW {
            return cell.into_any_element();
        }
        if !self.fast_capable(model) {
            let text = match tier {
                Some(tier) => format!("{tier} (n/a)"),
                None => "—".into(),
            };
            return cell
                .px_2()
                .text_xs()
                .text_color(ui::faint())
                .child(text)
                .into_any_element();
        }
        let fast = tier == Some("fast");
        let key = key.to_string();
        cell.child(
            Button::new(SharedString::from(format!("tier-{key}")))
                .xsmall()
                .when(fast, |b| b.outline().selected(true))
                .when(!fast, |b| b.ghost())
                .when(edited, |b| b.text_color(ui::warning()))
                .icon(Icon::new(IconName::Zap).when(fast, |i| i.text_color(ui::champagne())))
                .label(match tier {
                    Some("fast") => "Fast",
                    Some(_) => "Standard",
                    None => "Standard",
                })
                .tooltip("OpenAI Fast (service_tier priority): 2.5x subscription usage or 2x purchased credits")
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_tier(&key, cx))),
        )
        .into_any_element()
    }
}
