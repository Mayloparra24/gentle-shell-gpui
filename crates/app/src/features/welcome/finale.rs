//! The entrance after Continue: the green banner glides to the center with a
//! little spring, rings ripple out of it, pixel sparks burst around it,
//! "Welcome to Gentle Shell." comes in, and the whole screen dissolves into
//! the app. Paint and layout only; it takes no input.

use std::f32::consts::TAU;
use std::time::Duration;

use gpui_kit::component::v_flex;
use gpui_kit::*;

use crate::shared::banner::{BannerConfig, Palette, banner};
use crate::shared::ui;

/// From Continue to the app.
pub(super) const FINALE: Duration = Duration::from_millis(3000);
/// The dissolve starts this far in (0..1).
pub(super) const DISSOLVE: f32 = 0.7;

/// The box the banner, rings and sparks share.
const STAGE_W: f32 = 760.;
const STAGE_H: f32 = 360.;
const BANNER_SCALE: f32 = 2.6;
const RINGS: usize = 3;
const SPARKS: usize = 96;

fn ease_out(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

/// A stable pseudo-random value in 0..1 for spark `i`'s `salt`.
fn noise(i: usize, salt: u32) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.
}

/// The banner in the center, the rings and the sparks around it.
fn stage(config: BannerConfig) -> impl IntoElement {
    let mut config = config;
    config.palette = Palette::Green;
    // A burst of light out of the rose, under everything else: stacked
    // translucent discs, so it fades toward its edge instead of ending in one.
    let flash = (0..5usize).map(|layer| {
        let spread = 0.45 + layer as f32 * 0.14;
        div()
            .absolute()
            .rounded_full()
            .bg(ui::success())
            .with_animation(
                ("finale-flash", layer),
                Animation::new(Duration::from_millis(1200)),
                move |el, t| {
                    let size = (120. + 900. * ease_out(t)) * spread;
                    el.size(px(size))
                        .left(px((STAGE_W - size) / 2.))
                        .top(px((STAGE_H - size) / 2.))
                        .opacity(0.07 * (1. - t).powi(2))
                },
            )
    });
    let rings = (0..RINGS).map(|i| {
        let color = if i % 2 == 0 {
            ui::success()
        } else {
            ui::petal()
        };
        div()
            .absolute()
            .rounded_full()
            .border(px(3.))
            .border_color(color)
            .with_animation(
                ("finale-ring", i),
                Animation::new(Duration::from_millis(1700)),
                move |el, t| {
                    // Each ring leaves a little after the one before it.
                    let local = ((t * 1.7 - i as f32 * 0.22) / 1.1).clamp(0., 1.);
                    let size = 90. + 700. * ease_out(local);
                    let alpha = if local <= 0. {
                        0.
                    } else {
                        (1. - local).powf(1.3)
                    };
                    el.size(px(size))
                        .left(px((STAGE_W - size) / 2.))
                        .top(px((STAGE_H - size) / 2.))
                        .opacity(alpha)
                },
            )
    });
    let sparks = div().absolute().inset_0().with_animation(
        "finale-sparks",
        Animation::new(Duration::from_millis(2000)),
        |el, t| {
            el.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let center = bounds.center();
                        let colors = [ui::success(), ui::petal(), ui::champagne()];
                        for i in 0..SPARKS {
                            // Sparks leave in three quick waves.
                            let start = [0.02, 0.1, 0.18][i % 3];
                            let local = ((t - start) / (1. - start)).clamp(0., 1.);
                            if local <= 0. || local >= 1. {
                                continue;
                            }
                            let angle = i as f32 / SPARKS as f32 * TAU + noise(i, 1) * 0.5;
                            let reach = 180. + 320. * noise(i, 2);
                            let distance = reach * ease_out(local);
                            // A slight fall, like embers.
                            let fall = 50. * local * local;
                            let side = 4. + 5. * noise(i, 3);
                            let alpha = (1. - local).powf(1.2);
                            let color = colors[i % colors.len()];
                            let x = center.x + px(angle.cos() * distance * 1.4);
                            let y = center.y + px(angle.sin() * distance * 0.8 + fall);
                            // A soft glow under each spark, then the spark.
                            for (scale, strength) in [(2.2, 0.12), (1., 1.)] {
                                let s = side * scale;
                                window.paint_quad(
                                    fill(
                                        Bounds::new(
                                            point(x - px(s / 2.), y - px(s / 2.)),
                                            size(px(s), px(s)),
                                        ),
                                        color.opacity(alpha * strength),
                                    )
                                    .corner_radii(px(s / 2. * (scale - 1.).min(1.))),
                                );
                            }
                        }
                    },
                )
                .size_full(),
            )
        },
    );
    let mark = div().flex().items_center().justify_center().with_animation(
        "finale-banner",
        Animation::new(Duration::from_millis(900)),
        move |el, t| {
            let s = ui::spring(t);
            el.child(banner(config, BANNER_SCALE + 0.5 * s, 1.))
        },
    );
    div()
        .relative()
        .w(px(STAGE_W))
        .h(px(STAGE_H))
        .flex()
        .items_center()
        .justify_center()
        .children(flash)
        .children(rings)
        .child(sparks)
        .child(mark)
}

/// The whole entrance: the stage gliding down from where the banner sat,
/// then the welcome line.
pub(super) fn render(config: BannerConfig) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .child(div().child(stage(config)).with_animation(
            "finale-glide",
            Animation::new(Duration::from_millis(750)),
            |el, t| el.relative().top(px(-(1. - ease_out(t)) * 200.)),
        ))
        .child(ui::appear_after(
            "finale-title",
            Duration::from_millis(500),
            16.,
            div()
                .flex()
                .text_size(px(44.))
                .line_height(px(52.))
                .font_weight(FontWeight::BOLD)
                .text_color(ui::fg())
                .child("Welcome to\u{a0}")
                .child(div().text_color(ui::success()).child("Gentle Shell"))
                .child("."),
        ))
        .child(ui::appear_after(
            "finale-line",
            Duration::from_millis(750),
            12.,
            div()
                .max_w(px(520.))
                .text_base()
                .text_center()
                .text_color(ui::muted())
                .child(
                    "Your agents are waiting. Open a tab, describe the work, and let them cook.",
                ),
        ))
}
