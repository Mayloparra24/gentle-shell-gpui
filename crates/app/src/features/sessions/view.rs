//! Browser state and actions: construction, background reload, scope switch,
//! search filtering and opening a session.

use std::path::{Path, PathBuf};

use gentle_core::sessions::SessionSummary;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;

use super::discovery::list_sessions;
use super::{Scope, SessionsEvent, SessionsView};

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((dunce::canonicalize(a), dunce::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

impl SessionsView {
    pub fn new(
        cwd: PathBuf,
        current_session_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search by name, first message or path")
        });
        let subscriptions = vec![cx.subscribe_in(
            &filter,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event
                    && let Some(first) = this.visible(cx).first().map(|s| (*s).clone())
                {
                    this.open(&first, cx);
                }
                cx.notify();
            },
        )];
        let mut this = Self {
            cwd,
            current: current_session_file,
            scope: Scope::Project,
            sessions: Vec::new(),
            loading: false,
            filter,
            _load: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let agent = pi_rpc::resolve_homes().agent;
        let cwd = self.cwd.clone();
        let scope = self.scope;
        // Replacing the task drops (cancels) any load still in flight.
        self._load = cx.spawn(async move |this, cx| {
            let sessions = cx
                .background_executor()
                .spawn(async move { list_sessions(&agent, &cwd, scope) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.sessions = sessions;
                this.loading = false;
                cx.notify();
            });
        });
        cx.notify();
    }

    pub(super) fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope != scope {
            self.scope = scope;
            self.sessions.clear();
            self.reload(cx);
        }
    }

    pub(super) fn is_current(&self, session: &SessionSummary) -> bool {
        self.current
            .as_deref()
            .is_some_and(|c| same_file(c, &session.path))
    }

    pub(super) fn visible(&self, cx: &App) -> Vec<&SessionSummary> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        self.sessions.iter().filter(|s| s.matches(&query)).collect()
    }

    pub(super) fn open(&mut self, session: &SessionSummary, cx: &mut Context<Self>) {
        if self.is_current(session) {
            cx.emit(SessionsEvent::Close);
            return;
        }
        let cwd = if session.cwd.as_os_str().is_empty() {
            self.cwd.clone()
        } else {
            session.cwd.clone()
        };
        cx.emit(SessionsEvent::Open {
            path: session.path.clone(),
            cwd,
        });
    }
}
