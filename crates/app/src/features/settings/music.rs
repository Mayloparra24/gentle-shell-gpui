//! Music: gentle-stream, found or installed here. Install (uv, Python 3.13
//! and the player in the app's own folder), the sampled instruments, and
//! Remove. The work itself is `setup::music`; playing is the title bar's ♪.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::layout::{row, section};
use super::{SettingsEvent, SettingsView};
use crate::features::setup::music::{self, MusicStatus};
use crate::shared::ui;

/// Install output lines kept on screen.
const LOG_KEEP: usize = 8;

#[derive(Default)]
pub(super) struct Music {
    status: Option<MusicStatus>,
    job: Option<&'static str>,
    log: Vec<String>,
    note: Option<(String, bool)>,
    task: Option<Task<()>>,
}

type Work = fn(&(dyn Fn(String) + Send + Sync)) -> Result<(), String>;

impl SettingsView {
    pub(super) fn refresh_music(&mut self, cx: &mut Context<Self>) {
        self.music.task = Some(cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async { music::status() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.music.status = Some(status);
                cx.notify();
            });
        }));
    }

    fn run_music(&mut self, label: &'static str, work: Work, cx: &mut Context<Self>) {
        if self.music.job.is_some() {
            return;
        }
        self.music.job = Some(label);
        self.music.log.clear();
        self.music.note = None;
        let (tx, rx) = async_channel::unbounded::<String>();
        self.music.task = Some(cx.spawn(async move |this, cx| {
            let done = cx.background_executor().spawn(async move {
                let log = move |line: String| {
                    let _ = tx.send_blocking(line);
                };
                work(&log)
            });
            while let Ok(line) = rx.recv().await {
                let alive = this.update(cx, |this, cx| {
                    // Download progress lines replace each other.
                    if line.trim_start().ends_with(" MB")
                        && this.music.log.last().is_some_and(|l| l.ends_with(" MB"))
                    {
                        this.music.log.pop();
                    }
                    this.music.log.push(line);
                    let extra = this.music.log.len().saturating_sub(LOG_KEEP);
                    this.music.log.drain(..extra);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = done.await;
            let status = cx
                .background_executor()
                .spawn(async { music::status() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.music.job = None;
                this.music.status = Some(status);
                this.music.note = Some(match result {
                    Ok(()) => (format!("{label} done."), true),
                    Err(error) => (error, false),
                });
                // The title bar's ♪ appears or goes.
                cx.emit(SettingsEvent::MusicChanged);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn music_section(&self, cx: &mut Context<Self>) -> Div {
        let section = section(IconName::Music, "Music · gentle-stream").child(
            div().pb_2().text_xs().text_color(ui::muted()).child(
                "Endless music composed live while you work, steered by a mood you describe \
                 (\"rainy night in a small café\"). The same player as /gentle:music, from the \
                 ♪ button in the title bar. It installs into this app's own folder.",
            ),
        );
        if !music::SUPPORTED {
            return section.child(
                div()
                    .text_xs()
                    .text_color(ui::faint())
                    .child("gentle-stream runs on macOS and Linux (and WSL)."),
            );
        }
        let Some(status) = &self.music.status else {
            return section.child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(ui::muted())
                    .child(Spinner::new().xsmall().color(ui::accent()))
                    .child("Looking for gentle-stream…"),
            );
        };
        let busy = self.music.job.is_some();
        let (state, color) = if status.usable() {
            ("Ready: ♪ is in the title bar", ui::success())
        } else if status.player.is_some() {
            ("Found, but its Python environment is missing", ui::danger())
        } else {
            ("Not installed", ui::faint())
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
                "Player",
                status
                    .player
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| {
                        "Not installed (Python 3.13 and the player, about 150 MB)".into()
                    }),
            ))
            .child(row(
                "Instruments",
                if status.samples {
                    "Sampled (FluidR3_GM soundfont)"
                } else {
                    "Synthesized; sampled ones are an optional 120 MB download"
                },
            ))
            .child(
                h_flex()
                    .pt_3()
                    .gap_2()
                    .flex_wrap()
                    .when(!status.usable(), |d| {
                        d.child(
                            Button::new("music-install")
                                .small()
                                .primary()
                                .icon(Icon::new(IconName::ArrowDown))
                                .label("Install gentle-stream")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_music("Install", music::install, cx)
                                })),
                        )
                    })
                    .when(status.usable() && !status.samples, |d| {
                        d.child(
                            action("music-samples", IconName::ArrowDown, "Sampled instruments")
                                .tooltip("Download the FluidR3_GM soundfont (about 120 MB, once)")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_music(
                                        "Sampled instruments",
                                        music::install_samples,
                                        cx,
                                    )
                                })),
                        )
                    })
                    .when(status.app_owned, |d| {
                        d.child(
                            action("music-remove", IconName::Trash, "Remove")
                                .tooltip("Stop the music and delete what this app installed")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run_music("Remove", music::remove, cx)
                                })),
                        )
                    })
                    .child(
                        action("music-docs", IconName::ArrowUpRight, "Docs").on_click(
                            |_, _, cx| {
                                cx.open_url(
                                    "https://github.com/Gentleman-Programming/gentle-stream",
                                )
                            },
                        ),
                    )
                    .child(
                        action("music-refresh", IconName::RefreshCw, "Check again")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_music(cx))),
                    ),
            )
            .when_some(self.music.job, |d, job| {
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
            .when(!self.music.log.is_empty(), |d| {
                d.child(
                    ui::well_box().mt_2().px_3().py_2().gap_0p5().children(
                        self.music
                            .log
                            .iter()
                            .map(|line| div().text_color(ui::muted()).child(line.clone())),
                    ),
                )
            })
            .when_some(self.music.note.clone(), |d, (text, ok)| {
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
