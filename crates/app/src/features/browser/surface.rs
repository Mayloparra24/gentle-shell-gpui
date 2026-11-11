//! A web page drawn by the system's web view where GPUI lays this element
//! out. The browser card puts one in its body and the device panel one per
//! device tab. The native view sits above everything GPUI draws, so it hides
//! while a screen covers the window, and it only exists while the surface is
//! shown: a hidden page costs no web process.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{PUMP_EVERY, PUMP_IDLE, native, page_background};
use crate::shared::ui;

thread_local! {
    /// The GTK pump every live native view shares (see [`shared_pump`]).
    static PUMP: RefCell<Weak<Task<()>>> = const { RefCell::new(Weak::new()) };
}

/// A share of the one GTK pump: the first live view starts it, dropping the
/// last share stops it. One loop serves every view, so its cost doesn't
/// grow with the cards and device tabs open.
fn shared_pump(cx: &App) -> Rc<Task<()>> {
    PUMP.with(|pump| {
        if let Some(task) = pump.borrow().upgrade() {
            return task;
        }
        let task = Rc::new(cx.spawn(async move |cx| {
            loop {
                let every = if cx.update(|cx| cx.active_window().is_some()) {
                    PUMP_EVERY
                } else {
                    PUMP_IDLE
                };
                cx.background_executor().timer(every).await;
                native::pump();
            }
        }));
        *pump.borrow_mut() = Rc::downgrade(&task);
        task
    })
}

pub struct WebSurface {
    url: String,
    /// Runs before the page's own scripts, on every load.
    script: Option<String>,
    /// A screen covers the window: the native view must not show over it.
    covered: bool,
    /// Not on screen (the card is in the tray, the tab isn't active): no
    /// native view exists.
    hidden: bool,
    /// The native web view, created after the surface is first laid out.
    view: Option<native::NativeView>,
    /// The view is being built (see `place`).
    creating: bool,
    failed: Option<String>,
    /// This view's share of the GTK pump while a native view exists (Linux).
    _pump: Option<Rc<Task<()>>>,
    /// Re-places the view when the window moves (Windows hosts it in a popup
    /// positioned in screen coordinates).
    _window_moves: Option<Subscription>,
}

impl WebSurface {
    pub fn new(url: String, script: Option<String>) -> Self {
        Self {
            url,
            script,
            covered: false,
            hidden: false,
            view: None,
            creating: false,
            failed: None,
            _pump: None,
            _window_moves: None,
        }
    }

    /// Show `url`, or reload when it is already showing.
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

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.reload();
        }
        cx.notify();
    }

    /// A screen opened or closed over the window.
    pub fn set_covered(&mut self, covered: bool, cx: &mut Context<Self>) {
        if self.covered != covered {
            self.covered = covered;
            cx.notify();
        }
    }

    /// Off screen: drop the native view, and the web process behind it.
    /// Showing it again builds a fresh view on the current URL.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.hidden = true;
        self.view = None;
        self._pump = None;
        cx.notify();
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.hidden = false;
        cx.notify();
    }

    /// Place the native view over this element. The first call builds it in
    /// a task instead: that runs outside GPUI's update, which matters because
    /// WebView2 pumps the thread's messages while it starts and GPUI's window
    /// procedure would re-enter a borrowed window and abort the app.
    fn place(&mut self, bounds: Bounds<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if self.failed.is_some() || self.creating || self.hidden {
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
        let script = self.script.clone();
        let background = page_background();
        cx.spawn(async move |this, cx| {
            let view = native::NativeView::new(host, &url, background, script.as_deref());
            // A surface dropped meanwhile drops the view, and its popup with
            // it; one hidden meanwhile drops the view right here.
            let _ = this.update(cx, |surface, cx| {
                surface.creating = false;
                if surface.hidden {
                    return;
                }
                match view {
                    Ok(view) => {
                        // The page moved on while the view was starting.
                        if surface.url != url {
                            view.load_url(&surface.url);
                        }
                        surface.view = Some(view);
                        // GTK's loop only needs pumping while a view exists;
                        // `hide` drops the pump with the view.
                        if native::NEEDS_PUMP {
                            surface._pump = Some(shared_pump(cx));
                        }
                    }
                    Err(error) => surface.failed = Some(error),
                }
                // The next frame places the view.
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for WebSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._window_moves.is_none() {
            self._window_moves = Some(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));
        }
        let this = cx.entity();
        let note = self.failed.clone();
        div()
            .size_full()
            .relative()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        this.update(cx, |surface, cx| surface.place(bounds, window, cx));
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
}
