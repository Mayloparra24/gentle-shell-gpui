//! The in-app browser on Windows: WebView2 inside a borderless popup owned by
//! the app's window. GPUI composites its content with DirectComposition in
//! front of child windows, so a child web view would stay hidden behind it;
//! an owned popup sits above its owner instead, and is moved over the card
//! in screen coordinates (the card re-renders when the window moves). Windows
//! 11 rounds the popup's corners.

use std::ffi::c_void;
use std::num::NonZeroIsize;
use std::sync::Once;

use gpui_kit::{Bounds, Pixels, Window};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SWP_HIDEWINDOW, SWP_NOACTIVATE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_TOOLWINDOW,
    WS_POPUP,
};
use windows::core::w;

/// The Win32 loop GPUI already runs drives WebView2.
pub(super) const NEEDS_PUMP: bool = false;

pub(super) fn pump() {}

pub(super) struct NativeView {
    /// Dropped before the popup it lives in is destroyed.
    webview: Option<wry::WebView>,
    popup: HWND,
    owner: HWND,
    /// Screen rect (physical px) it was last placed at, and whether it showed.
    placed: Option<(i32, i32, i32, i32, bool)>,
}

/// A window handle for `wry` to build the web view in.
struct Parent(HWND);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0.0 as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(hwnd));
        // SAFETY: the popup outlives every use `wry` makes of this handle.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

const CLASS: windows::core::PCWSTR = w!("GentleShellBrowserPopup");

fn register_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return;
        };
        let class = WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: module.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&class);
    });
}

fn owner_hwnd(window: &Window) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

impl NativeView {
    pub(super) fn new(
        window: &Window,
        url: &str,
        background: (u8, u8, u8, u8),
    ) -> Result<Self, String> {
        let owner = owner_hwnd(window).ok_or("The browser could not find the app's window.")?;
        register_class();
        let popup = unsafe {
            let module = GetModuleHandleW(None).map_err(|error| error.to_string())?;
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS,
                w!(""),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(owner),
                None,
                Some(module.into()),
                None,
            )
            .map_err(|error| format!("The browser could not start: {error}"))?
        };
        unsafe {
            let corners = DWMWCP_ROUND;
            // Windows 10 has no rounded corners; the call just fails there.
            let _ = DwmSetWindowAttribute(
                popup,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corners as *const _ as *const c_void,
                std::mem::size_of_val(&corners) as u32,
            );
        }
        let webview = wry::WebViewBuilder::new()
            .with_url(url)
            .with_background_color(background)
            .with_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                size: wry::dpi::PhysicalSize::new(1u32, 1u32).into(),
            })
            .build_as_child(&Parent(popup));
        match webview {
            Ok(webview) => Ok(Self {
                webview: Some(webview),
                popup,
                owner,
                placed: None,
            }),
            Err(error) => {
                unsafe {
                    let _ = DestroyWindow(popup);
                }
                Err(format!(
                    "The browser could not start: {error}. It needs the Microsoft Edge WebView2 Runtime."
                ))
            }
        }
    }

    pub(super) fn place(&mut self, window: &Window, bounds: Bounds<Pixels>, visible: bool) {
        let scale = window.scale_factor();
        let mut origin = POINT {
            x: (bounds.origin.x.as_f32() * scale).round() as i32,
            y: (bounds.origin.y.as_f32() * scale).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(self.owner, &mut origin);
        }
        let width = ((bounds.size.width.as_f32() * scale).round() as i32).max(1);
        let height = ((bounds.size.height.as_f32() * scale).round() as i32).max(1);
        let key = (origin.x, origin.y, width, height, visible);
        if self.placed == Some(key) {
            return;
        }
        let show = if visible {
            SWP_SHOWWINDOW
        } else {
            SWP_HIDEWINDOW
        };
        unsafe {
            let _ = SetWindowPos(
                self.popup,
                None,
                origin.x,
                origin.y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER | show,
            );
        }
        if let Some(webview) = &self.webview {
            let _ = webview.set_bounds(wry::Rect {
                position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                size: wry::dpi::PhysicalSize::new(width as u32, height as u32).into(),
            });
        }
        self.placed = Some(key);
    }

    pub(super) fn load_url(&self, url: &str) {
        if let Some(webview) = &self.webview {
            let _ = webview.load_url(url);
        }
    }

    pub(super) fn reload(&self) {
        if let Some(webview) = &self.webview {
            let _ = webview.reload();
        }
    }
}

impl Drop for NativeView {
    fn drop(&mut self) {
        self.webview = None;
        unsafe {
            let _ = DestroyWindow(self.popup);
        }
    }
}
