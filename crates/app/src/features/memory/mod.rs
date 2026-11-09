//! Memory: browse and search Engram, Gentle Shell's persistent memory, over
//! its local HTTP API (`gentle-engram` talks to the same server). Read-only:
//! the agent owns writes through `mem_save` / `mem_update`.
//!
//! `client` talks HTTP to Engram, `view` renders; the protocol model and
//! decoding live in `gentle_core::engram`.

mod client;
mod view;

use std::path::PathBuf;

use gentle_core::engram::Observation;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;

pub enum MemoryEvent {
    Close,
}

pub struct MemoryView {
    cwd: PathBuf,
    project: Option<String>,
    all_projects: bool,
    query: Entity<InputState>,
    items: Vec<Observation>,
    selected: Option<i64>,
    error: Option<String>,
    loading: bool,
    _load: Task<()>,
    _subscription: Subscription,
}

impl EventEmitter<MemoryEvent> for MemoryView {}

impl MemoryView {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search memory (Enter)"));
        let subscription = cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.reload(cx);
            }
        });
        let mut this = Self {
            cwd,
            project: None,
            all_projects: false,
            query,
            items: Vec::new(),
            selected: None,
            error: None,
            loading: false,
            _load: Task::ready(()),
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        let known = self.project.clone();
        let all = self.all_projects;
        let query = self.query.read(cx).value().to_string();
        self.loading = true;
        self._load = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let project = known.or_else(|| client::current_project(&cwd));
                    let items = client::load(if all { None } else { project.as_deref() }, &query);
                    (project, items)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.project = result.0;
                match result.1 {
                    Ok(items) => {
                        this.error = None;
                        if !this
                            .selected
                            .is_some_and(|id| items.iter().any(|o| o.id == id))
                        {
                            this.selected = items.first().map(|o| o.id);
                        }
                        this.items = items;
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.items.clear();
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }
}
