//! Session lookup, layout snapshots and debounced saving.

use super::*;

impl Workspace {
    pub(super) fn active_cwd(&self, cx: &App) -> PathBuf {
        self.focused_pane()
            .and_then(|id| self.panes.get(&id))
            .map(|pane| self.pane_cwd(pane, cx))
            .unwrap_or_else(|| self.space().root.clone())
    }

    /// The agent the screens act on: the focused pane's, else the first agent
    /// in the tab, else any in the workspace.
    pub(super) fn active_session(&self) -> Option<Entity<SessionView>> {
        let session = |id: &PaneId| self.panes.get(id).and_then(|p| p.session.clone());
        let tab = self.tab()?;
        session(&tab.focused)
            .or_else(|| tab.tree.leaves().iter().find_map(session))
            .or_else(|| {
                self.space()
                    .tabs
                    .iter()
                    .flat_map(|t| t.tree.leaves())
                    .find_map(|id| session(&id))
            })
    }

    /// Start [`Self::active_session`]'s backend when a screen opens on it: it
    /// can sit in a tab not shown yet (when the shown tab has no agent).
    pub(super) fn start_active_session(&self, window: &mut Window, cx: &mut App) {
        if let Some(session) = self.active_session() {
            session.update(cx, |s, cx| s.ensure_started(window, cx));
        }
    }

    pub(super) fn sessions(&self) -> Vec<Entity<SessionView>> {
        self.panes
            .values()
            .filter_map(|p| p.session.clone())
            .collect()
    }

    // ── Persistence ─────────────────────────────────────────────────────────

    pub(super) fn saved_pane(&self, id: PaneId, cx: &App) -> Option<SavedPane> {
        let pane = self.panes.get(&id)?;
        Some(SavedPane {
            kind: pane.item.kind,
            cwd: self.pane_cwd(pane, cx),
            path: pane.path.clone(),
            session: pane
                .session
                .as_ref()
                .and_then(|s| s.read(cx).session_file()),
        })
    }

    pub(super) fn snapshot(&self, cx: &App) -> SavedState {
        let workspaces = self
            .spaces
            .iter()
            .map(|space| {
                let (tabs, active_tab) = match &space.saved {
                    Some((tabs, active)) => (tabs.clone(), *active),
                    None => (
                        space
                            .tabs
                            .iter()
                            .filter_map(|tab| {
                                let kept: Vec<PaneId> = tab
                                    .tree
                                    .leaves()
                                    .into_iter()
                                    .filter(|id| self.panes.contains_key(id))
                                    .collect();
                                Some(SavedTab {
                                    layout: tab
                                        .tree
                                        .filter_map(&mut |id| self.saved_pane(*id, cx))?,
                                    focused: kept
                                        .iter()
                                        .position(|id| *id == tab.focused)
                                        .unwrap_or(0),
                                })
                            })
                            .collect(),
                        space.active,
                    ),
                };
                SavedWorkspace {
                    name: space.name.clone(),
                    root: space.root.clone(),
                    tabs,
                    active_tab,
                    last_active: space.last_active,
                }
            })
            .collect();
        SavedState {
            version: workspaces::VERSION,
            current: self.current,
            workspaces,
            sidebar: self.sidebar_visible,
        }
    }

    /// Re-renders when a pane's title or busy state changed.
    pub(super) fn refresh_titles(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for pane in self.panes.values_mut() {
            changed |= pane.sync_chrome(cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn save_now(&mut self, cx: &App) {
        self.save_task = None;
        if let Err(err) = workspaces::save(&self.snapshot(cx)) {
            eprintln!("gentle-shell-gpui: could not save workspaces: {err}");
        }
    }

    /// Save soon, at most once per half second.
    pub(super) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.save_task.is_some() {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |this, cx| this.save_now(cx));
        }));
    }

    // ── Actions ─────────────────────────────────────────────────────────────
}
