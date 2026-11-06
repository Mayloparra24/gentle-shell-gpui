//! ANSI / 256 / truecolor → the active theme's terminal palette.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::{Hsla, Rgba};

use crate::themes::{self, TermColors};

const fn hex(v: u32) -> Rgb {
    Rgb {
        r: (v >> 16) as u8,
        g: (v >> 8) as u8,
        b: v as u8,
    }
}

/// The active theme's default background, opaque. Only used where an opaque
/// background is needed (inverse video, cursor text, OSC 11 replies); the
/// default cell background stays transparent so the pane glass shows.
pub(crate) fn background_default() -> Rgb {
    themes::term(|t| hex(t.bg))
}

fn indexed_in(t: &TermColors, index: u8) -> Rgb {
    match index {
        0..=15 => hex(t.ansi[index as usize]),
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb {
                r: level(i / 36),
                g: level((i / 6) % 6),
                b: level(i % 6),
            }
        }
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            Rgb { r: v, g: v, b: v }
        }
    }
}

fn dim(rgb: Rgb) -> Rgb {
    let f = |v: u8| (v as u16 * 2 / 3) as u8;
    Rgb {
        r: f(rgb.r),
        g: f(rgb.g),
        b: f(rgb.b),
    }
}

fn named_in(t: &TermColors, color: NamedColor) -> Rgb {
    use NamedColor::*;
    match color {
        Foreground | BrightForeground => hex(t.fg),
        Background => hex(t.bg),
        Cursor => hex(t.cursor),
        DimForeground => hex(t.dim_fg),
        DimBlack | DimRed | DimGreen | DimYellow | DimBlue | DimMagenta | DimCyan | DimWhite => {
            dim(hex(t.ansi[color as usize - DimBlack as usize]))
        }
        normal => hex(t.ansi[normal as usize]),
    }
}

/// Color for an index of the terminal color table (0–255 indexed, 256+ named),
/// honoring OSC 4/10/11 overrides.
pub(crate) fn for_index(index: usize, overrides: &Colors) -> Rgb {
    if let Some(rgb) = overrides[index] {
        return rgb;
    }
    themes::term(|t| match index {
        0..=255 => indexed_in(t, index as u8),
        256 => hex(t.fg),
        257 => hex(t.bg),
        258 => hex(t.cursor),
        259..=266 => dim(hex(t.ansi[index - 259])),
        267 => hex(t.fg),
        _ => hex(t.dim_fg),
    })
}

fn resolve_in(t: &TermColors, color: Color, overrides: &Colors) -> Rgb {
    match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(i) => overrides[i as usize].unwrap_or_else(|| indexed_in(t, i)),
        Color::Named(n) => overrides[n as usize].unwrap_or_else(|| named_in(t, n)),
    }
}

fn foreground_in(t: &TermColors, color: Color, overrides: &Colors, dimmed: bool) -> Rgb {
    let color = match (dimmed, color) {
        (true, Color::Named(n)) => Color::Named(n.to_dim()),
        _ => color,
    };
    let rgb = resolve_in(t, color, overrides);
    match (dimmed, color) {
        (true, Color::Named(_)) => rgb,
        (true, _) => dim(rgb),
        _ => rgb,
    }
}

/// Foreground with the DIM attribute applied.
pub(crate) fn foreground(color: Color, overrides: &Colors, dimmed: bool) -> Rgb {
    if let Color::Spec(rgb) = color {
        // Truecolor needs no palette.
        return if dimmed { dim(rgb) } else { rgb };
    }
    themes::term(|t| foreground_in(t, color, overrides, dimmed))
}

/// Cell background; `None` for the default background so the pane glass shows.
pub(crate) fn background(color: Color, overrides: &Colors) -> Option<Rgb> {
    match color {
        Color::Named(NamedColor::Background) => None,
        Color::Spec(rgb) => Some(rgb),
        other => Some(themes::term(|t| resolve_in(t, other, overrides))),
    }
}

pub(crate) fn hsla(rgb: Rgb) -> Hsla {
    Rgba {
        r: rgb.r as f32 / 255.,
        g: rgb.g as f32 / 255.,
        b: rgb.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::{background, foreground_in, indexed_in, named_in, resolve_in};
    use crate::themes::{self, TermColors};
    use alacritty_terminal::term::color::Colors;
    use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    fn theme(name: &str) -> TermColors {
        let catalog = themes::load_catalog(Vec::new(), Vec::new());
        TermColors::new(catalog.iter().find(|t| t.name == name).unwrap())
    }

    #[test]
    fn ansi_16_uses_gentleman_palette() {
        let t = theme("Gentleman-Cute");
        let none = Colors::default();
        let named = |c| resolve_in(&t, Color::Named(c), &none);
        assert_eq!(named(NamedColor::Red), rgb(0xff, 0x71, 0x8f));
        assert_eq!(named(NamedColor::Green), rgb(0xb4, 0xe7, 0xc7));
        assert_eq!(named(NamedColor::Yellow), rgb(0xf2, 0xb8, 0x6d));
        assert_eq!(named(NamedColor::BrightYellow), rgb(0xe0, 0xc2, 0x7a));
        assert_eq!(named(NamedColor::Blue), rgb(0xa9, 0xc7, 0xee));
        assert_eq!(named(NamedColor::Magenta), rgb(0xf0, 0x95, 0xc8));
        assert_eq!(named(NamedColor::Cyan), rgb(0xc4, 0xda, 0xf6));
        assert_eq!(named(NamedColor::White), rgb(0xf6, 0xef, 0xf3));
        assert_eq!(named(NamedColor::BrightBlack), rgb(0x76, 0x61, 0x6b));
        // Indexed 0–15 are the same table.
        assert_eq!(
            resolve_in(&t, Color::Indexed(1), &none),
            rgb(0xff, 0x71, 0x8f)
        );
        assert_eq!(named_in(&t, NamedColor::Foreground), rgb(0xf6, 0xef, 0xf3));
    }

    #[test]
    fn other_themes_swap_the_table() {
        let t = theme("Dracula");
        let none = Colors::default();
        assert_eq!(
            resolve_in(&t, Color::Named(NamedColor::Red), &none),
            rgb(0xff, 0x55, 0x55)
        );
        assert_eq!(named_in(&t, NamedColor::Background), rgb(0x28, 0x2a, 0x36));
        assert_eq!(indexed_in(&t, 12), rgb(0xd6, 0xac, 0xff));
    }

    #[test]
    fn xterm_256_cube_and_grays() {
        let t = theme("Gentleman-Cute");
        assert_eq!(indexed_in(&t, 16), rgb(0, 0, 0));
        assert_eq!(indexed_in(&t, 196), rgb(255, 0, 0));
        assert_eq!(indexed_in(&t, 46), rgb(0, 255, 0));
        assert_eq!(indexed_in(&t, 21), rgb(0, 0, 255));
        assert_eq!(indexed_in(&t, 231), rgb(255, 255, 255));
        assert_eq!(indexed_in(&t, 110), rgb(135, 175, 215));
        assert_eq!(indexed_in(&t, 232), rgb(8, 8, 8));
        assert_eq!(indexed_in(&t, 255), rgb(238, 238, 238));
    }

    #[test]
    fn truecolor_and_overrides() {
        let t = theme("Gentleman-Cute");
        let mut colors = Colors::default();
        assert_eq!(
            resolve_in(&t, Color::Spec(rgb(1, 2, 3)), &colors),
            rgb(1, 2, 3)
        );
        colors[1] = Some(rgb(9, 9, 9));
        assert_eq!(
            resolve_in(&t, Color::Named(NamedColor::Red), &colors),
            rgb(9, 9, 9)
        );
        assert_eq!(resolve_in(&t, Color::Indexed(1), &colors), rgb(9, 9, 9));
    }

    #[test]
    fn default_background_is_transparent_and_dim_darkens() {
        let t = theme("Gentleman-Cute");
        let none = Colors::default();
        assert_eq!(
            background(Color::Named(NamedColor::Background), &none),
            None
        );
        assert_eq!(
            background(Color::Spec(rgb(1, 2, 3)), &none),
            Some(rgb(1, 2, 3))
        );
        let dimmed = foreground_in(&t, Color::Named(NamedColor::Red), &none, true);
        assert!(dimmed.r < 0xff);
        let dim_true = foreground_in(&t, Color::Spec(rgb(30, 60, 90)), &none, true);
        assert_eq!(dim_true, rgb(20, 40, 60));
        assert_eq!(
            super::foreground(Color::Spec(rgb(30, 60, 90)), &none, true),
            rgb(20, 40, 60)
        );
    }
}
