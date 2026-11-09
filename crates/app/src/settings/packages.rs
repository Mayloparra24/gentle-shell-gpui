//! Packages: `gentle-shell install|remove`, Pi's own package manager
//! against the resolved home, and the recommended ones.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::claude::forget_claude_auth;
use super::layout::{row, section};
use super::snapshot::Snapshot;
use super::{SettingsEvent, SettingsView};
use crate::ui;

/// Recommended packages, with why. `pi-claude-bridge` is the provider Gentle
/// Shell's docs reference for Claude subscriptions (priced turns, usage headers).
pub const RECOMMENDED: &[(&str, &str)] = &[
    (
        "npm:pi-claude-bridge",
        "Claude Pro/Max through Claude Code (claude-bridge/… models)",
    ),
    (
        "npm:pi-claude-bridge-askclaude",
        "AskClaude tool: delegate to Claude Code from another provider",
    ),
];

impl SettingsView {
    /// `gentle-shell install|remove <source>`: Pi's own package manager
    /// against the resolved home.
    pub(super) fn run_package(
        &mut self,
        verb: &'static str,
        source: String,
        cx: &mut Context<Self>,
    ) {
        if self.running.is_some() {
            return;
        }
        self.running = Some(format!("{verb} {source}"));
        self.log.clear();
        let cwd = self.cwd.clone();
        self._job = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let spec = pi_rpc::resolve_backend(&cwd).map_err(|e| format!("{e:#}"))?;
                    let mut command = pi_rpc::command(&spec.program);
                    // Launcher options (home selectors) precede the subcommand.
                    command
                        .args(spec.args.iter().take_while(|a| *a != "--mode"))
                        .args([verb, &source])
                        .current_dir(&spec.cwd)
                        .stdin(std::process::Stdio::null());
                    for (k, v) in &spec.env {
                        command.env(k, v);
                    }
                    let out = command.output().map_err(|e| e.to_string())?;
                    let text = format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                    if out.status.success() {
                        Ok(text)
                    } else {
                        Err(text)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.running = None;
                forget_claude_auth();
                let (ok, text) = match result {
                    Ok(text) => (true, text),
                    Err(text) => (false, text),
                };
                this.log = text
                    .lines()
                    .rev()
                    .take(8)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if ok {
                    cx.emit(SettingsEvent::Changed);
                }
                this.reload(cx);
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Installed packages (remove each), the recommended ones not yet
    /// installed, and a field to install any source.
    pub(super) fn packages_section(&self, s: &Snapshot, cx: &mut Context<Self>) -> Div {
        section(IconName::Layers, "Packages")
            .children(s.packages.iter().enumerate().map(|(i, p)| {
                let source = p.source.clone();
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .hover(|d| d.bg(ui::surface_hover()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_family(ui::MONO)
                            .text_color(ui::fg())
                            .child(p.source.clone()),
                    )
                    .when_some(p.version.clone(), |d, v| {
                        d.child(ui::chip(v, ui::info()).flex_shrink_0())
                    })
                    .child(
                        Button::new(("remove-package", i))
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Trash))
                            .tooltip("gentle-shell remove")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_package("remove", source.clone(), cx)
                            })),
                    )
            }))
            .when(!s.extensions.is_empty(), |d| {
                d.child(row("Extension filters", s.extensions.join(", ")))
            })
            .when(
                RECOMMENDED.iter().any(|(src, _)| !s.has_package(src)),
                |d| d.child(div().pt_3().pb_1().child(ui::section_label("Recommended"))),
            )
            .children(
                RECOMMENDED
                    .iter()
                    .filter(|(src, _)| !s.has_package(src))
                    .map(|(src, why)| {
                        h_flex()
                            .gap_3()
                            .px_2()
                            .py_1p5()
                            .rounded_lg()
                            .bg(ui::accent().opacity(0.05))
                            .border_1()
                            .border_color(ui::accent().opacity(0.15))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .font_family(ui::MONO)
                                            .text_color(ui::accent())
                                            .child(*src),
                                    )
                                    .child(div().text_xs().text_color(ui::muted()).child(*why)),
                            )
                            .child(
                                Button::new(SharedString::from(format!("install-{src}")))
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Plus))
                                    .label("Install")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.run_package("install", src.to_string(), cx)
                                    })),
                            )
                    }),
            )
            .child(
                h_flex()
                    .pt_3()
                    .gap_2()
                    .child(
                        ui::field()
                            .flex_1()
                            .min_w_0()
                            .child(Icon::new(IconName::Plus).small().text_color(ui::faint()))
                            .child(Input::new(&self.package_input).appearance(false).small()),
                    )
                    .child(
                        Button::new("install-package")
                            .primary()
                            .icon(Icon::new(IconName::ArrowDown))
                            .label("Install")
                            .disabled(self.running.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                let source = this.package_input.read(cx).value().trim().to_string();
                                if !source.is_empty() {
                                    this.package_input
                                        .update(cx, |s, cx| s.set_value("", window, cx));
                                    this.run_package("install", source, cx);
                                }
                            })),
                    ),
            )
            .when_some(self.running.clone(), |d, job| {
                d.child(
                    h_flex()
                        .pt_2()
                        .gap_2()
                        .text_xs()
                        .text_color(ui::muted())
                        .child(Spinner::new().xsmall().color(ui::accent()))
                        .child(format!("gentle-shell {job}…")),
                )
            })
            .when(!self.log.is_empty(), |d| {
                d.child(
                    ui::well_box().mt_2().px_3().py_2().gap_0p5().children(
                        self.log
                            .iter()
                            .map(|l| div().text_color(ui::muted()).child(l.clone())),
                    ),
                )
            })
    }
}
