//! Render: the full-window frame with its glow, the current page sliding in,
//! and the footer (back and skip, the page dots, the main button). Keyboard:
//! ← and → page, ↵ is the main button.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{SETUP_STEP, STEPS, WelcomeEvent, WelcomeView, pages};
use crate::shared::banner::pixel_rose;
use crate::shared::ui;

/// The client-drawn title bar (Windows, Linux) keeps its window controls
/// visible above the tour; on macOS the tour fills the window under the
/// native traffic lights.
const TITLE_BAR: f32 = 36.;

impl WelcomeView {
    fn render_page(&self, cx: &App) -> Div {
        match self.step {
            0 => v_flex()
                .items_center()
                .gap_6()
                .child(ui::pop_in("welcome-rose", pages::rose_mark()))
                .child(pages::heading(
                    pages::hello_title(),
                    "A desktop home for Gentle Shell's coding agents. Every tab is an \
                     agent with its own conversation, tools and helpers, right in the \
                     folder you're working on.",
                ))
                .child(pages::tiles([
                    (IconName::Bot, ui::petal(), "Agents in tabs"),
                    (IconName::Command, ui::champagne(), "Keyboard first"),
                    (IconName::Brain, ui::info(), "Memory that lasts"),
                ])),
            1 => v_flex()
                .items_center()
                .gap_8()
                .child(pages::window_mock())
                .child(pages::heading(
                    pages::title("Many agents, one window."),
                    "Open agents and terminals side by side, split them, and reach \
                     every screen from the palette.",
                ))
                .child(pages::window_keys()),
            2 => v_flex()
                .items_center()
                .gap_8()
                .child(pages::profiles_mock())
                .child(pages::heading(
                    pages::title("Your models, your way."),
                    "Sign in with a subscription or an API key, then send each role \
                     to the model you trust. Save the routing as a profile and switch \
                     in one click.",
                ))
                .child(pages::tiles([
                    (IconName::KeyRound, ui::petal(), "Subscriptions & keys"),
                    (IconName::Layers, ui::champagne(), "Profiles"),
                    (IconName::Gauge, ui::info(), "Usage at a glance"),
                ])),
            3 => v_flex()
                .items_center()
                .gap_8()
                .child(pages::control_mock())
                .child(pages::heading(
                    pages::title("You stay in control."),
                    "Choose how much the agent may do on its own, and change it any \
                     time from the chip under the conversation.",
                ))
                .child(pages::control_keys()),
            _ => self.render_setup(cx),
        }
    }

    /// The last page: the setup checklist under a short heading.
    fn render_setup(&self, cx: &App) -> Div {
        let ready = self.ready(cx);
        v_flex()
            .w_full()
            .max_w(px(720.))
            .items_center()
            .gap_5()
            .child(pixel_rose(
                56.,
                if ready { ui::success() } else { ui::petal() },
            ))
            .child(pages::heading(
                pages::title(if ready {
                    "You're ready."
                } else {
                    "Let's get you set up."
                }),
                if ready {
                    "Everything Gentle Shell needs is here. The extras below make it better."
                } else {
                    "Install what's missing right here; anything that needs your \
                     password opens in a terminal for you to run."
                },
            ))
            // The checklist brings its own grouped frames.
            .child(div().w_full().child(self.setup.clone()))
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        let step = self.step;
        let busy = self.busy(cx);
        let ready = self.ready(cx);
        let left = h_flex()
            .flex_1()
            .gap_2()
            .when(step > 0, |d| {
                d.child(
                    Button::new("welcome-back")
                        .ghost()
                        .icon(Icon::new(IconName::ArrowLeft))
                        .label("Back")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                )
            })
            .when(step < SETUP_STEP, |d| {
                d.child(
                    Button::new("welcome-skip")
                        .ghost()
                        .label("Skip intro")
                        .on_click(cx.listener(|this, _, _, cx| this.go(SETUP_STEP, cx))),
                )
            })
            .when(step == SETUP_STEP && !ready, |d| {
                d.child(
                    Button::new("welcome-later")
                        .ghost()
                        .label("Skip for now")
                        .disabled(busy)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::Close))),
                )
            });
        let dots = h_flex().gap_1p5().children((0..STEPS).map(|i| {
            let on = i == step;
            div()
                .id(("welcome-dot", i))
                .h(px(6.))
                .w(px(if on { 22. } else { 6. }))
                .rounded_full()
                .bg(if on {
                    ui::petal()
                } else {
                    ui::faint().opacity(0.5)
                })
                .when(!busy, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.go(i, cx)))
                })
        }));
        let label = match step {
            0 => "Get started",
            SETUP_STEP => "Continue",
            _ => "Next",
        };
        let next = Button::new("welcome-next")
            .primary()
            .large()
            .rounded(px(999.))
            .label(label)
            .disabled(step == SETUP_STEP && (busy || !ready))
            .on_click(cx.listener(|this, _, _, cx| this.next(cx)));
        h_flex()
            .flex_none()
            .h(px(88.))
            .px_10()
            .child(left)
            .child(dots)
            .child(h_flex().flex_1().justify_end().child(next))
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "right" if self.step < SETUP_STEP => self.go(self.step + 1, cx),
            "left" if !self.busy(cx) => self.back(cx),
            "enter" => self.next(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for WelcomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step;
        let offset = if self.forward { 28. } else { -28. };
        let page = self.render_page(cx).with_animation(
            SharedString::from(format!("welcome-page-{step}")),
            Animation::new(Duration::from_millis(360)).with_easing(ease_out_quint()),
            move |el, t| el.opacity(t).relative().left(px((1. - t) * offset)),
        );
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
                // A soft petal glow from the top, fading out by mid-window.
                div().absolute().inset_0().bg(linear_gradient(
                    180.,
                    linear_color_stop(ui::petal().opacity(0.10), 0.),
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
                                    .min_h_full()
                                    .items_center()
                                    .justify_center()
                                    .px_8()
                                    .py_6()
                                    .child(page),
                            ),
                    )
                    .child(self.render_footer(cx)),
            )
    }
}
