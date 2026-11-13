//! The reminder itself: pixel water drops rain over the window, each one
//! splashing with a ripple where it lands, while a card in the middle
//! fills a pixel glass drop by drop and says it's time for some water.
//! Then everything fades. Paint and layout only; it never takes input.
//! With reduced motion only the card shows, its glass already full.

use std::time::Duration;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::kisses::{noise, paint_sprite};
use crate::shared::ui;

/// The whole show, first drop to fade.
pub const DURATION: Duration = Duration::from_millis(6000);

/// A pixel drop: `#` water, `o` the glint on its left.
const DROP: [&str; 9] = [
    "...#...", "..###..", ".#####.", ".#o###.", "#o#####", "#o#####", "#######", ".#####.",
    "..###..",
];
const DROPS: usize = 28;
/// When the drops start falling (they're spread over this much of the
/// timeline) and how long one falls.
const RAIN_FOR: f32 = 0.42;
const FALL: f32 = 0.13;
const FALL_SPREAD: f32 = 0.07;
/// A splash lasts this long after its drop lands.
const SPLASH: f32 = 0.1;
/// Ripple dots, and the splash pixels thrown up.
const RIPPLE_DOTS: usize = 18;
const SPRAY: usize = 5;
/// The card rises in, holds, then fades with the rain.
const CARD_IN: (f32, f32) = (0.08, 0.2);
const FADE: (f32, f32) = (0.86, 0.98);
/// The glass, in pixels of `GLASS_DOT` px.
const GLASS_COLS: usize = 13;
const GLASS_ROWS: usize = 16;
const GLASS_DOT: f32 = 4.;

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// 0 before `range`, 1 after it, smooth between.
fn between(t: f32, (from, to): (f32, f32)) -> f32 {
    smoothstep((t - from) / (to - from))
}

/// Drop `i`'s start and fall time, as fractions of the timeline.
fn timing(i: usize) -> (f32, f32) {
    (
        0.02 + RAIN_FOR * noise(i, 2),
        FALL + FALL_SPREAD * noise(i, 3),
    )
}

/// How full the glass is at `t`: the share of drops that have landed.
fn level(t: f32) -> f32 {
    let landed = (0..DROPS)
        .filter(|&i| {
            let (start, fall) = timing(i);
            t >= start + fall
        })
        .count();
    landed as f32 / DROPS as f32
}

/// Water and its glint, from the theme's informational blue.
fn water() -> (Hsla, Hsla) {
    let info = ui::info();
    let glint = hsla(info.h, info.s * 0.5, (info.l + 0.3).min(0.95), 1.);
    (info, glint)
}

/// Every reminder's paint; `run` restarts it, `every` (minutes) is shown
/// on the card.
pub(super) fn overlay(run: u64, every: u32, still: bool) -> impl IntoElement {
    div().absolute().inset_0().with_animation(
        ElementId::NamedInteger("water-reminder".into(), run),
        Animation::new(DURATION),
        move |el, t| {
            let fade = 1. - between(t, FADE);
            el.when(!still, |el| {
                el.child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| rain(bounds, t, fade, window),
                    )
                    .absolute()
                    .size_full(),
                )
            })
            .child(card(t, fade, every, if still { 1. } else { level(t) }))
        },
    )
}

fn rain(bounds: Bounds<Pixels>, t: f32, fade: f32, window: &mut Window) {
    let (body, glint) = water();
    let (left, top) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
    let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    for i in 0..DROPS {
        let (start, fall) = timing(i);
        let age = (t - start) / fall;
        if age < 0. {
            continue;
        }
        let x = left + w * (0.05 + 0.9 * noise(i, 1));
        let floor = top + h - 24. - 90. * noise(i, 4);
        let dot = 3. + 2. * noise(i, 5);
        if age < 1. {
            // Gravity: slow at first, fast at the end.
            let y = top - 30. + (floor - top + 30.) * age * age;
            paint_sprite(
                window,
                &DROP,
                point(px(x), px(y)),
                dot,
                body.opacity(fade),
                glint.opacity(fade),
            );
            continue;
        }
        let s = (t - start - fall) / SPLASH;
        if s >= 1. {
            continue;
        }
        let alpha = (1. - s) * fade;
        // A flat ripple spreading on the "surface".
        let radius = 6. + 34. * smoothstep(s);
        for k in 0..RIPPLE_DOTS {
            let a = k as f32 / RIPPLE_DOTS as f32 * std::f32::consts::TAU;
            let at = point(
                px(x + radius * a.cos() - 1.5),
                px(floor + radius * 0.28 * a.sin() - 1.5),
            );
            window.paint_quad(fill(
                Bounds::new(at, size(px(3.), px(3.))),
                glint.opacity(alpha * 0.8),
            ));
        }
        // A few pixels thrown up and out, falling back.
        for k in 0..SPRAY {
            let side = k as f32 - (SPRAY - 1) as f32 / 2.;
            let at = point(
                px(x + side * 9. * s - dot / 2.),
                px(floor - 26. * 4. * s * (1. - s) * (1. - 0.15 * side.abs()) - dot / 2.),
            );
            window.paint_quad(fill(
                Bounds::new(at, size(px(dot * 0.9), px(dot * 0.9))),
                body.opacity(alpha),
            ));
        }
    }
}

/// A pixel glass, `level` (0..1) full.
fn glass(level: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (body, glint) = water();
            let wall = ui::fg().opacity(0.55);
            let (left, top) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
            let cell = |window: &mut Window, col: usize, row: usize, color: Hsla| {
                let at = point(
                    px(left + col as f32 * GLASS_DOT),
                    px(top + row as f32 * GLASS_DOT),
                );
                let side = px(GLASS_DOT * 0.9);
                window.paint_quad(fill(Bounds::new(at, size(side, side)), color));
            };
            let inside = GLASS_ROWS - 2;
            let filled = (level.clamp(0., 1.) * inside as f32).round() as usize;
            for row in 0..GLASS_ROWS {
                // Slightly narrower at the bottom.
                let taper = row * 2 / GLASS_ROWS;
                let (first, last) = (taper, GLASS_COLS - 1 - taper);
                if row == GLASS_ROWS - 1 {
                    for col in first..=last {
                        cell(window, col, row, wall);
                    }
                    continue;
                }
                cell(window, first, row, wall);
                cell(window, last, row, wall);
                // Water fills the rows above the base, from the bottom up.
                let from_bottom = GLASS_ROWS - 1 - row;
                if (1..=filled).contains(&from_bottom) && row > 0 {
                    let color = if from_bottom == filled { glint } else { body };
                    for col in first + 1..last {
                        cell(window, col, row, color);
                    }
                }
            }
        },
    )
    .flex_none()
    .w(px(GLASS_COLS as f32 * GLASS_DOT))
    .h(px(GLASS_ROWS as f32 * GLASS_DOT))
}

fn card(t: f32, fade: f32, every: u32, level: f32) -> impl IntoElement {
    let shown = between(t, CARD_IN) * fade;
    let rise = (1. - ui::spring(((t - CARD_IN.0) / (CARD_IN.1 - CARD_IN.0)).clamp(0., 1.))) * 14.;
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div().relative().top(px(rise)).opacity(shown).child(
                h_flex()
                    .gap_5()
                    .px_6()
                    .py_5()
                    .rounded_2xl()
                    .bg(ui::popover_bg())
                    .border_1()
                    .border_color(ui::border_strong())
                    .shadow_2xl()
                    .child(glass(level))
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui::fg())
                                    .child("Time for some water"),
                            )
                            .child(div().text_sm().text_color(ui::muted()).child(format!(
                                "A glass now, and another in {}.",
                                super::label(every)
                            ))),
                    ),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::{FADE, level};

    #[test]
    fn the_glass_is_full_before_the_card_fades() {
        assert_eq!(level(0.), 0.);
        // Every drop has landed (and filled the glass) before the fade.
        assert_eq!(level(FADE.0), 1.);
    }
}
