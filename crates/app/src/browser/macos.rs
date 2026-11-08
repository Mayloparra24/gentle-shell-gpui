//! The in-app browser on macOS: WKWebView as a subview of the window's
//! content view. AppKit clips nothing GPUI draws, so its layer is rounded to
//! the card body's corners.

use gpui_kit::{Bounds, Pixels, Window};

/// AppKit runs the web view's loop itself.
pub(super) const NEEDS_PUMP: bool = false;

pub(super) fn pump() {}

pub(super) struct NativeView {
    webview: wry::WebView,
    /// Where it was last placed, and whether it showed.
    placed: Option<(Bounds<Pixels>, bool)>,
}

impl NativeView {
    pub(super) fn new(
        window: &Window,
        url: &str,
        background: (u8, u8, u8, u8),
    ) -> Result<Self, String> {
        let webview = wry::WebViewBuilder::new()
            .with_url(url)
            .with_background_color(background)
            .with_visible(false)
            .build_as_child(window)
            .map_err(|error| format!("The browser could not start: {error}"))?;
        round_corners(&webview);
        Ok(Self {
            webview,
            placed: None,
        })
    }

    pub(super) fn place(&mut self, _window: &Window, bounds: Bounds<Pixels>, visible: bool) {
        if self.placed == Some((bounds, visible)) {
            return;
        }
        let _ = self.webview.set_bounds(super::logical_rect(bounds));
        let _ = self.webview.set_visible(visible);
        self.placed = Some((bounds, visible));
    }

    pub(super) fn load_url(&self, url: &str) {
        let _ = self.webview.load_url(url);
    }

    pub(super) fn reload(&self) {
        let _ = self.webview.reload();
    }
}

/// Round the native view's own layer: GPUI's rounding doesn't clip a native
/// view, which would otherwise show square corners inside the card.
fn round_corners(webview: &wry::WebView) {
    use wry::WebViewExtMacOS as _;
    let view = webview.webview();
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(super::BODY_RADIUS as f64);
        layer.setMasksToBounds(true);
    }
}
