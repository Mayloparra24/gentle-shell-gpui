//! Design system: the active theme (`themes.rs`, Gentleman-Cute by default)
//! as translucent glass, the gpui-component theme mapped onto it so every
//! stock button, input and menu matches, and shared building blocks
//! (cards, chips, key caps, section labels, entrance animations).
//!
//! Views use these instead of ad-hoc colors so every screen looks alike.

use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationElement, AnimationExt as _, App, BoxShadow, Div, ElementId, FontWeight,
    Hsla, ParentElement as _, SharedString, Stateful, Styled, div, ease_out_quint, point, px, rgb,
};

use crate::shared::themes::{self, Theme, mix};

// ── Palette ────────────────────────────────────────────────────────────────

/// Every UI role, resolved from a [`Theme`]. Surfaces are the theme's
/// background tinted toward its text and accent, then made translucent so
/// the platform blur shows through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub light: bool,
    pub window_bg: Hsla,
    pub panel: Hsla,
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub raised: Hsla,
    pub well: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub fg: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_active: Hsla,
    pub accent_deep: Hsla,
    pub on_accent: Hsla,
    pub champagne: Hsla,
    pub info: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub danger_hover: Hsla,
    pub danger_active: Hsla,
    pub warning: Hsla,
    pub selection: Hsla,
    pub popover: Hsla,
    pub screen: Hsla,
    pub scrollbar_hover: Hsla,
    pub backdrop: Hsla,
}

fn color(hex: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = rgb(hex).into();
    c.a = alpha;
    c
}

impl Palette {
    /// `clear` is the window transparency, 0 (the most solid glass) to 1;
    /// each glass role fades from its solid alpha toward a floor that still
    /// keeps text readable over a busy desktop.
    pub fn new(t: &Theme, clear: f32) -> Self {
        let clear = clear.clamp(0., 1.);
        let fade = |solid: f32, floor: f32| solid - (solid - floor) * clear;
        let solid = |hex| color(hex, 1.);
        // Cards lean toward the text color with a hint of the accent.
        let tint = mix(t.fg, t.accent, 0.6);
        let glass = |amount: f32, alpha: f32| color(mix(t.bg, tint, amount), alpha);
        let accent_deep = mix(t.accent, t.bg, 0.2);
        Self {
            light: t.light,
            window_bg: color(
                mix(t.bg, t.fg, 0.03),
                fade(if t.light { 0.92 } else { 0.9 }, 0.3),
            ),
            panel: glass(0.045, fade(0.93, 0.16)),
            surface: glass(0.09, fade(0.94, 0.4)),
            surface_hover: glass(0.15, fade(0.96, 0.55)),
            raised: color(t.raised, fade(0.96, 0.7)),
            well: if t.light {
                color(mix(t.bg, t.fg, 0.035), fade(0.92, 0.45))
            } else {
                glass(0.02, fade(0.92, 0.45))
            },
            border: glass(0.2, if t.light { 0.8 } else { 0.69 }),
            border_strong: solid(t.border),
            fg: solid(t.fg),
            muted: solid(t.muted),
            faint: solid(t.faint),
            accent: solid(t.accent),
            accent_active: solid(t.accent_active),
            accent_deep: solid(accent_deep),
            on_accent: if t.light {
                solid(0xffffff)
            } else {
                solid(mix(t.bg, t.accent, 0.08))
            },
            champagne: solid(t.heading),
            info: solid(t.info),
            success: solid(t.success),
            danger: solid(t.danger),
            danger_hover: solid(mix(t.danger, 0xffffff, 0.2)),
            danger_active: solid(mix(t.danger, 0x000000, 0.1)),
            warning: solid(t.warning),
            selection: color(mix(t.selection, accent_deep, 0.5), 0.6),
            // Menus, the palette and screens float over content: solid, whatever
            // the transparency, so the conversation behind never bleeds through.
            popover: glass(0.065, 1.),
            screen: glass(0.055, 1.),
            scrollbar_hover: solid(mix(t.border, t.fg, 0.2)),
            backdrop: color(0x000000, if t.light { 0.25 } else { 0.44 }),
        }
    }
}

/// Panes, sidebars and screens.
pub fn panel() -> Hsla {
    themes::palette(|p| p.panel)
}
/// Cards and fields.
pub fn surface() -> Hsla {
    themes::palette(|p| p.surface)
}
pub fn surface_hover() -> Hsla {
    themes::palette(|p| p.surface_hover)
}
/// User messages, selected rows.
pub fn raised() -> Hsla {
    themes::palette(|p| p.raised)
}
/// Code blocks and terminal wells, darker than a card.
pub fn well() -> Hsla {
    themes::palette(|p| p.well)
}
pub fn border() -> Hsla {
    themes::palette(|p| p.border)
}
pub fn border_strong() -> Hsla {
    themes::palette(|p| p.border_strong)
}
pub fn fg() -> Hsla {
    themes::palette(|p| p.fg)
}
pub fn muted() -> Hsla {
    themes::palette(|p| p.muted)
}
pub fn faint() -> Hsla {
    themes::palette(|p| p.faint)
}
/// Interactive accent: links, focus, selected state.
pub fn accent() -> Hsla {
    themes::palette(|p| p.accent)
}
/// The brand color (the theme's accent; Gentle Shell's pink by default).
pub fn petal() -> Hsla {
    themes::palette(|p| p.accent)
}
pub fn petal_active() -> Hsla {
    themes::palette(|p| p.accent_active)
}
/// Tool titles, headings, inline code.
pub fn champagne() -> Hsla {
    themes::palette(|p| p.champagne)
}
/// Informational blue (running state, models).
pub fn info() -> Hsla {
    themes::palette(|p| p.info)
}
pub fn success() -> Hsla {
    themes::palette(|p| p.success)
}
pub fn danger() -> Hsla {
    themes::palette(|p| p.danger)
}
pub fn warning() -> Hsla {
    themes::palette(|p| p.warning)
}
/// Opaque-ish glass for full-window screens and palettes.
pub fn screen_bg() -> Hsla {
    themes::palette(|p| p.screen)
}
/// Floating cards, menus and the task island: solid at any transparency, so
/// what is behind them never bleeds through.
pub fn popover_bg() -> Hsla {
    themes::palette(|p| p.popover)
}
/// The dimmed layer behind screens and dialogs.
pub fn backdrop() -> Hsla {
    themes::palette(|p| p.backdrop)
}

pub const MONO: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

/// Map the gpui-component theme onto the active palette so stock components
/// match. Switches gpui-component's light/dark mode with the theme first.
pub fn apply_theme(cx: &mut App) {
    use gpui_kit::component::{Theme as KitTheme, ThemeMode};
    let p = themes::palette(|p| *p);
    let mode = if p.light {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    if KitTheme::global(cx).is_dark() == p.light {
        KitTheme::change(mode, None, cx);
    }
    let clear = color(0x000000, 0.);
    KitTheme::update(cx, |theme| {
        let c = &mut theme.colors;
        c.background = p.window_bg;
        c.foreground = p.fg;
        c.border = p.border;
        c.input = p.border_strong;
        c.ring = p.accent;
        c.caret = p.accent_active;
        c.selection = p.selection;
        c.muted = p.surface;
        c.muted_foreground = p.muted;
        c.accent = p.surface_hover;
        c.accent_foreground = p.fg;
        c.popover = p.popover;
        c.popover_foreground = p.fg;
        c.list = clear;
        c.list_hover = p.surface_hover;
        c.list_active = p.raised;
        c.list_active_border = p.accent;
        c.primary = p.accent;
        c.primary_hover = p.accent_active;
        c.primary_active = p.accent_deep;
        c.primary_foreground = p.on_accent;
        // Buttons read their own tokens, not `primary`/`secondary`.
        c.button_primary = p.accent;
        c.button_primary_hover = p.accent_active;
        c.button_primary_active = p.accent_deep;
        c.button_primary_foreground = p.on_accent;
        c.button = p.surface;
        c.button_hover = p.surface_hover;
        c.button_active = p.raised;
        c.button_foreground = p.fg;
        c.button_secondary = p.surface;
        c.button_secondary_hover = p.surface_hover;
        c.button_secondary_active = p.raised;
        c.button_secondary_foreground = p.fg;
        c.button_danger = p.danger;
        c.button_danger_hover = p.danger_hover;
        c.button_danger_active = p.danger_active;
        c.button_danger_foreground = p.on_accent;
        c.secondary = p.surface;
        c.secondary_hover = p.surface_hover;
        c.secondary_active = p.raised;
        c.secondary_foreground = p.fg;
        c.danger = p.danger;
        c.danger_hover = p.danger_hover;
        c.danger_active = p.danger_active;
        c.danger_foreground = p.on_accent;
        c.success = p.success;
        c.warning = p.warning;
        c.info = p.info;
        c.link = p.accent;
        c.link_hover = p.accent_active;
        c.scrollbar = clear;
        c.scrollbar_thumb = p.border_strong;
        c.scrollbar_thumb_hover = p.scrollbar_hover;
        c.tab_bar = clear;
        c.tab = clear;
        c.tab_active = p.raised;
        c.tab_foreground = p.muted;
        c.tab_active_foreground = p.fg;
        c.title_bar = clear;
        // Tern has no rule under the title bar; panes start right below it.
        c.title_bar_border = clear;
        c.overlay = color(0x000000, if p.light { 0.3 } else { 0.48 });
        c.switch = p.border_strong;
        c.progress_bar = p.accent;
        c.slider_bar = p.accent;
        c.skeleton = p.surface_hover;
        c.window_border = p.border_strong;
        c.sidebar = p.panel;
        c.sidebar_border = p.border;
        c.sidebar_foreground = p.fg;
        c.sidebar_accent = p.surface_hover;
        c.sidebar_accent_foreground = p.fg;
        c.sidebar_primary = p.accent;
        c.sidebar_primary_foreground = p.on_accent;
        theme.radius = px(8.);
        theme.radius_lg = px(12.);
    });
}

// ── Building blocks ────────────────────────────────────────────────────────

/// The standard card: tool calls, rail sections, settings groups.
pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_xl()
        .bg(surface())
        .border_1()
        .border_color(border())
}

/// A darker inset for code, command output and diffs.
pub fn well_box() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded_lg()
        .bg(well())
        .border_1()
        .border_color(border())
        .font_family(MONO)
        .text_xs()
}

/// Small pill: language, model, status.
pub fn chip(text: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .px_1p5()
        .py(px(1.))
        .rounded_md()
        .text_size(px(10.5))
        .text_color(color)
        .bg(color.opacity(0.12))
        .border_1()
        .border_color(color.opacity(0.22))
        .child(text.into())
}

/// A key cap for shortcut hints (`esc`, `↵`, `⌘`). Use [`key_button`] when the
/// hint stands for one action the user could click.
pub fn kbd(text: impl Into<SharedString>) -> Div {
    div()
        .min_w(px(20.))
        .h(px(20.))
        .px_1p5()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(border_strong())
        .bg(surface())
        .font_family(MONO)
        .text_size(px(10.5))
        .text_color(muted())
        .child(text.into())
}

/// A clickable [`kbd`]: same cap, with a pointer and a hover; the caller adds
/// `.on_click(...)` doing what the key does.
pub fn key_button(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Stateful<Div> {
    use gpui_kit::InteractiveElement as _;
    kbd(text)
        .id(id)
        .cursor_pointer()
        .hover(|d| d.text_color(fg()).border_color(petal().opacity(0.6)))
}

/// Uppercase micro heading for sections.
pub fn section_label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(faint())
        .child(text.into().to_uppercase())
}

/// The full-window screen chrome every overlay shares: dimmed backdrop,
/// centered glass panel with a header (icon, title, subtitle, trailing).
pub fn screen(width: Option<f32>) -> (Div, Div) {
    let backdrop = div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .p(px(32.))
        .child(scrim("screen-scrim"));
    let panel = div()
        .flex()
        .flex_col()
        .max_h_full()
        .when_some(width, |d, w| d.w(px(w)))
        .when(width.is_none(), |d| d.size_full().max_w(px(1280.)))
        .rounded_2xl()
        .border_1()
        .border_color(border_strong())
        .bg(screen_bg())
        .shadow_2xl()
        .overflow_hidden();
    (backdrop, panel)
}

/// Header row for [`screen`] panels.
pub fn screen_header() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .px_5()
        .py_3()
        .gap_3()
        .border_b_1()
        .border_color(border())
}

/// A text field frame for `Input::new(..).appearance(false)`: same height,
/// radius and border everywhere.
pub fn field() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(34.))
        .px_3()
        .rounded_lg()
        .bg(well())
        .border_1()
        .border_color(border_strong())
}

/// The todo island's floating card: solid, a soft outline, a deep shadow and
/// round corners. Composer menus (model, permissions, chip cards) open in it
/// with `Popover::appearance(false)`, so they match the island instead of
/// the stock popover frame. It sets the UI font and text color itself, as
/// that frame did: a menu opened from a monospace bar must not inherit it.
/// Rows inside should round to [`ISLAND_ROW`].
pub fn island(cx: &App) -> Div {
    div()
        .font_family(cx.theme().font_family.clone())
        .text_color(fg())
        .bg(popover_bg())
        .rounded(px(18.))
        .border_1()
        .border_color(border_strong().opacity(0.8))
        .overflow_hidden()
        .shadow(vec![BoxShadow {
            color: backdrop().opacity(0.34),
            offset: point(px(0.), px(10.)),
            blur_radius: px(30.),
            spread_radius: px(2.),
            inset: false,
        }])
}

/// Corner radius of a row inside an [`island`] with 6 px padding, so the
/// row's corners follow the card's.
pub const ISLAND_ROW: f32 = 12.;

/// Segmented toggle (tabs inside a well): one shared look for every
/// "This project / All", "Agents / Activity", "Pro / Max" switch.
pub fn segmented(
    id: &'static str,
    labels: &[&'static str],
    selected: Option<usize>,
    on_select: impl Fn(&usize, &mut gpui_kit::Window, &mut App) + 'static,
) -> Div {
    use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _};
    let on_select = std::rc::Rc::new(on_select);
    div()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .p(px(2.))
        .gap(px(2.))
        .rounded_lg()
        .bg(well())
        .border_1()
        .border_color(border())
        .children(labels.iter().enumerate().map(|(i, label)| {
            let active = Some(i) == selected;
            let on_select = on_select.clone();
            div()
                .id((id, i))
                .px_2p5()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .whitespace_nowrap()
                .when(active, |d| d.bg(raised()).text_color(fg()))
                .when(!active, |d| {
                    d.text_color(muted()).hover(|d| d.text_color(fg()))
                })
                .on_click(move |_, window, cx| on_select(&i, window, cx))
                .child(*label)
        }))
}

// ── Motion ─────────────────────────────────────────────────────────────────

/// A gentle spring with a ~3% overshoot, for anything that opens or unfolds.
pub fn spring(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    let c1 = 0.7;
    let c3 = c1 + 1.;
    1. + c3 * (t - 1.).powi(3) + c1 * (t - 1.).powi(2)
}

/// Fade and rise into place once, when first rendered. It slides with
/// `relative().top()`, like [`pop_in`], so the rows around it don't move.
pub fn appear(id: impl Into<ElementId>, element: Div) -> AnimationElement<Div> {
    element.with_animation(
        id,
        Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
        |el, t| el.opacity(t).relative().top(px((1.0 - t) * 6.0)),
    )
}

/// [`appear`] after `delay`, rising `rise` px: staggered entrances (the
/// welcome screen's intro, then its steps one by one).
pub fn appear_after(
    id: impl Into<ElementId>,
    delay: Duration,
    rise: f32,
    element: Div,
) -> AnimationElement<Div> {
    const RUN: f32 = 420.;
    let wait = delay.as_millis() as f32;
    element.with_animation(
        id,
        Animation::new(Duration::from_millis((wait + RUN) as u64)),
        move |el, t| {
            let local = ((t * (wait + RUN) - wait) / RUN).clamp(0., 1.);
            // ease-out-quint, applied here so the wait stays linear.
            let eased = 1. - (1. - local).powi(5);
            el.opacity(eased).relative().top(px((1. - eased) * rise))
        },
    )
}

/// Screens, dialogs and menus open with the same soft spring: they rise a
/// little, settle with a gentle overshoot, and fade in without a hard pop.
pub fn pop_in(id: impl Into<ElementId>, element: Div) -> AnimationElement<Div> {
    element.with_animation(id, Animation::new(Duration::from_millis(340)), |el, t| {
        // The spring runs in the animator: GPUI's easing must stay in 0..=1.
        let s = spring(t);
        // `top` on a relative element slides visually without nudging the
        // siblings it shares a flex row or column with.
        el.opacity(s.min(1.)).relative().top(px((1. - s) * 12.))
    })
}

/// Fade a popover's content in just behind its surface, without changing the
/// surface's measured size (so the positioner never jumps).
pub fn popover_in(id: impl Into<ElementId>, element: Div) -> AnimationElement<Div> {
    element.with_animation(id, Animation::new(Duration::from_millis(300)), |el, t| {
        el.opacity(spring(t).min(1.))
    })
}

/// The dimmed sheet behind a screen. It fades in with the panel's spring.
pub fn scrim(id: impl Into<ElementId>) -> AnimationElement<Div> {
    div().absolute().inset_0().bg(backdrop()).with_animation(
        id,
        Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
        |el, t| el.opacity(t),
    )
}

/// A soft breathing pulse for live indicators.
pub fn pulse(id: impl Into<ElementId>, element: Div) -> AnimationElement<Div> {
    element.with_animation(
        id,
        Animation::new(Duration::from_millis(1400))
            .repeat()
            .with_easing(gpui_kit::pulsating_between(0.35, 1.0)),
        |el, t| el.opacity(t),
    )
}

/// `~/a/b/leaf` split into a muted prefix and the leaf, for path labels.
pub fn split_path(path: &std::path::Path) -> (String, String) {
    let display = match dirs::home_dir() {
        Some(home) if path.starts_with(&home) => {
            let rest = path.strip_prefix(&home).unwrap_or(path);
            if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
            }
        }
        _ => path.display().to_string(),
    };
    match display.rfind(['/', '\\']) {
        Some(i) if i + 1 < display.len() => {
            (display[..=i].to_string(), display[i + 1..].to_string())
        }
        _ => (String::new(), display),
    }
}

/// The last `max` lines of `text`, plus how many were hidden.
pub fn tail(text: &str, max: usize) -> (String, usize) {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let hidden = lines.len().saturating_sub(max);
    (lines[hidden..].join("\n"), hidden)
}

pub fn format_duration(ms: u64) -> String {
    if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles_with_a_soft_overshoot() {
        assert!(spring(0.).abs() < 1e-6);
        assert!((spring(1.) - 1.).abs() < 1e-6);
        let peak = (0..=100)
            .map(|i| spring(i as f32 / 100.))
            .fold(0., f32::max);
        assert!(peak > 1.0 && peak < 1.05, "peak was {peak}");
    }

    #[test]
    fn tail_keeps_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), ("b\nc".to_string(), 1));
        assert_eq!(tail("a", 5), ("a".to_string(), 0));
    }

    #[test]
    fn palette_follows_the_theme() {
        let catalog = crate::shared::themes::load_catalog(Vec::new(), Vec::new());
        let find = |name: &str| catalog.iter().find(|t| t.name == name).unwrap();
        let cute = Palette::new(find("Gentleman-Cute"), 0.);
        assert!(!cute.light);
        assert_eq!(cute.fg, Hsla::from(rgb(0xf6eff3)));
        assert_eq!(cute.border_strong, Hsla::from(rgb(0x563040)));
        let latte = Palette::new(find("Catppuccin Latte"), 0.);
        assert!(latte.light);
        assert!(latte.fg.l < latte.window_bg.l);
    }

    #[test]
    fn transparency_fades_glass_but_not_text_or_modals() {
        let catalog = crate::shared::themes::load_catalog(Vec::new(), Vec::new());
        let cute = catalog.iter().find(|t| t.name == "Gentleman-Cute").unwrap();
        let (solid, clear) = (Palette::new(cute, 0.), Palette::new(cute, 1.));
        for (a, b) in [
            (solid.window_bg, clear.window_bg),
            (solid.panel, clear.panel),
            (solid.surface, clear.surface),
        ] {
            assert!(b.a < a.a, "{b:?} should be clearer than {a:?}");
        }
        assert_eq!(solid.fg, clear.fg);
        assert_eq!(solid.accent, clear.accent);
        // Menus, the palette and screens stay solid even at full transparency.
        assert_eq!(clear.popover.a, 1.);
        assert_eq!(clear.screen.a, 1.);
        // Out-of-range values clamp instead of going negative.
        assert_eq!(Palette::new(cute, 3.), clear);
    }
}
