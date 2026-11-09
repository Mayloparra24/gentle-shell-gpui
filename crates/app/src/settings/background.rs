//! The Gentle AI section, chiefly the background-subagents policy: resolved
//! as the shell does, and changed through the shell's own command.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;

use super::layout::{or_dash, row, section};
use super::snapshot::Snapshot;
use super::{SettingsEvent, SettingsView};
use crate::ui;

/// Which source decided the background-subagents policy, in Gentle Shell's
/// order: project file, then global file, then environment, then off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PolicySource {
    ProjectFile,
    GlobalFile,
    Environment,
    #[default]
    Default,
}

/// The effective background-subagents policy, resolved as
/// `lib/background-subagents-policy.ts` does so the app and the shell agree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackgroundPolicy {
    pub on: bool,
    pub source: PolicySource,
    /// The deciding file exists but fails the strict decode (reads as off).
    pub malformed: bool,
    pub project_file: PathBuf,
}

pub const BACKGROUND_SUBAGENTS_FILE: &str = "background-subagents.json";
const BACKGROUND_SUBAGENTS_SCHEMA: &str = "gentle-pi.background-subagents/v1";

/// Strict decode: exactly `{"schema": …/v1, "policy": "on"|"off"}`.
fn parse_policy_file(raw: &str) -> Option<bool> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if object.len() != 2 || object.get("schema")?.as_str()? != BACKGROUND_SUBAGENTS_SCHEMA {
        return None;
    }
    match object.get("policy")?.as_str()? {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// First hit wins; a present but malformed file fails closed to off and keeps
/// the decision (it does not fall through), exactly like the shell.
pub fn resolve_background_policy(cwd: &Path, config: &Path, env: Option<&str>) -> BackgroundPolicy {
    let project_file = cwd
        .join(".pi")
        .join("gentle-ai")
        .join(BACKGROUND_SUBAGENTS_FILE);
    let global_file = config.join(BACKGROUND_SUBAGENTS_FILE);
    for (source, path) in [
        (PolicySource::ProjectFile, &project_file),
        (PolicySource::GlobalFile, &global_file),
    ] {
        if !path.exists() {
            continue;
        }
        let decoded = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| parse_policy_file(&raw));
        return BackgroundPolicy {
            on: decoded == Some(true),
            source,
            malformed: decoded.is_none(),
            project_file: project_file.clone(),
        };
    }
    match env {
        Some(value @ ("on" | "off")) => BackgroundPolicy {
            on: value == "on",
            source: PolicySource::Environment,
            malformed: false,
            project_file,
        },
        _ => BackgroundPolicy {
            project_file,
            ..BackgroundPolicy::default()
        },
    }
}

impl BackgroundPolicy {
    /// `on · global file`, `off · default`, `off · project file (malformed)`.
    pub fn describe(&self) -> String {
        let source = match self.source {
            PolicySource::ProjectFile => "project file",
            PolicySource::GlobalFile => "global file",
            PolicySource::Environment => "GENTLE_PI_BACKGROUND_SUBAGENTS",
            PolicySource::Default => "default",
        };
        let state = if self.on { "on" } else { "off" };
        if self.malformed {
            format!("{state} · {source} (malformed)")
        } else {
            format!("{state} · {source}")
        }
    }
}

impl SettingsView {
    /// Ask the shell itself to change the policy, as typing
    /// `/gentle:background-subagents enable|disable` would: the shell owns the
    /// file and its validation, and posts its own status report.
    fn set_background(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.background_pending {
            return;
        }
        let verb = if on { "enable" } else { "disable" };
        self.background_pending = true;
        self.background_note = None;
        cx.emit(SettingsEvent::ShellCommand(format!(
            "/gentle:background-subagents {verb}"
        )));
        cx.notify();
    }

    /// The shell's answer to a [`SettingsEvent::ShellCommand`]; re-reads the
    /// files so the row shows what the shell actually wrote.
    pub fn shell_done(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.background_pending = false;
        self.background_note = Some(match result {
            Ok(()) => (
                "Gentle Shell applied it; its report is in 🔔. Helpers launched from now on use it."
                    .into(),
                false,
            ),
            Err(error) => (error, true),
        });
        self.reload(cx);
    }

    /// Gentle Shell's own settings: permissions, persona, background
    /// subagents (with its switch), banner, terminal theme and MCP servers.
    pub(super) fn gentle_ai_section(&self, s: &Snapshot, cx: &mut Context<Self>) -> Div {
        section(IconName::ShieldCheck, "Gentle AI")
            .child(row(
                "Permissions (new sessions)",
                or_dash(&s.permission_mode, "normal"),
            ))
            .child(row("Persona", or_dash(&s.persona, "gentleman")))
            .child(row(
                "Background subagents",
                s.background_subagents.describe(),
            ))
            .child(
                v_flex()
                    .pt_2()
                    .pb_1()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(ui::muted())
                                    .child("Send helpers to the background"),
                            )
                            .child(ui::segmented(
                                "background-subagents",
                                &["Off", "On"],
                                Some(usize::from(s.background_subagents.on)),
                                cx.listener(|this, index: &usize, _, cx| {
                                    this.set_background(*index == 1, cx)
                                }),
                            ))
                            .when(self.background_pending, |d| {
                                d.child(Spinner::new().xsmall())
                            }),
                    )
                    .child(
                        div().text_xs().text_color(ui::faint()).child(
                            "On: el Gentleman launches helpers in the background and keeps working; each answer arrives when it finishes. Applied by Gentle Shell (/gentle:background-subagents), so the terminal sees the same setting.",
                        ),
                    )
                    // The switch writes the global file; a project file wins over it.
                    .when(
                        s.background_subagents.source == PolicySource::ProjectFile,
                        |d| {
                            d.child(
                                div().text_xs().text_color(ui::warning()).child(
                                    format!(
                                        "{} decides for this project, so the switch (global) won't change it here.",
                                        s.background_subagents
                                            .project_file
                                            .display()
                                    ),
                                ),
                            )
                        },
                    )
                    .when_some(self.background_note.clone(), |d, (text, failed)| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(if failed {
                                    ui::danger()
                                } else {
                                    ui::success()
                                })
                                .child(text),
                        )
                    }),
            )
            .child(row(
                "Banner",
                or_dash(&s.banner, "pink · rose on · text on"),
            ))
            .child(row("Theme (terminal)", or_dash(&s.theme, "default")))
            .child(row(
                "MCP servers",
                if s.mcp_servers.is_empty() {
                    "—".into()
                } else {
                    s.mcp_servers.join(", ")
                },
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::{BACKGROUND_SUBAGENTS_FILE, PolicySource, resolve_background_policy};

    /// Same order and fail-closed rules as the shell's
    /// `resolveBackgroundSubagentsPolicy`, so the row never disagrees with it.
    #[test]
    fn background_policy_resolves_like_the_shell() {
        let root = std::env::temp_dir().join(format!("gs-bg-test-{}", std::process::id()));
        let (cwd, config) = (root.join("repo"), root.join("gentle-ai"));
        let project = cwd.join(".pi").join("gentle-ai");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        let on = r#"{"schema":"gentle-pi.background-subagents/v1","policy":"on"}"#;
        let off = r#"{"schema":"gentle-pi.background-subagents/v1","policy":"off"}"#;
        let resolve = |env| resolve_background_policy(&cwd, &config, env);

        // Nothing set: off by default; the environment applies next.
        assert_eq!(
            (resolve(None).on, resolve(None).source),
            (false, PolicySource::Default)
        );
        assert_eq!(
            (resolve(Some("on")).on, resolve(Some("on")).source),
            (true, PolicySource::Environment)
        );
        assert_eq!(
            resolve(Some("yes")).source,
            PolicySource::Default,
            "unknown env is inert"
        );

        // The global file beats the environment.
        std::fs::write(config.join(BACKGROUND_SUBAGENTS_FILE), on).unwrap();
        let global = resolve(Some("off"));
        assert_eq!((global.on, global.source), (true, PolicySource::GlobalFile));

        // A project file beats the global one.
        std::fs::write(project.join(BACKGROUND_SUBAGENTS_FILE), off).unwrap();
        let local = resolve(None);
        assert_eq!((local.on, local.source), (false, PolicySource::ProjectFile));

        // Malformed (an extra key) fails closed and keeps the decision there.
        std::fs::write(
            project.join(BACKGROUND_SUBAGENTS_FILE),
            r#"{"schema":"gentle-pi.background-subagents/v1","policy":"on","x":1}"#,
        )
        .unwrap();
        let broken = resolve(Some("on"));
        assert_eq!(
            (broken.on, broken.source, broken.malformed),
            (false, PolicySource::ProjectFile, true)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
