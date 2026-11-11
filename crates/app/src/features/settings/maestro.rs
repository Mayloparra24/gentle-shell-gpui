//! UI testing: Maestro, set up and managed from one place. Install (Java when
//! missing, the CLI, the agent's tools), test the connection, disconnect, or
//! remove what the app installed. The work itself is `setup::maestro`.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::layout::{row, section};
use super::{SettingsEvent, SettingsView};
use crate::features::setup::maestro::{self, MaestroStatus};
use crate::shared::ui;

/// Install output lines kept on screen.
const LOG_KEEP: usize = 8;

/// The section's own state.
#[derive(Default)]
pub(super) struct Maestro {
    status: Option<MaestroStatus>,
    /// What is running ("Installing Maestro…").
    job: Option<&'static str>,
    log: Vec<String>,
    /// The last test or job result: text and whether it went well.
    note: Option<(String, bool)>,
    task: Option<Task<()>>,
}

/// What a job does on the background executor.
type Work = fn(&(dyn Fn(String) + Send + Sync)) -> Result<(), String>;

impl SettingsView {
    /// Look at the machine again (CLI, Java, the agent's `mcp.json`).
    pub(super) fn refresh_maestro(&mut self, cx: &mut Context<Self>) {
        self.maestro.task = Some(cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { maestro::status() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.maestro.status = Some(status);
                cx.notify();
            });
        }));
    }

    /// Run `work`, streaming its output; `changes_agent` restarts the tabs'
    /// backends afterwards (when idle), so they pick the tools up or drop them.
    fn run_maestro(
        &mut self,
        label: &'static str,
        work: Work,
        changes_agent: bool,
        cx: &mut Context<Self>,
    ) {
        if self.maestro.job.is_some() {
            return;
        }
        self.maestro.job = Some(label);
        self.maestro.log.clear();
        self.maestro.note = None;
        let (tx, rx) = async_channel::unbounded::<String>();
        self.maestro.task = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move {
                let log = move |line: String| {
                    let _ = tx.send_blocking(line);
                };
                // The sender drops with `log` when this returns, ending the
                // stream below.
                work(&log)
            });
            while let Ok(line) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    // Download progress lines replace each other.
                    if line.trim_start().ends_with(" MB")
                        && this.maestro.log.last().is_some_and(|l| l.ends_with(" MB"))
                    {
                        this.maestro.log.pop();
                    }
                    this.maestro.log.push(line);
                    let extra = this.maestro.log.len().saturating_sub(LOG_KEEP);
                    this.maestro.log.drain(..extra);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = done.await;
            let status = cx
                .background_executor()
                .spawn(async { maestro::status() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.maestro.job = None;
                this.maestro.status = Some(status);
                this.maestro.note = Some(match result {
                    Ok(()) => (format!("{label} done."), true),
                    Err(error) => (error, false),
                });
                if changes_agent {
                    cx.emit(SettingsEvent::Changed);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn test_maestro(&mut self, cx: &mut Context<Self>) {
        if self.maestro.job.is_some() {
            return;
        }
        self.maestro.job = Some("Testing the connection");
        self.maestro.note = None;
        self.maestro.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { maestro::test_connection() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.maestro.job = None;
                this.maestro.note = Some(match result {
                    Ok(line) => {
                        let ok = line.contains("connected");
                        (line, ok)
                    }
                    Err(error) => (error, false),
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn maestro_section(&self, cx: &mut Context<Self>) -> Div {
        let section = section(IconName::Smartphone, "UI testing · Maestro").child(
            div().pb_2().text_xs().text_color(ui::muted()).child(
                "Lets the agent tap, type, swipe, check and take screenshots on Android \
                     emulators, iOS simulators and web pages (Chromium, beta), and run Maestro \
                     flows. Everything installs into this app's own folder, without your password.",
            ),
        );
        let Some(status) = &self.maestro.status else {
            return section.child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(ui::muted())
                    .child(Spinner::new().xsmall().color(ui::accent()))
                    .child("Looking for Maestro and Java…"),
            );
        };
        let busy = self.maestro.job.is_some();
        let (state, color) = match (status.cli.is_some(), status.connected) {
            (true, true) => ("Ready: the agent has the maestro tools", ui::success()),
            (true, false) => ("Installed, not connected to the agent", ui::warning()),
            (false, true) => ("Connected, but the CLI is missing", ui::danger()),
            (false, false) => ("Not installed", ui::faint()),
        };
        let action = |id: &'static str, icon: IconName, label: &'static str| {
            Button::new(id)
                .small()
                .ghost()
                .icon(Icon::new(icon))
                .label(label)
                .disabled(busy)
        };
        section
            .child(h_flex().pb_2().child(ui::chip(state, color)))
            .child(row(
                "Maestro CLI",
                status
                    .cli
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Not installed (about 350 MB)".into()),
            ))
            .child(row(
                "Java",
                status
                    .java
                    .as_ref()
                    .map(|(home, major)| format!("Java {major} · {}", home.display()))
                    .unwrap_or_else(|| {
                        "Not found: installs Temurin 21 (about 45 MB) with Maestro".into()
                    }),
            ))
            .child(row(
                "Agent tools",
                if status.connected {
                    "Connected (open tabs reload when idle)"
                } else {
                    "Not connected"
                },
            ))
            .child(
                h_flex()
                    .pt_3()
                    .gap_2()
                    .flex_wrap()
                    .when(!status.connected || status.cli.is_none(), |d| {
                        d.child(
                            Button::new("maestro-install")
                                .small()
                                .primary()
                                .icon(Icon::new(IconName::ArrowDown))
                                .label(if status.cli.is_some() {
                                    "Connect to the agent"
                                } else {
                                    "Install Maestro"
                                })
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_maestro("Install", maestro::install, true, cx)
                                })),
                        )
                    })
                    .when(status.connected, |d| {
                        d.child(
                            action("maestro-test", IconName::CircleCheck, "Test connection")
                                .on_click(cx.listener(|this, _, _, cx| this.test_maestro(cx))),
                        )
                        .child(
                            action("maestro-disconnect", IconName::Unlink, "Disconnect").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.run_maestro("Disconnect", maestro::disconnect, true, cx)
                                }),
                            ),
                        )
                    })
                    .when(status.app_owned, |d| {
                        d.child(
                            action("maestro-remove", IconName::Trash, "Remove")
                                .tooltip(
                                    "Disconnect and delete the Maestro and Java this app installed",
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_maestro(
                                        "Remove",
                                        maestro::remove,
                                        status_connected(this),
                                        cx,
                                    )
                                })),
                        )
                    })
                    .child(
                        action("maestro-docs", IconName::ArrowUpRight, "Docs")
                            .on_click(|_, _, cx| cx.open_url("https://docs.maestro.dev")),
                    )
                    .child(
                        action("maestro-refresh", IconName::RefreshCw, "Check again")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_maestro(cx))),
                    ),
            )
            .when_some(self.maestro.job, |d, job| {
                d.child(
                    h_flex()
                        .pt_2()
                        .gap_2()
                        .text_xs()
                        .text_color(ui::muted())
                        .child(Spinner::new().xsmall().color(ui::accent()))
                        .child(format!("{job}…")),
                )
            })
            .when(!self.maestro.log.is_empty(), |d| {
                d.child(
                    ui::well_box().mt_2().px_3().py_2().gap_0p5().children(
                        self.maestro
                            .log
                            .iter()
                            .map(|line| div().text_color(ui::muted()).child(line.clone())),
                    ),
                )
            })
            .when_some(self.maestro.note.clone(), |d, (text, ok)| {
                d.child(
                    div()
                        .pt_2()
                        .text_xs()
                        .text_color(if ok { ui::success() } else { ui::danger() })
                        .child(text),
                )
            })
    }
}

fn status_connected(view: &SettingsView) -> bool {
    view.maestro
        .status
        .as_ref()
        .is_some_and(|status| status.connected)
}
