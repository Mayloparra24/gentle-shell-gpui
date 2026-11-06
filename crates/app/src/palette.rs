//! The switcher (⌘K), after Tern's: one search field over segmented
//! categories — the app's screens and pane commands plus every Gentle Shell
//! slash command the backend reports, the window's workspaces (switch,
//! rename, delete, create), files under the workspace root and git branches.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    Providers,
    Profiles,
    Sessions,
    Usage,
    Memory,
    Settings,
    Agents,
    NewSession,
    /// A new tab with an agent pane.
    NewTab,
    NewTerminalTab,
    SplitRight,
    SplitDown,
    /// Split the focused pane with an agent / terminal beside it.
    NewAgentPane,
    NewTerminalPane,
    ClosePane,
    ToggleRail,
    ToggleSidebar,
    /// Send `/name` as a prompt in the active agent pane.
    Command(String),
    SwitchWorkspace(usize),
    RenameWorkspace {
        index: usize,
        name: String,
    },
    DeleteWorkspace(usize),
    /// A new workspace named `name`, rooted at the current folder or, with
    /// `pick_folder`, at one the user chooses (named after it when `name` is empty).
    CreateWorkspace {
        name: String,
        pick_folder: bool,
    },
    OpenFile(PathBuf),
    /// Asks for confirmation before running `git switch`.
    SwitchBranch(String),
}

pub enum PaletteEvent {
    Run(PaletteAction),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    All,
    Commands,
    Workspaces,
    Files,
    Branches,
}

const CATEGORIES: [(Category, &str); 5] = [
    (Category::All, "All"),
    (Category::Commands, "Commands"),
    (Category::Workspaces, "Workspaces"),
    (Category::Files, "Files"),
    (Category::Branches, "Branches"),
];

impl Category {
    fn placeholder(self) -> &'static str {
        match self {
            Category::All => "Search commands, workspaces, files…",
            Category::Commands => "Type a command or screen…",
            Category::Workspaces => "Find or create a workspace…",
            Category::Files => "Find a file…",
            Category::Branches => "Find a branch…",
        }
    }
}

/// A workspace as the switcher lists it.
#[derive(Clone, Debug)]
pub struct WorkspaceRow {
    /// Index in the window's workspace list.
    pub index: usize,
    pub name: String,
    pub tabs: usize,
    pub current: bool,
}

pub struct PaletteInput {
    /// Slash commands of the active agent pane: `(name, description)`.
    pub commands: Vec<(String, String)>,
    pub workspaces: Vec<WorkspaceRow>,
    /// The current workspace's root, for files and branches.
    pub root: PathBuf,
    pub category: Category,
}

#[derive(Clone)]
struct Row {
    icon: IconName,
    title: String,
    meta: String,
    detail: String,
    group: &'static str,
    action: PaletteAction,
    /// Workspace rows get rename/delete buttons.
    workspace: Option<usize>,
    current: bool,
}

enum Line {
    Header(&'static str),
    Row(usize),
}

const MAX_FILES: usize = 20_000;

pub struct PaletteView {
    query: Entity<InputState>,
    category: Category,
    selected: usize,
    commands: Vec<Row>,
    workspaces: Vec<WorkspaceRow>,
    root: PathBuf,
    files: Option<Vec<String>>,
    branches: Option<Vec<(String, bool)>>,
    renaming: Option<(usize, Entity<InputState>)>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<PaletteEvent> for PaletteView {}

/// Case-insensitive subsequence score; lower is better, `None` = no match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    if let Some(pos) = text.find(&query.to_lowercase()) {
        return Some(pos);
    }
    let mut chars = text.char_indices();
    let mut last = 0;
    let mut gaps = 0;
    for q in query.to_lowercase().chars() {
        let (i, _) = chars.find(|(_, c)| *c == q)?;
        gaps += i - last;
        last = i;
    }
    Some(100 + gaps)
}

/// Files under `root`, relative, honoring `.gitignore`; at most `cap`.
pub fn walk_files(root: &Path, cap: usize) -> Vec<String> {
    ignore::WalkBuilder::new(root)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|entry| {
            entry
                .path()
                .strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .take(cap)
        .collect()
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Local branches, `(name, is_current)`; empty outside a repository.
pub fn list_branches(root: &Path) -> Vec<(String, bool)> {
    let Ok(out) = git_command(root)
        .args(["branch", "--format=%(HEAD)%(refname:short)"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_branches(&String::from_utf8_lossy(&out.stdout))
}

fn parse_branches(raw: &str) -> Vec<(String, bool)> {
    raw.lines()
        .filter_map(|line| {
            let (head, name) = line.split_at_checked(1)?;
            let name = name.trim();
            (!name.is_empty() && !name.starts_with('(')).then(|| (name.to_string(), head == "*"))
        })
        .collect()
}

/// `git switch <branch>` in `root`; the error is git's own message.
pub fn switch_branch(root: &Path, branch: &str) -> Result<(), String> {
    let out = git_command(root)
        .args(["switch", branch])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn screen_rows() -> Vec<Row> {
    let row = |icon, title: &str, detail: &str, action| Row {
        icon,
        title: title.into(),
        meta: String::new(),
        detail: detail.into(),
        group: "Gentle Shell",
        action,
        workspace: None,
        current: false,
    };
    let cmd = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    };
    vec![
        row(
            IconName::Users,
            "Agents",
            &format!("Helpers working for this session ({cmd}⇧A)"),
            PaletteAction::Agents,
        ),
        row(
            IconName::Layers,
            "Profiles",
            "Model and effort per agent; profiles as tabs",
            PaletteAction::Profiles,
        ),
        row(
            IconName::RotateCcwClock,
            "Sessions",
            "Resume a previous conversation",
            PaletteAction::Sessions,
        ),
        row(
            IconName::Brain,
            "Memory",
            "Browse and search Engram, the persistent memory",
            PaletteAction::Memory,
        ),
        row(
            IconName::Gauge,
            "Usage & stats",
            "Subscription windows and local token/cost stats",
            PaletteAction::Usage,
        ),
        row(
            IconName::KeyRound,
            "Providers",
            "Sign in with a subscription or an API key",
            PaletteAction::Providers,
        ),
        row(
            IconName::Settings,
            "Settings",
            "Gentle Shell configuration",
            PaletteAction::Settings,
        ),
        row(
            IconName::Plus,
            "New session",
            &format!("Start a fresh conversation in this pane ({cmd}N)"),
            PaletteAction::NewSession,
        ),
        row(
            IconName::Bot,
            "New agent tab",
            &format!("Another agent in the same folder ({cmd}T)"),
            PaletteAction::NewTab,
        ),
        row(
            IconName::SquareTerminal,
            "New terminal tab",
            "A shell in the same folder",
            PaletteAction::NewTerminalTab,
        ),
        row(
            IconName::Columns2,
            "Split right",
            &format!("Same kind of pane beside this one ({cmd}D)"),
            PaletteAction::SplitRight,
        ),
        row(
            IconName::Rows2,
            "Split down",
            &format!("Same kind of pane below this one ({cmd}⇧D)"),
            PaletteAction::SplitDown,
        ),
        row(
            IconName::Bot,
            "New agent pane",
            "Split with an agent beside the focused pane",
            PaletteAction::NewAgentPane,
        ),
        row(
            IconName::SquareTerminal,
            "New terminal pane",
            "Split with a terminal beside the focused pane",
            PaletteAction::NewTerminalPane,
        ),
        row(
            IconName::X,
            "Close pane",
            &format!("The last pane closes its tab ({cmd}W)"),
            PaletteAction::ClosePane,
        ),
        row(
            IconName::PanelLeft,
            "Toggle sidebar",
            &format!("Files and the left panel ({cmd}B)"),
            PaletteAction::ToggleSidebar,
        ),
        row(
            IconName::PanelRight,
            "Toggle side panel",
            &format!("Changes, todo, jobs, helpers ({cmd}⌥B)"),
            PaletteAction::ToggleRail,
        ),
    ]
}

impl PaletteView {
    pub fn new(input: PaletteInput, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(input.category.placeholder()));
        query.update(cx, |state, cx| state.focus(window, cx));
        let subscription = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.selected = 0;
                this.scroll.scroll_to_item(0);
                cx.notify();
            }
        });
        let mut commands = screen_rows();
        commands.extend(input.commands.into_iter().map(|(name, detail)| {
            let (group, icon) = if name.starts_with("skill:") {
                ("Skills", IconName::Zap)
            } else if name.starts_with("gentle:") {
                ("Gentle commands", IconName::Command)
            } else {
                ("Commands", IconName::Command)
            };
            Row {
                icon,
                title: format!("/{name}"),
                meta: String::new(),
                detail,
                group,
                action: PaletteAction::Command(name),
                workspace: None,
                current: false,
            }
        }));

        let root = input.root.clone();
        let files = cx.spawn(async move |this, cx| {
            let files = cx
                .background_executor()
                .spawn(async move { walk_files(&root, MAX_FILES) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.files = Some(files);
                cx.notify();
            });
        });
        let root = input.root.clone();
        let branches = cx.spawn(async move |this, cx| {
            let branches = cx
                .background_executor()
                .spawn(async move { list_branches(&root) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.branches = Some(branches);
                cx.notify();
            });
        });

        Self {
            query,
            category: input.category,
            selected: 0,
            commands,
            workspaces: input.workspaces,
            root: input.root,
            files: None,
            branches: None,
            renaming: None,
            scroll: ScrollHandle::new(),
            _subscriptions: vec![subscription],
            _tasks: vec![files, branches],
        }
    }

    pub fn category(&self) -> Category {
        self.category
    }

    pub fn set_category(
        &mut self,
        category: Category,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.category = category;
        self.selected = 0;
        self.renaming = None;
        self.scroll.scroll_to_item(0);
        self.query.update(cx, |state, cx| {
            state.set_placeholder(category.placeholder(), window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).value().trim().to_string()
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let query = self.query_text(cx);
        let all = self.category == Category::All;
        let mut rows = Vec::new();
        if matches!(self.category, Category::All | Category::Workspaces) {
            rows.extend(self.workspace_rows(&query, all));
        }
        if matches!(self.category, Category::All | Category::Commands) {
            rows.extend(self.command_rows(&query, if all { 12 } else { 80 }));
        }
        if self.category == Category::Files || (all && !query.is_empty()) {
            rows.extend(self.file_rows(&query, if all { 8 } else { 60 }));
        }
        if self.category == Category::Branches || (all && !query.is_empty()) {
            rows.extend(self.branch_rows(&query, if all { 5 } else { 60 }));
        }
        rows
    }

    fn workspace_rows(&self, query: &str, all: bool) -> Vec<Row> {
        let mut scored: Vec<(usize, &WorkspaceRow)> = self
            .workspaces
            .iter()
            .filter_map(|w| Some((fuzzy_score(query, &w.name)?, w)))
            .collect();
        scored.sort_by_key(|(score, w)| (*score, w.index));
        let mut rows: Vec<Row> = scored
            .into_iter()
            .take(if all { 6 } else { usize::MAX })
            .map(|(_, w)| Row {
                icon: if w.current {
                    IconName::Check
                } else {
                    IconName::Layers
                },
                title: w.name.clone(),
                meta: format!(
                    "{} {}{}",
                    w.tabs,
                    if w.tabs == 1 { "tab" } else { "tabs" },
                    if w.current { " · current" } else { "" }
                ),
                detail: String::new(),
                group: "Workspaces",
                action: PaletteAction::SwitchWorkspace(w.index),
                workspace: Some(w.index),
                current: w.current,
            })
            .collect();
        if all {
            return rows;
        }
        let exists = self
            .workspaces
            .iter()
            .any(|w| w.name.eq_ignore_ascii_case(query));
        if !query.is_empty() && !exists {
            rows.insert(
                0,
                Row {
                    icon: IconName::Plus,
                    title: format!("Create workspace \u{201c}{query}\u{201d}"),
                    meta: String::new(),
                    detail: format!("in {}", ui::split_path(&self.root).1),
                    group: "Workspaces",
                    action: PaletteAction::CreateWorkspace {
                        name: query.to_string(),
                        pick_folder: false,
                    },
                    workspace: None,
                    current: false,
                },
            );
        }
        rows.push(Row {
            icon: IconName::FolderOpen,
            title: "Create workspace from folder…".into(),
            meta: String::new(),
            detail: if query.is_empty() {
                "Named after the folder".into()
            } else {
                format!("Named \u{201c}{query}\u{201d}")
            },
            group: "Workspaces",
            action: PaletteAction::CreateWorkspace {
                name: if exists {
                    String::new()
                } else {
                    query.to_string()
                },
                pick_folder: true,
            },
            workspace: None,
            current: false,
        });
        rows
    }

    fn command_rows(&self, query: &str, cap: usize) -> Vec<Row> {
        let query = query.trim_start_matches('/');
        let mut scored: Vec<(usize, usize, &Row)> = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                let score = fuzzy_score(query, &row.title)
                    .or_else(|| fuzzy_score(query, &row.detail).map(|s| s + 500))?;
                Some((score, i, row))
            })
            .collect();
        // Keep groups together when nothing is typed.
        if !query.is_empty() {
            scored.sort_by_key(|(score, i, _)| (*score, *i));
        }
        scored
            .into_iter()
            .take(cap)
            .map(|(_, _, r)| r.clone())
            .collect()
    }

    fn file_rows(&self, query: &str, cap: usize) -> Vec<Row> {
        let Some(files) = &self.files else {
            return Vec::new();
        };
        let mut scored: Vec<(usize, &String)> = files
            .iter()
            .filter_map(|path| {
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                let score = fuzzy_score(query, name)
                    .or_else(|| fuzzy_score(query, path).map(|s| s + 50))?;
                Some((score, path))
            })
            .collect();
        if !query.is_empty() {
            scored.sort_by_key(|(score, path)| (*score, path.len()));
        }
        scored
            .into_iter()
            .take(cap)
            .map(|(_, path)| {
                let (dir, name) = match path.rsplit_once(['/', '\\']) {
                    Some((dir, name)) => (dir.to_string(), name.to_string()),
                    None => (String::new(), path.clone()),
                };
                Row {
                    icon: IconName::File,
                    title: name,
                    meta: String::new(),
                    detail: dir,
                    group: "Files",
                    action: PaletteAction::OpenFile(self.root.join(path)),
                    workspace: None,
                    current: false,
                }
            })
            .collect()
    }

    fn branch_rows(&self, query: &str, cap: usize) -> Vec<Row> {
        let Some(branches) = &self.branches else {
            return Vec::new();
        };
        let mut scored: Vec<(usize, &(String, bool))> = branches
            .iter()
            .filter_map(|b| Some((fuzzy_score(query, &b.0)?, b)))
            .collect();
        scored.sort_by_key(|(score, (_, current))| (!*current, *score));
        scored
            .into_iter()
            .take(cap)
            .map(|(_, (name, current))| Row {
                icon: if *current {
                    IconName::Check
                } else {
                    IconName::GitBranch
                },
                title: name.clone(),
                meta: if *current {
                    "current".into()
                } else {
                    String::new()
                },
                detail: String::new(),
                group: "Branches",
                action: PaletteAction::SwitchBranch(name.clone()),
                workspace: None,
                current: *current,
            })
            .collect()
    }

    /// Group headers and rows in display order; the scroll container's children.
    fn lines(rows: &[Row]) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut group = "";
        for (i, row) in rows.iter().enumerate() {
            if row.group != group {
                group = row.group;
                lines.push(Line::Header(group));
            }
            lines.push(Line::Row(i));
        }
        lines
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        if rows.is_empty() {
            return;
        }
        let len = rows.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        let line = Self::lines(&rows)
            .iter()
            .position(|l| matches!(l, Line::Row(i) if *i == self.selected))
            .unwrap_or(0);
        // Keep the group header in view above the first row.
        self.scroll
            .scroll_to_item(if self.selected == 0 { 0 } else { line });
        cx.notify();
    }

    fn run_selected(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        if let Some(row) = rows.get(self.selected.min(rows.len().saturating_sub(1))) {
            cx.emit(PaletteEvent::Run(row.action.clone()));
        }
    }

    fn cycle_category(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let len = CATEGORIES.len();
        let index = CATEGORIES
            .iter()
            .position(|(c, _)| *c == self.category)
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        self.set_category(CATEGORIES[next].0, window, cx);
    }

    fn start_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .workspaces
            .iter()
            .find(|w| w.index == index)
            .map(|w| w.name.clone())
        else {
            return;
        };
        let state = cx.new(|cx| InputState::new(window, cx).default_value(name));
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.renaming = Some((index, state));
        cx.notify();
    }

    fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, state)) = self.renaming.take() else {
            return;
        };
        let name = state.read(cx).value().trim().to_string();
        if commit && !name.is_empty() {
            if let Some(row) = self.workspaces.iter_mut().find(|w| w.index == index) {
                row.name = name.clone();
            }
            cx.emit(PaletteEvent::Run(PaletteAction::RenameWorkspace {
                index,
                name,
            }));
        }
        self.query.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let renaming = self.renaming.is_some();
        match keystroke.key.as_str() {
            "escape" if renaming => self.finish_rename(false, window, cx),
            "escape" => cx.emit(PaletteEvent::Close),
            "enter" if renaming => self.finish_rename(true, window, cx),
            "enter" => self.run_selected(cx),
            "up" if !renaming => self.move_selection(-1, cx),
            "down" if !renaming => self.move_selection(1, cx),
            "p" if modifiers.control && !renaming => self.move_selection(-1, cx),
            "n" if modifiers.control && !renaming => self.move_selection(1, cx),
            "tab" if !renaming => self.cycle_category(!modifiers.shift, window, cx),
            // With a query typed, ←/→ keep moving the caret; Tab still switches.
            "left" | "right"
                if !renaming && !modifiers.modified() && self.query.read(cx).value().is_empty() =>
            {
                self.cycle_category(keystroke.key == "right", window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_categories(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .px_4()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(ui::border())
            .children(CATEGORIES.iter().map(|(category, label)| {
                let category = *category;
                let active = category == self.category;
                div()
                    .id(SharedString::from(format!("palette-cat-{label}")))
                    .h(px(28.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(ui::accent().opacity(0.14))
                            .border_color(ui::accent().opacity(0.45))
                            .text_color(ui::accent())
                    })
                    .when(!active, |d| {
                        d.border_color(gpui_kit::transparent_black())
                            .text_color(ui::muted())
                            .hover(|s| s.bg(ui::surface_hover()).text_color(ui::fg()))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_category(category, window, cx)
                    }))
                    .child(*label)
            }))
    }

    fn render_row(&self, i: usize, row: Row, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = i == self.selected;
        let action = row.action.clone();
        let renaming = self
            .renaming
            .as_ref()
            .filter(|(index, _)| Some(*index) == row.workspace)
            .map(|(_, state)| state.clone());
        h_flex()
            .id(("palette-row", i))
            .group("palette-row")
            .relative()
            .h(px(44.))
            .flex_none()
            .pl_3()
            .pr_2()
            .gap_3()
            .rounded_lg()
            .cursor_pointer()
            .when(selected, |d| d.bg(ui::raised()))
            .when(!selected, |d| d.hover(|s| s.bg(ui::surface_hover())))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.renaming.is_none() {
                    cx.emit(PaletteEvent::Run(action.clone()));
                }
            }))
            .when(selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(11.))
                        .h(px(22.))
                        .w(px(3.))
                        .rounded_full()
                        .bg(ui::accent()),
                )
            })
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .border_1()
                    .border_color(ui::border_strong())
                    .bg(ui::well())
                    .text_color(if selected || row.current {
                        ui::accent()
                    } else {
                        ui::muted()
                    })
                    .child(Icon::new(row.icon).small()),
            )
            .map(|d| match renaming {
                Some(state) => d.child(
                    ui::field()
                        .h(px(30.))
                        .w(px(280.))
                        .child(div().flex_1().child(Input::new(&state).appearance(false)))
                        .child(ui::kbd("↵")),
                ),
                None => d.child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(360.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(14.))
                                .text_color(ui::fg())
                                .child(row.title.clone()),
                        )
                        .when(!row.meta.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(ui::faint())
                                    .child(row.meta.clone()),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(row.detail.clone()),
                        ),
                ),
            })
            .when_some(row.workspace, |d, index| {
                let deletable = self.workspaces.len() > 1;
                d.child(
                    h_flex()
                        .gap_0p5()
                        .flex_none()
                        .invisible()
                        .when(selected, |d| d.visible())
                        .group_hover("palette-row", |s| s.visible())
                        .child(
                            Button::new(("ws-rename", index))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Pencil))
                                .tooltip("Rename")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.start_rename(index, window, cx);
                                })),
                        )
                        .when(deletable, |d| {
                            d.child(
                                Button::new(("ws-delete", index))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::X))
                                    .tooltip("Delete workspace")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.emit(PaletteEvent::Run(PaletteAction::DeleteWorkspace(
                                            index,
                                        )));
                                    })),
                            )
                        }),
                )
            })
    }

    fn render_empty(&self, cx: &App) -> AnyElement {
        let loading = match self.category {
            Category::Files => self.files.is_none(),
            Category::Branches => self.branches.is_none(),
            _ => false,
        };
        let text = if loading {
            if self.category == Category::Files {
                "Indexing files…"
            } else {
                "Reading branches…"
            }
        } else if self.category == Category::Branches && self.query_text(cx).is_empty() {
            "No local branches here"
        } else {
            "No matches"
        };
        h_flex()
            .justify_center()
            .gap_2()
            .py_8()
            .text_sm()
            .text_color(ui::muted())
            .when(loading, |d| d.child(Spinner::new().small()))
            .child(text)
            .into_any_element()
    }
}

fn hint(keys: &[&'static str], label: &'static str) -> Div {
    h_flex()
        .gap_1()
        .children(keys.iter().map(|k| ui::kbd(*k)))
        .child(
            div()
                .pl_0p5()
                .text_xs()
                .text_color(ui::muted())
                .child(label),
        )
}

impl Render for PaletteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        if !rows.is_empty() && self.selected >= rows.len() {
            self.selected = rows.len() - 1;
        }
        let lines = Self::lines(&rows);
        let empty = rows.is_empty();
        let mut list = v_flex()
            .id("palette-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px_2()
            .pb_2();
        for line in lines {
            list = match line {
                Line::Header(group) => list.child(
                    div()
                        .flex_none()
                        .px_3()
                        .pt_3()
                        .pb_1p5()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui::muted())
                        .child(group),
                ),
                Line::Row(i) => list.child(self.render_row(i, rows[i].clone(), cx)),
            };
        }
        if empty {
            list = list.child(self.render_empty(cx));
        }
        let switch_keys: &[&'static str] = if cfg!(target_os = "macos") {
            &["⌃", "⌘", "K"]
        } else {
            &["Ctrl", "Alt", "K"]
        };

        let panel = v_flex()
            .id("palette")
            .w(px(680.))
            .h(px(560.))
            .max_h_full()
            .rounded_2xl()
            .border_1()
            .border_color(ui::border_strong())
            .bg(ui::screen_bg())
            .shadow_2xl()
            .overflow_hidden()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .h(px(56.))
                    .flex_none()
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(ui::border())
                    .child(Icon::new(IconName::Search).text_color(ui::muted()))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(16.))
                            .child(Input::new(&self.query).appearance(false)),
                    )
                    .child(ui::kbd("esc")),
            )
            .child(self.render_categories(cx))
            .child(list)
            .child(
                h_flex()
                    .h(px(44.))
                    .flex_none()
                    .px_4()
                    .gap_4()
                    .border_t_1()
                    .border_color(ui::border())
                    .child(hint(&["↑", "↓"], "navigate"))
                    .child(hint(&["↵"], "run"))
                    .child(hint(&["←", "→"], "category"))
                    .child(hint(switch_keys, "switch workspaces"))
                    .child(div().flex_1())
                    .child(hint(&["esc"], "close")),
            );

        div()
            .id("palette-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(72.))
            .pb(px(32.))
            .bg(ui::backdrop())
            .occlude()
            .capture_key_down(cx.listener(Self::on_key_down))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Close)))
            .child(ui::pop_in(
                "palette-in",
                div().flex().max_h_full().child(panel),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::{fuzzy_score, parse_branches};

    #[test]
    fn fuzzy_prefers_substrings_then_tight_subsequences() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert_eq!(fuzzy_score("prof", "Profiles"), Some(0));
        assert!(fuzzy_score("gjb", "/gentle:jobs").unwrap() >= 100);
        assert!(
            fuzzy_score("gjb", "/gentle:jobs") < fuzzy_score("gjb", "/gentle:review-mode-jobs-x-b")
        );
        assert_eq!(fuzzy_score("zz", "Profiles"), None);
    }

    #[test]
    fn branches_parse_with_the_current_one_marked() {
        let raw = "*main\n feature/x\n (HEAD detached at 1234)\n\n";
        assert_eq!(
            parse_branches(raw),
            vec![("main".to_string(), true), ("feature/x".to_string(), false)]
        );
    }
}
