//! Gentle Shell's startup banner: the braille rose and the pixel "Gentle Shell"
//! wordmark, drawn as real pixels instead of terminal glyphs. Art, palettes and
//! `banner.json` semantics come from gentle-pi `extensions/startup-banner.ts`.

use std::path::Path;
use std::sync::LazyLock;

use gpui_kit::*;

const TEXT_LOGO: &[&str] = &[
    "                  ▄▄▄▀▀▀▀▀██                                ▄▄▀▄▄          ▄▄█▀▀▀██   ▄▄▀██               ▄▄▀▄▄   ▄▄▀▄▄",
    "              ▄▄█▀▀▒▒▒▒▒▄▄█▀▒                   ▄██     ▄▄█▀█▄█▀▒▒       ▄█▀▀▒▒▒▄█▀▒ ▄██▄█▀▒           ▄▄█▀█▄█▀▄▄█▀█▄█▀▒",
    "          ▄▄██▀▒▒▒▒▒▄▄▄▀▀▒▒▒▒        ▄▄▄  ▀▀▀▀██▀▀▀▀▀███▀█▄▀▀▒▒▒▒      ██▄▄▒▒▒▒▒▒▒ ▄██▀▀▒▒▒          ▄██▀█▄▀▀▄██▀█▄▀▀▒▒▒",
    "        ▄██▀▒▒▒▒     ▒▒▄▄█ ▄▄▄▀██ ▄▄▄▀▀▀▄  ▄██▀▒▒▒▒▄██▀▀▀▒▄▄███         ▀▀▀██▄▄  ▄██▀▒▄▄▀██   ▄▄▀▀██ ██▀▀▀▒▒▒██▀▀▀▒▒▒",
    "       ██▀▒▒▒     ▄▄▄███▀▄██▀▀▀▄▄██▀▀▄█▀▄▄██▀▒▒▒▄▄██▀▒▒▄██▀▀▀▄▄      ▄▄     ▀██▄▄██▀▄██▀██▒  ▄██▄▄▀▒ █▀▒▒▒   █▀▒▒▒",
    "       ▀█▄▄▄▄▄▀▀▀█▄▄███▄▒▀▀▀▀▀▀▒▀▀▒▒▀▀▀▀▒██▄▄▀▀▀ ▀█▄▀▀▀ ▀▀▀▀▀▒▒      ▀█▄▄▄▄█▀▀▒▒▀▀▒▒▀▀▒ ██▄▄▀▀█▄▄▄▄▀▀█▄▄▀▀▀▄▄▀█▄▄▀▀▀",
    "        ▒▄▄▄█▀▀▀█▄█▀▀▒▒▒▒ ▒▒▒▒▒▒ ▒▒  ▒▒▒▒ ▒▒▒▒▒▒▒ ▒▒▒▒▒▒ ▒▒▒▒▒        ▒▀▀▀▀▒▒▒▒  ▒▒▒▒▒▒ ▒▒▒▒▒ ▒▒▒▒▒▒  ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒",
    "     ▄▄▀▀ ▒▒▒▒▄██▀▒▒▒▒",
    "   ▄█ ▒▒▒▄▄██▀▀▒▒▒▒",
    "    ▀▀▀▀▀▀▒▒▒▒▒▒",
    "     ▒▒▒▒▒▒",
];

const ROSE: &[&str] = &[
    "             ⣠⣾⣷⣶⣦⣤⣤⣄⣠⣄⣀  ⢀⣀⣀",
    "          ⢀⣴⣿⣿⠿⣋⣭⣭⣯⣭⣍⣭⣿⣟⠛⠛⠿⣿⣷⣄",
    "      ⢀⣴⣾⡟⢻⣿⡟⠁⣼⣿⠏⣵⢻⣿⣻⣿⣿⢿⡻⣿⣿⣶⡌⢿⣿⣷⣦⣤⡄",
    "   ⣤⣶⣾⣿⣿⠏ ⠈⢿⣄ ⢹⣏⠠⠟⣾⣿⣿⣿⣿⣿⠷⣏⣼⠟⢡⣿⡟⠋⢻⣿⣿⡄",
    "   ⠈⣿⣿⣿⣿⡆   ⣽⢧⡘⠈⠳⣦⣍⠛⠛⢦⣉⣴⣛⣫⣭⣴⡟⠋  ⣾⣿⣿⡿",
    "   ⢀⠹⣿⣿⣿⣷⣤⡄ ⠋ ⠙⢆ ⣠⠴⠟⠛⣛⣛⣛⠟⠋⠁⠺⡇ ⣀⣴⣿⣿⡟⠁",
    "   ⠈⣀⠈⠛⠷⠿⣿⣿⣷⣤⣀ ⢠⠋   ⠈⠉⠉    ⣠⣴⣥⠾⠛⠉⣰⣿⣷",
    "          ⠹⣯⣝⠛⠛⠷⢶⣤⣤⣀   ⢀⡠⠖⠋⠉⢉⣀⣀⣴⣾⣿⠿⠟⠃",
    "             ⠘⠻⢿⣦⣄⡀  ⠉⠛⢦⠠⢊⠤⠴⢒⣛⣛⣩⣽⡿⠟⠁",
    "        ⠶⢶⣤⣄⡀⠨⠭⠽⠟⣓⢦⣀⠈⢇⡥⠖⠛⠋⠉⠉",
    "           ⠈⢷ ⠐⠂⢤⣽⣄ ⠰⡎⠙⠳⣄⡀ ⠈⢣⠘⢦⠋",
    "            ⠈⢳⣀⡒⠉⠉⣉⠙⡲⣽⣄ ⣏⠳⡄ ⠘⡇ ⡾⠁",
    "              ⠛⠻⢦⣄⣉⡁⣀⣀⣈⣙⣺⣌⡇⢠⢀⡇⡾",
    "                   ⠈⠉    ⠈⠳⡄⣸⢱⠇",
    "                           ⡷⠡⡯⢖⠉",
    "                        ⢀⡴⢪⠔⣉⠔⠋",
    "                           ⠐⠈",
];

/// One pixel of the wordmark: half-block cells are 1×2 pixels; `▒` is the
/// darker drop shadow.
#[derive(Clone, Copy)]
struct Pixel {
    x: u16,
    y: u16,
    shadow: bool,
}

struct Art {
    logo: Vec<Pixel>,
    logo_size: (u16, u16),
    rose: Vec<(u16, u16)>,
    rose_size: (u16, u16),
}

static ART: LazyLock<Art> = LazyLock::new(|| {
    let mut logo = Vec::new();
    let mut logo_w = 0;
    for (row, line) in TEXT_LOGO.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let (x, top, bottom) = (col as u16, row as u16 * 2, row as u16 * 2 + 1);
            let cells: &[(u16, bool)] = match ch {
                '▀' => &[(top, false)],
                '▄' => &[(bottom, false)],
                '█' => &[(top, false), (bottom, false)],
                '▒' => &[(top, true), (bottom, true)],
                _ => &[],
            };
            for &(y, shadow) in cells {
                logo.push(Pixel { x, y, shadow });
                logo_w = logo_w.max(x + 1);
            }
        }
    }
    // Shadows sit behind the ink.
    logo.sort_by_key(|p| !p.shadow);

    // Braille U+2800: bits 0-2 and 6 are the left column (rows 0-3), 3-5 and 7 the right.
    const DOTS: [(u16, u16); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let mut rose = Vec::new();
    let mut rose_w = 0;
    for (row, line) in ROSE.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let code = ch as u32;
            if !(0x2800..=0x28FF).contains(&code) {
                continue;
            }
            let bits = code - 0x2800;
            for (bit, (dx, dy)) in DOTS.iter().enumerate() {
                if bits & (1 << bit) != 0 {
                    rose.push((col as u16 * 2 + dx, row as u16 * 4 + dy));
                    rose_w = rose_w.max(col as u16 * 2 + dx + 1);
                }
            }
        }
    }
    Art {
        logo,
        logo_size: (logo_w, TEXT_LOGO.len() as u16 * 2),
        rose,
        rose_size: (rose_w, ROSE.len() as u16 * 4),
    }
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BannerConfig {
    pub show_rose: bool,
    pub show_text_logo: bool,
    pub palette: Palette,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Pink,
    Cyan,
    Yellow,
    Green,
}

impl Palette {
    /// (rose, logo ink, logo shadow)
    fn colors(self) -> (Rgba, Rgba, Rgba) {
        let c = |r: u32, g: u32, b: u32| rgb((r << 16) | (g << 8) | b);
        match self {
            Self::Pink => (c(255, 118, 195), c(255, 138, 206), c(95, 30, 60)),
            Self::Cyan => (c(95, 210, 255), c(105, 220, 255), c(25, 80, 100)),
            Self::Yellow => (c(255, 210, 95), c(255, 215, 105), c(105, 75, 25)),
            Self::Green => (c(110, 220, 145), c(120, 230, 150), c(30, 95, 50)),
        }
    }
}

impl Default for BannerConfig {
    fn default() -> Self {
        Self {
            show_rose: true,
            show_text_logo: true,
            palette: Palette::Pink,
        }
    }
}

/// `<config>/banner.json`, tolerant like the launcher's own read: missing or
/// malformed fields fall back to the defaults.
pub fn read_config(config_home: &Path) -> BannerConfig {
    let default = BannerConfig::default();
    let Ok(raw) = std::fs::read_to_string(config_home.join("banner.json")) else {
        return default;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return default;
    };
    BannerConfig {
        show_rose: value["showRose"].as_bool().unwrap_or(default.show_rose),
        show_text_logo: value["showTextLogo"]
            .as_bool()
            .unwrap_or(default.show_text_logo),
        palette: match value["color"].as_str() {
            Some("cyan") => Palette::Cyan,
            Some("yellow") => Palette::Yellow,
            Some("green") => Palette::Green,
            _ => Palette::Pink,
        },
    }
}

/// The rose's dots (x, y in dot units) and its size, for other effects that
/// draw it (the max-effort surge).
pub fn rose() -> (&'static [(u16, u16)], (u16, u16)) {
    (&ART.rose, ART.rose_size)
}

/// Gentle Shell's pixel rose, `height` px tall, in `color`.
pub fn pixel_rose(height: f32, color: Hsla) -> impl IntoElement {
    let (dots, (width, rows)) = rose();
    let dot = height / rows.max(1) as f32;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let side = px((dot * 0.55).max(1.0));
            for &(x, y) in dots {
                let origin = point(
                    bounds.origin.x + px(x as f32 * dot),
                    bounds.origin.y + px(y as f32 * dot),
                );
                window.paint_quad(fill(Bounds::new(origin, size(side, side)), color));
            }
        },
    )
    .w(px(width as f32 * dot))
    .h(px(height))
}

/// The banner at `scale` px per wordmark pixel. `reveal` in 0..=1 writes the
/// wordmark left to right and fades the rose in, like the terminal intro.
pub fn banner(config: BannerConfig, scale: f32, reveal: f32) -> impl IntoElement {
    let art = &*ART;
    let (rose_color, ink, shadow) = config.palette.colors();
    let logo_size = size(
        px(art.logo_size.0 as f32 * scale),
        px(art.logo_size.1 as f32 * scale),
    );
    // The rose stands about 1.75× the wordmark's height.
    let dot = logo_size.height.as_f32() * 1.75 / art.rose_size.1 as f32;
    let rose_size = size(
        px(art.rose_size.0 as f32 * dot),
        px(art.rose_size.1 as f32 * dot),
    );
    let reveal = reveal.clamp(0.0, 1.0);

    let rose = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let mut color = rose_color;
            color.a = reveal;
            let side = px((dot * 0.55).max(1.0));
            for &(x, y) in &ART.rose {
                let origin = point(
                    bounds.origin.x + px(x as f32 * dot),
                    bounds.origin.y + px(y as f32 * dot),
                );
                window.paint_quad(fill(Bounds::new(origin, size(side, side)), color));
            }
        },
    )
    .w(rose_size.width)
    .h(rose_size.height);

    let logo = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let limit = ART.logo_size.0 as f32 * reveal;
            // Shadows trail the pen a little, like ink drying.
            for p in &ART.logo {
                if (p.x as f32) > limit {
                    continue;
                }
                let origin = point(
                    bounds.origin.x + px(p.x as f32 * scale),
                    bounds.origin.y + px(p.y as f32 * scale),
                );
                let side = size(px(scale), px(scale));
                window.paint_quad(fill(
                    Bounds::new(origin, side),
                    if p.shadow { shadow } else { ink },
                ));
            }
        },
    )
    .w(logo_size.width)
    .h(logo_size.height);

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(scale * 8.0))
        .when(config.show_rose, |d| d.child(rose))
        .when(config.show_text_logo, |d| d.child(logo))
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    use super::{ART, BannerConfig, Palette, ROSE, read_config};

    #[test]
    fn art_decodes_to_pixels() {
        let art = &*ART;
        assert_eq!(art.logo_size.1, 22);
        assert!(art.logo_size.0 > 100);
        assert!(art.logo.iter().any(|p| p.shadow) && art.logo.iter().any(|p| !p.shadow));
        // `⣿` sets all eight dots.
        assert!(art.rose.len() > 500);
        assert_eq!(art.rose_size.1, ROSE.len() as u16 * 4);
    }

    #[test]
    fn config_falls_back_per_field() {
        let dir = std::env::temp_dir().join(format!("gs-banner-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_config(&dir), BannerConfig::default());
        std::fs::write(
            dir.join("banner.json"),
            r#"{"showRose":false,"color":"cyan"}"#,
        )
        .unwrap();
        assert_eq!(
            read_config(&dir),
            BannerConfig {
                show_rose: false,
                show_text_logo: true,
                palette: Palette::Cyan
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
