//! The entrance after Continue, in three beats: Gentle Shell's banner writes
//! itself in again in slow motion, the rose blows pixel kisses (and a few
//! hearts) that float up and pop, then "Sexyyyy" springs in letter by
//! letter, the y's stretching one after another. The screen then dissolves
//! into the app. Paint and layout only; it takes no input.

use std::time::Duration;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;

use crate::shared::banner::{BannerConfig, banner};
use crate::shared::kisses::{HEART, LIPS, noise, paint_sprite};
use crate::shared::ui;

/// From Continue to the app.
pub(super) const FINALE: Duration = Duration::from_millis(5600);
/// The dissolve starts this far in (0..1).
pub(super) const DISSOLVE: f32 = 0.86;

/// The box the banner and the kisses share.
const STAGE_W: f32 = 760.;
const STAGE_H: f32 = 300.;
const BANNER_SCALE: f32 = 2.6;
/// The banner is ~175 px wide per unit of scale; the rose is its left part.
const BANNER_W: f32 = 175. * BANNER_SCALE;

/// The slow-motion letters: when they start and how long they take.
const LETTERS_AT: f32 = 0.25;
const LETTERS_FOR: f32 = 2.2;
/// The kisses fly from here (seconds), one after another.
const KISSES_AT: f32 = 2.0;
const KISS_GAP: f32 = 0.16;
const KISS_LIFE: f32 = 1.7;
const KISSES: usize = 14;
/// "Sexyyyy" starts here (seconds), a letter at a time.
const WORD_AT: f32 = 3.0;
const LETTER_GAP: f32 = 0.11;
const WORD: &str = "Sexyyyy";

fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// The kisses the rose blows: each leaves the rose, floats up swaying,
/// swells, and fades. Every fourth one is a heart.
fn kisses() -> impl IntoElement {
    let total = FINALE.as_secs_f32();
    div().absolute().inset_0().with_animation(
        "finale-kisses",
        Animation::new(FINALE),
        move |el, t| {
            let now = t * total;
            el.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let center = bounds.center();
                        // The rose sits at the banner's left.
                        let rose = point(
                            center.x - px(BANNER_W / 2. - BANNER_W * 0.15),
                            center.y - px(STAGE_H * 0.08),
                        );
                        for i in 0..KISSES {
                            let born = KISSES_AT + i as f32 * KISS_GAP;
                            let age = (now - born) / KISS_LIFE;
                            if !(0. ..1.).contains(&age) {
                                continue;
                            }
                            // Mostly up and outward, alternating sides.
                            let side = if i % 2 == 0 { 1. } else { -1. };
                            let drift = side * (60. + 180. * noise(i, 1));
                            let rise = 160. + 160. * noise(i, 2);
                            let sway = (age * 9. + i as f32).sin() * 10.;
                            let travel = ease_out(age);
                            let at = point(
                                rose.x + px(drift * travel + sway),
                                rose.y - px(rise * travel),
                            );
                            // Pops in, swells a little, fades at the end.
                            let grow = ui::spring((age * 3.).min(1.));
                            let dot = (2.2 + 1.6 * noise(i, 3)) * (0.4 + 0.6 * grow);
                            let alpha = if age > 0.7 { (1. - age) / 0.3 } else { 1. };
                            let heart = i % 4 == 3;
                            let (body, line) = if heart {
                                (ui::petal(), ui::petal())
                            } else {
                                (
                                    hsla(340. / 360., 0.85, 0.62, alpha),
                                    hsla(340. / 360., 0.70, 0.30, alpha),
                                )
                            };
                            paint_sprite(
                                window,
                                if heart { &HEART } else { &LIPS },
                                at,
                                dot,
                                body.opacity(alpha),
                                line.opacity(alpha),
                            );
                        }
                    },
                )
                .size_full(),
            )
        },
    )
}

/// The banner writing itself in again, slowly.
fn letters(config: BannerConfig) -> impl IntoElement {
    let total = FINALE.as_secs_f32();
    div().flex().items_center().justify_center().with_animation(
        "finale-letters",
        Animation::new(FINALE),
        move |el, t| {
            let reveal = smoothstep((t * total - LETTERS_AT) / LETTERS_FOR);
            el.child(banner(config, BANNER_SCALE, reveal))
        },
    )
}

/// A big kiss that lands beside the word once it's spelled out, like a
/// lipstick mark.
fn stamp() -> impl IntoElement {
    const DOT: f32 = 5.;
    let total = FINALE.as_secs_f32();
    let at = WORD_AT + WORD.len() as f32 * LETTER_GAP + 0.45;
    div()
        .w(px(LIPS[0].len() as f32 * DOT))
        .h(px(LIPS.len() as f32 * DOT))
        .child(
            canvas(
                |_, _, _| {},
                |bounds, _, window, _| {
                    paint_sprite(
                        window,
                        &LIPS,
                        bounds.center(),
                        DOT,
                        hsla(340. / 360., 0.85, 0.62, 1.),
                        hsla(340. / 360., 0.70, 0.30, 1.),
                    );
                },
            )
            .size_full(),
        )
        .with_animation("finale-stamp", Animation::new(FINALE), move |el, t| {
            let local = ((t * total - at) / 0.45).clamp(0., 1.);
            let s = ui::spring(local);
            el.opacity(local).relative().top(px((1. - s) * -28.))
        })
}

/// "Sexyyyy", one letter at a time with a little spring; each y lands a
/// touch later and lower than the one before, so the word stretches out.
fn word() -> impl IntoElement {
    let total = FINALE.as_secs_f32();
    h_flex()
        .items_end()
        .children(WORD.chars().enumerate().map(move |(i, letter)| {
            let tail = WORD[..i].matches('y').count() as f32;
            let at = WORD_AT + i as f32 * LETTER_GAP + tail * 0.06;
            div()
                .text_size(px(72. + tail * 4.))
                .line_height(px(84.))
                .font_weight(FontWeight::EXTRA_BOLD)
                .text_color(ui::petal())
                .child(letter.to_string())
                .with_animation(
                    ("finale-letter", i),
                    Animation::new(FINALE),
                    move |el, t| {
                        let local = ((t * total - at) / 0.5).clamp(0., 1.);
                        let s = ui::spring(local);
                        el.opacity(local.min(1.) * 1.0)
                            .relative()
                            .top(px((1. - s) * 36. + tail * 2.))
                    },
                )
        }))
}

/// The whole entrance.
pub(super) fn render(config: BannerConfig) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_1()
        .child(
            div()
                .relative()
                .w(px(STAGE_W))
                .h(px(STAGE_H))
                .flex()
                .items_center()
                .justify_center()
                .child(letters(config))
                .child(kisses()),
        )
        .child(h_flex().items_center().gap_3().child(word()).child(stamp()))
}
