//! Onboarding: the first-run screen (and "Setup & requirements" any time).
//! It shows what this machine has for Gentle Shell (`crate::features::setup` checks),
//! installs what can be installed unattended with a live log, hands anything
//! that needs a password or an interactive login to a terminal tab, and sends
//! provider sign-in to the Providers screen.

use std::collections::VecDeque;

use gpui_kit::*;

use crate::features::setup::{self, Check, CheckId};

mod checks;
mod install;
mod view;

use checks::{required_ok, usable};

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
        Self::embedded_with(None, cx)
    }

    /// The embedded checklist, starting from a look already taken (`None`
    /// looks now).
    pub fn embedded_with(probe: Option<Probe>, cx: &mut Context<Self>) -> Self {
        let mut this = Self::new(probe, cx);
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

    /// Everything required is ready, as of the last look.
    pub fn ready(&self) -> bool {
        self.checks.as_deref().is_some_and(required_ok)
    }
}
