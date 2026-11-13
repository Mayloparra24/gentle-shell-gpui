//! Pane creation, preview placement, focus, splits, closing and swapping.

use super::*;

impl Workspace {
    /// Puts a freshly built file/diff pane in the current tab: it replaces the
    /// tab's existing preview pane, else splits the largest pane along its
    /// long side, else (no room for two panes) gets its own tab.
    pub(super) fn place_preview(
        &mut self,
        new: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tab() else {
            self.push_pane_tab(new, window, cx);
            return;
        };
        let preview = tab.tree.leaves().into_iter().find(|id| {
            *id != new
                && self
                    .panes
                    .get(id)
                    .is_some_and(|p| p.item.kind == PaneKind::File)
        });
        if let Some(old) = preview
            && let Some(pane) = self.panes.remove(&new)
        {
            self.panes.insert(old, pane);
            self.focus_pane(old, window, cx);
            self.schedule_save(cx);
            return;
        }
        let largest = self
            .pane_sizes(window)
            .into_iter()
            .max_by(|a, b| (a.1 * a.2).total_cmp(&(b.1 * b.2)));
        let Some((target, w, h)) = largest else {
            self.push_pane_tab(new, window, cx);
            return;
        };
        let axis = if w >= h * 1.2 {
            SplitAxis::Horizontal
        } else {
            SplitAxis::Vertical
        };
        if !fits_split(axis, w, h) {
            self.push_pane_tab(new, window, cx);
            return;
        }
        if let Some(tab) = self.tab_mut() {
            tab.tree.split(&target, new, axis);
        }
        self.focus_pane(new, window, cx);
        self.schedule_save(cx);
    }

    /// Opens an already built pane as a new tab.
    pub(super) fn push_pane_tab(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let space = self.space_mut();
        space.tabs.push(Tab {
            id,
            tree: PaneTree::Leaf(pane),
            focused: pane,
        });
        let index = space.tabs.len() - 1;
        self.activate(index, window, cx);
    }

    /// Approximate on-screen size of each pane in the current tab.
    pub(super) fn pane_sizes(&self, window: &Window) -> Vec<(PaneId, f32, f32)> {
        let viewport = window.viewport_size();
        let sidebar = if self.sidebar.is_some() && self.sidebar_visible {
            SIDEBAR_WIDTH + 6.
        } else {
            0.
        };
        let w = f32::from(viewport.width) - 12. - sidebar;
        let h = f32::from(viewport.height) - 36. - 6.;
        let mut out = Vec::new();
        if let Some(tab) = self.tab() {
            leaf_sizes(&tab.tree, w, h, &mut out);
        }
        out
    }

    // ── Workspaces ──────────────────────────────────────────────────────────

    pub(super) fn pane_cwd(&self, pane: &Pane, cx: &App) -> PathBuf {
        match &pane.session {
            Some(session) => session.read(cx).cwd().clone(),
            None => pane.cwd.clone(),
        }
    }

    /// An agent pane; its backend starts now with `start_now` (the pane is in
    /// the tab being shown), else the first time its tab is shown.
    pub(super) fn new_agent_pane(
        &mut self,
        cwd: PathBuf,
        prompt: Option<String>,
        resume: Option<PathBuf>,
        start_now: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PaneId {
        let session = cx.new(|cx| SessionView::new(cwd.clone(), prompt, window, cx, start_now));
        if let Some(path) = resume.clone() {
            session.update(cx, |s, cx| s.resume_when_ready(path, cx));
        }
        let id = self.alloc_id();
        let focus = session.read(cx).composer().read(cx).focus_handle(cx);
        let title = {
            let session = session.clone();
            Rc::new(move |cx: &App| SharedString::from(session.read(cx).title()))
        };
        let subscriptions = vec![cx.subscribe_in(
            &session,
            window,
            move |this, session, event: &SessionEvent, window, cx| match event {
                // Tab titles, running dots and the saved conversation follow
                // each session; its other changes don't touch the chrome.
                SessionEvent::ChromeChanged => {
                    let file = session.read(cx).session_file();
                    let Some(pane) = this.panes.get_mut(&id) else {
                        return;
                    };
                    let changed = pane.sync_chrome(cx);
                    if pane.session_file != file {
                        pane.session_file = file;
                        this.schedule_save(cx);
                    }
                    if changed {
                        // An open switcher lists every workspace's activity.
                        if let Some((Overlay::Palette(view), _)) = &this.overlay {
                            let (view, rows) = (view.clone(), this.workspace_rows());
                            view.update(cx, |view, cx| view.set_workspaces(rows, cx));
                        }
                        cx.notify();
                    }
                }
                SessionEvent::OpenProviders => this.open_providers(window, cx),
                SessionEvent::OpenScreen(screen) => {
                    this.open_screen(*screen, Some(session.clone()), window, cx)
                }
                SessionEvent::Effort(level) => {
                    if let Some(level) = crate::shared::surge::Level::of(level) {
                        this.play_surge(level, cx)
                    }
                }
                SessionEvent::DesignCanvas(canvas) => {
                    this.show_design_canvas(canvas.clone(), id, window, cx)
                }
                SessionEvent::Device(intent) => {
                    // A device an agent starts itself still shows in the app.
                    if crate::features::devices::hub(cx).read(cx).prefs().enabled {
                        let intent = intent.clone();
                        this.devices
                            .update(cx, |panel, cx| panel.follow(intent, cx));
                    }
                }
                SessionEvent::DeviceCommand(command) => {
                    let command = command.clone();
                    this.devices
                        .update(cx, |panel, cx| panel.activity(command, cx));
                }
                SessionEvent::OpenWeb(url) => {
                    this.open_web(url.clone(), Some(id), true, window, cx)
                }
                SessionEvent::AgentWeb(url) => {
                    this.open_web(url.clone(), Some(id), false, window, cx)
                }
                SessionEvent::Notice { level, message } => {
                    let source = this.pane_title(id);
                    let (level, message) = (*level, message.clone());
                    this.notices.update(cx, |store, cx| {
                        store.push(level, message, source, Some(id), std::time::Instant::now());
                        cx.notify();
                    });
                    cx.notify();
                }
            },
        )];
        let mut pane = Pane {
            item: PaneItem {
                kind: PaneKind::Agent,
                view: session.clone().into(),
                focus,
                title,
                unsaved: None,
            },
            session: Some(session),
            cwd,
            path: None,
            session_file: resume,
            title: SharedString::default(),
            running: false,
            needs_you: false,
            _subscriptions: subscriptions,
        };
        pane.sync_chrome(cx);
        self.panes.insert(id, pane);
        id
    }

    /// A terminal (`target` = folder) or file (`target` = file) pane from the
    /// registered factory; a notice when there is none.
    pub(super) fn new_factory_pane(
        &mut self,
        kind: PaneKind,
        target: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneId> {
        let factories = PaneFactories::get(cx);
        let factory = match kind {
            PaneKind::Terminal => factories.terminal,
            PaneKind::File => factories.file,
            PaneKind::Agent => None,
        };
        let Some(factory) = factory else {
            window.push_notification(
                Notification::info(format!(
                    "{} panes are not available in this build.",
                    match kind {
                        PaneKind::File => "File",
                        _ => "Terminal",
                    }
                )),
                cx,
            );
            return None;
        };
        let item = factory(target.clone(), window, cx);
        let id = self.alloc_id();
        let (cwd, path) = match kind {
            PaneKind::File => (
                target
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.space().root.clone()),
                Some(target),
            ),
            _ => (target, None),
        };
        let mut pane = Pane {
            item,
            session: None,
            cwd,
            path,
            session_file: None,
            title: SharedString::default(),
            running: false,
            needs_you: false,
            _subscriptions: Vec::new(),
        };
        pane.sync_chrome(cx);
        self.panes.insert(id, pane);
        Some(id)
    }

    pub(super) fn new_pane(
        &mut self,
        kind: PaneKind,
        target: PathBuf,
        prompt: Option<String>,
        resume: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneId> {
        match kind {
            PaneKind::Agent => Some(self.new_agent_pane(target, prompt, resume, true, window, cx)),
            PaneKind::Terminal | PaneKind::File => self.new_factory_pane(kind, target, window, cx),
        }
    }

    pub(super) fn restore_pane(
        &mut self,
        saved: &SavedPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneId> {
        let cwd = if saved.cwd.is_dir() {
            saved.cwd.clone()
        } else if self.space().root.is_dir() {
            self.space().root.clone()
        } else {
            crate::cli::default_cwd()
        };
        match saved.kind {
            PaneKind::Agent => {
                let resume = saved.session.clone().filter(|p| p.exists());
                // Started once its tab is shown (see `start_shown`).
                Some(self.new_agent_pane(cwd, None, resume, false, window, cx))
            }
            PaneKind::Terminal => {
                PaneFactories::get(cx).terminal.as_ref()?;
                self.new_factory_pane(PaneKind::Terminal, cwd, window, cx)
            }
            PaneKind::File => {
                let path = saved.path.clone().filter(|p| p.is_file())?;
                PaneFactories::get(cx).file.as_ref()?;
                self.new_factory_pane(PaneKind::File, path, window, cx)
            }
        }
    }

    pub(super) fn open_tab(
        &mut self,
        kind: PaneKind,
        target: PathBuf,
        prompt: Option<String>,
        resume: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.new_pane(kind, target, prompt, resume, window, cx) else {
            return;
        };
        self.push_pane_tab(pane, window, cx);
    }

    pub(super) fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let space = self.space_mut();
        space.active = index.min(space.tabs.len().saturating_sub(1));
        self.focus_active(window, cx);
        self.start_shown(window, cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Start the backends of the shown tab's agents: restored tabs start the
    /// first time they are shown rather than all at launch.
    pub(super) fn start_shown(&self, window: &mut Window, cx: &mut App) {
        let Some(tab) = self.tab() else { return };
        for id in tab.tree.leaves() {
            if let Some(session) = self.panes.get(&id).and_then(|p| p.session.as_ref()) {
                session.update(cx, |s, cx| s.ensure_started(window, cx));
            }
        }
    }

    pub(super) fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(pane) = self.focused_pane().and_then(|id| self.panes.get(&id)) {
            pane.item.focus.focus(window, cx);
        }
    }

    pub(super) fn focus_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab_mut()
            && tab.tree.contains(&pane)
        {
            tab.focused = pane;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Show the pane wherever it lives: its workspace, its tab, then focus.
    pub(super) fn reveal_pane(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let found = self.spaces.iter().enumerate().find_map(|(s, space)| {
            space
                .tabs
                .iter()
                .position(|tab| tab.tree.contains(&pane))
                .map(|t| (s, t))
        });
        let Some((space, tab)) = found else {
            return;
        };
        if space != self.current {
            self.switch_workspace(space, window, cx);
        }
        self.activate(tab, window, cx);
        self.focus_pane(pane, window, cx);
    }

    /// Split the focused pane: with `what`, that kind and target; otherwise
    /// the focused pane's kind in its folder (or on its file).
    pub(super) fn split(
        &mut self,
        axis: SplitAxis,
        what: Option<(PaneKind, PathBuf)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(focused) = self.focused_pane() else {
            let root = self.space().root.clone();
            self.open_tab(PaneKind::Agent, root, None, None, window, cx);
            return;
        };
        let (kind, target) = match what {
            Some(what) => what,
            None => {
                let pane = &self.panes[&focused];
                let kind = pane.item.kind;
                let target = match (kind, &pane.path) {
                    (PaneKind::File, Some(path)) => path.clone(),
                    _ => self.pane_cwd(pane, cx),
                };
                (kind, target)
            }
        };
        let Some(pane) = self.new_pane(kind, target, None, None, window, cx) else {
            return;
        };
        let room = self
            .pane_sizes(window)
            .into_iter()
            .find(|(id, ..)| *id == focused)
            .is_none_or(|(_, w, h)| fits_split(axis, w, h));
        if !room {
            self.push_pane_tab(pane, window, cx);
            return;
        }
        if let Some(tab) = self.tab_mut() {
            tab.tree.split(&focused, pane, axis);
        }
        self.focus_pane(pane, window, cx);
        self.schedule_save(cx);
    }

    /// A new pane tiled like Hyprland's dwindle: the focused pane splits
    /// along its longer side (the other one if only that fits), so panes
    /// spiral in instead of lining up in one row. No room: a new tab.
    pub(super) fn tile(
        &mut self,
        what: Option<(PaneKind, PathBuf)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self.focused_pane();
        let axis = self
            .pane_sizes(window)
            .into_iter()
            .find(|(id, ..)| Some(*id) == focused)
            .and_then(|(_, w, h)| tile_axis(w, h))
            // No size yet, or no room either way: `split` opens a tab.
            .unwrap_or(SplitAxis::Horizontal);
        self.split(axis, what, window, cx);
    }

    pub(super) fn split_with_kind(
        &mut self,
        kind: PaneKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.active_cwd(cx);
        self.tile(Some((kind, cwd)), window, cx);
    }

    /// Drop `moving` on `target` at `zone`: beside it on that side, or swap
    /// the two in the middle. A side whose halves wouldn't fit swaps instead.
    pub(super) fn move_pane(
        &mut self,
        moving: PaneId,
        target: PaneId,
        zone: DropZone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placement = zone.placement().filter(|(axis, _)| {
            self.pane_sizes(window)
                .into_iter()
                .find(|(id, ..)| *id == target)
                .is_none_or(|(_, w, h)| fits_split(*axis, w, h))
        });
        let Some((axis, before)) = placement else {
            self.swap_panes(moving, target, cx);
            return;
        };
        if moving == target {
            return;
        }
        if let Some(tab) = self.tab_mut() {
            let tree = std::mem::replace(&mut tab.tree, PaneTree::Leaf(moving));
            tab.tree = tree.moved(&moving, &target, axis, before);
            tab.focused = moving;
        }
        self.schedule_save(cx);
        cx.notify();
    }

    /// Close `pane`, asking first when it holds unsaved edits.
    pub(super) fn close_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let unsaved: Vec<SharedString> = self.unsaved_titles([pane], cx);
        if unsaved.is_empty() {
            self.close_pane_now(pane, window, cx);
        } else {
            self.confirm_discard_edits(unsaved, window, cx, move |this, window, cx| {
                this.close_pane_now(pane, window, cx)
            });
        }
    }

    /// Titles of the file panes among `panes` with edits not yet saved.
    pub(super) fn unsaved_titles(
        &self,
        panes: impl IntoIterator<Item = PaneId>,
        cx: &App,
    ) -> Vec<SharedString> {
        panes
            .into_iter()
            .filter_map(|id| self.panes.get(&id))
            .filter(|p| p.item.unsaved.as_ref().is_some_and(|u| u(cx)))
            .map(|p| (p.item.title)(cx))
            .collect()
    }

    /// "Close without saving?" for `titles`; `then` runs only on confirm.
    pub(super) fn confirm_discard_edits(
        &mut self,
        titles: Vec<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let names = titles
            .iter()
            .map(|t| t.trim_end_matches(" •").to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let title = if titles.len() == 1 {
            format!("Close {names} without saving?")
        } else {
            format!("Close {} files without saving?", titles.len())
        };
        let description = format!("Unsaved edits in {names} are lost. ⌘S saves first.");
        let this = cx.entity().downgrade();
        let then = Rc::new(then);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, then) = (this.clone(), then.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .ok_text("Close without saving")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    let then = then.clone();
                    let _ = this.update(cx, |this, cx| then(this, window, cx));
                    true
                })
        });
    }

    pub(super) fn close_pane_now(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .space()
            .tabs
            .iter()
            .position(|t| t.tree.contains(&pane))
        else {
            return;
        };
        let active = index == self.space().active;
        let tab = &mut self.space_mut().tabs[index];
        if tab.tree.len() == 1 {
            self.close_tab_now(index, window, cx);
            return;
        }
        let next = tab.tree.focus_after_close(&pane);
        if let Some(tree) = tab.tree.clone().without(&pane) {
            tab.tree = tree;
        }
        if tab.focused == pane
            && let Some(next) = next
        {
            tab.focused = next;
        }
        // Dropping a session drops its PiProcess, which shuts the backend down.
        self.panes.remove(&pane);
        if active {
            self.focus_active(window, cx);
        }
        self.schedule_save(cx);
        cx.notify();
    }

    /// Close tab `index`, asking first when any of its panes holds unsaved edits.
    pub(super) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.space().tabs.get(index) else {
            return;
        };
        let unsaved = self.unsaved_titles(tab.tree.leaves(), cx);
        if unsaved.is_empty() {
            self.close_tab_now(index, window, cx);
        } else {
            let id = tab.id;
            self.confirm_discard_edits(unsaved, window, cx, move |this, window, cx| {
                // The tab may have moved while the dialog was open.
                if let Some(index) = this.space().tabs.iter().position(|t| t.id == id) {
                    this.close_tab_now(index, window, cx);
                }
            });
        }
    }

    pub(super) fn close_tab_now(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.space().tabs.len() {
            return;
        }
        let tab = self.space_mut().tabs.remove(index);
        for pane in tab.tree.leaves() {
            self.panes.remove(&pane);
        }
        if self.space().tabs.is_empty() {
            // Closing the last pane never closes the window (⌘Q quits):
            // the workspace starts over with a fresh agent tab.
            let root = self.space().root.clone();
            self.open_tab(PaneKind::Agent, root, None, None, window, cx);
            self.save_now(cx);
            return;
        }
        let active = self.space().active;
        let next = if active > index { active - 1 } else { active };
        self.activate(next, window, cx);
    }

    pub(super) fn swap_panes(&mut self, a: PaneId, b: PaneId, cx: &mut Context<Self>) {
        if a == b {
            return;
        }
        if let Some(tab) = self.tab_mut()
            && tab.tree.contains(&a)
            && tab.tree.contains(&b)
        {
            tab.tree.swap(&a, &b);
            self.schedule_save(cx);
            cx.notify();
        }
    }
}
