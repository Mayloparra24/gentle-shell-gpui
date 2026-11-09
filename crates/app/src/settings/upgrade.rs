//! Gentle Shell's own updates: the "is available" card, `gentle-shell
//! upgrade`, and how it ended as the app sees it afterwards.

use std::path::Path;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::claude::forget_claude_auth;
use super::{SettingsEvent, SettingsView};
use crate::ui;

/// How `gentle-shell upgrade` ended, as the app sees it afterwards.
#[derive(Clone, Debug, PartialEq)]
enum UpgradeOutcome {
    /// The launcher the app resolves now runs this version.
    Installed(String),
    /// The upgrade ran, but the launcher the app resolves still runs
    /// `running` (another install comes first).
    Stale {
        running: String,
        launcher: String,
    },
    Failed,
}

#[derive(Clone)]
pub(super) struct UpgradeResult {
    target: String,
    outcome: UpgradeOutcome,
    /// Gentle Shell's last output lines.
    lines: Vec<String>,
}

/// Decide the outcome from the upgrade's exit and what the launcher the app
/// now resolves (`launcher`) reports as its version (`running`).
fn upgrade_outcome(
    succeeded: bool,
    target: &str,
    running: Option<&str>,
    launcher: Option<&Path>,
) -> UpgradeOutcome {
    match (succeeded, running) {
        (false, _) | (true, None) => UpgradeOutcome::Failed,
        (true, Some(running)) if running == target => {
            UpgradeOutcome::Installed(running.to_string())
        }
        (true, Some(running)) => UpgradeOutcome::Stale {
            running: running.to_string(),
            launcher: launcher.map_or_else(
                || "gentle-shell".to_string(),
                |path| path.display().to_string(),
            ),
        },
    }
}

impl SettingsView {
    /// Gentle Shell's own `upgrade` toward `target` (it knows how it was
    /// installed; since 4.7 it moves to its self-contained install), then ask
    /// the launcher the app resolves now which version it runs: that's the
    /// result shown.
    fn run_upgrade(&mut self, target: String, cx: &mut Context<Self>) {
        if self.upgrading {
            return;
        }
        self.upgrading = true;
        self.upgrade = None;
        let cwd = self.cwd.clone();
        self._job = cx.spawn(async move |this, cx| {
            let (succeeded, text, running, launcher) = cx
                .background_executor()
                .spawn(async move {
                    let spec = match pi_rpc::resolve_backend(&cwd) {
                        Ok(spec) => spec,
                        Err(error) => return (false, format!("{error:#}"), None, None),
                    };
                    let mut command = pi_rpc::command(&spec.program);
                    command
                        .args(spec.args.iter().take_while(|a| *a != "--mode"))
                        .arg("upgrade")
                        .current_dir(&spec.cwd)
                        .stdin(std::process::Stdio::null());
                    for (k, v) in &spec.env {
                        command.env(k, v);
                    }
                    let (succeeded, text) = match command.output() {
                        Ok(out) => (
                            out.status.success(),
                            format!(
                                "{}{}",
                                String::from_utf8_lossy(&out.stdout),
                                String::from_utf8_lossy(&out.stderr)
                            ),
                        ),
                        Err(error) => (false, error.to_string()),
                    };
                    // The upgrade may have installed Gentle Shell somewhere
                    // new (its self-contained install): resolve it afresh.
                    pi_rpc::forget_cached();
                    let now = pi_rpc::resolve_backend(&cwd).ok();
                    let running = now.as_ref().and_then(pi_rpc::backend_version);
                    (succeeded, text, running, now.map(|spec| spec.program))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.upgrading = false;
                forget_claude_auth();
                let lines: Vec<String> = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(str::to_string)
                    .collect();
                let outcome =
                    upgrade_outcome(succeeded, &target, running.as_deref(), launcher.as_deref());
                if matches!(outcome, UpgradeOutcome::Installed(_)) {
                    cx.emit(SettingsEvent::BackendUpdated);
                }
                this.upgrade = Some(UpgradeResult {
                    target,
                    outcome,
                    lines: lines[lines.len().saturating_sub(8)..].to_vec(),
                });
                this.reload(cx);
                cx.notify();
            });
        });
        cx.notify();
    }

    fn skip_update(&mut self, version: String, cx: &mut Context<Self>) {
        if let Err(error) = crate::updates::skip(&version) {
            self.skip_error = Some(format!("Could not remember the skip: {error}"));
        }
        self.reload(cx);
    }

    /// "Gentle Shell X is available": update, release notes or skip, as the
    /// terminal's startup prompt offers.
    pub(super) fn render_update(
        &self,
        offer: crate::updates::UpdateOffer,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let notes = offer.notes_url.clone();
        let skipped = offer.latest.clone();
        let target = offer.latest.clone();
        v_flex()
            .mt_2()
            .p_3()
            .gap_2()
            .rounded_lg()
            .bg(ui::accent().opacity(0.10))
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::ArrowUp).small().text_color(ui::accent()))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui::fg())
                            .child(format!("Gentle Shell {} is available", offer.latest)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui::muted())
                            .child(format!("you have {}", offer.current)),
                    ),
            )
            .child(div().text_xs().text_color(ui::muted()).child(
                "Runs gentle-shell upgrade. Open agent tabs then restart on the new version and keep their conversations; a tab mid-turn switches when the turn ends.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("update-now")
                            .primary()
                            .small()
                            .label(if self.upgrading { "Updating…" } else { "Update now" })
                            .loading(self.upgrading)
                            .disabled(self.upgrading)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_upgrade(target.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new("update-notes")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::ExternalLink))
                            .label("Release notes")
                            .on_click(move |_, _, cx| cx.open_url(&notes)),
                    )
                    .child(
                        Button::new("update-skip")
                            .ghost()
                            .small()
                            .label("Skip this version")
                            .disabled(self.upgrading)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.skip_update(skipped.clone(), cx)
                            })),
                    ),
            )
            .children(
                self.skip_error
                    .clone()
                    .map(|error| upgrade_output(vec![error], true)),
            )
    }
}

/// The upgrade's result: what happened and what's left to do, then
/// Gentle Shell's own last lines.
pub(super) fn upgrade_result(result: &UpgradeResult) -> Div {
    let (icon, color, title, line) = match &result.outcome {
        UpgradeOutcome::Installed(version) => (
            IconName::CircleCheck,
            ui::success(),
            format!("Gentle Shell {version} is installed"),
            "Open agent tabs restarted on it and kept their conversations. Nothing else to do."
                .to_string(),
        ),
        UpgradeOutcome::Stale { running, launcher } => (
            IconName::TriangleAlert,
            ui::warning(),
            format!(
                "Gentle Shell {} installed, but the app still runs {running}",
                result.target
            ),
            format!(
                "{launcher} comes first. Remove the old package with `npm uninstall -g gentle-pi`, then open Configuration again."
            ),
        ),
        UpgradeOutcome::Failed => (
            IconName::CircleX,
            ui::danger(),
            "The update didn't finish".to_string(),
            "Gentle Shell's output is below; the version you had keeps running.".to_string(),
        ),
    };
    let failed = result.outcome == UpgradeOutcome::Failed;
    v_flex()
        .mt_2()
        .p_3()
        .gap_1()
        .rounded_lg()
        .bg(color.opacity(0.10))
        .child(
            h_flex()
                .gap_2()
                .child(Icon::new(icon).small().text_color(color))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui::fg())
                        .child(title),
                ),
        )
        .child(div().text_xs().text_color(ui::muted()).child(line))
        .when(!result.lines.is_empty(), |d| {
            d.child(upgrade_output(result.lines.clone(), failed))
        })
}

/// The last lines `gentle-shell upgrade` printed, tinted by outcome.
fn upgrade_output(lines: Vec<String>, failed: bool) -> Div {
    v_flex()
        .mt_1()
        .p_2()
        .rounded_md()
        .bg(ui::well())
        .font_family(ui::MONO)
        .text_xs()
        .text_color(if failed { ui::danger() } else { ui::muted() })
        .children(lines.into_iter().map(|line| div().child(line)))
}

#[cfg(test)]
mod tests {
    use super::{UpgradeOutcome, upgrade_outcome};

    #[test]
    fn upgrade_result_follows_what_the_app_runs_afterwards() {
        use std::path::Path;
        let old = Path::new("/fnm/bin/gentle-shell");
        // The app now runs the new version.
        assert_eq!(
            upgrade_outcome(true, "4.7.0", Some("4.7.0"), Some(old)),
            UpgradeOutcome::Installed("4.7.0".into())
        );
        // It installed, but an older launcher still comes first.
        assert_eq!(
            upgrade_outcome(true, "4.7.0", Some("4.6.0"), Some(old)),
            UpgradeOutcome::Stale {
                running: "4.6.0".into(),
                launcher: "/fnm/bin/gentle-shell".into()
            }
        );
        // A failed run, or no launcher answering afterwards, is a failure.
        assert_eq!(
            upgrade_outcome(false, "4.7.0", Some("4.7.0"), Some(old)),
            UpgradeOutcome::Failed
        );
        assert_eq!(
            upgrade_outcome(true, "4.7.0", None, None),
            UpgradeOutcome::Failed
        );
    }
}
