//! The window's notification center: extensions' informational `notify`
//! messages (plus a record of warnings and errors, which also toast) collected
//! from every tab behind a bell in the title bar instead of toasts.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{App, SharedString, Window};

use gentle_core::transcript::NoticeLevel;

mod bell;
mod panel;
mod store;

pub use bell::bell;

/// Most notices kept; the oldest drop first.
pub const CAP: usize = 200;

pub struct Notice {
    pub id: u64,
    pub level: NoticeLevel,
    pub message: String,
    /// The tab it came from, as titled when it arrived.
    pub source: SharedString,
    /// The pane it came from, to jump back to it.
    pub origin: Option<usize>,
    /// When it last arrived.
    pub at: Instant,
    pub read: bool,
    /// Further arrivals of the same message folded into this one.
    pub repeats: u32,
}

/// Notices, newest first.
#[derive(Default)]
pub struct Notices {
    items: VecDeque<Notice>,
    next_id: u64,
}

/// Jumps to the pane a notice came from.
pub type OnSelect = Rc<dyn Fn(usize, &mut Window, &mut App)>;
