//! The Water reminder section: on or off, how often, and a preview.

use gentle_core::hydration::{INTERVALS, Reminder};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

use super::SettingsView;
use super::layout::section;
use crate::{features::hydration, shared::ui};

/// A labelled control row, like the Notifications switches.
fn control(label: &'static str, dim: bool, control: impl IntoElement) -> Div {
    h_flex()
        .pt_2()
        .gap_2()
        .child(
            div()
                .w(px(120.))
                .flex_shrink_0()
                .text_xs()
                .text_color(if dim { ui::faint() } else { ui::muted() })
                .child(label),
        )
        .child(control)
}

impl SettingsView {
    pub(super) fn hydration_section(&self, cx: &mut Context<Self>) -> Div {
        let reminder = hydration::reminder(cx);
        let set = |change: fn(Reminder, usize) -> Reminder| {
            move |_: &mut Self, index: &usize, _: &mut Window, cx: &mut Context<Self>| {
                hydration::set(change(hydration::reminder(cx), *index), cx);
                cx.notify();
            }
        };
        section(IconName::Droplets, "Water reminder")
            .child(control(
                "Reminder",
                false,
                ui::segmented(
                    "hydration-enabled",
                    &["Off", "On"],
                    Some(usize::from(reminder.enabled)),
                    cx.listener(set(|r, index| Reminder {
                        enabled: index == 1,
                        ..r
                    })),
                ),
            ))
            .child(control(
                "Every",
                !reminder.enabled,
                ui::segmented(
                    "hydration-every",
                    &hydration::LABELS,
                    INTERVALS.iter().position(|m| *m == reminder.every_minutes),
                    cx.listener(set(|r, index| Reminder {
                        every_minutes: INTERVALS[index],
                        ..r
                    })),
                ),
            ))
            .child(
                v_flex()
                    .pt_2()
                    .gap_2()
                    .child(div().text_xs().text_color(ui::faint()).child(
                        "Drops fall over the window and fill a glass. The clock starts when \
                             the app opens; changing how often keeps counting from then.",
                    ))
                    .child(
                        h_flex().child(
                            Button::new("hydration-preview")
                                .ghost()
                                .small()
                                .icon(Icon::new(IconName::Droplets))
                                .label("Show it now")
                                .on_click(|_, _, cx| hydration::show(cx)),
                        ),
                    ),
            )
    }
}
