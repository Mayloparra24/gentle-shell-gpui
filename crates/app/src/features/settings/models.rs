//! The Models section: default model and effort, scoped models, profiles,
//! sign-ins and custom providers.

use gpui_kit::assets::IconName;
use gpui_kit::*;

use super::layout::{or_dash, row, section};
use super::snapshot::Snapshot;

pub(super) fn models_section(s: &Snapshot) -> Div {
    section(IconName::Cpu, "Models")
        .child(row(
            "Default model",
            or_dash(&s.default_model, "first signed-in model"),
        ))
        .child(row(
            "Default effort",
            or_dash(&s.default_thinking, "medium"),
        ))
        .child(row(
            "Scoped models",
            if s.enabled_models.is_empty() {
                "all".into()
            } else {
                s.enabled_models.join(", ")
            },
        ))
        .child(row("Active profile", or_dash(&s.active_profile, "none")))
        .child(row(
            "Profiles",
            if s.profiles.is_empty() {
                "—".into()
            } else {
                s.profiles.join(", ")
            },
        ))
        .child(row(
            "Sign-ins",
            if s.sign_ins.is_empty() {
                "none".into()
            } else {
                s.sign_ins
                    .iter()
                    .map(|(p, t)| format!("{p} ({t})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ))
        .child(row(
            "Custom providers",
            if s.custom_providers.is_empty() {
                "—".into()
            } else {
                s.custom_providers.join(", ")
            },
        ))
}
