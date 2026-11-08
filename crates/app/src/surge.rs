//! The max-effort surge: a pixel comet laps the window's edge, the edge
//! flares, and Gentle Shell's pixel rose blooms in the middle with a burst
//! of sparks. Paint only, over everything; it never takes input.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

use gpui_kit::*;

use crate::{banner, ui};

/// The whole show, start to fade.
pub const DURATION: Duration = Duration::from_millis(2200);

/// The comet's path: this far inside the window, following its corners.
const INSET: f32 = 3.;
const RADIUS: f32 = 11.;
/// The comet finishes its lap at this point of the timeline.
const LAP_END: f32 = 0.5;
/// Trail pixels, their spacing along the edge and their size.
const TRAIL: usize = 120;
const STEP: f32 = 4.;
const PIXEL: f32 = 6.;
/// The line the comet leaves behind: dot spacing and size along the edge.
const LINE_STEP: f32 = 2.;
const LINE: f32 = 2.5;
/// Sparks in the rose's burst.
const SPARKS: usize = 40;

/// The surge for run `run` (a new run restarts it).
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

fn paint(bounds: Bounds<Pixels>, t: f32, window: &mut Window) {
    let petal = ui::petal();
    let spark = hsla(petal.h, 0.9, 0.93, 1.);
    let w = bounds.size.width.as_f32() - 2. * INSET;
    let h = bounds.size.height.as_f32() - 2. * INSET;
    if w <= 2. * RADIUS || h <= 2. * RADIUS {
        return;
    }
    let ox = bounds.origin.x.as_f32() + INSET;
    let oy = bounds.origin.y.as_f32() + INSET;
    let square = |x: f32, y: f32, side: f32| {
        Bounds::new(
            point(px(x - side / 2.), px(y - side / 2.)),
            size(px(side), px(side)),
        )
    };

    // 1. The comet laps the edge. The path it covered stays lit as a thin
    //    line (it draws the window's outline), and pixels trail its head,
    //    dimming and shrinking.
    let comet_fade = 1. - ((t - LAP_END) / 0.15).clamp(0., 1.);
    if comet_fade > 0. {
        let length = perimeter(w, h, RADIUS);
        let head = ease_in_out((t / LAP_END).min(1.)) * length;
        let line = petal.opacity(0.6 * comet_fade);
        let mut along = 0.;
        while along < head {
            let (x, y) = perimeter_point(w, h, RADIUS, along);
            window.paint_quad(fill(square(ox + x, oy + y, LINE), line));
            along += LINE_STEP;
        }
        for i in (0..TRAIL).rev() {
            let along = head - i as f32 * STEP;
            if along < 0. {
                continue;
            }
            let (x, y) = perimeter_point(w, h, RADIUS, along);
            let k = 1. - i as f32 / TRAIL as f32;
            let color = if i < 6 { spark } else { petal };
            window.paint_quad(fill(
                square(ox + x, oy + y, PIXEL * (0.4 + 0.6 * k)),
                color.opacity(k * k * comet_fade),
            ));
        }
        // The head's glow.
        let (x, y) = perimeter_point(w, h, RADIUS, head);
        for (side, alpha) in [(56., 0.06), (34., 0.14), (20., 0.3), (11., 0.6)] {
            window.paint_quad(
                fill(
                    square(ox + x, oy + y, side),
                    petal.opacity(alpha * comet_fade),
                )
                .corner_radii(px(side / 2.)),
            );
        }
    }

    // 2. The lap closes and the whole edge flares, then fades.
    let flare = bump(t, LAP_END - 0.05, LAP_END + 0.05, 0.95);
    if flare > 0. {
        let edge = Bounds::new(point(px(ox), px(oy)), size(px(w), px(h)));
        for (width, alpha) in [(14., 0.10), (7., 0.22), (2., 0.85)] {
            window.paint_quad(
                outline(edge, petal.opacity(alpha * flare), BorderStyle::Solid)
                    .border_widths(px(width))
                    .corner_radii(px(RADIUS)),
            );
        }
    }

    // 3. The rose blooms in the middle, in a soft halo.
    let bloom = bump(t, 0.48, 0.64, 1.);
    let cx = ox + w / 2.;
    let cy = oy + h / 2.;
    if bloom > 0. {
        let (dots, (rose_w, rose_h)) = banner::rose();
        let grow = ease_out(((t - 0.48) / 0.52).clamp(0., 1.));
        let height = w.min(h) * 0.42 * (0.85 + 0.15 * grow);
        let dot = height / rose_h as f32;
        let left = cx - rose_w as f32 * dot / 2.;
        let top = cy - height / 2.;
        let halo = height * 0.75;
        for ring in 0..10 {
            let side = halo * (1. - ring as f32 * 0.08);
            window.paint_quad(
                fill(square(cx, cy, side), petal.opacity(0.035 * bloom))
                    .corner_radii(px(side / 2.)),
            );
        }
        let side = px((dot * 0.6).max(1.));
        let ink = petal.opacity(bloom);
        for &(x, y) in dots {
            let origin = point(px(left + x as f32 * dot), px(top + y as f32 * dot));
            window.paint_quad(fill(Bounds::new(origin, size(side, side)), ink));
        }
    }

    // 4. Sparks burst out of the rose.
    let burst = ((t - 0.52) / 0.48).clamp(0., 1.);
    if burst > 0. && burst < 1. {
        let reach = w.min(h) * 0.45;
        for i in 0..SPARKS {
            // Fixed pseudo-random spread so every run looks the same.
            let jitter = ((i * 37 % 11) as f32 / 11.) - 0.5;
            let angle = i as f32 / SPARKS as f32 * TAU + jitter * 0.3;
            let speed = 0.55 + ((i * 53 % 17) as f32 / 17.) * 0.45;
            let r = reach * (0.18 + 0.82 * ease_out(burst) * speed);
            let side = if i % 3 == 0 { 5. } else { 3. };
            let color = if i % 2 == 0 { spark } else { petal };
            window.paint_quad(fill(
                square(cx + angle.cos() * r, cy + angle.sin() * r, side),
                color.opacity((1. - burst).powf(1.5)),
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
    use super::{bump, perimeter, perimeter_point};

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
    fn bump_rises_peaks_and_falls_inside_its_window() {
        assert_eq!(bump(0.1, 0.2, 0.5, 0.9), 0.);
        assert_eq!(bump(0.95, 0.2, 0.5, 0.9), 0.);
        assert!((bump(0.5, 0.2, 0.5, 0.9) - 1.).abs() < 1e-6);
        assert!(bump(0.3, 0.2, 0.5, 0.9) > 0. && bump(0.3, 0.2, 0.5, 0.9) < 1.);
        assert!(bump(0.8, 0.2, 0.5, 0.9) > 0. && bump(0.8, 0.2, 0.5, 0.9) < 1.);
    }
}
