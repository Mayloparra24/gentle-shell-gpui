//! Welcome: the first-run screen. Gentle Shell's banner writes itself in like
//! the terminal intro, then the install sequence ([`OnboardingView`] in
//! sequence mode) plays below it, step by step, until everything required
//! is ready. It opens by itself only on a first run; ⌘K → "Welcome screen"
//! shows it any time.

use std::time::Duration;

use gpui_kit::*;

use crate::features::onboarding::{OnboardingEvent, OnboardingView, Probe};
use crate::features::setup;
use crate::shared::banner::{self, BannerConfig};

mod view;

/// How long the banner takes to write itself in.
const INTRO: Duration = Duration::from_millis(1500);
/// The heading and the sequence start coming in this far into the intro.
const AFTER_INTRO: Duration = Duration::from_millis(1100);

#[derive(Clone, Debug)]
pub enum WelcomeEvent {
    /// Continue (after recording completion) or Skip.
    Close,
    /// An action from the install sequence.
    Setup(OnboardingEvent),
}

pub struct WelcomeView {
    setup: Entity<OnboardingView>,
    banner: BannerConfig,
    /// The intro played; showing the screen again (back from Providers)
    /// draws everything in place.
    intro_done: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _intro: Task<()>,
}

impl EventEmitter<WelcomeEvent> for WelcomeView {}

impl WelcomeView {
    /// `probe` is a look at the machine already taken (the startup check).
    pub fn new(probe: Option<Probe>, cx: &mut Context<Self>) -> Self {
        let setup = cx.new(|cx| OnboardingView::sequence(probe, AFTER_INTRO, cx));
        let subscriptions = vec![
            cx.subscribe(&setup, |_, _, event: &OnboardingEvent, cx| {
                cx.emit(WelcomeEvent::Setup(event.clone()));
            }),
            // Installs and re-checks change the heading and the footer.
            cx.observe(&setup, |_, _, cx| cx.notify()),
        ];
        let intro = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INTRO + AFTER_INTRO).await;
            let _ = this.update(cx, |this, cx| {
                this.intro_done = true;
                this.setup.update(cx, |setup, _| setup.skip_entrance());
            });
        });
        Self {
            setup,
            banner: banner::read_config(&pi_rpc::resolve_homes().config),
            intro_done: false,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
            _intro: intro,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// An install is running: the screen stays open until it ends.
    pub fn busy(&self, cx: &App) -> bool {
        self.setup.read(cx).busy()
    }

    /// gentle-shell can start, as of the sequence's last look.
    pub fn usable(&self, cx: &App) -> bool {
        self.setup.read(cx).usable()
    }

    fn ready(&self, cx: &App) -> bool {
        self.setup.read(cx).ready()
    }

    /// The footer's main button: Continue once everything required is
    /// ready, else Install everything.
    fn primary(&mut self, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        if self.ready(cx) {
            setup::mark_done();
            cx.emit(WelcomeEvent::Close);
        } else if self.setup.read(cx).can_install() {
            self.setup
                .update(cx, |setup, cx| setup.install_everything(cx));
        }
    }

    fn check_again(&mut self, cx: &mut Context<Self>) {
        self.setup.update(cx, |setup, cx| setup.check_again(cx));
    }
}
