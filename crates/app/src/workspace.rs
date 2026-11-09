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

use crate::files::FileTreeView;
use crate::memory::{MemoryEvent, MemoryView};
use crate::notices::{self, Notices};
use crate::onboarding::{OnboardingEvent, OnboardingView, Probe};
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
use gentle_core::commands::Screen;

mod actions;
mod layout;
mod overlays;
mod panes;
mod persistence;
mod render;
mod spaces;
mod tabs;
mod view;

use layout::*;
use view::*;

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
    Onboarding(Entity<OnboardingView>),
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
    /// The in-app browser showing Gentle Design's canvas, if open.
    browser: Option<(Entity<crate::browser::BrowserPanel>, Subscription)>,
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
            browser: None,
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
            None if this.spaces.is_empty() => this.push_space(None, crate::cli::default_cwd()),
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
        // First run, or something required missing: onboarding opens by
        // itself once the checks (blocking; PATH probes) come back.
        cx.spawn_in(window, async move |this, cx| {
            let probe = cx.background_executor().spawn(async { Probe::run() }).await;
            if !probe.needs_onboarding() {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if this.overlay.is_none() {
                    this.open_onboarding(Some(probe), window, cx);
                }
            });
        })
        .detach();
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
}
