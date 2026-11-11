//! The in-app browser on Linux: WebKitGTK as a child of the app's X11 window.
//! GPUI hands out an XCB handle and `wry` takes an Xlib one; both name the
//! same X window. GTK must be initialized once and its loop pumped while a
//! web view exists (one task shared by every view calls `pump`). Wayland has
//! no child windows, so there the canvas stays in the browser Gentle Shell
//! opened.

use std::ffi::c_ulong;
use std::sync::LazyLock;

use gpui_kit::{Bounds, Pixels, Window};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle,
};

/// GTK's loop runs only when someone pumps it.
pub(super) const NEEDS_PUMP: bool = true;

/// Run GTK's pending events (WebKitGTK's drawing and input). Only called on
/// the main thread while a web view exists.
pub(super) fn pump() {
    if GTK.is_ok() {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
    }
}

/// GTK, initialized on first use (always from the main thread).
static GTK: LazyLock<Result<(), String>> = LazyLock::new(|| {
    gtk::init().map_err(|error| format!("The browser could not start GTK: {error}"))
});

/// The app's X window, as the Xlib handle `wry` asks for.
pub(super) struct Host(c_ulong);

impl HasWindowHandle for Host {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Xlib(XlibWindowHandle::new(self.0));
        // SAFETY: the app's window outlives the web view.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

pub(super) fn host(window: &Window) -> Result<Host, String> {
    // GPUI's own `Window::window_handle` shadows the trait's.
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Xcb(handle)) => Ok(Host(handle.window.get() as c_ulong)),
        Ok(RawWindowHandle::Xlib(handle)) => Ok(Host(handle.window)),
        _ => Err(
            "The in-app browser needs an X11 session; on Wayland the canvas \
                  opens in your browser."
                .into(),
        ),
    }
}

pub(super) struct NativeView {
    webview: wry::WebView,
    /// Where it was last placed, and whether it showed.
    placed: Option<(Bounds<Pixels>, bool)>,
}

impl NativeView {
    pub(super) fn new(
        host: Host,
        url: &str,
        background: (u8, u8, u8, u8),
        script: Option<&str>,
    ) -> Result<Self, String> {
        GTK.clone()?;
        let webview = wry::WebViewBuilder::new()
            .with_url(url)
            // Runs before the page's own scripts (a device panel names its device).
            .with_initialization_script(script.unwrap_or_default())
            .with_background_color(background)
            .with_visible(false)
            .build_as_child(&host)
            .map_err(|error| {
                format!("The browser could not start: {error}. It needs WebKitGTK 4.1.")
            })?;
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

/// The page lives in its own native window here, which gives the keyboard
/// back when the app's window is clicked.
pub(super) fn reclaim_keyboard(_window: &Window) {}
