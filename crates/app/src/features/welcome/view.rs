//! Render: the full-window frame with its glow, the banner writing itself in
//! (green once everything required is ready), the heading, the install
//! sequence, and the footer. ↵ is the main button.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{AFTER_INTRO, INTRO, WelcomeEvent, WelcomeView};
use crate::shared::banner::{Palette, banner};
use crate::shared::ui;

/// The client-drawn title bar (Windows, Linux) keeps its window controls
/// visible above the screen; on macOS the screen fills the window under the
/// native traffic lights.
const TITLE_BAR: f32 = 36.;
/// The banner is ~175 px wide per unit of scale.
const BANNER_SCALE: f32 = 2.6;

impl WelcomeView {
    fn render_banner(&self, ready: bool) -> AnyElement {
        let mut config = self.banner;
        if ready {
            // Everything required is in place: the rose blooms green.
            config.palette = Palette::Green;
            return ui::pop_in(
                "welcome-banner-ready",
                div().child(banner(config, BANNER_SCALE, 1.)),
            )
            .into_any_element();
        }
        if self.intro_done {
            return div()
                .child(banner(config, BANNER_SCALE, 1.))
                .into_any_element();
        }
        div()
            .with_animation(
                "welcome-banner",
                Animation::new(INTRO).with_easing(ease_out_quint()),
                move |el, t| el.child(banner(config, BANNER_SCALE, t)),
            )
            .into_any_element()
    }

    fn render_heading(&self, cx: &App) -> AnyElement {
        let setup = self.setup.read(cx);
        let (title, line) = if setup.looking() {
            (
                "Welcome to Gentle Shell.",
                "Looking at what this machine already has…",
            )
        } else if setup.ready() {
            (
                "You're ready.",
                "Everything Gentle Shell needs is here. Open your first agent whenever \
                 you like; the extras below make it better.",
            )
        } else {
            (
                "Let's get you set up.",
                "A few steps and you're in. Everything installs right here; anything \
                 that needs your password opens in a terminal for you to run.",
            )
        };
        let heading = v_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(34.))
                    .line_height(px(42.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(ui::fg())
                    .child(title),
            )
            .child(
                div()
                    .max_w(px(560.))
                    .text_base()
                    .text_center()
                    .line_height(px(24.))
                    .text_color(ui::muted())
                    .child(line),
            );
        let delay = if self.intro_done {
            std::time::Duration::ZERO
        } else {
            AFTER_INTRO
        };
        ui::appear_after("welcome-heading", delay, 10., heading).into_any_element()
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let setup = self.setup.read(cx);
        let busy = setup.busy();
        let ready = setup.ready();
        let checking = setup.checking();
        let can_install = setup.can_install();
        let (label, enabled) = if ready {
            ("Continue", !busy)
        } else if busy {
            ("Installing…", false)
        } else if can_install {
            ("Install everything", !checking)
        } else {
            // Only steps this screen can't run alone are left (sign in, a
            // terminal command): Continue waits for them.
            ("Continue", false)
        };
        let primary = Button::new("welcome-primary")
            .primary()
            .large()
            .rounded(px(999.))
            .when(!ready && can_install, |b| {
                b.icon(Icon::new(IconName::ArrowDown))
            })
            .label(label)
            .loading(busy)
            .disabled(!enabled)
            .on_click(cx.listener(|this, _, _, cx| this.primary(cx)));
        let footer = h_flex()
            .flex_none()
            .h(px(88.))
            .px_10()
            .gap_2()
            .when(!ready, |d| {
                d.child(
                    Button::new("welcome-skip")
                        .ghost()
                        .label("Skip for now")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::Close))),
                )
            })
            .child(
                Button::new("welcome-recheck")
                    .ghost()
                    .icon(Icon::new(IconName::RefreshCw))
                    .label("Check again")
                    .loading(checking)
                    .disabled(busy || checking)
                    .on_click(cx.listener(|this, _, _, cx| this.check_again(cx))),
            )
            .child(div().flex_1())
            .child(primary);
        let delay = if self.intro_done {
            std::time::Duration::ZERO
        } else {
            AFTER_INTRO
        };
        ui::appear_after("welcome-footer", delay, 0., footer).into_any_element()
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "enter" {
            self.primary(cx);
            cx.stop_propagation();
        }
    }
}

impl Render for WelcomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.setup.read(cx).ready();
        let macos = cfg!(target_os = "macos");
        div()
            .id("welcome")
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .top(px(if macos { 0. } else { TITLE_BAR }))
            .occlude()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .bg(ui::screen_bg())
            .child(
                // A soft glow from the top, petal while setting up, green once
                // everything required is ready.
                div().absolute().inset_0().bg(linear_gradient(
                    180.,
                    linear_color_stop(
                        if ready { ui::success() } else { ui::petal() }.opacity(0.10),
                        0.,
                    ),
                    linear_color_stop(ui::petal().opacity(0.), 0.6),
                )),
            )
            .child(
                v_flex()
                    .size_full()
                    .when(macos, |d| {
                        // Under the traffic lights: drag the window from here.
                        d.child(
                            div()
                                .id("welcome-drag")
                                .flex_none()
                                .h(px(TITLE_BAR))
                                .window_control_area(WindowControlArea::Drag)
                                .on_mouse_down(MouseButton::Left, |_, window, _| {
                                    window.start_window_move()
                                }),
                        )
                    })
                    .child(
                        div()
                            .id("welcome-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(
                                v_flex()
                                    .w_full()
                                    .items_center()
                                    .px_8()
                                    .pt_6()
                                    .pb_6()
                                    .gap_6()
                                    .child(self.render_banner(ready))
                                    .child(self.render_heading(cx))
                                    .child(
                                        div().w_full().max_w(px(680.)).child(self.setup.clone()),
                                    ),
                            ),
                    )
                    .child(self.render_footer(cx)),
            )
    }
}
