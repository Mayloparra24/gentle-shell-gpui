//! The window, after Tern: named workspaces (one shown at a time), each with
//! its own tabs; every tab tiles panes (agents, terminals, files) in a split
//! tree with draggable dividers. A left sidebar slot, the screens drawn over
//! everything, and the layout saved to disk as it changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable as _, TitleBar, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::commands::Screen;
use crate::files::FileTreeView;
use crate::memory::{MemoryEvent, MemoryView};
use crate::notices::{self, Notices};
use crate::palette::{
    Category, PaletteAction, PaletteEvent, PaletteInput, PaletteView, WorkspaceRow,
};
use crate::pane::{PaneFactories, PaneId, PaneItem, PaneKind, PaneTree, SplitAxis};
use crate::profiles::{ModelChoice, ProfilesEvent, ProfilesView};
use crate::providers::{ProvidersEvent, ProvidersView};
use crate::session::{SessionEvent, SessionView};
use crate::sessions::{SessionsEvent, SessionsView};
use crate::settings::{SettingsEvent, SettingsView};
use crate::ui;
use crate::usage::{UsageEvent, UsageView};
use crate::workspaces::{self, SavedPane, SavedState, SavedTab, SavedWorkspace};

actions!(
    gentle,
    [
        NewTab,
        NewTerminalTab,
        CloseTab,
        NextTab,
        PrevTab,
        SplitRight,
        SplitDown,
        NewAgentPane,
        NewTerminalPane,
        ClosePane,
        FocusNextPane,
        FocusPrevPane,
        ToggleSidebar,
        OpenProviders,
        OpenSessions,
        OpenPalette,
        OpenWorkspaces,
        CloseOverlay,
        Quit
    ]
);

pub const SIDEBAR_WIDTH: f32 = 260.;
/// Smallest useful pane; splits and divider drags stop here.
const MIN_PANE_WIDTH: f32 = 320.;
const MIN_PANE_HEIGHT: f32 = 200.;
/// Space a divider takes between two panes.
const DIVIDER: f32 = 6.;

/// Whether a `w`×`h` pane split along `axis` leaves both halves usable.
fn fits_split(axis: SplitAxis, w: f32, h: f32) -> bool {
    match axis {
        SplitAxis::Horizontal => (w - DIVIDER) / 2. >= MIN_PANE_WIDTH,
        SplitAxis::Vertical => (h - DIVIDER) / 2. >= MIN_PANE_HEIGHT,
    }
}

/// Each leaf's size when `node` fills `w`×`h`.
fn leaf_sizes(node: &PaneTree<PaneId>, w: f32, h: f32, out: &mut Vec<(PaneId, f32, f32)>) {
    match node {
        PaneTree::Leaf(id) => out.push((*id, w, h)),
        PaneTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            if *axis == SplitAxis::Horizontal {
                let inner = w - DIVIDER;
                leaf_sizes(first, inner * ratio, h, out);
                leaf_sizes(second, inner * (1. - ratio), h, out);
            } else {
                let inner = h - DIVIDER;
                leaf_sizes(first, w, inner * ratio, out);
                leaf_sizes(second, w, inner * (1. - ratio), out);
            }
        }
    }
}

/// What the window tells whoever embeds things in it (the sidebar's owner).
pub enum WorkspaceEvent {
    /// The shown workspace changed; its root folder.
    RootChanged(PathBuf),
    /// The left sidebar was shown or hidden.
    SidebarToggled(bool),
}

/// A window-level screen drawn over the active tab.
enum Overlay {
    Providers(Entity<ProvidersView>),
    Sessions(Entity<SessionsView>),
    Palette(Entity<PaletteView>),
    Usage(Entity<UsageView>),
    Memory(Entity<MemoryView>),
    Settings(Entity<SettingsView>),
    Profiles(Entity<ProfilesView>),
}

struct Pane {
    item: PaneItem,
    /// Set for agent panes.
    session: Option<Entity<SessionView>>,
    /// Folder of a terminal; parent folder of a file.
    cwd: PathBuf,
    /// The file a file pane shows.
    path: Option<PathBuf>,
    /// Last conversation file seen, to save when it changes.
    session_file: Option<PathBuf>,
    /// What the tab bar shows for this pane, kept here so drawing the chrome
    /// never reads the views of tabs that aren't shown (reading an entity
    /// while drawing makes its every notify redraw the window). Agents
    /// refresh it on `SessionEvent::ChromeChanged`, everything on the title poll.
    title: SharedString,
    running: bool,
    _subscriptions: Vec<Subscription>,
}

impl Pane {
    /// Re-read the title and busy state; whether either changed.
    fn sync_chrome(&mut self, cx: &App) -> bool {
        let title = (self.item.title)(cx);
        let running = self
            .session
            .as_ref()
            .is_some_and(|s| s.read(cx).is_running());
        let changed = title != self.title || running != self.running;
        self.title = title;
        self.running = running;
        changed
    }
}

struct Tab {
    id: usize,
    tree: PaneTree<PaneId>,
    focused: PaneId,
}

/// One named workspace.
struct Space {
    name: String,
    root: PathBuf,
    last_active: u64,
    tabs: Vec<Tab>,
    active: usize,
    /// Restored layout not built yet: workspaces start their panes the first
    /// time they are shown.
    saved: Option<(Vec<SavedTab>, usize)>,
}

impl Space {
    fn new(name: String, root: PathBuf) -> Self {
        Self {
            name,
            root,
            last_active: workspaces::now(),
            tabs: Vec::new(),
            active: 0,
            saved: None,
        }
    }

    fn tab_count(&self) -> usize {
        match &self.saved {
            Some((tabs, _)) => tabs.len(),
            None => self.tabs.len(),
        }
    }
}

/// Dragged by a divider: which split it resizes.
#[derive(Clone)]
struct DividerDrag {
    tab: usize,
    path: Vec<bool>,
}

/// Dragged by a pane's grip pill: dropping on another pane swaps them.
#[derive(Clone)]
struct PaneDrag {
    pane: PaneId,
    title: SharedString,
}

/// The inline name field of an agent tab being renamed (double-click).
struct TabRename {
    pane: PaneId,
    input: Entity<InputState>,
    _subscription: Subscription,
}

struct DragChip(SharedString);

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded_lg()
            .bg(ui::raised())
            .border_1()
            .border_color(ui::petal().opacity(0.5))
            .shadow_lg()
            .text_xs()
            .text_color(ui::fg())
            .child(
                Icon::new(IconName::GripHorizontal)
                    .xsmall()
                    .text_color(ui::petal()),
            )
            .child(self.0.clone())
    }
}

struct Invisible;

impl Render for Invisible {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub struct Workspace {
    spaces: Vec<Space>,
    current: usize,
    panes: HashMap<PaneId, Pane>,
    next_id: usize,
    overlay: Option<(Overlay, Subscription)>,
    sidebar: Option<AnyView>,
    sidebar_visible: bool,
    save_task: Option<Task<()>>,
    /// Re-reads every pane's title and busy state once a second (non-agent
    /// views can't be observed through `AnyView`), redrawing only on change.
    _title_poll: Task<()>,
    _lifecycle: Vec<Subscription>,
    /// The bell's notification center, shared by every tab.
    notices: Entity<Notices>,
    renaming: Option<TabRename>,
    /// The max-effort surge playing over the window (its run number and when
    /// it started), and the timer that clears it.
    surge: Option<(u64, std::time::Instant)>,
    surge_runs: u64,
    _surge_end: Option<Task<()>>,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Workspace {
    /// Restores the saved workspaces. `folder` (the command-line argument)
    /// selects the workspace rooted there, creating it when new; `prompt`
    /// opens in a new agent tab.
    pub fn new(
        folder: Option<PathBuf>,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let saved = workspaces::load();
        let spaces: Vec<Space> = saved
            .workspaces
            .into_iter()
            .map(|w| Space {
                name: w.name,
                root: w.root,
                last_active: w.last_active,
                tabs: Vec::new(),
                active: 0,
                saved: Some((w.tabs, w.active_tab)),
            })
            .collect();
        let lifecycle = vec![
            cx.on_app_quit(|this, cx| {
                this.save_now(cx);
                async {}
            }),
            cx.on_release(|this, cx| this.save_now(cx)),
            // The title-bar update pill follows this app's own update state.
            cx.observe_global::<crate::app_update::AppUpdate>(|_, cx| cx.notify()),
        ];
        let mut this = Self {
            current: saved.current.min(spaces.len().saturating_sub(1)),
            spaces,
            panes: HashMap::new(),
            next_id: 0,
            overlay: None,
            sidebar: None,
            sidebar_visible: saved.sidebar,
            save_task: None,
            notices: cx.new(|_| Notices::default()),
            renaming: None,
            surge: None,
            surge_runs: 0,
            _surge_end: None,
            _title_poll: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let Ok(()) = this.update(cx, |this, cx| this.refresh_titles(cx)) else {
                        break;
                    };
                }
            }),
            _lifecycle: lifecycle,
        };
        let target = match folder {
            Some(folder) => match this.spaces.iter().position(|s| s.root == folder) {
                Some(index) => index,
                None => this.push_space(None, folder),
            },
            None if this.spaces.is_empty() => this.push_space(None, crate::default_cwd()),
            None => this.current,
        };
        this.current = target;
        this.ensure_open(target, prompt.is_none(), window, cx);
        if let Some(prompt) = prompt {
            let root = this.space().root.clone();
            this.open_tab(PaneKind::Agent, root, Some(prompt), None, window, cx);
        }
        this.space_mut().last_active = workspaces::now();
        this.focus_active(window, cx);
        this.start_shown(window, cx);
        this.schedule_save(cx);
        this
    }

    // ── Public hooks ────────────────────────────────────────────────────────

    /// A later launch's request (see `crate::instance`): show the workspace
    /// rooted at `folder` (creating it), and open `prompt` in a new agent tab.
    pub fn open_request(
        &mut self,
        folder: Option<PathBuf>,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(folder) = folder {
            let index = match self.spaces.iter().position(|s| s.root == folder) {
                Some(index) => index,
                None => self.push_space(None, folder),
            };
            self.switch_workspace(index, window, cx);
        }
        if let Some(prompt) = prompt {
            let root = self.space().root.clone();
            self.open_tab(PaneKind::Agent, root, Some(prompt), None, window, cx);
        }
        cx.notify();
    }

    /// Put `view` in the left sidebar slot (cmd-b toggles it).
    pub fn set_sidebar(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.sidebar = Some(view);
        cx.notify();
    }

    /// The shown workspace's root folder.
    pub fn root(&self) -> &Path {
        &self.space().root
    }

    /// Show `path` in the tab's preview pane, or a new split beside the largest pane.
    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab().is_none() {
            self.open_tab(PaneKind::File, path, None, None, window, cx);
            return;
        }
        if let Some(pane) = self.new_pane(PaneKind::File, path, None, None, window, cx) {
            self.place_preview(pane, window, cx);
        }
    }

    /// Show `path`'s changes against HEAD, placed like [`Self::open_file`].
    pub fn open_diff(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(factory), Some(_)) = (PaneFactories::get(cx).diff, self.focused_pane()) else {
            self.open_file(path, window, cx);
            return;
        };
        let item = factory(path.clone(), window, cx);
        let id = self.alloc_id();
        let cwd = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.space().root.clone());
        let mut pane = Pane {
            item,
            session: None,
            cwd,
            path: Some(path),
            session_file: None,
            title: SharedString::default(),
            running: false,
            _subscriptions: Vec::new(),
        };
        pane.sync_chrome(cx);
        self.panes.insert(id, pane);
        self.place_preview(id, window, cx);
    }

    /// Open `path` in a new split right of the focused pane (its own tab when
    /// there is no room), or like [`Self::open_file`] in an empty workspace.
    pub fn open_file_beside(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused_pane().is_none() {
            self.open_file(path, window, cx);
            return;
        }
        self.split(
            SplitAxis::Horizontal,
            Some((PaneKind::File, path)),
            window,
            cx,
        );
    }

    /// Open `path` as a tab of its own.
    pub fn open_file_in_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(PaneKind::File, path, None, None, window, cx);
    }

    /// Type `text` at the active agent's prompt cursor, spaced from what is
    /// before it, and focus the prompt. `false` when no agent is open.
    pub fn insert_into_composer(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.active_session() else {
            return false;
        };
        let pane = self
            .panes
            .iter()
            .find(|(_, pane)| pane.session.as_ref() == Some(&session))
            .map(|(id, _)| *id);
        if let Some(pane) = pane {
            self.reveal_pane(pane, window, cx);
        }
        let composer = session.read(cx).composer();
        composer.update(cx, |state, cx| {
            let value = state.value();
            let before = value
                .get(..state.cursor())
                .and_then(|s| s.chars().next_back());
            let lead = if before.is_some_and(|c| !c.is_whitespace()) {
                " "
            } else {
                ""
            };
            state.insert(format!("{lead}{text} "), window, cx);
            state.focus(window, cx);
        });
        true
    }

    /// Puts a freshly built file/diff pane in the current tab: it replaces the
    /// tab's existing preview pane, else splits the largest pane along its
    /// long side, else (no room for two panes) gets its own tab.
    fn place_preview(&mut self, new: PaneId, window: &mut Window, cx: &mut Context<Self>) {
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
    fn push_pane_tab(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
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
    fn pane_sizes(&self, window: &Window) -> Vec<(PaneId, f32, f32)> {
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

    fn space(&self) -> &Space {
        &self.spaces[self.current]
    }

    fn space_mut(&mut self) -> &mut Space {
        &mut self.spaces[self.current]
    }

    fn push_space(&mut self, name: Option<String>, root: PathBuf) -> usize {
        let base = name.unwrap_or_else(|| ui::split_path(&root).1);
        let name = workspaces::unique_name(self.spaces.iter().map(|s| s.name.as_str()), &base);
        self.spaces.push(Space::new(name, root));
        self.spaces.len() - 1
    }

    /// Build a restored workspace's panes; with `fill`, give an empty one an
    /// agent tab.
    fn ensure_open(
        &mut self,
        index: usize,
        fill: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = self.current;
        self.current = index;
        if let Some((tabs, active)) = self.spaces[index].saved.take() {
            for saved in tabs {
                let leaves = saved.layout.leaves();
                let mut built = Vec::new();
                let tree = saved.layout.filter_map(&mut |pane: &SavedPane| {
                    let id = self.restore_pane(pane, window, cx);
                    built.push(id);
                    id
                });
                let Some(tree) = tree else { continue };
                let focused = built
                    .get(saved.focused.min(leaves.len().saturating_sub(1)))
                    .copied()
                    .flatten()
                    .or_else(|| tree.leaves().first().copied())
                    .unwrap_or_default();
                let id = self.alloc_id();
                self.spaces[index].tabs.push(Tab { id, tree, focused });
            }
            let space = &mut self.spaces[index];
            space.active = active.min(space.tabs.len().saturating_sub(1));
        }
        if fill && self.spaces[index].tabs.is_empty() {
            let root = self.spaces[index].root.clone();
            self.open_tab(PaneKind::Agent, root, None, None, window, cx);
        }
        self.current = previous;
    }

    fn switch_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.spaces.len() {
            return;
        }
        self.ensure_open(index, true, window, cx);
        let changed = index != self.current;
        self.current = index;
        self.space_mut().last_active = workspaces::now();
        self.focus_active(window, cx);
        self.start_shown(window, cx);
        if changed {
            cx.emit(WorkspaceEvent::RootChanged(self.space().root.clone()));
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn create_workspace(
        &mut self,
        name: Option<String>,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.push_space(name.filter(|n| !n.trim().is_empty()), root);
        self.switch_workspace(index, window, cx);
    }

    fn pick_workspace_folder(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Create workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.pop() else { return };
            let _ = this.update_in(cx, |this, window, cx| {
                this.create_workspace(Some(name), path, window, cx)
            });
        })
        .detach();
    }

    fn rename_workspace(&mut self, index: usize, name: String, cx: &mut Context<Self>) {
        let others = self
            .spaces
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, s)| s.name.as_str());
        let name = workspaces::unique_name(others, &name);
        if let Some(space) = self.spaces.get_mut(index) {
            space.name = name;
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn confirm_delete_workspace(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(space) = self.spaces.get(index) else {
            return;
        };
        if self.spaces.len() < 2 {
            return;
        }
        let title = format!("Delete workspace \u{201c}{}\u{201d}?", space.name);
        let tabs = space.tab_count();
        let description = format!(
            "Its {tabs} {} close and any running agent in them stops. Files and conversations stay on disk.",
            if tabs == 1 { "tab" } else { "tabs" }
        );
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(title.clone())
                .description(description.clone())
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.delete_workspace(index, window, cx));
                    true
                })
        });
    }

    fn delete_workspace(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.spaces.len() || self.spaces.len() < 2 {
            return;
        }
        if index == self.current {
            let next = (0..self.spaces.len())
                .filter(|i| *i != index)
                .max_by_key(|i| self.spaces[*i].last_active)
                .unwrap_or(0);
            self.switch_workspace(next, window, cx);
        }
        let space = self.spaces.remove(index);
        for tab in space.tabs {
            for pane in tab.tree.leaves() {
                // Dropping a session drops its PiProcess, which shuts the backend down.
                self.panes.remove(&pane);
            }
        }
        if self.current > index {
            self.current -= 1;
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn confirm_switch_branch(
        &mut self,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.space().root.clone();
        let title = format!("Switch to \u{201c}{branch}\u{201d}?");
        let description = format!(
            "Runs git switch {branch} in {}. Uncommitted changes that conflict make git refuse.",
            ui::split_path(&root).1
        );
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, root, branch) = (this.clone(), root.clone(), branch.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .ok_text("Switch")
                .show_cancel(true)
                .on_ok(move |_, window, cx| {
                    let (root, branch) = (root.clone(), branch.clone());
                    let _ = this.update(cx, |_, cx| {
                        cx.spawn_in(window, async move |_, cx| {
                            let label = branch.clone();
                            let result = cx
                                .background_executor()
                                .spawn(async move { crate::palette::switch_branch(&root, &branch) })
                                .await;
                            let _ = cx.update(|window, cx| {
                                let note = match result {
                                    Ok(()) => Notification::success(format!("Switched to {label}")),
                                    Err(message) => Notification::error(message),
                                };
                                window.push_notification(note, cx);
                            });
                        })
                        .detach();
                    });
                    true
                })
        });
    }

    // ── Panes ───────────────────────────────────────────────────────────────

    fn alloc_id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    fn tab(&self) -> Option<&Tab> {
        let space = self.space();
        space.tabs.get(space.active)
    }

    /// Open the inline name field on tab `index` when its focused pane is an
    /// agent; terminal and file tabs keep their titles.
    fn start_tab_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.space().tabs.get(index).map(|t| t.focused) else {
            return;
        };
        if self.panes.get(&pane).is_none_or(|p| p.session.is_none()) {
            return;
        }
        self.finish_tab_rename(false, window, cx);
        let title = self.pane_title(pane).to_string();
        let len = title.len();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                // Enter commits, and so does clicking away (like the file tree).
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.finish_tab_rename(true, window, cx)
                }
                _ => {}
            },
        );
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(0..len, cx);
        });
        self.renaming = Some(TabRename {
            pane,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn cancel_tab_rename(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_tab_rename(false, window, cx);
    }

    /// Close the tab's name field; with `commit`, a changed non-empty name
    /// becomes the session's name (RPC `set_session_name`).
    fn finish_tab_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.renaming.take() else {
            return;
        };
        let focused = rename.input.focus_handle(cx).is_focused(window);
        let name = rename.input.read(cx).value().trim().to_string();
        cx.notify();
        let Some(pane) = self.panes.get(&rename.pane) else {
            return;
        };
        // Enter and Escape hand the keyboard back to the agent; a click
        // elsewhere keeps its new focus.
        if focused {
            pane.item.focus.focus(window, cx);
        }
        if commit
            && !name.is_empty()
            && name != pane.title
            && let Some(session) = pane.session.clone()
        {
            session.update(cx, |s, cx| s.rename(name, cx));
        }
    }

    fn tab_mut(&mut self) -> Option<&mut Tab> {
        let space = self.space_mut();
        space.tabs.get_mut(space.active)
    }

    fn focused_pane(&self) -> Option<PaneId> {
        self.tab().map(|t| t.focused)
    }

    fn pane_cwd(&self, pane: &Pane, cx: &App) -> PathBuf {
        match &pane.session {
            Some(session) => session.read(cx).cwd().clone(),
            None => pane.cwd.clone(),
        }
    }

    /// An agent pane; its backend starts now with `start_now` (the pane is in
    /// the tab being shown), else the first time its tab is shown.
    fn new_agent_pane(
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
                        cx.notify();
                    }
                }
                SessionEvent::OpenProviders => this.open_providers(window, cx),
                SessionEvent::OpenScreen(screen) => {
                    this.open_screen(*screen, Some(session.clone()), window, cx)
                }
                SessionEvent::MaxEffort => this.play_surge(cx),
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
            _subscriptions: subscriptions,
        };
        pane.sync_chrome(cx);
        self.panes.insert(id, pane);
        id
    }

    /// A terminal (`target` = folder) or file (`target` = file) pane from the
    /// registered factory; a notice when there is none.
    fn new_factory_pane(
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
            _subscriptions: Vec::new(),
        };
        pane.sync_chrome(cx);
        self.panes.insert(id, pane);
        Some(id)
    }

    fn new_pane(
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

    fn restore_pane(
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
            crate::default_cwd()
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

    fn open_tab(
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

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let space = self.space_mut();
        space.active = index.min(space.tabs.len().saturating_sub(1));
        self.focus_active(window, cx);
        self.start_shown(window, cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Start the backends of the shown tab's agents: restored tabs start the
    /// first time they are shown rather than all at launch.
    fn start_shown(&self, window: &mut Window, cx: &mut App) {
        let Some(tab) = self.tab() else { return };
        for id in tab.tree.leaves() {
            if let Some(session) = self.panes.get(&id).and_then(|p| p.session.as_ref()) {
                session.update(cx, |s, cx| s.ensure_started(window, cx));
            }
        }
    }

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(pane) = self.focused_pane().and_then(|id| self.panes.get(&id)) {
            pane.item.focus.focus(window, cx);
        }
    }

    fn focus_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab_mut()
            && tab.tree.contains(&pane)
        {
            tab.focused = pane;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Show the pane wherever it lives: its workspace, its tab, then focus.
    fn reveal_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
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
    fn split(
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

    fn split_with_kind(&mut self, kind: PaneKind, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.active_cwd(cx);
        self.split(SplitAxis::Horizontal, Some((kind, cwd)), window, cx);
    }

    /// Close `pane`, asking first when it holds unsaved edits.
    fn close_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
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
    fn unsaved_titles(
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
    fn confirm_discard_edits(
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

    fn close_pane_now(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
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
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
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

    fn close_tab_now(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
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

    fn swap_panes(&mut self, a: PaneId, b: PaneId, cx: &mut Context<Self>) {
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

    fn active_cwd(&self, cx: &App) -> PathBuf {
        self.focused_pane()
            .and_then(|id| self.panes.get(&id))
            .map(|pane| self.pane_cwd(pane, cx))
            .unwrap_or_else(|| self.space().root.clone())
    }

    /// The agent the screens act on: the focused pane's, else the first agent
    /// in the tab, else any in the workspace.
    fn active_session(&self) -> Option<Entity<SessionView>> {
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
    fn start_active_session(&self, window: &mut Window, cx: &mut App) {
        if let Some(session) = self.active_session() {
            session.update(cx, |s, cx| s.ensure_started(window, cx));
        }
    }

    fn sessions(&self) -> Vec<Entity<SessionView>> {
        self.panes
            .values()
            .filter_map(|p| p.session.clone())
            .collect()
    }

    // ── Persistence ─────────────────────────────────────────────────────────

    fn saved_pane(&self, id: PaneId, cx: &App) -> Option<SavedPane> {
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

    fn snapshot(&self, cx: &App) -> SavedState {
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
    fn refresh_titles(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for pane in self.panes.values_mut() {
            changed |= pane.sync_chrome(cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn save_now(&mut self, cx: &App) {
        self.save_task = None;
        if let Err(err) = workspaces::save(&self.snapshot(cx)) {
            eprintln!("gentle-shell-gpui: could not save workspaces: {err}");
        }
    }

    /// Save soon, at most once per half second.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
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

    fn on_new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.active_cwd(cx);
        self.open_tab(PaneKind::Agent, cwd, None, None, window, cx);
    }

    fn on_new_terminal_tab(
        &mut self,
        _: &NewTerminalTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self.active_cwd(cx);
        self.open_tab(PaneKind::Terminal, cwd, None, None, window, cx);
    }

    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.space().active;
        self.close_tab(active, window, cx);
    }

    fn on_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        let space = self.space();
        let next = (space.active + 1) % space.tabs.len().max(1);
        self.activate(next, window, cx);
    }

    fn on_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let space = self.space();
        let len = space.tabs.len().max(1);
        let prev = (space.active + len - 1) % len;
        self.activate(prev, window, cx);
    }

    fn on_split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split(SplitAxis::Horizontal, None, window, cx);
    }

    fn on_split_down(&mut self, _: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split(SplitAxis::Vertical, None, window, cx);
    }

    fn on_new_agent_pane(&mut self, _: &NewAgentPane, window: &mut Window, cx: &mut Context<Self>) {
        self.split_with_kind(PaneKind::Agent, window, cx);
    }

    fn on_new_terminal_pane(
        &mut self,
        _: &NewTerminalPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_with_kind(PaneKind::Terminal, window, cx);
    }

    fn on_close_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.focused_pane() {
            self.close_pane(pane, window, cx);
        }
    }

    fn on_focus_next_pane(
        &mut self,
        _: &FocusNextPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(next) = self.tab().and_then(|t| t.tree.cycle(&t.focused, true)) {
            self.focus_pane(next, window, cx);
        }
    }

    fn on_focus_prev_pane(
        &mut self,
        _: &FocusPrevPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(prev) = self.tab().and_then(|t| t.tree.cycle(&t.focused, false)) {
            self.focus_pane(prev, window, cx);
        }
    }

    fn on_toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_sidebar(cx);
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.emit(WorkspaceEvent::SidebarToggled(self.sidebar_visible));
        self.schedule_save(cx);
        cx.notify();
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    fn on_open_providers(
        &mut self,
        _: &OpenProviders,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_providers(window, cx);
    }

    fn on_close_overlay(&mut self, _: &CloseOverlay, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        } else {
            cx.propagate();
        }
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn on_open_sessions(&mut self, _: &OpenSessions, window: &mut Window, cx: &mut Context<Self>) {
        self.open_sessions(window, cx);
    }

    fn on_open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(Category::All, window, cx);
    }

    fn on_open_workspaces(
        &mut self,
        _: &OpenWorkspaces,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_palette(Category::Workspaces, window, cx);
    }

    // ── Screens ─────────────────────────────────────────────────────────────

    /// Open the switcher on `category`; the same shortcut again closes it, the
    /// other one changes category.
    fn open_palette(&mut self, category: Category, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((Overlay::Palette(view), _)) = &self.overlay {
            if view.read(cx).category() == category {
                self.close_overlay(window, cx);
            } else {
                view.update(cx, |view, cx| view.set_category(category, window, cx));
            }
            return;
        }
        self.start_active_session(window, cx);
        let commands = self
            .active_session()
            .map(|t| t.read(cx).commands())
            .unwrap_or_default();
        let workspaces = self
            .spaces
            .iter()
            .enumerate()
            .map(|(index, space)| WorkspaceRow {
                index,
                name: space.name.clone(),
                tabs: space.tab_count(),
                current: index == self.current,
            })
            .collect();
        let input = PaletteInput {
            commands,
            workspaces,
            root: self.space().root.clone(),
            category,
        };
        let view = cx.new(|cx| PaletteView::new(input, window, cx));
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &PaletteEvent, window, cx| match event {
                PaletteEvent::Close => this.close_overlay(window, cx),
                PaletteEvent::Run(PaletteAction::RenameWorkspace { index, name }) => {
                    // Renaming happens in place; the switcher stays open.
                    this.rename_workspace(*index, name.clone(), cx);
                }
                PaletteEvent::Run(action) => {
                    this.close_overlay(window, cx);
                    this.run_palette_action(action.clone(), window, cx);
                }
            },
        );
        self.overlay = Some((Overlay::Palette(view), subscription));
        cx.notify();
    }

    fn run_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.active_session();
        match action {
            PaletteAction::Providers => self.open_providers(window, cx),
            PaletteAction::Sessions => self.open_sessions(window, cx),
            PaletteAction::Profiles => self.open_profiles(window, cx),
            PaletteAction::Usage => self.open_usage(window, cx),
            PaletteAction::Memory => self.open_memory(window, cx),
            PaletteAction::Settings => self.open_settings(window, cx),
            PaletteAction::NewTab => self.on_new_tab(&NewTab, window, cx),
            PaletteAction::NewTerminalTab => self.on_new_terminal_tab(&NewTerminalTab, window, cx),
            PaletteAction::SplitRight => self.split(SplitAxis::Horizontal, None, window, cx),
            PaletteAction::SplitDown => self.split(SplitAxis::Vertical, None, window, cx),
            PaletteAction::NewAgentPane => self.split_with_kind(PaneKind::Agent, window, cx),
            PaletteAction::NewTerminalPane => self.split_with_kind(PaneKind::Terminal, window, cx),
            PaletteAction::ClosePane => self.on_close_pane(&ClosePane, window, cx),
            PaletteAction::ToggleSidebar => self.toggle_sidebar(cx),
            PaletteAction::Agents => {
                if let Some(tab) = tab {
                    tab.update(cx, |s, cx| s.open_agents_panel(cx));
                }
            }
            PaletteAction::NewSession => {
                if let Some(tab) = tab {
                    tab.update(cx, |s, cx| s.new_session(window, cx));
                }
            }
            PaletteAction::Command(name) => {
                if let Some(tab) = tab {
                    tab.update(cx, |s, cx| s.run_prompt(format!("/{name}"), cx));
                }
            }
            PaletteAction::Screen(screen) => self.open_screen(screen, tab, window, cx),
            PaletteAction::SwitchWorkspace(index) => self.switch_workspace(index, window, cx),
            PaletteAction::RenameWorkspace { index, name } => {
                self.rename_workspace(index, name, cx)
            }
            PaletteAction::DeleteWorkspace(index) => {
                self.confirm_delete_workspace(index, window, cx)
            }
            PaletteAction::CreateWorkspace {
                name,
                pick_folder: false,
            } => {
                let root = self.active_cwd(cx);
                self.create_workspace(Some(name), root, window, cx);
            }
            PaletteAction::CreateWorkspace {
                name,
                pick_folder: true,
            } => self.pick_workspace_folder(name, window, cx),
            PaletteAction::OpenFile(path) => self.open_file(path, window, cx),
            PaletteAction::SwitchBranch(branch) => self.confirm_switch_branch(branch, window, cx),
        }
    }

    /// Play the max-effort surge over the window; choosing max again
    /// restarts it.
    fn play_surge(&mut self, cx: &mut Context<Self>) {
        self.surge_runs += 1;
        let run = self.surge_runs;
        self.surge = Some((run, std::time::Instant::now()));
        self._surge_end = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(crate::surge::DURATION).await;
            let _ = this.update(cx, |this, cx| {
                if this.surge.is_some_and(|(playing, _)| playing == run) {
                    this.surge = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// The native screen of a terminal-only Gentle Shell command; `session`
    /// is the agent it concerns (the one that ran it, else the active one).
    fn open_screen(
        &mut self,
        screen: Screen,
        session: Option<Entity<SessionView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match screen {
            Screen::Usage => self.open_usage(window, cx),
            Screen::Stats => {
                self.open_usage(window, cx);
                if let Some((Overlay::Usage(view), _)) = &self.overlay {
                    view.update(cx, |view, cx| view.show_stats(cx));
                }
            }
            // Background jobs have no window of their own: the Agents window
            // is where the session's background work lives.
            Screen::Agents | Screen::Jobs => {
                if let Some(session) = session.or_else(|| self.active_session()) {
                    session.update(cx, |s, cx| s.open_agents_panel(cx));
                }
            }
            Screen::Profiles => self.open_profiles(window, cx),
            Screen::Palette => {
                self.close_overlay(window, cx);
                self.open_palette(Category::Commands, window, cx);
            }
            Screen::Changes => {
                let Some(tree) = self
                    .sidebar
                    .clone()
                    .and_then(|view| view.downcast::<FileTreeView>().ok())
                else {
                    return;
                };
                if !self.sidebar_visible {
                    self.toggle_sidebar(cx);
                }
                tree.update(cx, |tree, cx| tree.show_changes(window, cx));
            }
        }
    }

    fn open_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_active_session(window, cx);
        let cwd = self.active_cwd(cx);
        let current = self
            .active_session()
            .and_then(|t| t.read(cx).session_file());
        let view = cx.new(|cx| SessionsView::new(cwd, current, window, cx));
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &SessionsEvent, window, cx| match event {
                SessionsEvent::Close => this.close_overlay(window, cx),
                SessionsEvent::NewSession => {
                    this.close_overlay(window, cx);
                    if let Some(tab) = this.active_session() {
                        tab.update(cx, |s, cx| s.new_session(window, cx));
                    }
                }
                SessionsEvent::Open { path, cwd } => {
                    this.close_overlay(window, cx);
                    let path = path.clone();
                    if *cwd == this.active_cwd(cx) {
                        if let Some(tab) = this.active_session() {
                            tab.update(cx, |s, cx| s.open_session(path, cx));
                        }
                    } else {
                        this.open_tab(PaneKind::Agent, cwd.clone(), None, Some(path), window, cx);
                    }
                }
            },
        );
        self.overlay = Some((Overlay::Sessions(view), subscription));
        cx.notify();
    }

    fn open_profiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_active_session(window, cx);
        let cwd = self.active_cwd(cx);
        let models = self
            .active_session()
            .map(|t| t.read(cx).model_choices())
            .unwrap_or_default()
            .into_iter()
            .map(|(provider, id, name, reasoning)| ModelChoice {
                provider,
                id,
                name,
                reasoning,
            })
            .collect();
        let view = cx.new(|cx| ProfilesView::new(cwd, models, window, cx));
        let subscription = cx.subscribe_in(&view, window, |this, _, event: &ProfilesEvent, window, cx| {
            let tab = this.active_session();
            match event {
                ProfilesEvent::Close => this.close_overlay(window, cx),
                ProfilesEvent::Changed => {}
                ProfilesEvent::RunCommand(text) => {
                    if let Some(tab) = tab {
                        let text = text.clone();
                        tab.update(cx, |s, cx| s.run_prompt(text, cx));
                    }
                }
                ProfilesEvent::SwitchOrchestrator { model, thinking } => {
                    if let Some(tab) = tab {
                        let ((provider, id), thinking) = (model.clone(), thinking.clone());
                        tab.update(cx, |s, cx| {
                            s.send_command(serde_json::json!({"type": "set_model", "provider": provider, "modelId": id}), cx);
                            if let Some(level) = thinking {
                                s.send_command(serde_json::json!({"type": "set_thinking_level", "level": level}), cx);
                            }
                        });
                    }
                }
                ProfilesEvent::UseInSession { profile, model, thinking } => {
                    if let Some(tab) = tab {
                        let (model, thinking) = (model.clone(), thinking.clone());
                        let note = format!("Profile \u{201c}{profile}\u{201d} applied to this session's orchestrator; helpers keep the routing this session started with.");
                        tab.update(cx, |s, cx| {
                            s.note(note, cx);
                            if let Some((provider, id)) = model {
                                s.send_command(serde_json::json!({"type": "set_model", "provider": provider, "modelId": id}), cx);
                            }
                            if let Some(level) = thinking {
                                s.send_command(serde_json::json!({"type": "set_thinking_level", "level": level}), cx);
                            }
                        });
                    }
                }
            }
        });
        self.overlay = Some((Overlay::Profiles(view), subscription));
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_active_session(window, cx);
        let cwd = self.active_cwd(cx);
        let view = cx.new(|cx| SettingsView::new(cwd, window, cx));
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, view, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::Close => this.close_overlay(window, cx),
                SettingsEvent::Changed => {
                    for tab in this.sessions() {
                        tab.update(cx, |session, cx| session.reload_models(window, cx));
                    }
                }
                SettingsEvent::BackendUpdated => {
                    for tab in this.sessions() {
                        tab.update(cx, |session, cx| session.restart_for_update(window, cx));
                    }
                }
                SettingsEvent::ShellCommand(text) => {
                    let view = view.downgrade();
                    let report = move |result: Result<(), String>, cx: &mut App| {
                        let _ = view.update(cx, |view, cx| view.shell_done(result, cx));
                    };
                    match this.active_session() {
                        Some(session) => session.update(cx, |session, cx| {
                            session.run_extension_command(text.clone(), cx, report)
                        }),
                        None => report(
                            Err(
                                "Open an agent tab first: Gentle Shell applies this setting."
                                    .into(),
                            ),
                            cx,
                        ),
                    }
                }
            },
        );
        self.overlay = Some((Overlay::Settings(view), subscription));
        cx.notify();
    }

    fn open_memory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.active_cwd(cx);
        let view = cx.new(|cx| MemoryView::new(cwd, window, cx));
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &MemoryEvent, window, cx| match event {
                MemoryEvent::Close => this.close_overlay(window, cx),
            },
        );
        self.overlay = Some((Overlay::Memory(view), subscription));
        cx.notify();
    }

    fn open_usage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| UsageView::new(window, cx));
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &UsageEvent, window, cx| match event {
                UsageEvent::Close => this.close_overlay(window, cx),
            },
        );
        self.overlay = Some((Overlay::Usage(view), subscription));
        cx.notify();
    }

    fn open_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.active_cwd(cx);
        let view = cx.new(|cx| ProvidersView::new(cwd, window, cx));
        let subscription = cx.subscribe_in(
            &view,
            window,
            |this, _, event: &ProvidersEvent, window, cx| match event {
                ProvidersEvent::Close => this.close_overlay(window, cx),
                ProvidersEvent::Changed => {
                    for tab in this.sessions() {
                        tab.update(cx, |session, cx| session.reload_models(window, cx));
                    }
                }
            },
        );
        self.overlay = Some((Overlay::Providers(view), subscription));
        cx.notify();
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    // The chrome reads each pane's `title`/`running` snapshot, never the
    // views themselves: see `Pane::title`.
    fn tab_running(&self, tab: &Tab) -> bool {
        tab.tree
            .leaves()
            .iter()
            .any(|id| self.panes.get(id).is_some_and(|p| p.running))
    }

    fn pane_title(&self, id: PaneId) -> SharedString {
        self.panes
            .get(&id)
            .map(|p| p.title.clone())
            .unwrap_or_default()
    }

    fn render_workspace_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.space().name.clone();
        let color = avatar_color(&name);
        let initial = name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_else(|| "·".into());
        h_flex()
            .id("workspace-chip")
            .h(px(TITLE_ITEM))
            .pl(px(5.))
            .pr(px(6.))
            .gap(px(7.))
            .flex_none()
            // Same shape and hover as a tab, but no resting fill: it labels the
            // workspace (as in Tern) instead of competing with the active tab.
            .rounded(px(8.))
            .text_size(px(12.5))
            .cursor_pointer()
            .hover(|s| s.bg(ui::surface_hover().opacity(0.6)))
            .tooltip(|window, cx| Tooltip::new("Switch workspace").build(window, cx))
            // Keep clicks on the chip from turning into window drags.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_palette(Category::Workspaces, window, cx)
            }))
            .child(
                div()
                    .size(px(18.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .bg(color.opacity(0.18))
                    .text_color(color)
                    .text_size(px(10.5))
                    .font_weight(FontWeight::BOLD)
                    .line_height(relative(1.))
                    .child(initial),
            )
            .child(
                div()
                    .max_w(px(160.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui::fg())
                    .child(name),
            )
            .child(
                Icon::new(IconName::ChevronDown)
                    .xsmall()
                    .text_color(ui::faint()),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let space = self.space();
        let weak = cx.entity().downgrade();
        h_flex()
            .gap(px(2.))
            .min_w_0()
            .children(space.tabs.iter().enumerate().map(|(i, tab)| {
                let active = i == space.active;
                let running = self.tab_running(tab);
                let kind = self
                    .panes
                    .get(&tab.focused)
                    .map(|p| p.item.kind)
                    .unwrap_or(PaneKind::Agent);
                let count = tab.tree.len();
                let icon_color = if active { ui::petal() } else { ui::faint() };
                let item = h_flex()
                    .id(("tab", tab.id))
                    .group("tab")
                    .h(px(TITLE_ITEM))
                    .pl(px(10.))
                    .pr(px(4.))
                    .gap(px(7.))
                    .rounded(px(8.))
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .relative()
                    // The fill alone marks the active tab, like Tern; the petal
                    // icon carries the accent.
                    .when(active, |this| {
                        this.bg(ui::surface_hover().opacity(0.7))
                            .text_color(ui::fg())
                    })
                    .when(!active, |this| {
                        this.text_color(ui::muted())
                            .hover(|s| s.bg(ui::surface_hover().opacity(0.6)).text_color(ui::fg()))
                    })
                    // Keep clicks on tabs from turning into window drags.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    // A double-click on an agent tab renames its session.
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.start_tab_rename(i, window, cx);
                        } else {
                            this.activate(i, window, cx);
                        }
                    }))
                    .child(
                        div()
                            .size(px(14.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if running {
                                ui::pulse(
                                    ("tab-live", tab.id),
                                    div().size(px(7.)).rounded_full().bg(ui::accent()),
                                )
                                .into_any_element()
                            } else if kind == PaneKind::Agent {
                                div()
                                    .text_size(px(13.))
                                    .line_height(relative(1.))
                                    .text_color(icon_color)
                                    .child("❀")
                                    .into_any_element()
                            } else {
                                Icon::new(kind.icon())
                                    .small()
                                    .text_color(icon_color)
                                    .into_any_element()
                            }),
                    )
                    .child(
                        match self.renaming.as_ref().filter(|r| r.pane == tab.focused) {
                            Some(rename) => div()
                                .w(px(160.))
                                .h(px(20.))
                                .px_1()
                                .flex()
                                .items_center()
                                .rounded(px(5.))
                                .bg(ui::well())
                                // Clicks in the field edit it rather than the tab.
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_action(cx.listener(Self::cancel_tab_rename))
                                .child(Input::new(&rename.input).appearance(false).xsmall())
                                .into_any_element(),
                            None => div()
                                .max_w(px(180.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .when(active, |d| d.font_weight(FontWeight::MEDIUM))
                                .child(self.pane_title(tab.focused))
                                .into_any_element(),
                        },
                    )
                    .when(count > 1, |d| {
                        d.child(
                            div()
                                .h(px(16.))
                                .min_w(px(16.))
                                .px(px(4.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .bg(ui::surface_hover().opacity(0.7))
                                .text_size(px(10.))
                                .line_height(relative(1.))
                                .text_color(ui::muted())
                                .child(count.to_string()),
                        )
                    })
                    .child(
                        div()
                            .id(("close-tab", tab.id))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .text_color(ui::faint())
                            .invisible()
                            .group_hover("tab", |s| s.visible())
                            .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, window, cx);
                            }))
                            .child(Icon::new(IconName::X).xsmall()),
                    );
                ui::appear(("tab-in", tab.id), div().child(item))
            }))
            .child(
                div()
                    .ml(px(2.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(title_button("new-tab", IconName::Plus).dropdown_menu(
                        move |menu, _, _| {
                            let item = |icon: IconName,
                                        label: &'static str,
                                        detail: &'static str,
                                        keys: &'static str| {
                                PopupMenuItem::element(move |_, _| {
                                    menu_row(icon, label, detail, keys)
                                })
                            };
                            let (agent, terminal) = (weak.clone(), weak.clone());
                            menu.min_w(px(260.))
                                .item(
                                    item(
                                        IconName::Bot,
                                        "Agent tab",
                                        "Gentle Shell in this folder",
                                        if cfg!(target_os = "macos") {
                                            "⌘T"
                                        } else {
                                            "Ctrl+T"
                                        },
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = agent.update(cx, |this, cx| {
                                                this.on_new_tab(&NewTab, window, cx)
                                            });
                                        },
                                    ),
                                )
                                .item(
                                    item(
                                        IconName::SquareTerminal,
                                        "Terminal tab",
                                        "A shell in this folder",
                                        if cfg!(target_os = "macos") {
                                            "⌘⇧T"
                                        } else {
                                            "Ctrl+Shift+T"
                                        },
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            let _ = terminal.update(cx, |this, cx| {
                                                this.on_new_terminal_tab(
                                                    &NewTerminalTab,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                )
                        },
                    )),
            )
    }

    fn render_tree(
        &self,
        tab: &Tab,
        node: &PaneTree<PaneId>,
        path: &mut Vec<bool>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            PaneTree::Leaf(id) => self.render_pane(tab, *id, window, cx),
            PaneTree::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let axis = *axis;
                let horizontal = axis == SplitAxis::Horizontal;
                path.push(false);
                let first = self.render_tree(tab, first, path, window, cx);
                path.pop();
                path.push(true);
                let second = self.render_tree(tab, second, path, window, cx);
                path.pop();
                let key = path
                    .iter()
                    .map(|b| if *b { '1' } else { '0' })
                    .collect::<String>();
                let drag = DividerDrag {
                    tab: tab.id,
                    path: path.clone(),
                };
                let (tab_id, here) = (tab.id, path.clone());
                let cell = |child: AnyElement, share: f32| {
                    div()
                        .flex()
                        // Zero basis, grow by share: the space left after the
                        // divider splits exactly at the ratio.
                        .flex_basis(px(0.))
                        .flex_grow(share)
                        .flex_shrink(1.)
                        .min_w_0()
                        .min_h_0()
                        .when(horizontal, |d| d.h_full())
                        .when(!horizontal, |d| d.w_full())
                        .child(child)
                };
                div()
                    .flex()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .when(horizontal, |d| d.flex_row())
                    .when(!horizontal, |d| d.flex_col())
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                            let drag = event.drag(cx);
                            if drag.tab != tab_id || drag.path != here {
                                return;
                            }
                            let bounds = event.bounds;
                            let position = event.event.position;
                            let share = if horizontal {
                                (position.x - bounds.left()) / bounds.size.width
                            } else {
                                (position.y - bounds.top()) / bounds.size.height
                            };
                            // Keep both sides at least the minimum pane size
                            // (when the split has room for that at all).
                            let (total, min) = if horizontal {
                                (f32::from(bounds.size.width), MIN_PANE_WIDTH)
                            } else {
                                (f32::from(bounds.size.height), MIN_PANE_HEIGHT)
                            };
                            let bound = min / total.max(1.);
                            let share = if bound < 0.5 {
                                share.clamp(bound, 1. - bound)
                            } else {
                                share
                            };
                            let path = here.clone();
                            if let Some(tab) = this.tab_mut()
                                && tab.id == tab_id
                                && tab.tree.set_ratio(&path, share)
                            {
                                this.schedule_save(cx);
                                cx.notify();
                            }
                        },
                    ))
                    .child(cell(first, *ratio))
                    .child(
                        div()
                            .id(SharedString::from(format!("divider-{}-{key}", tab.id)))
                            .group("divider")
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(horizontal, |d| d.w(px(6.)).h_full().cursor_col_resize())
                            .when(!horizontal, |d| d.h(px(6.)).w_full().cursor_row_resize())
                            .on_drag(drag, |_, _, _, cx| cx.new(|_| Invisible))
                            .child(
                                div()
                                    .rounded_full()
                                    .bg(ui::border_strong())
                                    .opacity(0.0)
                                    .group_hover("divider", |s| s.opacity(1.0).bg(ui::petal()))
                                    .when(horizontal, |d| d.w(px(2.)).h(px(40.)))
                                    .when(!horizontal, |d| d.h(px(2.)).w(px(40.))),
                            ),
                    )
                    .child(cell(second, 1.0 - *ratio))
                    .into_any_element()
            }
        }
    }

    fn render_pane(
        &self,
        tab: &Tab,
        id: PaneId,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(pane) = self.panes.get(&id) else {
            return div().into_any_element();
        };
        let tiled = tab.tree.len() > 1;
        let focused = tiled && tab.focused == id;
        let title = pane.title.clone();
        let body = div()
            .id(("pane", id))
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .rounded_xl()
            .bg(ui::panel())
            // Panes read as glass on their own; only a split's focused pane gets
            // an edge. The width stays so focus changes don't shift content.
            .border_1()
            .border_color(if focused {
                ui::petal().opacity(0.45)
            } else {
                gpui_kit::transparent_black()
            })
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                if let Some(tab) = this.tab_mut()
                    && tab.focused != id
                    && tab.tree.contains(&id)
                {
                    tab.focused = id;
                    cx.notify();
                }
            }))
            .drag_over::<PaneDrag>(|style, _, _, _| style.border_color(ui::accent()))
            .on_drop(
                cx.listener(move |this, drag: &PaneDrag, _, cx| this.swap_panes(drag.pane, id, cx)),
            )
            // The pink border alone marks focus: a straight accent line on top
            // can't follow the rounded corners now that panes have no header.
            // Cached: a notify elsewhere (another pane, the chrome) reuses this
            // pane's last frame instead of rebuilding it.
            .child(
                div().flex_1().min_h_0().child(
                    pane.item
                        .view
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                ),
            );
        // The grip: a pill straddling the pane's top edge (outside the
        // clipped body); drag onto another pane to swap the two.
        let grip = h_flex()
            .absolute()
            // Inside the ~4 px gap under the title bar, not over the tabs.
            .top(px(-4.))
            .left_0()
            .right_0()
            .justify_center()
            .child(
                div()
                    .id(("pane-grip", id))
                    .w(px(34.))
                    .h(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(ui::surface())
                    .border_1()
                    .border_color(if focused {
                        ui::petal().opacity(0.45)
                    } else {
                        ui::border()
                    })
                    .text_color(ui::faint())
                    .cursor_grab()
                    .invisible()
                    .when(focused, |d| d.visible())
                    .group_hover(SharedString::from(format!("pane-wrap-{id}")), |s| {
                        s.visible()
                    })
                    .hover(|s| s.text_color(ui::fg()).border_color(ui::border_strong()))
                    .on_drag(
                        PaneDrag {
                            pane: id,
                            title: title.clone(),
                        },
                        |drag, _, _, cx| cx.new(|_| DragChip(drag.title.clone())),
                    )
                    .child(Icon::new(IconName::GripHorizontal).xsmall()),
            );
        ui::appear(
            ("pane-in", id),
            div()
                .group(SharedString::from(format!("pane-wrap-{id}")))
                .relative()
                .flex()
                .size_full()
                .child(body)
                .when(tiled, |d| d.child(grip)),
        )
        .into_any_element()
    }

    fn render_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        match self.tab() {
            Some(tab) => {
                let mut path = Vec::new();
                self.render_tree(tab, &tab.tree, &mut path, window, cx)
            }
            None => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .text_sm()
                .text_color(ui::muted())
                .child("No tabs in this workspace")
                .child(
                    Button::new("empty-new-tab")
                        .primary()
                        .small()
                        .icon(Icon::new(IconName::Plus))
                        .label("New agent tab")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_new_tab(&NewTab, window, cx)),
                        ),
                )
                .into_any_element(),
        }
    }
}

/// Height of the title bar's pills, tabs and buttons.
const TITLE_ITEM: f32 = 28.;

/// A quiet square icon button for the title bar.
fn title_button(id: &'static str, icon: IconName) -> Button {
    Button::new(id).ghost().xsmall().icon(Icon::new(icon))
}

/// A stable hue per workspace name, for its letter badge.
fn avatar_color(name: &str) -> Hsla {
    let hash = name
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    hsla((hash % 360) as f32 / 360., 0.55, 0.65, 1.)
}

/// A two-line menu entry: icon, label, muted description, shortcut.
fn menu_row(
    icon: IconName,
    label: &'static str,
    detail: &'static str,
    keys: &'static str,
) -> impl IntoElement {
    h_flex()
        .gap_2p5()
        .py_0p5()
        .w_full()
        .child(
            div()
                .size(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_1()
                .border_color(ui::border_strong())
                .bg(ui::well())
                .text_color(ui::petal())
                .child(Icon::new(icon).small()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().text_color(ui::fg()).child(label))
                .child(div().text_xs().text_color(ui::muted()).child(detail)),
        )
        .child(ui::kbd(keys))
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Follow keyboard focus into panes (a terminal focused by its own click).
        if let Some(tab) = self.tab()
            && let Some(inside) = tab.tree.leaves().into_iter().find(|id| {
                self.panes
                    .get(id)
                    .is_some_and(|p| p.item.focus.contains_focused(window, cx))
            })
            && inside != tab.focused
            && let Some(tab) = self.tab_mut()
        {
            tab.focused = inside;
        }
        let sidebar = self.sidebar.clone().filter(|_| self.sidebar_visible);
        let has_sidebar = self.sidebar.is_some();
        // Below this the screen buttons fold into one overflow menu.
        let narrow = window.viewport_size().width < px(900.);
        // While the surge plays, the window shakes before the rose hatches:
        // the root moves in place and asks for the next frame, so the element
        // tree (and every element's state) stays the same.
        let (shake_x, shake_y) = match self.surge {
            Some((_, started)) => {
                window.request_animation_frame();
                crate::surge::shake_at(started.elapsed())
            }
            None => (0., 0.),
        };
        v_flex()
            .size_full()
            .text_color(ui::fg())
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_new_terminal_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_split_right))
            .on_action(cx.listener(Self::on_split_down))
            .on_action(cx.listener(Self::on_new_agent_pane))
            .on_action(cx.listener(Self::on_new_terminal_pane))
            .on_action(cx.listener(Self::on_close_pane))
            .on_action(cx.listener(Self::on_focus_next_pane))
            .on_action(cx.listener(Self::on_focus_prev_pane))
            .on_action(cx.listener(Self::on_toggle_sidebar))
            .on_action(cx.listener(Self::on_open_providers))
            .on_action(cx.listener(Self::on_open_sessions))
            .on_action(cx.listener(Self::on_open_palette))
            .on_action(cx.listener(Self::on_open_workspaces))
            .on_action(cx.listener(Self::on_close_overlay))
            .key_context("GentleWorkspace")
            .relative()
            .left(px(shake_x))
            .top(px(shake_y))
            .child(
                TitleBar::new()
                    .h(px(36.))
                    .bg(gpui_kit::transparent_black())
                    .child(
                        h_flex()
                            .w_full()
                            .pr_2()
                            .gap_3()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(self.render_workspace_chip(cx))
                                    .child(div().flex_none().w(px(1.)).h(px(14.)).bg(ui::border()))
                                    .child(self.render_tabs(cx)),
                            )
                            .child(
                                h_flex()
                                    .flex_none()
                                    .gap_1()
                                    // Tern keeps pane layout in the title bar, so
                                    // panes carry no header; these act on the
                                    // focused pane.
                                    .when(self.tab().is_some(), |d| {
                                        let mac = cfg!(target_os = "macos");
                                        d.child(
                                            Button::new("title-split-right")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::Columns2))
                                                .tooltip(format!(
                                                    "Split right ({})",
                                                    if mac { "⌘D" } else { "Ctrl+Shift+D" }
                                                ))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.on_split_right(&SplitRight, window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("title-split-down")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::Rows2))
                                                .tooltip(format!(
                                                    "Split down ({})",
                                                    if mac { "⌘⇧D" } else { "Ctrl+Shift+E" }
                                                ))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.on_split_down(&SplitDown, window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("title-close-pane")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::X))
                                                .tooltip(format!(
                                                    "Close pane ({})",
                                                    if mac { "⌘W" } else { "Ctrl+W" }
                                                ))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.on_close_pane(&ClosePane, window, cx)
                                                })),
                                        )
                                        .child(div().w(px(1.)).h(px(14.)).mx_1().bg(ui::border()))
                                    })
                                    .when_some(crate::app_update::badge(cx), |d, (label, tip)| {
                                        d.child(
                                            div()
                                                .id("app-update-pill")
                                                .cursor_pointer()
                                                .hover(|s| s.opacity(0.8))
                                                .tooltip(move |window, cx| {
                                                    Tooltip::new(tip.clone()).build(window, cx)
                                                })
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_settings(window, cx)
                                                }))
                                                .child(ui::chip(label, ui::accent())),
                                        )
                                    })
                                    .child({
                                        let weak = cx.entity().downgrade();
                                        div()
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .child(notices::bell(
                                                &self.notices,
                                                Rc::new(move |pane, window, cx| {
                                                    let _ = weak.update(cx, |this, cx| {
                                                        this.reveal_pane(pane, window, cx)
                                                    });
                                                }),
                                                cx,
                                            ))
                                    })
                                    .child(
                                        Button::new("open-palette")
                                            .ghost()
                                            .xsmall()
                                            .icon(Icon::new(IconName::Command))
                                            .tooltip("Commands (⌘K)")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.on_open_palette(&OpenPalette, window, cx)
                                            })),
                                    )
                                    .when(narrow, |d| {
                                        let weak = cx.entity().downgrade();
                                        d.child(
                                            div()
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .child(
                                                    Button::new("title-overflow")
                                                        .ghost()
                                                        .xsmall()
                                                        .icon(Icon::new(IconName::Ellipsis))
                                                        .dropdown_menu(move |menu, _, _| {
                                                            type Open = fn(
                                                                &mut Workspace,
                                                                &mut Window,
                                                                &mut Context<Workspace>,
                                                            );
                                                            let entry = |icon: IconName,
                                                                         label: &'static str,
                                                                         detail: &'static str,
                                                                         open: Open| {
                                                                let weak = weak.clone();
                                                                PopupMenuItem::element(
                                                                    move |_, _| {
                                                                        menu_row(
                                                                            icon, label, detail,
                                                                            "",
                                                                        )
                                                                    },
                                                                )
                                                                .on_click(move |_, window, cx| {
                                                                    let _ = weak.update(
                                                                        cx,
                                                                        |this, cx| {
                                                                            open(this, window, cx)
                                                                        },
                                                                    );
                                                                })
                                                            };
                                                            menu.min_w(px(260.))
                                                                .item(entry(
                                                                    IconName::Settings,
                                                                    "Configuration",
                                                                    "Settings for this folder",
                                                                    |this, window, cx| {
                                                                        this.open_settings(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                ))
                                                                .item(entry(
                                                                    IconName::Layers,
                                                                    "Profiles",
                                                                    "Model and effort per agent",
                                                                    |this, window, cx| {
                                                                        this.open_profiles(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                ))
                                                                .item(entry(
                                                                    IconName::Brain,
                                                                    "Memory",
                                                                    "Engram observations",
                                                                    |this, window, cx| {
                                                                        this.open_memory(window, cx)
                                                                    },
                                                                ))
                                                                .item(entry(
                                                                    IconName::RotateCcwClock,
                                                                    "Sessions",
                                                                    "Resume a past session",
                                                                    |this, window, cx| {
                                                                        this.open_sessions(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                ))
                                                                .item(entry(
                                                                    IconName::KeyRound,
                                                                    "Providers",
                                                                    "Sign-in and API keys",
                                                                    |this, window, cx| {
                                                                        this.open_providers(
                                                                            window, cx,
                                                                        )
                                                                    },
                                                                ))
                                                        }),
                                                ),
                                        )
                                    })
                                    .when(!narrow, |d| {
                                        d.child(
                                            Button::new("open-settings")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::Settings))
                                                .tooltip("Configuration")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_settings(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("open-profiles")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::Layers))
                                                .tooltip("Profiles: model and effort per agent")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_profiles(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("open-memory")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::Brain))
                                                .tooltip("Memory (Engram)")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_memory(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("open-sessions")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::RotateCcwClock))
                                                .tooltip("Sessions")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_sessions(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("open-providers")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(IconName::KeyRound))
                                                .tooltip("Providers and sign-in")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.open_providers(window, cx)
                                                })),
                                        )
                                    })
                                    .when(has_sidebar, |d| {
                                        d.child(
                                            Button::new("toggle-sidebar")
                                                .ghost()
                                                .xsmall()
                                                .icon(Icon::new(if self.sidebar_visible {
                                                    IconName::PanelLeftClose
                                                } else {
                                                    IconName::PanelLeftOpen
                                                }))
                                                .tooltip("Toggle sidebar (⌘B)")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.toggle_sidebar(cx)
                                                })),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .px_1p5()
                    .pb_1p5()
                    .gap_1p5()
                    .children(sidebar.map(|view| {
                        ui::appear(
                            "sidebar-in",
                            div()
                                .w(px(SIDEBAR_WIDTH))
                                .flex_none()
                                .h_full()
                                .rounded_xl()
                                .bg(ui::panel())
                                .border_1()
                                .border_color(gpui_kit::transparent_black())
                                .overflow_hidden()
                                .child(view.cached(StyleRefinement::default().size_full())),
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_body(window, cx)),
                    ),
            )
            .children(self.overlay.as_ref().map(|(overlay, _)| match overlay {
                Overlay::Providers(view) => view.clone().into_any_element(),
                Overlay::Sessions(view) => view.clone().into_any_element(),
                Overlay::Palette(view) => view.clone().into_any_element(),
                Overlay::Usage(view) => view.clone().into_any_element(),
                Overlay::Memory(view) => view.clone().into_any_element(),
                Overlay::Settings(view) => view.clone().into_any_element(),
                Overlay::Profiles(view) => view.clone().into_any_element(),
            }))
            .when_some(self.surge, |root, (run, _)| {
                root.child(crate::surge::overlay(run))
            })
    }
}
