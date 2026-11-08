//! The max-effort surge. A pixel comet laps the window's edge, shedding
//! sparks and drawing a shimmering outline; the edge flares; the window
//! shakes harder and harder while energy spirals into a glowing core; then
//! the core hatches: a flash, a shockwave, one last jolt, and Gentle Shell's
//! pixel rose bursts out with a rebound and a shower of sparks. Paint only
//! (plus the root's shake offset); it never takes input.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

use gpui_kit::*;

use crate::{banner, ui};

/// The whole show, start to fade.
pub const DURATION: Duration = Duration::from_millis(4400);

// The timeline, as fractions of `DURATION`.
/// The comet finishes its lap.
const LAP_END: f32 = 0.42;
/// The window starts to tremble and energy gathers in the middle.
const SHAKE_START: f32 = 0.46;
/// The core hatches and the rose bursts out.
const BLOOM: f32 = 0.64;
/// The hard jolt as the rose bursts out lasts this long.
const JOLT: f32 = 0.08;

/// The comet's path: this far inside the window, following its corners.
const INSET: f32 = 3.;
const RADIUS: f32 = 11.;
/// Trail pixels, their spacing along the edge and their size.
const TRAIL: usize = 140;
const STEP: f32 = 4.;
const PIXEL: f32 = 6.;
/// The line the comet leaves behind: dot spacing and size along the edge.
const LINE_STEP: f32 = 2.;
const LINE: f32 = 2.5;
/// Sparks the comet's head sheds, and how long each lives (timeline units).
const SHED: usize = 90;
const SHED_LIFE: f32 = 0.09;
/// Energy pixels spiralling into the core while the window shakes.
const MOTES: usize = 36;
/// Sparks in the hatching burst.
const SPARKS: usize = 56;
/// Peak tremble, in px; the jolt reaches `JOLT_GAIN` times that.
const SHAKE: f32 = 7.;
const JOLT_GAIN: f32 = 1.6;

/// The surge's paint for run `run` (a new run restarts it).
pub fn overlay(run: u64) -> impl IntoElement {
    div().absolute().inset_0().with_animation(
        ElementId::NamedInteger("max-surge".into(), run),
        Animation::new(DURATION),
        |el, t| {
            el.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| paint(bounds, t, window),
                )
                .size_full(),
            )
        },
    )
}

/// The window's shake offset `elapsed` into a run. The window's root applies
/// it in place (it re-renders each frame while a surge plays) rather than
/// wrapping itself in an animation, which would re-key every element inside.
pub fn shake_at(elapsed: Duration) -> (f32, f32) {
    shake(elapsed.as_secs_f32() / DURATION.as_secs_f32())
}

/// The window's offset at `t`: a tremble that builds up before the hatch,
/// one hard jolt as the rose bursts out, then stillness.
fn shake(t: f32) -> (f32, f32) {
    let (amp, speed) = if (SHAKE_START..BLOOM).contains(&t) {
        let build = (t - SHAKE_START) / (BLOOM - SHAKE_START);
        (SHAKE * build * build, 260.)
    } else if (BLOOM..BLOOM + JOLT).contains(&t) {
        let after = (t - BLOOM) / JOLT;
        (SHAKE * JOLT_GAIN * (1. - after).powi(2), 200.)
    } else {
        return (0., 0.);
    };
    // Two detuned waves per axis read as a rumble, not a wobble.
    let k = t * speed;
    let dx = amp * (0.6 * k.sin() + 0.4 * (k * 2.3 + 1.).sin());
    let dy = amp * (0.6 * (k * 1.3 + 2.).sin() + 0.4 * (k * 2.9).sin());
    (dx, dy)
}

fn paint(bounds: Bounds<Pixels>, t: f32, window: &mut Window) {
    let petal = ui::petal();
    let spark = hsla(petal.h, 0.9, 0.93, 1.);
    let wine = hsla(petal.h, petal.s * 0.7, petal.l * 0.4, 1.);
    let w = bounds.size.width.as_f32() - 2. * INSET;
    let h = bounds.size.height.as_f32() - 2. * INSET;
    if w <= 2. * RADIUS || h <= 2. * RADIUS {
        return;
    }
    let ox = bounds.origin.x.as_f32() + INSET;
    let oy = bounds.origin.y.as_f32() + INSET;
    let cx = ox + w / 2.;
    let cy = oy + h / 2.;
    let square = |x: f32, y: f32, side: f32| {
        Bounds::new(
            point(px(x - side / 2.), px(y - side / 2.)),
            size(px(side), px(side)),
        )
    };
    let glow = |window: &mut Window, x: f32, y: f32, rings: &[(f32, f32)], color: Hsla| {
        for &(side, alpha) in rings {
            window.paint_quad(
                fill(square(x, y, side), color.opacity(alpha)).corner_radii(px(side / 2.)),
            );
        }
    };
    let length = perimeter(w, h, RADIUS);
    let lap = |t: f32| ease_in_out((t / LAP_END).clamp(0., 1.)) * length;

    // 1. The comet. The path it covered stays lit as a shimmering line (a
    //    wave of light runs along it); its tail fades from white through rose
    //    to wine; its head sheds sparks that drift into the window.
    let comet_fade = 1. - ((t - LAP_END) / 0.12).clamp(0., 1.);
    if comet_fade > 0. {
        let head = lap(t);
        let mut along = 0.;
        while along < head {
            let (x, y) = perimeter_point(w, h, RADIUS, along);
            let wave = 0.5 + 0.5 * (along / 38. - t * 70.).sin();
            window.paint_quad(fill(
                square(ox + x, oy + y, LINE),
                petal.opacity((0.35 + 0.45 * wave) * comet_fade),
            ));
            along += LINE_STEP;
        }
        for i in 0..SHED {
            let born = i as f32 / SHED as f32 * LAP_END;
            let age = (t - born) / SHED_LIFE;
            if !(0. ..1.).contains(&age) {
                continue;
            }
            let (x, y) = perimeter_point(w, h, RADIUS, lap(born));
            let (sx, sy) = (ox + x, oy + y);
            // Drift toward the middle, a little sideways, slowing down.
            let (vx, vy) = (cx - sx, cy - sy);
            let norm = vx.hypot(vy).max(1.);
            let (dx, dy) = (vx / norm, vy / norm);
            let side_kick = ((i * 29 % 13) as f32 / 13. - 0.5) * 0.8;
            let travel = (26. + (i * 41 % 23) as f32 * 1.6) * ease_out(age);
            let color = if i % 3 == 0 { spark } else { petal };
            window.paint_quad(fill(
                square(
                    sx + (dx - dy * side_kick) * travel,
                    sy + (dy + dx * side_kick) * travel,
                    if i % 4 == 0 { 4. } else { 3. },
                ),
                color.opacity((1. - age) * comet_fade),
            ));
        }
        for i in (0..TRAIL).rev() {
            let along = head - i as f32 * STEP;
            if along < 0. {
                continue;
            }
            let (x, y) = perimeter_point(w, h, RADIUS, along);
            let k = 1. - i as f32 / TRAIL as f32;
            let color = if k > 0.94 {
                spark
            } else if k > 0.45 {
                petal
            } else {
                wine
            };
            window.paint_quad(fill(
                square(ox + x, oy + y, PIXEL * (0.35 + 0.65 * k)),
                color.opacity(k.powf(1.6) * comet_fade),
            ));
        }
        let (x, y) = perimeter_point(w, h, RADIUS, head);
        let pulse = 1. + 0.15 * (t * 90.).sin();
        glow(
            window,
            ox + x,
            oy + y,
            &[
                (64. * pulse, 0.05 * comet_fade),
                (38. * pulse, 0.13 * comet_fade),
                (22., 0.3 * comet_fade),
                (12., 0.65 * comet_fade),
            ],
            petal,
        );
        window.paint_quad(fill(square(ox + x, oy + y, 5.), spark.opacity(comet_fade)));
    }

    // 2. The lap closes and the edge flares; it glows on, beating with the
    //    tremble, until the hatch.
    let flare = bump(t, LAP_END - 0.04, LAP_END + 0.04, BLOOM + 0.1);
    if flare > 0. {
        let beat = if (SHAKE_START..BLOOM).contains(&t) {
            0.75 + 0.25 * (t * 120.).sin()
        } else {
            1.
        };
        let edge = Bounds::new(point(px(ox), px(oy)), size(px(w), px(h)));
        for (width, alpha) in [(22., 0.06), (12., 0.12), (6., 0.25), (2., 0.9)] {
            window.paint_quad(
                outline(
                    edge,
                    petal.opacity(alpha * flare * beat),
                    BorderStyle::Solid,
                )
                .border_widths(px(width))
                .corner_radii(px(RADIUS)),
            );
        }
    }

    // 3. Energy gathers: pixels spiral into a core that swells and pulses.
    if (SHAKE_START..BLOOM).contains(&t) {
        let build = (t - SHAKE_START) / (BLOOM - SHAKE_START);
        let reach = w.min(h) * 0.48;
        for i in 0..MOTES {
            // Each mote starts at its own moment and falls in along a spiral.
            let delay = (i * 7 % MOTES) as f32 / MOTES as f32 * 0.6;
            let p = ((build - delay) / 0.4).clamp(0., 1.);
            if p <= 0. || p >= 1. {
                continue;
            }
            let angle = i as f32 / MOTES as f32 * TAU + p * 2.4;
            let r = reach * (1. - ease_in_out(p));
            let color = if i % 2 == 0 { spark } else { petal };
            window.paint_quad(fill(
                square(cx + angle.cos() * r, cy + angle.sin() * r, 4.),
                color.opacity(0.3 + 0.7 * p),
            ));
        }
        let core = 6. + 34. * build * build;
        let pulse = 1. + 0.18 * (t * 140.).sin() * build;
        glow(
            window,
            cx,
            cy,
            &[
                (core * 4. * pulse, 0.05 + 0.08 * build),
                (core * 2.2 * pulse, 0.12 + 0.15 * build),
                (core * pulse, 0.5 + 0.4 * build),
            ],
            petal,
        );
        window.paint_quad(fill(square(cx, cy, core * 0.35), spark.opacity(0.9)));
    }

    // 4. The hatch: a flash across the window and a shockwave ring.
    let flash = bump(t, BLOOM - 0.005, BLOOM + 0.01, BLOOM + 0.09);
    if flash > 0. {
        window.paint_quad(fill(bounds, spark.opacity(0.32 * flash)));
    }
    let wave = (t - BLOOM) / 0.22;
    if (0. ..1.).contains(&wave) {
        let r = w.hypot(h) * 0.55 * ease_out(wave);
        for (width, alpha) in [(18., 0.12), (6., 0.35), (2., 0.9)] {
            window.paint_quad(
                outline(
                    square(cx, cy, 2. * r),
                    petal.opacity(alpha * (1. - wave)),
                    BorderStyle::Solid,
                )
                .border_widths(px(width))
                .corner_radii(px(r)),
            );
        }
    }

    // 5. The rose bursts out, overshooting and settling, in a soft halo,
    //    then holds and fades. It is white-hot for an instant.
    let shown = if t < BLOOM {
        0.
    } else if t < BLOOM + 0.03 {
        (t - BLOOM) / 0.03
    } else if t < 0.88 {
        1.
    } else {
        1. - ease_in_out((t - 0.88) / 0.12)
    };
    if shown > 0. {
        let (dots, (rose_w, rose_h)) = banner::rose();
        let grow = ui::spring(((t - BLOOM) / 0.16).clamp(0., 1.));
        let height = w.min(h) * 0.46 * (0.35 + 0.65 * grow);
        let dot = height / rose_h as f32;
        let left = cx - rose_w as f32 * dot / 2.;
        let top = cy - height / 2.;
        let halo = height * 0.8;
        for ring in 0..12 {
            let side = halo * (1. - ring as f32 * 0.07);
            window.paint_quad(
                fill(square(cx, cy, side), petal.opacity(0.03 * shown)).corner_radii(px(side / 2.)),
            );
        }
        let side = px((dot * 0.6).max(1.));
        let heat = (1. - (t - BLOOM) / 0.08).clamp(0., 1.);
        let ink = hsla(
            petal.h,
            petal.s + (spark.s - petal.s) * heat,
            petal.l + (spark.l - petal.l) * heat,
            shown,
        );
        for &(x, y) in dots {
            let origin = point(px(left + x as f32 * dot), px(top + y as f32 * dot));
            window.paint_quad(fill(Bounds::new(origin, size(side, side)), ink));
        }
    }

    // 6. Sparks shower out of the hatch and fall a little as they fade.
    let burst = (t - BLOOM) / (1. - BLOOM);
    if (0. ..1.).contains(&burst) {
        let reach = w.min(h) * 0.55;
        for i in 0..SPARKS {
            // Fixed pseudo-random spread so every run looks the same.
            let jitter = ((i * 37 % 11) as f32 / 11.) - 0.5;
            let angle = i as f32 / SPARKS as f32 * TAU + jitter * 0.35;
            let speed = 0.45 + ((i * 53 % 17) as f32 / 17.) * 0.55;
            let r = reach * (0.1 + 0.9 * ease_out(burst) * speed);
            let fall = 90. * burst * burst;
            let side = if i % 3 == 0 { 5. } else { 3. };
            let color = if i % 2 == 0 { spark } else { petal };
            window.paint_quad(fill(
                square(cx + angle.cos() * r, cy + angle.sin() * r + fall, side),
                color.opacity((1. - burst).powf(1.3)),
            ));
        }
    }
}

/// Length of a `w` × `h` rectangle's edge with corners rounded to `r`.
fn perimeter(w: f32, h: f32, r: f32) -> f32 {
    2. * (w - 2. * r) + 2. * (h - 2. * r) + TAU * r
}

/// The point `along` that edge, clockwise from the middle of the top side
/// (the rectangle's top-left corner is the origin).
fn perimeter_point(w: f32, h: f32, r: f32, along: f32) -> (f32, f32) {
    let (side_w, side_h, arc) = (w - 2. * r, h - 2. * r, FRAC_PI_2 * r);
    let mut d = (along + side_w / 2.).rem_euclid(perimeter(w, h, r));
    if d < side_w {
        return (r + d, 0.);
    }
    d -= side_w;
    if d < arc {
        let a = d / r;
        return (w - r + r * a.sin(), r - r * a.cos());
    }
    d -= arc;
    if d < side_h {
        return (w, r + d);
    }
    d -= side_h;
    if d < arc {
        let a = d / r;
        return (w - r + r * a.cos(), h - r + r * a.sin());
    }
    d -= arc;
    if d < side_w {
        return (w - r - d, h);
    }
    d -= side_w;
    if d < arc {
        let a = d / r;
        return (r - r * a.sin(), h - r + r * a.cos());
    }
    d -= arc;
    if d < side_h {
        return (0., h - r - d);
    }
    d -= side_h;
    let a = d / r;
    (r - r * a.cos(), r - r * a.sin())
}

/// 0 before `start`, rising to 1 at `peak`, back to 0 at `end`.
fn bump(t: f32, start: f32, peak: f32, end: f32) -> f32 {
    if t <= start || t >= end {
        0.
    } else if t < peak {
        ease_out((t - start) / (peak - start))
    } else {
        1. - ease_in_out((t - peak) / (end - peak))
    }
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4. * t * t * t
    } else {
        1. - (-2. * t + 2.).powi(3) / 2.
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BLOOM, JOLT, JOLT_GAIN, SHAKE, SHAKE_START, bump, perimeter, perimeter_point, shake,
    };

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn the_comet_laps_the_rounded_edge_clockwise_from_the_top() {
        let (w, h, r) = (800., 600., 10.);
        let length = perimeter(w, h, r);
        // Starts at the top's middle and ends a lap where it began.
        assert!(close(perimeter_point(w, h, r, 0.), (400., 0.)));
        assert!(close(perimeter_point(w, h, r, length), (400., 0.)));
        // Clockwise: a quarter of the top side later it is to the right.
        assert!(close(perimeter_point(w, h, r, 100.), (500., 0.)));
        // Middle of the right side, bottom and left.
        let arc = std::f32::consts::FRAC_PI_2 * r;
        let to_right = (w - 2. * r) / 2. + arc + (h - 2. * r) / 2.;
        assert!(close(perimeter_point(w, h, r, to_right), (800., 300.)));
        assert!(close(perimeter_point(w, h, r, length / 2.), (400., 600.)));
        let to_left = length - ((w - 2. * r) / 2. + arc + (h - 2. * r) / 2.);
        assert!(close(perimeter_point(w, h, r, to_left), (0., 300.)));
        // Every point stays on the edge: on a side, or `r` from a corner's center.
        for i in 0..400 {
            let (x, y) = perimeter_point(w, h, r, length * i as f32 / 400.);
            let on_side =
                x.abs() < 0.01 || (x - w).abs() < 0.01 || y.abs() < 0.01 || (y - h).abs() < 0.01;
            let cx = x.clamp(r, w - r);
            let cy = y.clamp(r, h - r);
            let on_corner = ((x - cx).hypot(y - cy) - r).abs() < 0.01;
            assert!(on_side || on_corner, "({x}, {y}) is off the edge");
        }
    }

    #[test]
    fn the_window_trembles_harder_before_the_hatch_then_stills() {
        let peak = |from: f32, to: f32| {
            (0..200)
                .map(|i| {
                    let (dx, dy) = shake(from + (to - from) * i as f32 / 200.);
                    dx.abs().max(dy.abs())
                })
                .fold(0., f32::max)
        };
        // Still during the comet and after the jolt.
        assert_eq!(shake(0.2), (0., 0.));
        assert_eq!(shake(BLOOM + JOLT + 0.01), (0., 0.));
        assert_eq!(shake(0.99), (0., 0.));
        // The tremble builds: its last stretch is stronger than its first.
        let span = BLOOM - SHAKE_START;
        assert!(
            peak(BLOOM - span * 0.25, BLOOM) > 2. * peak(SHAKE_START, SHAKE_START + span * 0.25)
        );
        // The jolt is the hardest moment, within its limit.
        let jolt = peak(BLOOM, BLOOM + JOLT * 0.3);
        assert!(jolt > peak(SHAKE_START, BLOOM));
        assert!(jolt <= SHAKE * JOLT_GAIN);
    }

    #[test]
    fn bump_rises_peaks_and_falls_inside_its_window() {
        assert_eq!(bump(0.1, 0.2, 0.5, 0.9), 0.);
        assert_eq!(bump(0.95, 0.2, 0.5, 0.9), 0.);
        assert!((bump(0.5, 0.2, 0.5, 0.9) - 1.).abs() < 1e-6);
        assert!(bump(0.3, 0.2, 0.5, 0.9) > 0. && bump(0.3, 0.2, 0.5, 0.9) < 1.);
        assert!(bump(0.8, 0.2, 0.5, 0.9) > 0. && bump(0.8, 0.2, 0.5, 0.9) < 1.);
    }
}
