//! Draft edits to the shown profile's routes (`RoutingTablePanel`
//! `setModel` / `setThinking` / `toggleTier`): nothing is written until Save.

use gentle_core::profiles::{ORCHESTRATOR, RouteEntry, normalize_model_id};
use gpui_kit::*;

use super::{ProfilesView, Tone};

impl ProfilesView {
    pub(super) fn set_model(&mut self, key: &str, model: Option<String>, cx: &mut Context<Self>) {
        for target in self.target_keys(key) {
            if target == ORCHESTRATOR && model.is_none() {
                self.draft.remove(&target);
                continue;
            }
            let mut entry = self.draft.get(&target).cloned().unwrap_or_default();
            if entry.model != model {
                entry.tier = None;
            }
            entry.model = model.clone();
            self.draft.set(
                &target,
                if entry.is_clear() {
                    RouteEntry::default()
                } else {
                    entry
                },
            );
        }
        let label = model.unwrap_or_else(|| self.inherit_label(key).to_string());
        let selected = self.selected.clone().unwrap_or_default();
        self.say(
            format!(
                "{}: model {label}. Save keeps it in {selected}.",
                Self::row_label(key)
            ),
            Tone::Info,
        );
        cx.notify();
    }

    pub(super) fn set_thinking(
        &mut self,
        key: &str,
        thinking: Option<String>,
        cx: &mut Context<Self>,
    ) {
        for target in self.target_keys(key) {
            if target == ORCHESTRATOR
                && self
                    .draft
                    .get(&target)
                    .and_then(|e| e.model.as_ref())
                    .is_none()
            {
                continue;
            }
            let mut entry = self.draft.get(&target).cloned().unwrap_or_default();
            entry.thinking = thinking.clone();
            self.draft.set(
                &target,
                if entry.is_clear() {
                    RouteEntry::default()
                } else {
                    entry
                },
            );
        }
        let selected = self.selected.clone().unwrap_or_default();
        let level = thinking.unwrap_or_else(|| "inherit".into());
        self.say(
            format!(
                "{}: effort {level}. Save keeps it in {selected}.",
                Self::row_label(key)
            ),
            Tone::Info,
        );
        cx.notify();
    }

    pub(super) fn toggle_tier(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(mut entry) = self.draft.get(key).cloned() else {
            return;
        };
        if !self.fast_capable(entry.model.as_deref()) {
            self.say(
                "Standard/Fast is available only for compatible OpenAI models.",
                Tone::Bad,
            );
            cx.notify();
            return;
        }
        let fast = entry.tier.as_deref() != Some("fast");
        entry.tier = Some(if fast { "fast" } else { "standard" }.into());
        self.draft.set(key, entry);
        let selected = self.selected.clone().unwrap_or_default();
        self.say(
            format!(
                "{}: {}. Save keeps it in {selected}.{}",
                Self::row_label(key),
                if fast { "Fast" } else { "Standard" },
                if fast { " Fast depends on plan/model; subscription usage 2.5x or purchased credits 2x. API pricing differs." } else { "" }
            ),
            Tone::Info,
        );
        cx.notify();
    }

    pub(super) fn all_fast(&mut self, cx: &mut Context<Self>) {
        let mut count = 0;
        for key in self.row_keys() {
            let Some(mut entry) = self.draft.get(&key).cloned() else {
                continue;
            };
            if !self.fast_capable(entry.model.as_deref()) {
                continue;
            }
            entry.tier = Some("fast".into());
            self.draft.set(&key, entry);
            count += 1;
        }
        if count == 0 {
            self.say("No compatible OpenAI routes in this profile.", Tone::Info);
        } else {
            self.say(
                format!(
                    "All Fast: {count} compatible route{}. Save to keep. Subscription 2.5x; purchased credits 2x. API pricing differs.",
                    if count == 1 { "" } else { "s" }
                ),
                Tone::Info,
            );
        }
        cx.notify();
    }

    pub(super) fn open_custom(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.custom_target = Some(key.to_string());
        let current = self
            .draft
            .get(key)
            .and_then(|e| e.model.clone())
            .unwrap_or_default();
        self.custom_input.update(cx, |s, cx| {
            s.set_value(current, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    pub(super) fn submit_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.custom_target.take() else {
            return;
        };
        let answer = self.custom_input.read(cx).value().trim().to_string();
        self.custom_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        if answer.is_empty() {
            self.say("No custom model set.", Tone::Info);
        } else if let Some(model) = normalize_model_id(&answer) {
            self.set_model(&key, Some(model), cx);
        } else {
            self.say(
                "Custom model id must be one provider/model line using letters, numbers, '.', '-', '_', '~', ':', '@', '/', '+', '%' only.",
                Tone::Bad,
            );
        }
        cx.notify();
    }
}
