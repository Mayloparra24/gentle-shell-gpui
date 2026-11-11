//! The in-app browser: a floating card over the window that embeds the
//! system's web view (WKWebView on macOS, WebView2 on Windows, WebKitGTK on
//! Linux). It shows Gentle Design's live canvas and any other http(s) page,
//! opened with `gentle-open`, `/web` or a loopback URL printed by a tool.
//! Pages an agent opens wait in the window's instance tray; the native view
//! (and its web process) only exists while the card is shown. A lone card
//! sits at the bottom-right by default; multiple cards are tiled by the
//! workspace. Drag its header to move it, any edge or corner to resize it, or
//! maximize it. The canvas reloads itself as artboards change, so the card
//! shows the design in real time.
//!
//! The web view is a native view above everything GPUI draws, so it hides
//! while a screen covers the window. It stays up while the card is dragged:
//! AppKit keeps sending the drag to the view the mouse went down in.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::floating::{self, CardState, DragKind};
use crate::shared::ui;

mod surface;
mod tool;
pub use surface::WebSurface;
pub use tool::install_open_tool;

// The native web view, one module per system with the same interface:
// `host` (the window to build in, taken during a frame), `NativeView::{new,
// place, load_url, reload}`, plus `pump` for a toolkit loop GPUI doesn't run
// (`NEEDS_PUMP`).
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod native;

/// How often GTK's loop is pumped while a native view exists (Linux).
const PUMP_EVERY: Duration = Duration::from_millis(8);

/// `bounds` (window coordinates) as `wry` takes them.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn logical_rect(bounds: Bounds<Pixels>) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(
            bounds.origin.x.as_f32() as f64,
            bounds.origin.y.as_f32() as f64,
        )
        .into(),
        size: wry::dpi::LogicalSize::new(
            bounds.size.width.as_f32().max(1.) as f64,
            bounds.size.height.as_f32().max(1.) as f64,
        )
        .into(),
    }
}

/// The page's background before it paints: the card body's color.
fn page_background() -> (u8, u8, u8, u8) {
    let well = Rgba::from(ui::well());
    let byte = |c: f32| (c.clamp(0., 1.) * 255.) as u8;
    (byte(well.r), byte(well.g), byte(well.b), 255)
}

/// What Gentle Shell reported about its design canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasNotice {
    /// The canvas shows `url`; `refreshed` when it was already open.
    Open {
        url: String,
        refreshed: bool,
    },
    Stopped,
}

/// The canvas notice in a `notify` message or a `gentle_design` tool result
/// text: "Design canvas: <url>", "Design canvas refreshed: <url>" or "Design
/// canvas stopped.". Only the canvas server's loopback URLs count.
pub fn canvas_notice(text: &str) -> Option<CanvasNotice> {
    if text.trim_start().starts_with("Design canvas stopped.") {
        return Some(CanvasNotice::Stopped);
    }
    for (prefix, refreshed) in [
        ("Design canvas refreshed: ", true),
        ("Design canvas: ", false),
    ] {
        let Some(at) = text.find(prefix) else {
            continue;
        };
        let url = text[at + prefix.len()..]
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if is_loopback(url) {
            return Some(CanvasNotice::Open {
                url: url.to_string(),
                refreshed,
            });
        }
    }
    None
}

fn is_loopback(url: &str) -> bool {
    ["http://127.0.0.1:", "http://localhost:"]
        .iter()
        .any(|prefix| url.starts_with(prefix) && url.len() > prefix.len())
}

/// The design a canvas URL shows (`/<token>/d/<slug>/`), for the title.
fn design_slug(url: &str) -> Option<&str> {
    let (_, path) = url.split_once("/d/")?;
    path.split('/').next().filter(|slug| !slug.is_empty())
}

/// A generic web page's title: the host, without the scheme or path.
fn host_label(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if host.is_empty() {
        "Web".to_string()
    } else {
        host.to_string()
    }
}

/// The card's place when it opens: bottom-right. Docking scales it.
pub const DEFAULT_FRAME: floating::Frame = floating::Frame {
    right: 16.,
    bottom: 16.,
    w: 560.,
    h: 380.,
};

// ── The panel ───────────────────────────────────────────────────────────────

pub enum BrowserEvent {
    /// The card's button: send it to the instance tray.
    Hide,
}

pub struct BrowserPanel {
    url: String,
    /// What the header calls this page. `None` derives it from the URL (the
    /// host), so any web app can use the card, not only Gentle Design.
    label: Option<String>,
    card: CardState,
    /// The page itself.
    surface: Entity<WebSurface>,
}

impl EventEmitter<BrowserEvent> for BrowserPanel {}

impl BrowserPanel {
    pub fn new(url: String, label: Option<String>, cx: &mut Context<Self>) -> Self {
        let surface = cx.new(|_| WebSurface::new(url.clone(), None));
        Self {
            url,
            label,
            card: CardState::new(DEFAULT_FRAME),
            surface,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Put the card in a stage slot; see [`crate::shared::floating::CardState`].
    pub fn set_frame(&mut self, frame: floating::Frame, cx: &mut Context<Self>) {
        self.card.set_frame(frame);
        cx.notify();
    }

    /// Whether the card is showing Gentle Design's canvas (so a canvas
    /// `Stopped` notice may close it) rather than a generic web app.
    pub fn is_design(&self) -> bool {
        self.label.is_some()
    }

    /// Show `url`, or reload when it is already showing (the canvas was
    /// refreshed).
    pub fn navigate(&mut self, url: String, label: Option<String>, cx: &mut Context<Self>) {
        self.surface
            .update(cx, |surface, cx| surface.navigate(url.clone(), cx));
        self.url = url;
        self.label = label;
        cx.notify();
    }

    /// A screen opened or closed over the window.
    pub fn set_covered(&mut self, covered: bool, cx: &mut Context<Self>) {
        self.surface
            .update(cx, |surface, cx| surface.set_covered(covered, cx));
    }

    /// The card went to the instance tray: drop the native view, and the web
    /// process behind it, so a hidden page costs nothing. Showing it again
    /// builds a fresh view on [`Self::url`].
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.surface.update(cx, |surface, cx| surface.hide(cx));
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.surface.update(cx, |surface, cx| surface.show(cx));
    }

    /// What the header and the tray call this page.
    pub fn title(&self) -> String {
        match (&self.label, design_slug(&self.url)) {
            (Some(_), Some(slug)) => format!("Design · {slug}"),
            (Some(label), None) => label.clone(),
            (None, _) => host_label(&self.url),
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.surface.update(cx, |surface, cx| surface.reload(cx));
    }

    fn toggle_maximized(&mut self, cx: &mut Context<Self>) {
        self.card.toggle_maximized();
        cx.notify();
    }

    fn start_drag(&mut self, kind: DragKind, start: Point<Pixels>, cx: &mut Context<Self>) {
        self.card.start(kind, start);
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.card.drag_to(position) {
            cx.notify();
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.card.end_drag() {
            cx.notify();
        }
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title();
        let icon = if self.label.is_some() {
            IconName::Palette
        } else {
            IconName::Globe
        };
        let address = self
            .url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .to_string();
        let url = self.url.clone();
        let maximized = self.card.maximized();
        h_flex()
            .id("browser-header")
            .h(px(floating::HEADER))
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .items_center()
            .cursor(CursorStyle::OpenHand)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if event.click_count == 2 {
                        this.toggle_maximized(cx);
                    } else {
                        this.start_drag(DragKind::Move, event.position, cx);
                    }
                }),
            )
            .child(Icon::new(icon).small().text_color(ui::petal()))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui::fg())
                    .whitespace_nowrap()
                    .child(title),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .font_family(ui::MONO)
                    .text_size(px(10.5))
                    .text_color(ui::faint())
                    .child(address),
            )
            .child(
                // Buttons keep their mouse-down from starting a drag.
                h_flex()
                    .gap_0p5()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("browser-reload")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::RotateCw))
                            .tooltip("Reload")
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(
                        Button::new("browser-external")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::ArrowUpRight))
                            .tooltip("Open in your browser")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .child(
                        Button::new("browser-maximize")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(if maximized {
                                IconName::Minimize2
                            } else {
                                IconName::Maximize2
                            }))
                            .tooltip(if maximized { "Restore" } else { "Maximize" })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_maximized(cx))),
                    )
                    .child(
                        Button::new("browser-close")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Minus))
                            .tooltip("Hide · it stays in the top bar")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserEvent::Hide))),
                    ),
            )
    }

    fn body(&self) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .mx(px(floating::GRIP))
            .mb(px(floating::GRIP))
            .rounded(px(floating::BODY_RADIUS))
            .bg(ui::well())
            .overflow_hidden()
            .child(self.surface.clone())
    }

    /// The bands along the card's edges and corners that resize it.
    fn grips(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        floating::grips(&self.card, cx, "browser-grip", |this, kind, start, cx| {
            this.start_drag(kind, start, cx)
        })
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.card.fit();
        let card = ui::island(cx)
            .absolute()
            .flex()
            .flex_col()
            .occlude()
            .shadow(floating::shadow(self.card.dragging()))
            .map(|card| floating::position(&self.card, card))
            .child(self.header(cx))
            .child(self.body())
            .children(self.grips(cx));
        let drag_layer = floating::drag_layer(
            &self.card,
            cx,
            "browser-drag",
            |this, position, cx| this.drag_to(position, cx),
            |this, cx| this.end_drag(cx),
        );
        div()
            .absolute()
            .inset_0()
            .child(floating::area_canvas(&self.card))
            .child(card)
            .when_some(drag_layer, |root, layer| root.child(layer))
    }
}

#[cfg(test)]
mod tests {
    use super::{CanvasNotice, canvas_notice, design_slug, host_label};

    #[test]
    fn canvas_notices_come_from_gentle_design_messages() {
        let open = canvas_notice("Design canvas: http://127.0.0.1:51234/tok/d/login/");
        assert_eq!(
            open,
            Some(CanvasNotice::Open {
                url: "http://127.0.0.1:51234/tok/d/login/".into(),
                refreshed: false
            })
        );
        // The browser-failed warning still carries the URL on its first line.
        let warned = canvas_notice(
            "Design canvas: http://localhost:4000/tok/\nThe browser could not be opened. Open the URL manually.",
        );
        assert_eq!(
            warned,
            Some(CanvasNotice::Open {
                url: "http://localhost:4000/tok/".into(),
                refreshed: false
            })
        );
        assert_eq!(
            canvas_notice("Design canvas refreshed: http://127.0.0.1:1/t/d/x/"),
            Some(CanvasNotice::Open {
                url: "http://127.0.0.1:1/t/d/x/".into(),
                refreshed: true
            })
        );
        assert_eq!(
            canvas_notice("Design canvas stopped."),
            Some(CanvasNotice::Stopped)
        );
        // Anything else, or a URL that isn't the loopback canvas, is ignored.
        assert_eq!(canvas_notice("No design canvas is running."), None);
        assert_eq!(canvas_notice("Design canvas: https://example.com/"), None);
        assert_eq!(canvas_notice("Design canvas: http://127.0.0.1:"), None);
        assert_eq!(
            design_slug("http://127.0.0.1:1/tok/d/login/"),
            Some("login")
        );
        assert_eq!(design_slug("http://127.0.0.1:1/tok/"), None);
        assert_eq!(host_label("http://localhost:3000/app"), "localhost:3000");
        assert_eq!(host_label("https://example.com/"), "example.com");
    }
}
