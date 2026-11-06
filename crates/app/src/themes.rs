//! Themes: the catalog (Gentle Shell's own Pi themes, the user's Pi themes,
//! popular terminal schemes), the active choice and its persistence.
//!
//! The catalog and the selection live in the [`Themes`] gpui global. Views read
//! colors through `ui::*` and the terminal through [`term`], which take no
//! `cx`, so the resolved colors of the active theme are mirrored in a
//! process-wide lock that [`select`] and [`init`] keep in step.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use gpui_kit::{App, Global};
use serde_json::Value;

use crate::ui;

/// The theme used when nothing else is chosen (and the fallback for unknown names).
pub const DEFAULT_THEME: &str = "Gentleman-Cute";

/// Gentle Shell's themes, embedded so the app looks right before (or
/// without) finding the installed `gentle-pi`; the installed files win.
const EMBEDDED: &[&str] = &[
    include_str!("../themes/Gentleman-Cute.json"),
    include_str!("../themes/Gentleman-Sexy.json"),
    include_str!("../themes/Gentle.json"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// `<gentle-pi>/themes/`.
    Gentle,
    /// `<agent dir>/themes/`.
    Pi,
    /// Popular terminal schemes shipped with the app.
    Builtin,
}

/// One theme, as opaque `0xRRGGBB` colors. UI roles are resolved from it by
/// `ui::Palette::new`; the terminal reads `ansi`, `fg`, `bg`, `cursor`.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub source: Source,
    pub light: bool,
    pub bg: u32,
    pub fg: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub accent_active: u32,
    /// User messages, selected rows.
    pub raised: u32,
    /// The strong border.
    pub border: u32,
    /// Tool titles, headings.
    pub heading: u32,
    pub info: u32,
    pub success: u32,
    pub danger: u32,
    pub warning: u32,
    pub cursor: u32,
    pub selection: u32,
    /// Normal 0–7, bright 8–15.
    pub ansi: [u32; 16],
}

// ── Color math ─────────────────────────────────────────────────────────────

/// Linear blend of two `0xRRGGBB` colors: `t = 0` is `a`, `t = 1` is `b`.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let channel = |shift: u32| {
        let x = ((a >> shift) & 0xff) as f32;
        let y = ((b >> shift) & 0xff) as f32;
        ((x + (y - x) * t).round().clamp(0., 255.) as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// Relative luminance (0 black … 1 white), good enough to tell light from dark.
fn luminance(c: u32) -> f32 {
    let r = ((c >> 16) & 0xff) as f32 / 255.;
    let g = ((c >> 8) & 0xff) as f32 / 255.;
    let b = (c & 0xff) as f32 / 255.;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// xterm's default 256-color table (Pi themes may name colors by index).
fn xterm256(index: u8) -> u32 {
    const BASE: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5, 0x7f7f7f,
        0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    match index {
        0..=15 => BASE[index as usize],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        232..=255 => {
            let v = 8 + (index - 232) as u32 * 10;
            (v << 16) | (v << 8) | v
        }
    }
}

fn parse_hex(text: &str) -> Option<u32> {
    let hex = text.strip_prefix('#')?;
    match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok(),
        3 => {
            let v = u32::from_str_radix(hex, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(((r * 0x11) << 16) | ((g * 0x11) << 8) | (b * 0x11))
        }
        _ => None,
    }
}

// ── Pi theme files ─────────────────────────────────────────────────────────

/// Pi's theme format (`theme-schema.json`): `vars` of hex colors or 256-color
/// indices, and `colors` whose values are a hex color, an index, a var name, or
/// `""` (terminal default). Terminal colors are not part of the format, so the
/// ANSI table comes from ANSI-named vars when present, else from the roles.
pub fn parse_pi_theme(text: &str, source: Source) -> Option<Theme> {
    let value: Value = serde_json::from_str(text).ok()?;
    let name = value.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let vars = value.get("vars").cloned().unwrap_or(Value::Null);
    let colors = value.get("colors").cloned().unwrap_or(Value::Null);
    let export = value.get("export").cloned().unwrap_or(Value::Null);

    fn resolve(value: &Value, vars: &Value, depth: u8) -> Option<u32> {
        match value {
            Value::Number(n) => n.as_u64().filter(|i| *i < 256).map(|i| xterm256(i as u8)),
            Value::String(s) if s.starts_with('#') => parse_hex(s),
            Value::String(s) if !s.is_empty() && depth < 8 => {
                resolve(vars.get(s.as_str())?, vars, depth + 1)
            }
            _ => None,
        }
    }
    let color = |key: &str| colors.get(key).and_then(|v| resolve(v, &vars, 0));
    let var = |key: &str| vars.get(key).and_then(|v| resolve(v, &vars, 0));

    let fg_guess = color("text");
    let bg = export
        .get("pageBg")
        .and_then(|v| resolve(v, &vars, 0))
        .or_else(|| var("bg"));
    let light = match (bg, fg_guess) {
        (Some(bg), _) => luminance(bg) > 0.5,
        (None, Some(fg)) => luminance(fg) < 0.5,
        (None, None) => false,
    };
    let bg = bg.unwrap_or(if light { 0xffffff } else { 0x0e0e11 });
    let fg = fg_guess.unwrap_or(if light { 0x1f2328 } else { 0xe6e6e6 });
    let muted = color("muted").unwrap_or_else(|| mix(fg, bg, 0.35));
    let faint = color("dim").unwrap_or_else(|| mix(fg, bg, 0.55));
    let accent = color("accent").unwrap_or(fg);
    let accent_active = color("borderAccent")
        .filter(|c| *c != accent)
        .unwrap_or_else(|| mix(accent, fg, 0.3));
    let success = color("success").unwrap_or(0x8ccf7e);
    let danger = color("error").unwrap_or(0xe06c75);
    let warning = color("warning").unwrap_or(0xe5c07b);
    let keyword = color("syntaxKeyword").unwrap_or(accent);
    let operator = color("syntaxOperator").unwrap_or(muted);

    let mut ansi = derived_ansi(
        light,
        bg,
        fg,
        faint,
        danger,
        success,
        warning,
        color("syntaxFunction").unwrap_or(accent),
        keyword,
        operator,
    );
    const ANSI_VARS: [&str; 8] = [
        "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
    ];
    for (i, base) in ANSI_VARS.iter().enumerate() {
        if let Some(c) = var(base) {
            ansi[i] = c;
        }
        let capitalized = format!("{}{}", base[..1].to_uppercase(), &base[1..]);
        if let Some(c) = var(&format!("bright{capitalized}")) {
            ansi[i + 8] = c;
        }
    }
    if let Some(c) = var("purple") {
        ansi[5] = c;
    }
    if let Some(table) = gentle_ansi(name) {
        ansi = table;
    }
    let info = var("blue").unwrap_or_else(|| color("syntaxFunction").unwrap_or(ansi[4]));

    Some(Theme {
        name: name.to_string(),
        source,
        light,
        bg,
        fg,
        muted,
        faint,
        accent,
        accent_active,
        raised: color("userMessageBg").unwrap_or_else(|| mix(bg, accent, 0.14)),
        border: color("border").unwrap_or_else(|| mix(bg, fg, 0.25)),
        heading: color("toolTitle")
            .or_else(|| color("mdHeading"))
            .unwrap_or(warning),
        info,
        success,
        danger,
        warning,
        cursor: accent_active,
        selection: color("selectedBg").unwrap_or_else(|| mix(bg, accent, 0.25)),
        ansi,
    })
}

/// A 16-color table from a theme's roles.
#[allow(clippy::too_many_arguments)]
fn derived_ansi(
    light: bool,
    bg: u32,
    fg: u32,
    faint: u32,
    red: u32,
    green: u32,
    yellow: u32,
    blue: u32,
    magenta: u32,
    cyan: u32,
) -> [u32; 16] {
    let (black, white) = if light {
        (mix(fg, bg, 0.1), mix(bg, fg, 0.25))
    } else {
        (mix(bg, fg, 0.12), mix(fg, bg, 0.05))
    };
    let bright = |c: u32| {
        if light {
            mix(c, 0x000000, 0.15)
        } else {
            mix(c, 0xffffff, 0.25)
        }
    };
    [
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        faint,
        bright(red),
        bright(green),
        bright(yellow),
        bright(blue),
        bright(magenta),
        bright(cyan),
        if light { fg } else { 0xffffff },
    ]
}

/// Hand-tuned terminal tables for Gentle Shell's own themes.
fn gentle_ansi(name: &str) -> Option<[u32; 16]> {
    Some(match name {
        "Gentleman-Cute" => [
            0x1b1219, 0xff718f, 0xb4e7c7, 0xf2b86d, 0xa9c7ee, 0xf095c8, 0xc4daf6, 0xf6eff3,
            0x76616b, 0xff94ab, 0xc9f0d7, 0xe0c27a, 0xc0d6f3, 0xffb1dd, 0xd8e7fa, 0xffffff,
        ],
        "Gentleman-Sexy" => [
            0x1b1219, 0xff718f, 0xb4e7c7, 0xf2b86d, 0xa9c7ee, 0xf43888, 0xc4daf6, 0xf6eff3,
            0x76616b, 0xff94ab, 0xc9f0d7, 0xe0c27a, 0xc49bff, 0xff4f9a, 0xd8e7fa, 0xffffff,
        ],
        "Gentle" => [
            0x161d2e, 0xcb7c94, 0xb7cc85, 0xdeba87, 0x7fb4ca, 0xc99ad6, 0xa3b5d6, 0xf3f6f9,
            0x8394a3, 0xe0a3b5, 0xd1e8a9, 0xe0c15a, 0x8fb8dd, 0xb99bf2, 0xc3d1e8, 0xffffff,
        ],
        _ => return None,
    })
}

/// Every `*.json` Pi theme in `dir`, sorted by name; unreadable files are skipped.
pub fn read_theme_dir(dir: &Path, source: Source) -> Vec<Theme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut themes: Vec<Theme> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| parse_pi_theme(&std::fs::read_to_string(p).ok()?, source))
        .collect();
    themes.sort_by_key(|t| t.name.to_lowercase());
    themes
}

// ── Popular schemes ────────────────────────────────────────────────────────

struct Scheme {
    name: &'static str,
    light: bool,
    bg: u32,
    fg: u32,
    cursor: u32,
    selection: u32,
    accent: u32,
    ansi: [u32; 16],
}

#[rustfmt::skip]
const SCHEMES: &[Scheme] = &[
    Scheme { name: "Dracula", light: false, bg: 0x282a36, fg: 0xf8f8f2, cursor: 0xf8f8f2, selection: 0x44475a, accent: 0xbd93f9,
        ansi: [0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
               0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff] },
    Scheme { name: "Nord", light: false, bg: 0x2e3440, fg: 0xd8dee9, cursor: 0xd8dee9, selection: 0x434c5e, accent: 0x88c0d0,
        ansi: [0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
               0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4] },
    Scheme { name: "Tokyo Night", light: false, bg: 0x1a1b26, fg: 0xc0caf5, cursor: 0xc0caf5, selection: 0x283457, accent: 0x7aa2f7,
        ansi: [0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
               0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5] },
    Scheme { name: "Catppuccin Mocha", light: false, bg: 0x1e1e2e, fg: 0xcdd6f4, cursor: 0xf5e0dc, selection: 0x585b70, accent: 0xcba6f7,
        ansi: [0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
               0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8] },
    Scheme { name: "Catppuccin Macchiato", light: false, bg: 0x24273a, fg: 0xcad3f5, cursor: 0xf4dbd6, selection: 0x5b6078, accent: 0xc6a0f6,
        ansi: [0x494d64, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xb8c0e0,
               0x5b6078, 0xed8796, 0xa6da95, 0xeed49f, 0x8aadf4, 0xf5bde6, 0x8bd5ca, 0xa5adcb] },
    Scheme { name: "Gruvbox Dark", light: false, bg: 0x282828, fg: 0xebdbb2, cursor: 0xebdbb2, selection: 0x504945, accent: 0xfe8019,
        ansi: [0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
               0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2] },
    Scheme { name: "One Dark", light: false, bg: 0x282c34, fg: 0xabb2bf, cursor: 0x528bff, selection: 0x3e4451, accent: 0x61afef,
        ansi: [0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
               0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff] },
    Scheme { name: "Solarized Dark", light: false, bg: 0x002b36, fg: 0x839496, cursor: 0x93a1a1, selection: 0x073642, accent: 0x268bd2,
        ansi: [0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
               0x586e75, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3] },
    Scheme { name: "Rosé Pine", light: false, bg: 0x191724, fg: 0xe0def4, cursor: 0xebbcba, selection: 0x403d52, accent: 0xc4a7e7,
        ansi: [0x26233a, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4,
               0x6e6a86, 0xeb6f92, 0x31748f, 0xf6c177, 0x9ccfd8, 0xc4a7e7, 0xebbcba, 0xe0def4] },
    Scheme { name: "Kanagawa", light: false, bg: 0x1f1f28, fg: 0xdcd7ba, cursor: 0xc8c093, selection: 0x2d4f67, accent: 0x7e9cd8,
        ansi: [0x090618, 0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589, 0xc8c093,
               0x727169, 0xe82424, 0x98bb6c, 0xe6c384, 0x7fb4ca, 0x938aa9, 0x7aa89f, 0xdcd7ba] },
    Scheme { name: "Everforest", light: false, bg: 0x2d353b, fg: 0xd3c6aa, cursor: 0xd3c6aa, selection: 0x543a48, accent: 0xa7c080,
        ansi: [0x475258, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xd3c6aa,
               0x859289, 0xe67e80, 0xa7c080, 0xdbbc7f, 0x7fbbb3, 0xd699b6, 0x83c092, 0xe4e1cd] },
    Scheme { name: "Monokai", light: false, bg: 0x272822, fg: 0xf8f8f2, cursor: 0xf8f8f0, selection: 0x49483e, accent: 0x66d9ef,
        ansi: [0x272822, 0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xf8f8f2,
               0x75715e, 0xf92672, 0xa6e22e, 0xf4bf75, 0x66d9ef, 0xae81ff, 0xa1efe4, 0xf9f8f5] },
    Scheme { name: "Ayu Dark", light: false, bg: 0x0b0e14, fg: 0xbfbdb6, cursor: 0xe6b450, selection: 0x273747, accent: 0xe6b450,
        ansi: [0x01060e, 0xea6c73, 0x7fd962, 0xf9af4f, 0x53bdfa, 0xcda1fa, 0x90e1c6, 0xc7c7c7,
               0x686868, 0xf07178, 0xaad94c, 0xffb454, 0x59c2ff, 0xd2a6ff, 0x95e6cb, 0xffffff] },
    Scheme { name: "Night Owl", light: false, bg: 0x011627, fg: 0xd6deeb, cursor: 0x80a4c2, selection: 0x1d3b53, accent: 0x82aaff,
        ansi: [0x011627, 0xef5350, 0x22da6e, 0xaddb67, 0x82aaff, 0xc792ea, 0x21c7a8, 0xffffff,
               0x575656, 0xef5350, 0x22da6e, 0xffeb95, 0x82aaff, 0xc792ea, 0x7fdbca, 0xffffff] },
    Scheme { name: "GitHub Dark", light: false, bg: 0x0d1117, fg: 0xe6edf3, cursor: 0x2f81f7, selection: 0x264f78, accent: 0x58a6ff,
        ansi: [0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4,
               0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xffffff] },
    Scheme { name: "Material", light: false, bg: 0x263238, fg: 0xeeffff, cursor: 0xffcc00, selection: 0x37474f, accent: 0x80cbc4,
        ansi: [0x37474f, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xeeffff,
               0x546e7a, 0xff5370, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff, 0xffffff] },
    Scheme { name: "Synthwave '84", light: false, bg: 0x262335, fg: 0xf0eff1, cursor: 0x72f1b8, selection: 0x463465, accent: 0xff7edb,
        ansi: [0x34294f, 0xfe4450, 0x72f1b8, 0xfede5d, 0x03edf9, 0xff7edb, 0x03edf9, 0xf0eff1,
               0x614d85, 0xfe4450, 0x72f1b8, 0xf97e72, 0x03edf9, 0xff7edb, 0x03edf9, 0xffffff] },
    Scheme { name: "GitHub Light", light: true, bg: 0xffffff, fg: 0x1f2328, cursor: 0x0969da, selection: 0xb6e3ff, accent: 0x0969da,
        ansi: [0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
               0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f] },
    Scheme { name: "Catppuccin Latte", light: true, bg: 0xeff1f5, fg: 0x4c4f69, cursor: 0xdc8a78, selection: 0xacb0be, accent: 0x8839ef,
        ansi: [0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xacb0be,
               0x6c6f85, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5, 0xea76cb, 0x179299, 0xbcc0cc] },
];

fn from_scheme(s: &Scheme) -> Theme {
    let a = s.ansi;
    Theme {
        name: s.name.to_string(),
        source: Source::Builtin,
        light: s.light,
        bg: s.bg,
        fg: s.fg,
        muted: mix(s.fg, s.bg, 0.35),
        faint: mix(s.fg, s.bg, 0.55),
        accent: s.accent,
        accent_active: mix(s.accent, s.fg, 0.3),
        raised: mix(s.bg, s.accent, if s.light { 0.1 } else { 0.14 }),
        border: mix(s.bg, s.fg, 0.25),
        heading: a[3],
        info: a[4],
        success: a[2],
        danger: a[1],
        warning: mix(a[3], a[1], 0.4),
        cursor: s.cursor,
        selection: s.selection,
        ansi: a,
    }
}

pub fn builtin_themes() -> Vec<Theme> {
    SCHEMES.iter().map(from_scheme).collect()
}

// ── Catalog and selection ──────────────────────────────────────────────────

fn embedded_gentle() -> Vec<Theme> {
    EMBEDDED
        .iter()
        .map(|text| parse_pi_theme(text, Source::Gentle).expect("embedded Gentle theme parses"))
        .collect()
}

/// Gentle Shell's themes (installed files over the embedded copies), then the
/// user's Pi themes, then the popular schemes. Names are unique, first wins.
pub fn load_catalog(gentle: Vec<Theme>, user: Vec<Theme>) -> Vec<Theme> {
    let mut catalog = embedded_gentle();
    for theme in gentle {
        match catalog.iter_mut().find(|t| same_name(&t.name, &theme.name)) {
            Some(slot) => *slot = theme,
            None => catalog.push(theme),
        }
    }
    for theme in user.into_iter().chain(builtin_themes()) {
        if !catalog.iter().any(|t| same_name(&t.name, &theme.name)) {
            catalog.push(theme);
        }
    }
    catalog
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Persisted choice, then Pi's `settings.json` `theme`, then Gentleman-Cute;
/// a name the catalog doesn't have falls through to the next.
pub fn pick_default(catalog: &[Theme], persisted: Option<&str>, pi: Option<&str>) -> usize {
    [persisted, pi, Some(DEFAULT_THEME)]
        .into_iter()
        .flatten()
        .find_map(|name| catalog.iter().position(|t| same_name(&t.name, name)))
        .unwrap_or(0)
}

/// `dirs::data_dir()/gentle-shell-gpui/settings.json`.
pub fn store_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("gentle-shell-gpui").join("settings.json"))
}

fn read_settings(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn read_theme_key(path: &Path) -> Option<String> {
    let value = read_settings(path)?;
    let name = value.get("theme")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The persisted choice in the app's settings file.
pub fn read_persisted(path: &Path) -> Option<String> {
    read_theme_key(path)
}

/// The persisted window transparency (0–1); out-of-range values clamp.
pub fn read_transparency(path: &Path) -> Option<f32> {
    let value = read_settings(path)?.get("transparency")?.as_f64()?;
    Some((value as f32).clamp(0., 1.))
}

/// Pi's `settings.json` `theme` in the agent dir.
pub fn read_pi_setting(agent: &Path) -> Option<String> {
    read_theme_key(&agent.join("settings.json"))
}

/// Set `key` in the app's settings file, keeping every other key.
fn write_key(path: &Path, key: &str, new: Value) -> std::io::Result<()> {
    let mut value = read_settings(path)
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()));
    value[key] = new;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
    std::fs::write(path, text + "\n")
}

/// Set `theme` in the app's settings file, keeping every other key.
pub fn write_persisted(path: &Path, name: &str) -> std::io::Result<()> {
    write_key(path, "theme", Value::String(name.to_string()))
}

/// Set `transparency` in the app's settings file, keeping every other key.
pub fn write_transparency(path: &Path, clear: f32) -> std::io::Result<()> {
    // Two decimals: the slider's step, and no float noise in the file.
    let rounded = (f64::from(clear.clamp(0., 1.)) * 100.).round() / 100.;
    write_key(path, "transparency", Value::from(rounded))
}

// ── Active theme ───────────────────────────────────────────────────────────

/// The terminal's view of the active theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermColors {
    pub ansi: [u32; 16],
    pub fg: u32,
    pub bg: u32,
    pub dim_fg: u32,
    pub cursor: u32,
}

impl TermColors {
    pub fn new(theme: &Theme) -> Self {
        Self {
            ansi: theme.ansi,
            fg: theme.fg,
            bg: theme.bg,
            dim_fg: theme.muted,
            cursor: theme.cursor,
        }
    }
}

struct Resolved {
    palette: ui::Palette,
    term: TermColors,
}

impl Resolved {
    fn new(theme: &Theme, clear: f32) -> Self {
        Self {
            palette: ui::Palette::new(theme, clear),
            term: TermColors::new(theme),
        }
    }
}

/// Window transparency until the user moves the slider: clear enough for the
/// desktop blur to show through every pane, as in Tern.
pub const DEFAULT_TRANSPARENCY: f32 = 0.6;

/// The resolved active theme; Gentleman-Cute until [`init`] runs.
static ACTIVE: LazyLock<RwLock<Resolved>> = LazyLock::new(|| {
    let cute = embedded_gentle()
        .into_iter()
        .find(|t| t.name == DEFAULT_THEME)
        .expect("Gentleman-Cute is embedded");
    RwLock::new(Resolved::new(&cute, DEFAULT_TRANSPARENCY))
});

/// Read the active UI palette.
pub fn palette<R>(read: impl FnOnce(&ui::Palette) -> R) -> R {
    let active = ACTIVE.read().unwrap_or_else(|e| e.into_inner());
    read(&active.palette)
}

/// Read the active terminal colors.
pub fn term<R>(read: impl FnOnce(&TermColors) -> R) -> R {
    let active = ACTIVE.read().unwrap_or_else(|e| e.into_inner());
    read(&active.term)
}

fn install(theme: &Theme, clear: f32) {
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = Resolved::new(theme, clear);
}

/// The catalog, the active theme and the window transparency, for pickers.
pub struct Themes {
    pub catalog: Vec<Theme>,
    pub active: usize,
    /// 0 (most solid) to 1 (clearest).
    pub transparency: f32,
}

impl Global for Themes {}

impl Themes {
    pub fn active_theme(&self) -> &Theme {
        &self.catalog[self.active]
    }
}

/// Load the catalog, pick the starting theme and apply it. The installed
/// `gentle-pi` themes are located off the main thread (finding the launcher
/// can take a login shell) and merged in when found.
pub fn init(cx: &mut App) {
    let agent = pi_rpc::resolve_homes().agent;
    let store = store_path();
    let persisted = store.as_deref().and_then(read_persisted);
    let transparency = store
        .as_deref()
        .and_then(read_transparency)
        .unwrap_or(DEFAULT_TRANSPARENCY);
    let pi = read_pi_setting(&agent);
    let catalog = load_catalog(
        Vec::new(),
        read_theme_dir(&agent.join("themes"), Source::Pi),
    );
    let active = pick_default(&catalog, persisted.as_deref(), pi.as_deref());
    install(&catalog[active], transparency);
    cx.set_global(Themes {
        catalog,
        active,
        transparency,
    });
    ui::apply_theme(cx);

    cx.spawn(async move |cx| {
        let gentle = cx
            .background_executor()
            .spawn(async move {
                let cwd = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
                pi_rpc::resolve_backend(&cwd)
                    .ok()
                    .and_then(|spec| crate::profiles::gentle_pi_root(&spec.program))
                    .map(|root| read_theme_dir(&root.join("themes"), Source::Gentle))
                    .unwrap_or_default()
            })
            .await;
        if gentle.is_empty() {
            return;
        }
        cx.update(|cx| merge_gentle(gentle, persisted, pi, cx));
    })
    .detach();
}

/// Swap the installed Gentle themes in, re-applying when the active one changed.
fn merge_gentle(gentle: Vec<Theme>, persisted: Option<String>, pi: Option<String>, cx: &mut App) {
    let themes = cx.global::<Themes>();
    let before = themes.active_theme().clone();
    let transparency = themes.transparency;
    let explicit =
        themes.active != pick_default(&themes.catalog, persisted.as_deref(), pi.as_deref());
    let user: Vec<Theme> = themes
        .catalog
        .iter()
        .filter(|t| t.source == Source::Pi)
        .cloned()
        .collect();
    let catalog = load_catalog(gentle, user);
    // A choice made this session stays; otherwise re-resolve, since a
    // persisted name may only exist in the installed files.
    let active = if explicit {
        catalog
            .iter()
            .position(|t| same_name(&t.name, &before.name))
            .unwrap_or(0)
    } else {
        pick_default(&catalog, persisted.as_deref(), pi.as_deref())
    };
    let changed = catalog[active] != before;
    if changed {
        install(&catalog[active], transparency);
    }
    cx.set_global(Themes {
        catalog,
        active,
        transparency,
    });
    if changed {
        ui::apply_theme(cx);
    }
    cx.refresh_windows();
}

/// Apply the catalog theme at `index` everywhere and persist the choice.
pub fn select(index: usize, cx: &mut App) {
    let themes = cx.global_mut::<Themes>();
    let Some(theme) = themes.catalog.get(index).cloned() else {
        return;
    };
    themes.active = index;
    let clear = themes.transparency;
    install(&theme, clear);
    ui::apply_theme(cx);
    cx.refresh_windows();
    if let Some(path) = store_path() {
        let name = theme.name.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = write_persisted(&path, &name);
            })
            .detach();
    }
}

/// Re-tint every window at transparency `clear` (0–1); `persist` writes it
/// to the settings file (the slider persists on release, not on every drag).
pub fn set_transparency(clear: f32, persist: bool, cx: &mut App) {
    let clear = clear.clamp(0., 1.);
    let themes = cx.global_mut::<Themes>();
    themes.transparency = clear;
    let theme = themes.active_theme().clone();
    install(&theme, clear);
    ui::apply_theme(cx);
    cx.refresh_windows();
    if persist && let Some(path) = store_path() {
        cx.background_executor()
            .spawn(async move {
                let _ = write_transparency(&path, clear);
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "gentle-themes-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_gentle_theme_json() {
        let cute = parse_pi_theme(EMBEDDED[0], Source::Gentle).unwrap();
        assert_eq!(cute.name, "Gentleman-Cute");
        assert!(!cute.light);
        assert_eq!(cute.bg, 0x060407);
        assert_eq!(cute.fg, 0xf6eff3);
        assert_eq!(cute.muted, 0xa78e9b);
        assert_eq!(cute.faint, 0x76616b);
        assert_eq!(cute.accent, 0xf095c8);
        assert_eq!(cute.accent_active, 0xffb1dd);
        assert_eq!(cute.raised, 0x2a1523);
        assert_eq!(cute.border, 0x563040);
        assert_eq!(cute.heading, 0xe0c27a);
        assert_eq!(cute.info, 0xa9c7ee);
        assert_eq!(cute.success, 0xb4e7c7);
        assert_eq!(cute.danger, 0xff718f);
        assert_eq!(cute.warning, 0xf2b86d);
        assert_eq!(cute.ansi[1], 0xff718f);

        // Gentle: accent is a var reference; ANSI vars feed the info color.
        let gentle = parse_pi_theme(EMBEDDED[2], Source::Gentle).unwrap();
        assert_eq!(gentle.name, "Gentle");
        assert_eq!(gentle.accent, 0x7fb4ca);
        assert_eq!(gentle.info, 0x7fb4ca);
        assert_eq!(gentle.bg, 0x06080f);
    }

    #[test]
    fn parses_indices_hex_and_ansi_vars() {
        let text = r##"{
            "name": "Mine",
            "vars": { "base": "#ffffff", "red": "#aa0000", "brightRed": 196, "ink": "base" },
            "colors": { "text": 16, "accent": "#0af", "muted": "", "error": "red" },
            "export": { "pageBg": "ink" }
        }"##;
        let theme = parse_pi_theme(text, Source::Pi).unwrap();
        assert!(theme.light);
        assert_eq!(theme.bg, 0xffffff);
        assert_eq!(theme.fg, 0x000000);
        assert_eq!(theme.accent, 0x00aaff);
        assert_eq!(theme.danger, 0xaa0000);
        assert_eq!(theme.ansi[1], 0xaa0000);
        assert_eq!(theme.ansi[9], 0xff0000);
        assert!(parse_pi_theme("{\"vars\":{}}", Source::Pi).is_none());
        assert!(parse_pi_theme("not json", Source::Pi).is_none());
    }

    #[test]
    fn default_selection_precedence() {
        let catalog = load_catalog(Vec::new(), Vec::new());
        let at = |name: &str| catalog.iter().position(|t| t.name == name).unwrap();
        assert_eq!(
            pick_default(&catalog, Some("Nord"), Some("Dracula")),
            at("Nord")
        );
        assert_eq!(pick_default(&catalog, None, Some("dracula")), at("Dracula"));
        assert_eq!(pick_default(&catalog, None, None), at("Gentleman-Cute"));
    }

    #[test]
    fn unknown_names_fall_back() {
        let catalog = load_catalog(Vec::new(), Vec::new());
        let at = |name: &str| catalog.iter().position(|t| t.name == name).unwrap();
        assert_eq!(
            pick_default(&catalog, Some("Gone"), Some("Dracula")),
            at("Dracula")
        );
        assert_eq!(
            pick_default(&catalog, Some("Gone"), Some("dark")),
            at("Gentleman-Cute")
        );
    }

    #[test]
    fn catalog_merges_without_duplicates() {
        let mut installed = parse_pi_theme(EMBEDDED[0], Source::Gentle).unwrap();
        installed.accent = 0x123456;
        let mut user = from_scheme(&SCHEMES[0]);
        user.source = Source::Pi;
        user.name = "dracula".into();
        let catalog = load_catalog(vec![installed], vec![user]);
        assert_eq!(catalog[0].accent, 0x123456);
        assert_eq!(
            catalog
                .iter()
                .filter(|t| same_name(&t.name, "Dracula"))
                .count(),
            1
        );
        assert_eq!(
            catalog
                .iter()
                .find(|t| same_name(&t.name, "Dracula"))
                .unwrap()
                .source,
            Source::Pi
        );
        // 17 dark + 2 light, minus the Dracula the user's file shadows.
        assert_eq!(
            catalog
                .iter()
                .filter(|t| t.source == Source::Builtin)
                .count(),
            18
        );
        assert!(catalog.iter().filter(|t| t.light).count() >= 2);
    }

    #[test]
    fn persisting_keeps_other_keys() {
        let dir = scratch("persist");
        let path = dir.join("nested").join("settings.json");
        write_persisted(&path, "Nord").unwrap();
        assert_eq!(read_persisted(&path).as_deref(), Some("Nord"));
        std::fs::write(&path, r#"{"keep": 1, "theme": "Nord"}"#).unwrap();
        write_persisted(&path, "Dracula").unwrap();
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["keep"], 1);
        assert_eq!(value["theme"], "Dracula");
        // Theme and transparency share the file without clobbering each other.
        write_transparency(&path, 0.456_7).unwrap();
        write_persisted(&path, "Nord").unwrap();
        assert_eq!(read_persisted(&path).as_deref(), Some("Nord"));
        assert_eq!(read_transparency(&path), Some(0.46));
        std::fs::write(&path, r#"{"transparency": 7}"#).unwrap();
        assert_eq!(read_transparency(&path), Some(1.));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_user_theme_dir_and_pi_setting() {
        let dir = scratch("dir");
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        std::fs::write(
            dir.join("themes").join("b.json"),
            r##"{"name":"Zeta","colors":{"text":"#eeeeee"}}"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("themes").join("a.json"),
            r##"{"name":"Alpha","colors":{"text":"#eeeeee"}}"##,
        )
        .unwrap();
        std::fs::write(dir.join("themes").join("bad.json"), "{").unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"theme":"Zeta"}"#).unwrap();
        let themes = read_theme_dir(&dir.join("themes"), Source::Pi);
        assert_eq!(
            themes.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["Alpha", "Zeta"]
        );
        assert_eq!(read_pi_setting(&dir).as_deref(), Some("Zeta"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mix_blends_channels() {
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(mix(0x102030, 0x102030, 0.7), 0x102030);
        assert_eq!(mix(0xff0000, 0x0000ff, 0.0), 0xff0000);
    }
}
