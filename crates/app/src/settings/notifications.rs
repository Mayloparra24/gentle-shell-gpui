//! The Notifications section: Gentle Shell's `notifications.json`, shared
//! with the terminal.

use gentle_core::notifications;
use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::SettingsView;
use super::layout::{row, section};
use crate::{alerts, ui};

/// Copy for each alert's switch.
fn alert_hint(alert: notifications::Alert) -> &'static str {
    match alert {
        notifications::Alert::TurnEnded => "When a turn finishes (agent.completed).",
        notifications::Alert::NeedsYou => "When el Gentleman asks you something (agent.attention).",
        notifications::Alert::ContextHigh => {
            "When the context reaches the threshold (context.high)."
        }
    }
}

/// One Off/On row of the Notifications section.
fn alert_switch(
    id: &'static str,
    label: &'static str,
    on: bool,
    hint: &'static str,
    dim: bool,
    change: impl Fn(bool) -> notifications::Change + 'static,
    cx: &mut Context<SettingsView>,
) -> Div {
    v_flex()
        .pt_2()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(if dim { ui::faint() } else { ui::muted() })
                        .child(label),
                )
                .child(ui::segmented(
                    id,
                    &["Off", "On"],
                    Some(usize::from(on)),
                    cx.listener(move |this, index: &usize, _, cx| {
                        this.set_notification(change(*index == 1), cx)
                    }),
                )),
        )
        .child(div().text_xs().text_color(ui::faint()).child(hint))
}

impl SettingsView {
    /// Write one change to Gentle Shell's notifications.json; the section
    /// re-reads the file on the next render.
    fn set_notification(&mut self, change: notifications::Change, cx: &mut Context<Self>) {
        self.notifications_note = alerts::apply(change)
            .err()
            .map(|e| format!("Could not write notifications.json: {e}"));
        cx.notify();
    }

    /// Gentle Shell's notification policy, shared with the terminal.
    pub(super) fn notifications_section(&self, cx: &mut Context<Self>) -> Div {
        let policy = alerts::policy();
        let enabled = policy.enabled();
        let mut card = section(IconName::Bell, "Notifications")
            .child(row("File", policy.file.display().to_string()))
            .child(row(
                "Context threshold",
                format!(
                    "{:.0} % (fixed by Gentle Shell)",
                    alerts::context_threshold(cx)
                ),
            ))
            .child(alert_switch(
                "alerts-enabled",
                "Sounds",
                enabled,
                "Master switch, off by default. Off also stops the Dock bounce / taskbar flash while the app is in the background.",
                false,
                notifications::Change::Enabled,
                cx,
            ));
        for alert in notifications::Alert::ALL {
            let id = match alert {
                notifications::Alert::TurnEnded => "alert-turn-ended",
                notifications::Alert::NeedsYou => "alert-needs-you",
                notifications::Alert::ContextHigh => "alert-context-high",
            };
            card = card.child(alert_switch(
                id,
                alert.label(),
                policy.sound(alert).is_some(),
                alert_hint(alert),
                !enabled,
                move |on| notifications::Change::Event(alert, on),
                cx,
            ));
        }
        let warning = match policy.state {
            notifications::FileState::Invalid => Some(
                "notifications.json isn't valid for Gentle Shell, so sounds are off. A change here replaces it with the default preset.",
            ),
            notifications::FileState::Unreadable => Some(
                "notifications.json can't be read, so sounds are off. A change here rewrites it with the default preset.",
            ),
            notifications::FileState::Missing | notifications::FileState::Valid => None,
        };
        card.child(
            div().pt_2().text_xs().text_color(ui::faint()).child(
                "Same file as /gentle:customize in the terminal. Gentle Shell plays nothing for app tabs (RPC), so the app plays the same tones itself; other events (failures, subagents) keep their terminal settings.",
            ),
        )
        .when_some(warning, |d, text| {
            d.child(div().text_xs().text_color(ui::warning()).child(text))
        })
        .when_some(self.notifications_note.clone(), |d, text| {
            d.child(div().text_xs().text_color(ui::danger()).child(text))
        })
    }
}
