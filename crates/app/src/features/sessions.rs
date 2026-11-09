//! Sessions: browse and resume previous conversations, like Pi's `/resume`.
//!
//! Pi stores one JSONL file per session under `<agent>/sessions/--<cwd>--/`
//! (`<timestamp>_<id>.jsonl`), unless `settings.json` sets `sessionDir`, in
//! which case every session lives flat in that directory and is matched to a
//! project by the header's `cwd`. RPC mode has no listing command, so we read
//! the files ourselves, mirroring `SessionManager.list`/`listAll`.

use std::path::PathBuf;

use gentle_core::sessions::SessionSummary;
use gpui_kit::component::input::InputState;
use gpui_kit::*;

mod discovery;
mod render;
mod summary;
#[cfg(test)]
mod tests;
mod view;

pub enum SessionsEvent {
    /// Resume `path`. When `cwd` is the tab's cwd send RPC
    /// `switch_session {sessionPath: path}`; otherwise the session belongs to
    /// another project and should open in a new tab rooted at `cwd`.
    Open {
        path: PathBuf,
        cwd: PathBuf,
    },
    /// Start a fresh conversation in this tab (RPC `new_session`).
    NewSession,
    Close,
}

/// Which sessions to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Sessions recorded for the tab's cwd (`--continue` / `/resume`).
    Project,
    /// Every project (`/resume` "all").
    All,
}

pub struct SessionsView {
    cwd: PathBuf,
    current: Option<PathBuf>,
    scope: Scope,
    sessions: Vec<SessionSummary>,
    loading: bool,
    filter: Entity<InputState>,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SessionsEvent> for SessionsView {}
