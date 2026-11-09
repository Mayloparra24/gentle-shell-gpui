//! Welcome: the first-run tour. Four pages introduce Gentle Shell (agents in
//! tabs, splits and the palette, models and profiles, permissions), the last
//! one is the setup checklist ([`OnboardingView`], embedded). It opens by
//! itself only on a first run; ⌘K → "Welcome tour" replays it any time.

use gpui_kit::*;

use crate::features::onboarding::{OnboardingEvent, OnboardingView, Probe};
use crate::features::setup;

mod pages;
mod view;

/// Pages, the setup checklist last.
const STEPS: usize = 5;
const SETUP_STEP: usize = STEPS - 1;

#[derive(Clone, Debug)]
pub enum WelcomeEvent {
    /// Continue (after recording completion) or Skip.
    Close,
    /// An action from the setup checklist on the last page.
    Setup(OnboardingEvent),
}

pub struct WelcomeView {
    step: usize,
    /// The step shown before the current one, so pages slide the right way.
    forward: bool,
    setup: Entity<OnboardingView>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WelcomeEvent> for WelcomeView {}

impl WelcomeView {
    /// `probe` is a look at the machine already taken (the startup check).
    pub fn new(probe: Option<Probe>, cx: &mut Context<Self>) -> Self {
        let setup = cx.new(|cx| OnboardingView::embedded_with(probe, cx));
        let subscriptions = vec![
            cx.subscribe(&setup, |_, _, event: &OnboardingEvent, cx| {
                cx.emit(WelcomeEvent::Setup(event.clone()));
            }),
            // Installs and re-checks change what the footer offers.
            cx.observe(&setup, |_, _, cx| cx.notify()),
        ];
        Self {
            step: 0,
            forward: true,
            setup,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// An install is running: the tour stays open until it ends.
    pub fn busy(&self, cx: &App) -> bool {
        self.setup.read(cx).busy()
    }

    /// gentle-shell can start, as of the setup checklist's last look.
    pub fn usable(&self, cx: &App) -> bool {
        self.setup.read(cx).usable()
    }

    fn ready(&self, cx: &App) -> bool {
        self.setup.read(cx).ready()
    }

    fn go(&mut self, step: usize, cx: &mut Context<Self>) {
        let step = step.min(SETUP_STEP);
        if step != self.step {
            self.forward = step > self.step;
            self.step = step;
            cx.notify();
        }
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        if self.step == SETUP_STEP {
            self.finish(cx);
        } else {
            self.go(self.step + 1, cx);
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.go(self.step.saturating_sub(1), cx);
    }

    /// Continue from the last page: records completion like the onboarding
    /// screen does, once everything required is ready.
    fn finish(&mut self, cx: &mut Context<Self>) {
        if self.ready(cx) && !self.busy(cx) {
            setup::mark_done();
            cx.emit(WelcomeEvent::Close);
        }
    }
}
