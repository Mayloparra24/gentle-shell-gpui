//! Pixel kisses: Gentle Shell's lips and heart sprites, painted dot by dot.
//! The welcome finale blows a handful from the rose; the composer blows
//! small ones from the caret while you type.

use gpui_kit::*;

/// Pixel lips: `#` the lips, `o` the line between them.
pub const LIPS: [&str; 6] = [
    "..###.###..",
    ".#########.",
    "###########",
    "#ooooooooo#",
    ".#########.",
    "..#######..",
];
/// A pixel heart.
pub const HEART: [&str; 6] = [
    ".##.##.", "#######", "#######", ".#####.", "..###..", "...#...",
];

/// A stable pseudo-random value in 0..1 for kiss `i`'s `salt`.
pub fn noise(i: usize, salt: u32) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.
}

/// Paint `sprite` centered on `center`, `dot` px per pixel.
pub fn paint_sprite(
    window: &mut Window,
    sprite: &[&str],
    center: Point<Pixels>,
    dot: f32,
    body: Hsla,
    line: Hsla,
) {
    let rows = sprite.len() as f32;
    let cols = sprite[0].len() as f32;
    let left = center.x - px(cols * dot / 2.);
    let top = center.y - px(rows * dot / 2.);
    for (y, row) in sprite.iter().enumerate() {
        for (x, cell) in row.chars().enumerate() {
            let color = match cell {
                '#' => body,
                'o' => line,
                _ => continue,
            };
            let origin = point(left + px(x as f32 * dot), top + px(y as f32 * dot));
            window.paint_quad(fill(
                Bounds::new(origin, size(px(dot * 0.9), px(dot * 0.9))),
                color,
            ));
        }
    }
}
