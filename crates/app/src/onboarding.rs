//! Onboarding: the first-run screen (and "Setup & requirements" any time).
//! It shows what this machine has for Gentle Shell (`crate::setup` checks),
//! installs what can be installed unattended with a live log, hands anything
//! that needs a password or an interactive login to a terminal tab, and sends
//! provider sign-in to the Providers screen.

use std::collections::VecDeque;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::setup::{self, Check, CheckId, Fix, Need, Status};
use crate::ui;

/// Install output lines a running row shows.
const LOG_LINES: usize = 8;
/// Install output kept per run (the failure view shows the tail).
const LOG_KEEP: usize = 200;

/// One look at the machine: what the screen shows and whether it must open.
pub struct Probe {
    pub platform: String,
    pub first_run: bool,
    pub checks: Vec<Check>,
}

impl Probe {
    /// BLOCKING: runs every check; call it on the background executor.
    pub fn run() -> Self {
        Self {
            platform: setup::platform_label(),
            first_run: setup::first_run(),
            checks: setup::run_checks(),
        }
    }

    /// Onboarding opens by itself on a first run or when something required
    /// is missing.
    pub fn needs_onboarding(&self) -> bool {
        self.first_run || !required_ok(&self.checks)
    }
}

fn is_ok(check: &Check) -> bool {
    matches!(check.status, Status::Ok(_))
}

/// Every Required check passes.
fn required_ok(checks: &[Check]) -> bool {
    checks
        .iter()
        .filter(|c| c.need == Need::Required)
        .all(is_ok)
}

/// Everything but the optional extras passes.
fn all_set(checks: &[Check]) -> bool {
    checks
        .iter()
        .filter(|c| c.need != Need::Optional)
        .all(is_ok)
}

/// gentle-shell can start: the launcher and what it runs on are in place
/// (setup and sign-in can still be pending; the launcher handles those).
fn usable(checks: &[Check]) -> bool {
    checks
        .iter()
        .filter(|c| matches!(c.id, CheckId::Node | CheckId::Go | CheckId::GentleShell))
        .all(is_ok)
}

/// What "Install everything" runs, in screen order: every Required or
/// Recommended check an unattended install fixes that isn't done.
fn install_plan(checks: &[Check]) -> VecDeque<CheckId> {
    checks
        .iter()
        .filter(|c| c.need != Need::Optional && c.fix == Fix::Auto && !is_ok(c))
        .map(|c| c.id)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Good,
    Warn,
    Bad,
}

/// Green when done; red for a missing requirement or a failure; amber for
/// anything outdated or missing that isn't required.
fn tone(check: &Check) -> Tone {
    match &check.status {
        Status::Ok(_) => Tone::Good,
        Status::Failed(_) => Tone::Bad,
        Status::Missing(_) if check.need == Need::Required => Tone::Bad,
        Status::Missing(_) | Status::Outdated(_) => Tone::Warn,
    }
}

fn status_text(status: &Status) -> &str {
    match status {
        Status::Ok(s) | Status::Missing(s) | Status::Outdated(s) | Status::Failed(s) => s,
    }
}

/// Append install output, one entry per non-empty line, keeping the tail.
fn push_lines(log: &mut Vec<String>, chunk: &str) {
    log.extend(
        chunk
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string),
    );
    if log.len() > LOG_KEEP {
        log.drain(..log.len() - LOG_KEEP);
    }
}

#[derive(Clone, Debug)]
pub enum OnboardingEvent {
    /// Continue (after recording completion) or Skip.
    Close,
    /// An install finished; tabs whose backend failed may start now.
    Installed,
    /// Open the Providers screen.
    SignIn,
    /// Open a terminal tab with this command typed (needs the user: a
    /// password, an interactive login).
    Terminal(String),
}

pub struct OnboardingView {
    /// Shown inside Configuration: the checklist and its actions, without
    /// the screen's hero, footer and frame.
    embedded: bool,
    platform: Option<String>,
    first_run: bool,
    /// `None` until the first look finishes.
    checks: Option<Vec<Check>>,
    checking: bool,
    installing: Option<CheckId>,
    /// "Install everything": what still runs after the current install.
    queue: VecDeque<CheckId>,
    /// Output of the last install, and the check it belongs to.
    log: Vec<String>,
    log_for: Option<CheckId>,
    failure: Option<(CheckId, String)>,
    /// Emit [`OnboardingEvent::Installed`] once the checks after an install land.
    announce: bool,
    _probe: Task<()>,
    _job: Task<()>,
}

impl EventEmitter<OnboardingEvent> for OnboardingView {}

impl OnboardingView {
    /// `probe` is a look already taken (the startup check); `None` looks now.
    pub fn new(probe: Option<Probe>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            embedded: false,
            platform: None,
            first_run: false,
            checks: None,
            checking: false,
            installing: None,
            queue: VecDeque::new(),
            log: Vec::new(),
            log_for: None,
            failure: None,
            announce: false,
            _probe: Task::ready(()),
            _job: Task::ready(()),
        };
        match probe {
            Some(probe) => this.apply(probe, cx),
            None => this.refresh(cx),
        }
        this
    }

    /// The checklist for Configuration → Setup & requirements: looks at the
    /// machine now, installs and hands off actions like the screen does.
    pub fn embedded(cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(None, cx);
        this.embedded = true;
        this
    }

    /// An install is running: the screen must stay open until it ends.
    pub fn busy(&self) -> bool {
        self.installing.is_some()
    }

    /// gentle-shell can start, as of the last look.
    pub fn usable(&self) -> bool {
        self.checks.as_deref().is_some_and(usable)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.checking = true;
        self._probe = cx.spawn(async move |this, cx| {
            let probe = cx.background_executor().spawn(async { Probe::run() }).await;
            let _ = this.update(cx, |this, cx| this.apply(probe, cx));
        });
        cx.notify();
    }

    fn apply(&mut self, probe: Probe, cx: &mut Context<Self>) {
        self.checking = false;
        self.platform = Some(probe.platform);
        self.first_run = probe.first_run;
        self.checks = Some(probe.checks);
        if std::mem::take(&mut self.announce) {
            cx.emit(OnboardingEvent::Installed);
        }
        self.next_queued(cx);
        cx.notify();
    }

    /// Start the next queued install that is still needed.
    fn next_queued(&mut self, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        while let Some(id) = self.queue.pop_front() {
            let needed = self.checks.as_deref().is_some_and(|checks| {
                checks
                    .iter()
                    .any(|c| c.id == id && c.fix == Fix::Auto && !is_ok(c))
            });
            if needed {
                self.install(id, cx);
                return;
            }
        }
    }

    fn install_all(&mut self, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        let Some(checks) = &self.checks else { return };
        self.queue = install_plan(checks);
        self.failure = None;
        self.next_queued(cx);
    }

    /// Run `setup::install` on the background executor, streaming its output
    /// into the row, then look at the machine again.
    fn install(&mut self, id: CheckId, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        self.installing = Some(id);
        self.log_for = Some(id);
        self.log.clear();
        self.failure = None;
        let (tx, rx) = async_channel::unbounded::<String>();
        self._job = cx.spawn(async move |this, cx| {
            let work = cx.background_executor().spawn(async move {
                let log = move |line: String| {
                    let _ = tx.send_blocking(line);
                };
                // `log` (and the sender) drops when this returns, which ends
                // the stream below.
                setup::install(id, &log)
            });
            while let Ok(first) = rx.recv().await {
                let mut chunk = first;
                while let Ok(more) = rx.try_recv() {
                    chunk.push('\n');
                    chunk.push_str(&more);
                }
                let alive = this.update(cx, |this, cx| {
                    push_lines(&mut this.log, &chunk);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let result = work.await;
            let _ = this.update(cx, |this, cx| this.finished(id, result, cx));
        });
        cx.notify();
    }

    fn finished(&mut self, id: CheckId, result: Result<(), String>, cx: &mut Context<Self>) {
        self.installing = None;
        match result {
            Ok(()) => self.announce = true,
            Err(message) => {
                self.failure = Some((id, message));
                self.queue.clear();
            }
        }
        self.refresh(cx);
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        setup::mark_done();
        cx.emit(OnboardingEvent::Close);
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    fn render_hero(&self) -> Div {
        let checks = self.checks.as_deref();
        let done = checks.is_some_and(all_set);
        let (title, line) = match checks {
            None => (
                "Welcome to Gentle Shell",
                "Looking at what this machine already has…",
            ),
            Some(checks) if !required_ok(checks) => (
                "Let's get you set up",
                "A few things Gentle Shell needs are missing. Install them here; anything \
                 that needs your password opens in a terminal for you to run.",
            ),
            Some(_) if done => (
                "You're all set",
                "Everything Gentle Shell needs is here. Optional extras are below.",
            ),
            Some(_) if self.first_run => (
                "Welcome to Gentle Shell",
                "Everything required is ready. The recommended extras below make it better.",
            ),
            Some(_) => (
                "Setup & requirements",
                "Everything required is ready. The recommended extras below make it better.",
            ),
        };
        let rose_size = if done { 88. } else { 56. };
        let mark = div().flex().justify_center().child(rose(
            rose_size,
            if done { ui::success() } else { ui::petal() },
        ));
        v_flex()
            .items_center()
            .gap_2()
            .px_8()
            .pt_8()
            .pb_4()
            .child(if done {
                // A new id when the state flips, so the rose blooms again.
                ui::pop_in("onboarding-rose-done", mark).into_any_element()
            } else {
                ui::pop_in("onboarding-rose", mark).into_any_element()
            })
            .child(
                div()
                    .pt_2()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::fg())
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(520.))
                    .text_sm()
                    .text_center()
                    .text_color(ui::muted())
                    .child(line),
            )
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .when_some(self.platform.clone(), |d, platform| {
                        d.child(ui::chip(platform, ui::petal()))
                    })
                    .when(self.checking && self.checks.is_some(), |d| {
                        d.child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(ui::faint())
                                .child(Spinner::new().xsmall().color(ui::faint()))
                                .child("Checking again…"),
                        )
                    }),
            )
    }

    fn render_row(&self, check: &Check, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.installing.is_some();
        let mine = self.installing == Some(check.id);
        let tone = tone(check);
        let color = match tone {
            Tone::Good => ui::success(),
            Tone::Warn => ui::warning(),
            Tone::Bad => ui::danger(),
        };
        let icon = if mine {
            Spinner::new().small().color(ui::petal()).into_any_element()
        } else {
            Icon::new(match tone {
                Tone::Good => IconName::CircleCheck,
                Tone::Warn => IconName::TriangleAlert,
                Tone::Bad => IconName::CircleX,
            })
            .small()
            .text_color(color)
            .into_any_element()
        };
        let label = match (&check.status, mine) {
            (_, true) => "Installing",
            (Status::Ok(_), _) => "Ready",
            (Status::Missing(_), _) => "Missing",
            (Status::Outdated(_), _) => "Outdated",
            (Status::Failed(_), _) => "Failed",
        };
        let id = check.id;
        let action = (!is_ok(check))
            .then(|| match &check.fix {
                Fix::Auto => {
                    let button = Button::new(("onboarding-install", index))
                        .small()
                        .icon(Icon::new(match check.status {
                            Status::Failed(_) => IconName::RefreshCw,
                            _ => IconName::ArrowDown,
                        }))
                        .label(match check.status {
                            Status::Failed(_) => "Retry",
                            Status::Outdated(_) => "Update",
                            _ => "Install",
                        })
                        .loading(mine)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| this.install(id, cx)));
                    Some(if check.need == Need::Required {
                        button.primary()
                    } else {
                        button.ghost()
                    })
                }
                Fix::Terminal(command) => {
                    let command = command.clone();
                    Some(
                        Button::new(("onboarding-terminal", index))
                            .small()
                            .ghost()
                            .icon(Icon::new(IconName::SquareTerminal))
                            .label("Open terminal")
                            .tooltip(command.clone())
                            .disabled(busy)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(OnboardingEvent::Terminal(command.clone()))
                            })),
                    )
                }
                Fix::SignIn => Some(
                    Button::new(("onboarding-sign-in", index))
                        .small()
                        .primary()
                        .icon(Icon::new(IconName::KeyRound))
                        .label("Sign in")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::SignIn))),
                ),
                Fix::None => None,
            })
            .flatten();
        let failure = self
            .failure
            .as_ref()
            .filter(|(failed, _)| *failed == check.id)
            .map(|(_, message)| message.clone());
        let show_log = self.log_for == Some(check.id) && (mine || failure.is_some());
        let log = show_log.then(|| {
            let start = self.log.len().saturating_sub(LOG_LINES);
            v_flex()
                .ml(px(32.))
                .px_3()
                .py_2()
                .gap_0p5()
                .rounded_lg()
                .bg(ui::well())
                .font_family(ui::MONO)
                .text_xs()
                .when(self.log.is_empty() && mine, |d| {
                    d.child(div().text_color(ui::faint()).child("Starting…"))
                })
                .children(
                    self.log[start..]
                        .iter()
                        .map(|line| div().text_color(ui::muted()).truncate().child(line.clone())),
                )
                .when_some(failure, |d, message| {
                    d.child(div().text_color(ui::danger()).child(message))
                })
        });
        let row = v_flex()
            .gap_2()
            .px_3()
            .py_2p5()
            .rounded(px(ui::ISLAND_ROW))
            .when(tone == Tone::Good, |d| d.bg(ui::success().opacity(0.08)))
            .when(mine, |d| d.bg(ui::petal().opacity(0.06)))
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .size_5()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(ui::fg())
                                            .child(check.title),
                                    )
                                    .child(ui::chip(label, if mine { ui::petal() } else { color })),
                            )
                            .child(div().text_xs().text_color(ui::muted()).child(check.summary))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(ui::MONO)
                                    .text_color(ui::faint())
                                    .truncate()
                                    .child(status_text(&check.status).to_string()),
                            ),
                    )
                    .children(action),
            )
            .children(log);
        // Keyed by tone too, so a row that turns green settles in again.
        ui::appear(
            ElementId::Name(format!("onboarding-row-{index}-{tone:?}").into()),
            row,
        )
        .into_any_element()
    }

    fn render_checks(&self, checks: &[Check], cx: &mut Context<Self>) -> Div {
        let mut column = v_flex()
            .gap_5()
            .when(!self.embedded, |column| column.px_6().pb_2());
        let mut index = 0;
        for (need, label) in [
            (Need::Required, "Required"),
            (Need::Recommended, "Recommended"),
            (Need::Optional, "Optional"),
        ] {
            let group: Vec<&Check> = checks.iter().filter(|c| c.need == need).collect();
            if group.is_empty() {
                continue;
            }
            let rows: Vec<AnyElement> = group
                .into_iter()
                .map(|check| {
                    index += 1;
                    self.render_row(check, index, cx)
                })
                .collect();
            column = column.child(
                v_flex()
                    .gap_2()
                    .child(div().px_1().child(ui::section_label(label)))
                    .child(
                        ui::island(cx)
                            .p(px(6.))
                            .child(v_flex().gap_1().children(rows)),
                    ),
            );
        }
        column
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.installing.is_some();
        let checks = self.checks.as_deref();
        let ready = checks.is_some_and(required_ok);
        let plan = checks.map(install_plan).unwrap_or_default();
        h_flex()
            .px_6()
            .py_4()
            .gap_2()
            .child(
                Button::new("onboarding-recheck")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::RefreshCw))
                    .label("Check again")
                    .loading(self.checking)
                    .disabled(busy || self.checking)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(div().flex_1())
            .when(!ready, |d| {
                d.child(
                    Button::new("onboarding-skip")
                        .small()
                        .ghost()
                        .label("Skip for now")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::Close))),
                )
            })
            .when(!plan.is_empty() || busy, |d| {
                d.child(
                    Button::new("onboarding-install-all")
                        .small()
                        .primary()
                        .icon(Icon::new(IconName::ArrowDown))
                        .label(if busy {
                            "Installing…"
                        } else {
                            "Install everything"
                        })
                        .loading(busy)
                        .disabled(busy || self.checking)
                        .on_click(cx.listener(|this, _, _, cx| this.install_all(cx))),
                )
            })
            .child({
                let button = Button::new("onboarding-continue")
                    .small()
                    .icon(Icon::new(IconName::ArrowRight))
                    .label("Continue")
                    .disabled(busy || !ready)
                    .on_click(cx.listener(|this, _, _, cx| this.finish(cx)));
                if plan.is_empty() && !busy {
                    button.primary()
                } else {
                    button.ghost()
                }
            })
    }

    /// Configuration's header for the embedded checklist: the platform, then
    /// Check again and Install everything.
    fn render_embedded_bar(&self, cx: &mut Context<Self>) -> Div {
        let busy = self.installing.is_some();
        let plan = self.checks.as_deref().map(install_plan).unwrap_or_default();
        h_flex()
            .pb_3()
            .gap_2()
            .items_center()
            .when_some(self.platform.clone(), |d, platform| {
                d.child(ui::chip(platform, ui::muted()).border_0().rounded_full())
            })
            .child(div().flex_1())
            .child(
                Button::new("setup-recheck")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::RefreshCw))
                    .label("Check again")
                    .loading(self.checking)
                    .disabled(busy || self.checking)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .when(!plan.is_empty() || busy, |d| {
                d.child(
                    Button::new("setup-install-all")
                        .small()
                        .primary()
                        .icon(Icon::new(IconName::ArrowDown))
                        .label(if busy {
                            "Installing…"
                        } else {
                            "Install everything"
                        })
                        .loading(busy)
                        .disabled(busy || self.checking)
                        .on_click(cx.listener(|this, _, _, cx| this.install_all(cx))),
                )
            })
    }
}

/// Gentle Shell's pixel rose, `height` px tall, in `color`.
fn rose(height: f32, color: Hsla) -> impl IntoElement {
    let (dots, (width, rows)) = crate::banner::rose();
    let dot = height / rows.max(1) as f32;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let side = px((dot * 0.55).max(1.0));
            for &(x, y) in dots {
                let origin = point(
                    bounds.origin.x + px(x as f32 * dot),
                    bounds.origin.y + px(y as f32 * dot),
                );
                window.paint_quad(fill(Bounds::new(origin, size(side, side)), color));
            }
        },
    )
    .w(px(width as f32 * dot))
    .h(px(height))
}

impl Render for OnboardingView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.checks {
            None => h_flex()
                .py_8()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::petal()))
                .child("Checking Node, Gentle Shell and the rest…")
                .into_any_element(),
            Some(checks) => self.render_checks(checks, cx).into_any_element(),
        };
        let busy = self.busy();
        if self.embedded {
            return v_flex()
                .w_full()
                .child(self.render_embedded_bar(cx))
                .child(body)
                .into_any_element();
        }
        let (backdrop, panel) = ui::screen(Some(720.));
        backdrop
            .occlude()
            .child(ui::pop_in(
                "onboarding-screen",
                panel
                    .w_full()
                    .max_w(px(720.))
                    .relative()
                    .child(
                        div()
                            .id("onboarding-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(self.render_hero())
                            .child(body),
                    )
                    .child(self.render_footer(cx))
                    .child(
                        h_flex()
                            .absolute()
                            .top_3()
                            .right_3()
                            .gap_2()
                            .child(ui::kbd("esc"))
                            .child(
                                Button::new("onboarding-close")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::X))
                                    .disabled(busy)
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(OnboardingEvent::Close)),
                                    ),
                            ),
                    ),
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{
        Check, CheckId, Fix, LOG_KEEP, Need, Status, Tone, all_set, install_plan, push_lines,
        required_ok, tone, usable,
    };

    fn check(id: CheckId, need: Need, status: Status, fix: Fix) -> Check {
        Check {
            id,
            title: "t",
            summary: "s",
            need,
            status,
            fix,
        }
    }

    fn ok(id: CheckId, need: Need) -> Check {
        check(id, need, Status::Ok("1".into()), Fix::None)
    }

    #[test]
    fn plan_runs_needed_unattended_fixes_in_order() {
        let checks = vec![
            ok(CheckId::Node, Need::Required),
            check(
                CheckId::GentleShell,
                Need::Required,
                Status::Missing("not found".into()),
                Fix::Auto,
            ),
            check(
                CheckId::Setup,
                Need::Required,
                Status::Failed("exit 1".into()),
                Fix::Auto,
            ),
            check(
                CheckId::Provider,
                Need::Required,
                Status::Missing("none".into()),
                Fix::SignIn,
            ),
            check(
                CheckId::Git,
                Need::Recommended,
                Status::Missing("none".into()),
                Fix::Terminal("sudo apt install git".into()),
            ),
            check(
                CheckId::Engram,
                Need::Optional,
                Status::Missing("none".into()),
                Fix::Auto,
            ),
        ];
        assert_eq!(
            install_plan(&checks),
            VecDeque::from([CheckId::GentleShell, CheckId::Setup])
        );
    }

    #[test]
    fn readiness_ignores_extras() {
        let mut checks = vec![
            ok(CheckId::Node, Need::Required),
            ok(CheckId::GentleShell, Need::Required),
            check(
                CheckId::Engram,
                Need::Optional,
                Status::Missing("none".into()),
                Fix::Auto,
            ),
        ];
        assert!(required_ok(&checks));
        assert!(all_set(&checks));
        assert!(usable(&checks));
        checks.push(check(
            CheckId::Git,
            Need::Recommended,
            Status::Outdated("2.0".into()),
            Fix::None,
        ));
        assert!(required_ok(&checks));
        assert!(!all_set(&checks));
        checks.push(check(
            CheckId::Provider,
            Need::Required,
            Status::Missing("none".into()),
            Fix::SignIn,
        ));
        assert!(!required_ok(&checks));
        // No sign-in yet, but gentle-shell itself can start.
        assert!(usable(&checks));
    }

    #[test]
    fn tones_follow_need_and_status() {
        let missing = |need| check(CheckId::Git, need, Status::Missing("x".into()), Fix::None);
        assert_eq!(tone(&missing(Need::Required)), Tone::Bad);
        assert_eq!(tone(&missing(Need::Recommended)), Tone::Warn);
        assert_eq!(tone(&missing(Need::Optional)), Tone::Warn);
        assert_eq!(
            tone(&check(
                CheckId::Node,
                Need::Required,
                Status::Outdated("v20".into()),
                Fix::Auto
            )),
            Tone::Warn
        );
        assert_eq!(
            tone(&check(
                CheckId::Engram,
                Need::Optional,
                Status::Failed("x".into()),
                Fix::Auto
            )),
            Tone::Bad
        );
        assert_eq!(tone(&ok(CheckId::Node, Need::Required)), Tone::Good);
    }

    #[test]
    fn log_keeps_non_empty_lines_and_the_tail() {
        let mut log = Vec::new();
        push_lines(&mut log, "a\n\n  \nb  \r\nc");
        assert_eq!(log, ["a", "b", "c"]);
        for i in 0..LOG_KEEP {
            push_lines(&mut log, &i.to_string());
        }
        assert_eq!(log.len(), LOG_KEEP);
        assert_eq!(log.last().map(String::as_str), Some("199"));
    }
}
