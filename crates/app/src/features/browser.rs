//! The in-app browser: a floating card over the window that embeds the
//! system's web view (WKWebView as a child view, macOS only), for Gentle
//! Design's live canvas. It sits at the bottom-right by default; drag its
//! header to move it, any edge or corner to resize it, or maximize it. The
//! canvas reloads itself as artboards change, so the card shows the design
//! in real time.
//!
//! The web view is a native view above everything GPUI draws, so it hides
//! while a screen covers the window. It stays up while the card is dragged:
//! AppKit keeps sending the drag to the view the mouse went down in.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::shared::ui;

// The native web view, one module per system with the same interface:
// `host` (the window to build in, taken during a frame), `NativeView::{new,
// place, load_url, reload}`, plus `pump` for a toolkit loop GPUI doesn't run
// (`NEEDS_PUMP`).
#[cfg(target_os = "linux")]
#[path = "browser/linux.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "browser/macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "browser/windows.rs"]
mod native;

/// How often GTK's loop is pumped while the card is open (Linux).
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

// ── Geometry ────────────────────────────────────────────────────────────────

/// The card's place, anchored to the window's bottom-right so it stays put
/// when the window resizes: margins from the right and bottom, and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    right: f32,
    bottom: f32,
    w: f32,
    h: f32,
}

const DEFAULT_FRAME: Frame = Frame {
    right: 16.,
    bottom: 16.,
    w: 560.,
    h: 380.,
};
const MIN_W: f32 = 320.;
const MIN_H: f32 = 220.;
/// Gap around a maximized card; the title bar stays clear.
const MAX_INSET: f32 = 10.;
const TITLE_BAR: f32 = 36.;
/// The header you drag to move the card.
const HEADER: f32 = 38.;
/// The grab band along each edge, inside the card.
const GRIP: f32 = 6.;
/// The body's corner radius: the card's 18 px less the grip inset, so its
/// corners follow the card's.
const BODY_RADIUS: f32 = 12.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edges {
    left: bool,
    top: bool,
    right: bool,
    bottom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    Move,
    Resize(Edges),
}

#[derive(Clone, Copy)]
struct Drag {
    kind: DragKind,
    start: Point<Pixels>,
    frame: Frame,
}

/// `frame` after dragging by (`dx`, `dy`) in a `w` × `h` area. Edges you
/// don't drag stay where they were; the card keeps a minimum size and stays
/// inside the area.
fn dragged(frame: Frame, kind: DragKind, dx: f32, dy: f32, (aw, ah): (f32, f32)) -> Frame {
    let mut next = frame;
    match kind {
        DragKind::Move => {
            next.right = frame.right - dx;
            next.bottom = frame.bottom - dy;
        }
        DragKind::Resize(edges) => {
            // The far edges in area coordinates, which a resize keeps fixed.
            let left = aw - frame.right - frame.w;
            let top = ah - frame.bottom - frame.h;
            if edges.left {
                next.w = (frame.w - dx).clamp(MIN_W, (aw - frame.right).max(MIN_W));
            }
            if edges.right {
                next.w = (frame.w + dx).clamp(MIN_W, (aw - left).max(MIN_W));
                next.right = aw - left - next.w;
            }
            if edges.top {
                next.h = (frame.h - dy).clamp(MIN_H, (ah - frame.bottom).max(MIN_H));
            }
            if edges.bottom {
                next.h = (frame.h + dy).clamp(MIN_H, (ah - top).max(MIN_H));
                next.bottom = ah - top - next.h;
            }
        }
    }
    fitted(next, (aw, ah))
}

/// `frame` kept inside a `w` × `h` area (the window may have shrunk).
fn fitted(frame: Frame, (aw, ah): (f32, f32)) -> Frame {
    if aw <= 0. || ah <= 0. {
        return frame;
    }
    let w = frame.w.min(aw).max(MIN_W.min(aw));
    let h = frame.h.min(ah - TITLE_BAR).max(MIN_H.min(ah));
    Frame {
        w,
        h,
        right: frame.right.clamp(0., (aw - w).max(0.)),
        bottom: frame.bottom.clamp(0., (ah - TITLE_BAR - h).max(0.)),
    }
}

// ── The panel ───────────────────────────────────────────────────────────────

pub enum BrowserEvent {
    Close,
}

pub struct BrowserPanel {
    url: String,
    frame: Frame,
    maximized: bool,
    /// A screen covers the window: the native view must not show over it.
    covered: bool,
    drag: Option<Drag>,
    /// The window area the card lives in, as last laid out.
    area: Rc<Cell<(f32, f32)>>,
    /// The native web view, created after the card is first laid out.
    view: Option<native::NativeView>,
    /// The view is being built (see `place_webview`).
    creating: bool,
    failed: Option<String>,
    /// Pumps GTK's loop while the card is open (Linux).
    _pump: Option<Task<()>>,
    /// Re-places the view when the window moves (Windows hosts it in a popup
    /// positioned in screen coordinates).
    _window_moves: Option<Subscription>,
}

impl EventEmitter<BrowserEvent> for BrowserPanel {}

impl BrowserPanel {
    pub fn new(url: String, cx: &mut Context<Self>) -> Self {
        let pump = native::NEEDS_PUMP.then(|| {
            cx.spawn(async move |_, cx| {
                loop {
                    cx.background_executor().timer(PUMP_EVERY).await;
                    native::pump();
                }
            })
        });
        Self {
            url,
            frame: DEFAULT_FRAME,
            maximized: false,
            covered: false,
            drag: None,
            area: Rc::new(Cell::new((0., 0.))),
            view: None,
            creating: false,
            failed: None,
            _pump: pump,
            _window_moves: None,
        }
    }

    /// Show `url`, or reload when it is already showing (the canvas was
    /// refreshed).
    pub fn navigate(&mut self, url: String, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            if url == self.url {
                view.reload();
            } else {
                view.load_url(&url);
            }
        }
        self.url = url;
        cx.notify();
    }

    /// A screen opened or closed over the window.
    pub fn set_covered(&mut self, covered: bool, cx: &mut Context<Self>) {
        if self.covered != covered {
            self.covered = covered;
            cx.notify();
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.reload();
        }
        cx.notify();
    }

    fn toggle_maximized(&mut self, cx: &mut Context<Self>) {
        self.maximized = !self.maximized;
        cx.notify();
    }

    fn start_drag(&mut self, kind: DragKind, start: Point<Pixels>, cx: &mut Context<Self>) {
        if self.maximized {
            // Dragging a maximized card first restores it where it was.
            if kind != DragKind::Move {
                return;
            }
            self.maximized = false;
        }
        self.drag = Some(Drag {
            kind,
            start,
            frame: self.frame,
        });
        cx.notify();
    }

    fn drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else { return };
        let dx = (position.x - drag.start.x).as_f32();
        let dy = (position.y - drag.start.y).as_f32();
        let next = dragged(drag.frame, drag.kind, dx, dy, self.area.get());
        if next != self.frame {
            self.frame = next;
            cx.notify();
        }
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    /// Place the native view over the card's body. The first call builds it in
    /// a task instead: that runs outside GPUI's update, which matters because
    /// WebView2 pumps the thread's messages while it starts and GPUI's window
    /// procedure would re-enter a borrowed window and abort the app.
    fn place_webview(&mut self, bounds: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if self.failed.is_some() || self.creating {
            return;
        }
        if let Some(view) = &mut self.view {
            view.place(window, bounds, !self.covered);
            return;
        }
        let host = match native::host(window) {
            Ok(host) => host,
            Err(error) => {
                self.failed = Some(error);
                cx.notify();
                return;
            }
        };
        self.creating = true;
        let url = self.url.clone();
        let background = page_background();
        cx.spawn(async move |this, cx| {
            let view = native::NativeView::new(host, &url, background);
            // A card closed meanwhile drops the view, and its popup with it.
            let _ = this.update(cx, |panel, cx| {
                panel.creating = false;
                match view {
                    Ok(view) => {
                        // The canvas moved on while the view was starting.
                        if panel.url != url {
                            view.load_url(&panel.url);
                        }
                        panel.view = Some(view);
                    }
                    Err(error) => panel.failed = Some(error),
                }
                // The next frame places the view.
                cx.notify();
            });
        })
        .detach();
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match design_slug(&self.url) {
            Some(slug) => format!("Design · {slug}"),
            None => "Design canvas".into(),
        };
        let address = self
            .url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string();
        let url = self.url.clone();
        let maximized = self.maximized;
        h_flex()
            .id("browser-header")
            .h(px(HEADER))
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
            .child(Icon::new(IconName::Palette).small().text_color(ui::petal()))
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
                            .icon(Icon::new(IconName::X))
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserEvent::Close))),
                    ),
            )
    }

    fn body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let note = self.failed.clone();
        div()
            .flex_1()
            .min_h_0()
            .mx(px(GRIP))
            .mb(px(GRIP))
            .relative()
            .rounded(px(BODY_RADIUS))
            .bg(ui::well())
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        this.update(cx, |panel, cx| panel.place_webview(bounds, window, cx));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .when_some(note, |d, note| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_size(px(12.))
                        .text_color(ui::muted())
                        .child(note),
                )
            })
    }

    /// The bands along the card's edges and corners that resize it.
    fn grips(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let edge = |left, top, right, bottom| Edges {
            left,
            top,
            right,
            bottom,
        };
        let corner = GRIP * 2.;
        [
            (
                "grip-l",
                edge(true, false, false, false),
                CursorStyle::ResizeLeftRight,
            ),
            (
                "grip-r",
                edge(false, false, true, false),
                CursorStyle::ResizeLeftRight,
            ),
            (
                "grip-t",
                edge(false, true, false, false),
                CursorStyle::ResizeUpDown,
            ),
            (
                "grip-b",
                edge(false, false, false, true),
                CursorStyle::ResizeUpDown,
            ),
            (
                "grip-tl",
                edge(true, true, false, false),
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "grip-br",
                edge(false, false, true, true),
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                "grip-tr",
                edge(false, true, true, false),
                CursorStyle::ResizeUpRightDownLeft,
            ),
            (
                "grip-bl",
                edge(true, false, false, true),
                CursorStyle::ResizeUpRightDownLeft,
            ),
        ]
        .into_iter()
        .map(|(id, edges, cursor)| {
            let band = div().id(id).absolute().cursor(cursor);
            let band = match (edges.left, edges.top, edges.right, edges.bottom) {
                (true, false, false, false) => {
                    band.left_0().top(px(corner)).bottom(px(corner)).w(px(GRIP))
                }
                (false, false, true, false) => band
                    .right_0()
                    .top(px(corner))
                    .bottom(px(corner))
                    .w(px(GRIP)),
                (false, true, false, false) => {
                    band.top_0().left(px(corner)).right(px(corner)).h(px(GRIP))
                }
                (false, false, false, true) => band
                    .bottom_0()
                    .left(px(corner))
                    .right(px(corner))
                    .h(px(GRIP)),
                (true, true, ..) => band.left_0().top_0().size(px(corner)),
                (_, _, true, true) => band.right_0().bottom_0().size(px(corner)),
                (_, true, true, _) => band.right_0().top_0().size(px(corner)),
                _ => band.left_0().bottom_0().size(px(corner)),
            };
            band.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.start_drag(DragKind::Resize(edges), event.position, cx);
                }),
            )
            .into_any_element()
        })
        .collect()
    }
}

/// A card floating above the window: a wide soft ambient shadow, a tighter
/// key shadow and a faint rose glow. Picked up (`lifted`, while dragged) it
/// rises: everything spreads further and drops lower.
fn floating_shadow(lifted: bool) -> Vec<BoxShadow> {
    let lift = if lifted { 1.6 } else { 1. };
    let shadow = |alpha: f32, y: f32, blur: f32, spread: f32, color: Hsla| BoxShadow {
        color: color.opacity(alpha),
        offset: point(px(0.), px(y * lift)),
        blur_radius: px(blur * lift),
        spread_radius: px(spread),
        inset: false,
    };
    vec![
        shadow(0.5, 28., 70., -6., ui::backdrop()),
        shadow(0.45, 10., 24., -2., ui::backdrop()),
        shadow(if lifted { 0.16 } else { 0.1 }, 0., 40., 2., ui::petal()),
    ]
}

impl Render for BrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._window_moves.is_none() {
            self._window_moves = Some(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));
        }
        let area = self.area.clone();
        let size = area.get();
        if size.0 > 0. && !self.maximized {
            self.frame = fitted(self.frame, size);
        }
        let card = ui::island(cx)
            .absolute()
            .flex()
            .flex_col()
            .occlude()
            .shadow(floating_shadow(self.drag.is_some()))
            .map(|card| {
                if self.maximized {
                    card.left(px(MAX_INSET))
                        .right(px(MAX_INSET))
                        .top(px(TITLE_BAR + MAX_INSET / 2.))
                        .bottom(px(MAX_INSET))
                } else {
                    card.right(px(self.frame.right))
                        .bottom(px(self.frame.bottom))
                        .w(px(self.frame.w))
                        .h(px(self.frame.h))
                }
            })
            .child(self.header(cx))
            .child(self.body(cx))
            .when(!self.maximized, |card| card.children(self.grips(cx)));
        let dragging = self.drag.map(|drag| match drag.kind {
            DragKind::Move => CursorStyle::ClosedHand,
            DragKind::Resize(edges) => match (edges.left || edges.right, edges.top || edges.bottom)
            {
                (true, false) => CursorStyle::ResizeLeftRight,
                (false, true) => CursorStyle::ResizeUpDown,
                _ if edges.left == edges.top => CursorStyle::ResizeUpLeftDownRight,
                _ => CursorStyle::ResizeUpRightDownLeft,
            },
        });
        div()
            .absolute()
            .inset_0()
            .child(
                canvas(
                    move |bounds, _, _| {
                        area.set((bounds.size.width.as_f32(), bounds.size.height.as_f32()))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(card)
            // While dragging, a layer over the whole window follows the
            // mouse, so a fast drag never slips off the band it started on.
            .when_some(dragging, |layer, cursor| {
                layer.child(
                    div()
                        .id("browser-drag")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .cursor(cursor)
                        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                            this.drag_to(event.position, cx)
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.end_drag(cx)),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.end_drag(cx)),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CanvasNotice, DEFAULT_FRAME, DragKind, Edges, Frame, MIN_H, MIN_W, TITLE_BAR,
        canvas_notice, design_slug, dragged, fitted,
    };

    const AREA: (f32, f32) = (1400., 900.);

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
    }

    fn edges(left: bool, top: bool, right: bool, bottom: bool) -> DragKind {
        DragKind::Resize(Edges {
            left,
            top,
            right,
            bottom,
        })
    }

    #[test]
    fn dragging_moves_and_resizes_from_the_grabbed_edges_only() {
        let f = DEFAULT_FRAME;
        // Moving left and up grows the right and bottom margins.
        let moved = dragged(f, DragKind::Move, -100., -50., AREA);
        assert_eq!(
            (moved.right, moved.bottom, moved.w, moved.h),
            (116., 66., f.w, f.h)
        );
        // The left edge grows the card leftward; its right side stays.
        let wider = dragged(f, edges(true, false, false, false), -200., 0., AREA);
        assert_eq!((wider.w, wider.right), (f.w + 200., f.right));
        // The right edge keeps the left side in place.
        let left_side = AREA.0 - f.right - f.w;
        let narrower = dragged(f, edges(false, false, true, false), -100., 0., AREA);
        assert_eq!(narrower.w, f.w - 100.);
        assert_eq!(AREA.0 - narrower.right - narrower.w, left_side);
        // The top-left corner grows both ways.
        let big = dragged(f, edges(true, true, false, false), -300., -200., AREA);
        assert_eq!((big.w, big.h), (f.w + 300., f.h + 200.));
        // Never below the minimum size.
        let tiny = dragged(f, edges(true, true, false, false), 900., 900., AREA);
        assert_eq!((tiny.w, tiny.h), (MIN_W, MIN_H));
    }

    #[test]
    fn the_card_stays_inside_the_window() {
        // Dragged past the window's edges, it stops at them.
        let away = dragged(DEFAULT_FRAME, DragKind::Move, 500., 500., AREA);
        assert_eq!((away.right, away.bottom), (0., 0.));
        let up = dragged(DEFAULT_FRAME, DragKind::Move, -5000., -5000., AREA);
        assert_eq!(up.right, AREA.0 - up.w);
        assert_eq!(up.bottom, AREA.1 - TITLE_BAR - up.h);
        // A window that shrank keeps the card whole.
        let big = Frame {
            right: 400.,
            bottom: 300.,
            w: 900.,
            h: 600.,
        };
        let fit = fitted(big, (800., 500.));
        assert!(fit.w <= 800. && fit.right + fit.w <= 800.);
        assert!(fit.bottom + fit.h <= 500. - TITLE_BAR);
    }
}
