//! The App section: this app's own version and its updates from GitHub
//! releases.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::layout::{row, section};
use crate::{app_update, ui};

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
}

/// This app's own version and its updates from GitHub releases.
pub(super) fn app_section(cx: &App) -> Div {
    let status = app_update::status(cx).clone();
    let version = if app_update::is_dev(app_update::CURRENT) {
        format!("{} (development build)", app_update::CURRENT)
    } else {
        app_update::CURRENT.to_string()
    };
    let line = |icon: Option<IconName>, color: Hsla, text: String| {
        h_flex()
            .pt_2()
            .gap_2()
            .text_xs()
            .text_color(color)
            .children(icon.map(|icon| Icon::new(icon).xsmall().flex_shrink_0()))
            .child(div().flex_1().min_w_0().child(text))
    };
    let releases_button = |id: &'static str, url: String| {
        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(IconName::ExternalLink))
            .label("GitHub releases")
            .on_click(move |_, _, cx| cx.open_url(&url))
    };
    let state: AnyElement = match &status {
        app_update::Status::Idle if app_update::is_dev(app_update::CURRENT) => line(
            None,
            ui::faint(),
            "Development build: automatic update checks are off.".into(),
        )
        .into_any_element(),
        app_update::Status::Idle => div().into_any_element(),
        app_update::Status::Checking => h_flex()
            .pt_2()
            .gap_2()
            .text_xs()
            .text_color(ui::muted())
            .child(Spinner::new().xsmall().color(ui::accent()))
            .child("Checking GitHub for a newer release…")
            .into_any_element(),
        app_update::Status::UpToDate => line(
            Some(IconName::CircleCheck),
            ui::success(),
            "You're on the latest release.".into(),
        )
        .into_any_element(),
        app_update::Status::Skipped(version) => line(
            None,
            ui::muted(),
            format!("Skipping {version}. Check for updates to see it again."),
        )
        .into_any_element(),
        app_update::Status::Available(release)
        | app_update::Status::Failed {
            release: Some(release),
            ..
        } => {
            let problem = match &status {
                app_update::Status::Failed { problem, .. } => Some(problem.clone()),
                _ => None,
            };
            let notes = release.notes_url.clone();
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
                                .child(format!("Gentle Shell {} is available", release.version)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(format!("you have {}", app_update::CURRENT)),
                        ),
                )
                .child(div().text_xs().text_color(ui::muted()).child(
                    "Downloads the release for this computer, checks it against the published SHA-256 and replaces the app. The running app keeps working until you restart.",
                ))
                .when_some(problem.clone(), |d, problem| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(ui::danger())
                            .child(problem.message),
                    )
                })
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("app-update-install")
                                .primary()
                                .small()
                                .icon(Icon::new(IconName::ArrowDown))
                                .label(if problem.is_some() {
                                    "Try again"
                                } else {
                                    "Download & install"
                                })
                                .on_click(|_, _, cx| app_update::install(cx)),
                        )
                        .child(
                            Button::new("app-update-notes")
                                .ghost()
                                .small()
                                .icon(Icon::new(IconName::ExternalLink))
                                .label("Release notes")
                                .on_click(move |_, _, cx| cx.open_url(&notes)),
                        )
                        .when_some(problem.and_then(|p| p.link), |d, url| {
                            d.child(releases_button("app-update-releases", url))
                        })
                        .child(
                            Button::new("app-update-skip")
                                .ghost()
                                .small()
                                .label("Skip this version")
                                .on_click(|_, _, cx| app_update::skip(cx)),
                        ),
                )
                .into_any_element()
        }
        app_update::Status::Downloading {
            release,
            done,
            total,
        } => {
            let fraction = total
                .filter(|t| *t > 0)
                .map(|t| (*done as f32 / t as f32).clamp(0., 1.));
            v_flex()
                .mt_2()
                .p_3()
                .gap_2()
                .rounded_lg()
                .bg(ui::accent().opacity(0.10))
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(Spinner::new().xsmall().color(ui::accent()))
                        .child(
                            div()
                                .flex_1()
                                .text_color(ui::fg())
                                .child(format!("Downloading Gentle Shell {}…", release.version)),
                        )
                        .child(div().font_family(ui::MONO).text_color(ui::muted()).child(
                            match total {
                                Some(total) => format!("{} of {}", mb(*done), mb(*total)),
                                None => mb(*done),
                            },
                        )),
                )
                .when_some(fraction, |d, fraction| {
                    d.child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded_full()
                            .bg(ui::well())
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .rounded_full()
                                    .bg(ui::accent()),
                            ),
                    )
                })
                .into_any_element()
        }
        app_update::Status::Installing(release) => h_flex()
            .pt_2()
            .gap_2()
            .text_xs()
            .text_color(ui::muted())
            .child(Spinner::new().xsmall().color(ui::accent()))
            .child(format!(
                "Verified. Installing Gentle Shell {}…",
                release.version
            ))
            .into_any_element(),
        app_update::Status::Ready { release, .. } => v_flex()
            .mt_2()
            .p_3()
            .gap_2()
            .rounded_lg()
            .bg(ui::success().opacity(0.10))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(IconName::CircleCheck)
                            .small()
                            .text_color(ui::success()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui::fg())
                            .child(format!("Gentle Shell {} is installed", release.version)),
                    ),
            )
            .child(div().text_xs().text_color(ui::muted()).child(
                "Restart to use it. Workspaces are saved first and reopen as you left them.",
            ))
            .child(
                h_flex().child(
                    Button::new("app-update-restart")
                        .primary()
                        .small()
                        .icon(Icon::new(IconName::RotateCw))
                        .label("Restart now")
                        .on_click(|_, _, cx| app_update::restart(cx)),
                ),
            )
            .into_any_element(),
        app_update::Status::Failed {
            problem,
            release: None,
        } => v_flex()
            .child(line(
                Some(IconName::CircleAlert),
                ui::danger(),
                problem.message.clone(),
            ))
            .when_some(problem.link.clone(), |d, url| {
                d.child(
                    h_flex()
                        .pt_1()
                        .child(releases_button("app-update-releases", url)),
                )
            })
            .into_any_element(),
    };
    section(IconName::Info, "App")
        .child(row("Version", version))
        .child(state)
        .child(
            h_flex().pt_2().child(
                Button::new("app-update-check")
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::RefreshCw))
                    .label("Check for updates")
                    .loading(matches!(status, app_update::Status::Checking))
                    .disabled(status.busy())
                    .on_click(|_, _, cx| app_update::check(true, cx)),
            ),
        )
}
