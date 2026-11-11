//! The player as it shows: the ♪ button in the title bar and the popover it
//! opens (the piece, a live spectrum, controls, volume and the mood field).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Icon, Selectable, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{BANDS, Job, Music};
use crate::shared::ui;

/// The spectrum's height.
const SPECTRUM_H: f32 = 52.;

impl Music {
    fn render_header(&self, cx: &mut Context<Self>) -> Div {
        h_flex()
            .gap_2p5()
            .child(
                div()
                    .size(px(26.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .border_1()
                    .border_color(ui::border_strong())
                    .bg(ui::well())
                    .text_color(ui::petal())
                    .child(Icon::new(IconName::Music).small()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::fg())
                            .child("Music"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui::faint())
                            .child("Composed live by gentle-stream"),
                    ),
            )
            .when(self.state.is_some(), |d| {
                d.child(
                    Button::new("music-stop")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Square))
                        .tooltip("Stop the music")
                        .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
                )
            })
    }

    /// One bar per band, rose and taller with the level; flat while paused
    /// or before the first frame.
    fn render_spectrum(&self, playing: bool) -> Div {
        h_flex()
            .h(px(SPECTRUM_H))
            .items_end()
            .gap(px(2.))
            .children((0..BANDS).map(|i| {
                let level = if playing {
                    self.shown.get(i).copied().unwrap_or(0.)
                } else {
                    0.
                };
                div()
                    .flex_1()
                    .h(px(3. + level * (SPECTRUM_H - 3.)))
                    .rounded_t(px(2.))
                    .bg(if playing {
                        ui::petal().opacity(0.35 + 0.65 * level)
                    } else {
                        ui::border()
                    })
            }))
    }

    fn render_now(&self, cx: &mut Context<Self>) -> Div {
        let Some(state) = &self.state else {
            return v_flex()
                .py_3()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(ui::muted())
                        .child("Nothing is playing."),
                )
                .child(
                    Button::new("music-start")
                        .small()
                        .primary()
                        .icon(Icon::new(IconName::Play))
                        .label("Start music")
                        .disabled(self.job.is_some())
                        .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                );
        };
        let playing = state.playing;
        let volume = self.volume.read(cx).value().start();
        v_flex()
            .gap_2p5()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::fg())
                            .truncate()
                            .child(state.name.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui::muted())
                            .child(state.details()),
                    )
                    .when(!state.mood.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .italic()
                                .text_color(ui::faint())
                                .truncate()
                                .child(format!("“{}”", state.mood)),
                        )
                    }),
            )
            .child(self.render_spectrum(playing))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("music-toggle")
                            .primary()
                            .small()
                            .icon(Icon::new(if playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            }))
                            .tooltip(if playing { "Pause" } else { "Play" })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
                    )
                    .child(
                        Button::new("music-next")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::SkipForward))
                            .tooltip("Next piece, same mood")
                            .on_click(cx.listener(|this, _, _, cx| this.next(cx))),
                    )
                    .child(Icon::new(IconName::Volume2).small().text_color(ui::muted()))
                    .child(
                        div().flex_1().child(
                            Slider::new(&self.volume)
                                .horizontal()
                                .accessibility_label("Music volume"),
                        ),
                    )
                    .child(
                        div()
                            .w(px(34.))
                            .text_right()
                            .text_xs()
                            .font_family(ui::MONO)
                            .text_color(ui::muted())
                            .child(format!("{volume:.0}%")),
                    ),
            )
    }

    fn render_mood(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .gap_1p5()
            .child(
                ui::field()
                    .child(
                        Icon::new(IconName::Sparkles)
                            .small()
                            .text_color(ui::faint()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.mood).appearance(false).small()),
                    )
                    .child(
                        ui::key_button("music-mood-send", "↵").on_click(
                            cx.listener(|this, _, window, cx| this.submit_mood(window, cx)),
                        ),
                    ),
            )
            .child(match (&self.job, &self.note) {
                (Some(job), _) => h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(ui::muted())
                    .child(Spinner::new().xsmall().color(ui::accent()))
                    .child(match job {
                        Job::Starting => "Starting the player…".to_string(),
                        Job::Composing(mood) => format!("Composing “{mood}”…"),
                    }),
                (None, Some((text, ok))) => div()
                    .text_xs()
                    .text_color(if *ok { ui::success() } else { ui::danger() })
                    .child(text.clone()),
                (None, None) => div()
                    .text_xs()
                    .text_color(ui::faint())
                    .child("Any language; Gentle Shell's model reads it."),
            })
    }
}

impl Render for Music {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .p_3()
            .gap_3()
            .child(self.render_header(cx))
            .child(self.render_now(cx))
            .child(self.render_mood(cx))
    }
}

/// The title bar's ♪ button, once gentle-stream is installed. It wears a
/// small equalizer while the music plays.
pub fn title_button(music: &Entity<Music>, cx: &App) -> Option<AnyElement> {
    let model = music.read(cx);
    if !model.available() {
        return None;
    }
    let trigger = MusicTrigger {
        playing: model.playing(),
        open: false,
    };
    let content = music.clone();
    let watch = music.clone();
    Some(
        div()
            // Clicks here are not title-bar drags.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Popover::new("music")
                    .anchor(Anchor::TopRight)
                    .trigger(trigger)
                    .on_open_change(move |open, _, cx| {
                        let open = *open;
                        watch.update(cx, |music, cx| music.set_watching(open, cx));
                    })
                    .w(px(340.))
                    .p_0()
                    .content(move |_, _, _| {
                        ui::popover_in(
                            ElementId::Name("music-in".into()),
                            div().child(content.clone()),
                        )
                    }),
            )
            .into_any_element(),
    )
}

#[derive(IntoElement)]
struct MusicTrigger {
    playing: bool,
    open: bool,
}

impl Selectable for MusicTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for MusicTrigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        const BARS: [f32; 3] = [6., 11., 8.];
        div()
            .id("music-button")
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .hover(|s| s.bg(ui::surface_hover()))
            .when(self.open, |d| d.bg(ui::surface_hover()))
            .when(!self.open, |d| {
                d.tooltip(|window, cx| Tooltip::new("Music").build(window, cx))
            })
            .child(if self.playing {
                h_flex()
                    .gap(px(2.))
                    .items_end()
                    .h(px(12.))
                    .children(
                        BARS.map(|h| div().w(px(2.5)).h(px(h)).rounded_full().bg(ui::petal())),
                    )
                    .into_any_element()
            } else {
                Icon::new(IconName::Music)
                    .small()
                    .text_color(ui::fg())
                    .into_any_element()
            })
    }
}
