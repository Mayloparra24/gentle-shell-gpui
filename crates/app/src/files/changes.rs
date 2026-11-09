//! The sidebar's Changes tab, after Tern: the working tree split into
//! Unstaged and Staged (checkbox per file to move it across), a list or
//! by-folder view, stash, and a commit box whose message Gentle Shell can
//! write from the staged diff.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _, DropdownButton};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::status_color;
use crate::git::{self, EntryKind, StatusEntry, WorkTree};
use crate::ui;

const POLL: Duration = Duration::from_secs(3);
const ROW_HEIGHT: f32 = 26.;
/// The most diff handed to the model for a commit message.
const DIFF_LIMIT: usize = 24_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesEvent {
    /// A change was clicked: show its diff (absolute path).
    OpenDiff(PathBuf),
    /// "Open file" from a change's menu (absolute path).
    OpenFile(PathBuf),
    /// [`ChangesView::change_count`] changed.
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Side {
    Unstaged,
    Staged,
}

#[derive(Clone, PartialEq)]
struct Repo {
    /// Status paths are relative to this, not to the sidebar's root.
    toplevel: PathBuf,
    tree: WorkTree,
}

/// One row of the virtualized list; every kind is [`ROW_HEIGHT`] tall.
enum Line {
    Header {
        side: Side,
        count: usize,
    },
    Untracked(usize),
    Dir {
        side: Side,
        dir: String,
        count: usize,
        collapsed: bool,
    },
    Entry {
        side: Side,
        /// Index into the work tree's entries.
        entry: usize,
        badge: char,
        /// Under a folder row in the by-folder view.
        nested: bool,
    },
}

pub struct ChangesView {
    root: PathBuf,
    /// `None` until the first read, `Some(None)` outside a git work tree.
    repo: Option<Option<Repo>>,
    tree_mode: bool,
    collapsed: HashSet<(Side, String)>,
    selected: Option<(Side, String)>,
    /// The list's rows, rebuilt when the tree, mode or collapsed folders change.
    lines: Rc<Vec<Line>>,
    scroll: UniformListScrollHandle,
    summary: Entity<InputState>,
    description: Entity<TextareaState>,
    stash_message: Entity<InputState>,
    amend: bool,
    generating: bool,
    /// What a running commit/push is doing, for the status line.
    busy: Option<&'static str>,
    error: Option<String>,
    visible: bool,
    _poll: Task<()>,
    _commit: Task<()>,
    _generate: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChangesEvent> for ChangesView {}

impl ChangesView {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let summary = cx.new(|cx| InputState::new(window, cx).placeholder("Summary"));
        let description = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder("Why this change…")
        });
        let stash_message =
            cx.new(|cx| InputState::new(window, cx).placeholder("Message (optional)"));
        // The primary button reads "Generate message" until a summary exists.
        let subscriptions = vec![cx.subscribe(&summary, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        })];
        let mut this = Self {
            root,
            repo: None,
            tree_mode: false,
            collapsed: HashSet::new(),
            selected: None,
            lines: Rc::new(Vec::new()),
            scroll: UniformListScrollHandle::new(),
            summary,
            description,
            stash_message,
            amend: false,
            generating: false,
            busy: None,
            error: None,
            visible: true,
            _poll: Task::ready(()),
            _commit: Task::ready(()),
            _generate: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this._poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                // Only while shown and the window is in front.
                let Ok(due) = this.update_in(cx, |this, window, _| {
                    (this.visible && window.is_window_active()).then(|| this.status_job())
                }) else {
                    break;
                };
                if let Some(job) = due {
                    let repo = cx.background_executor().spawn(job).await;
                    if this.update(cx, |this, cx| this.apply(repo, cx)).is_err() {
                        break;
                    }
                }
                cx.background_executor().timer(POLL).await;
            }
        });
        this
    }

    /// Pause polling while hidden; showing the tab re-reads status.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let shown = visible && !self.visible;
        self.visible = visible;
        if shown {
            self.refresh(cx);
        }
    }

    /// Files with any change, staged or not (one per path).
    pub fn change_count(&self) -> usize {
        self.tree().map_or(0, |tree| tree.entries.len())
    }

    fn tree(&self) -> Option<&WorkTree> {
        self.repo.as_ref()?.as_ref().map(|repo| &repo.tree)
    }

    fn toplevel(&self) -> Option<PathBuf> {
        self.repo
            .as_ref()?
            .as_ref()
            .map(|repo| repo.toplevel.clone())
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let job = self.status_job();
        cx.spawn(async move |this, cx| {
            let repo = cx.background_executor().spawn(job).await;
            let _ = this.update(cx, |this, cx| this.apply(repo, cx));
        })
        .detach();
    }

    fn status_job(&self) -> impl Future<Output = Option<Repo>> + Send + 'static + use<> {
        let root = self.root.clone();
        let known = self.toplevel();
        async move {
            let toplevel = known.or_else(|| git::toplevel(&root))?;
            let tree = git::work_tree(&toplevel)?;
            Some(Repo { toplevel, tree })
        }
    }

    fn apply(&mut self, repo: Option<Repo>, cx: &mut Context<Self>) {
        if self.repo.as_ref() == Some(&repo) {
            return;
        }
        let before = self.change_count();
        self.repo = Some(repo);
        self.relayout();
        if self.change_count() != before {
            cx.emit(ChangesEvent::Changed);
        }
        cx.notify();
    }

    /// Rebuild the flat row model: per side a header, the untracked count,
    /// then the entries (grouped under folder rows in the by-folder view).
    fn relayout(&mut self) {
        let mut lines = Vec::new();
        if let Some(tree) = self.tree() {
            let untracked = tree
                .entries
                .iter()
                .filter(|e| e.kind == EntryKind::Untracked)
                .count();
            for side in [Side::Unstaged, Side::Staged] {
                let entries: Vec<(usize, char)> = tree
                    .entries
                    .iter()
                    .enumerate()
                    .filter_map(|(i, e)| {
                        match side {
                            Side::Unstaged => e.unstaged(),
                            Side::Staged => e.staged(),
                        }
                        .map(|badge| (i, badge))
                    })
                    .collect();
                if entries.is_empty() {
                    continue;
                }
                lines.push(Line::Header {
                    side,
                    count: entries.len(),
                });
                if side == Side::Unstaged && untracked > 0 {
                    lines.push(Line::Untracked(untracked));
                }
                if !self.tree_mode {
                    lines.extend(entries.into_iter().map(|(entry, badge)| Line::Entry {
                        side,
                        entry,
                        badge,
                        nested: false,
                    }));
                    continue;
                }
                let mut groups: BTreeMap<&str, Vec<(usize, char)>> = BTreeMap::new();
                for (entry, badge) in entries {
                    let path = &tree.entries[entry].path;
                    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
                    groups.entry(dir).or_default().push((entry, badge));
                }
                for (dir, files) in groups {
                    let nested = !dir.is_empty();
                    if nested {
                        let collapsed = self.collapsed.contains(&(side, dir.to_string()));
                        lines.push(Line::Dir {
                            side,
                            dir: dir.to_string(),
                            count: files.len(),
                            collapsed,
                        });
                        if collapsed {
                            continue;
                        }
                    }
                    lines.extend(files.into_iter().map(|(entry, badge)| Line::Entry {
                        side,
                        entry,
                        badge,
                        nested,
                    }));
                }
            }
        }
        self.lines = Rc::new(lines);
    }

    /// Run a git operation in the background, then re-read status; a
    /// failure shows git's message under the commit box.
    fn git_op(
        &mut self,
        job: impl FnOnce(&Path) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(toplevel) = self.toplevel() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let (result, tree) = cx
                .background_executor()
                .spawn(async move {
                    let result = job(&toplevel);
                    let tree = git::work_tree(&toplevel).map(|tree| Repo { toplevel, tree });
                    (result, tree)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.error = result.err();
                this.apply(tree, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle(&mut self, entry: &StatusEntry, side: Side, cx: &mut Context<Self>) {
        let mut paths = vec![entry.path.clone()];
        // Unstaging a rename has to bring its source back to the index too.
        paths.extend(entry.orig.clone());
        match side {
            Side::Unstaged => self.git_op(move |top| git::stage(top, &paths), cx),
            Side::Staged => self.git_op(move |top| git::unstage(top, &paths), cx),
        }
    }

    fn click_entry(&mut self, path: &str, side: Side, cx: &mut Context<Self>) {
        let Some(toplevel) = self.toplevel() else {
            return;
        };
        self.selected = Some((side, path.to_string()));
        cx.emit(ChangesEvent::OpenDiff(toplevel.join(path)));
        cx.notify();
    }

    fn confirm_discard(
        &mut self,
        entry: StatusEntry,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = entry
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&entry.path)
            .to_string();
        let title = format!("Discard changes to \u{201c}{name}\u{201d}?");
        let description = match (side, entry.kind, entry.index) {
            (Side::Unstaged, EntryKind::Untracked, _) => "The untracked file is deleted.",
            (Side::Unstaged, _, _) => "Unstaged edits go back to the staged version.",
            (Side::Staged, _, 'A' | 'C') => "The new file is removed from the index and deleted.",
            (Side::Staged, _, _) => "The file goes back to the last commit, staged edits included.",
        };
        let description = format!("{description} This can't be undone.");
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, entry) = (this.clone(), entry.clone());
            alert
                .title(title.clone())
                .description(description.clone())
                .ok_text("Discard")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let entry = entry.clone();
                    let staged = side == Side::Staged;
                    let _ = this.update(cx, |this, cx| {
                        this.git_op(move |top| git::discard(top, &entry, staged), cx)
                    });
                    true
                })
        });
    }

    fn open_stash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stash_message
            .update(cx, |state, cx| state.set_value("", window, cx));
        let input = self.stash_message.clone();
        let this = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_alert_dialog(cx, move |alert, window, cx| {
            if first.replace(false) {
                input.update(cx, |state, cx| state.focus(window, cx));
            }
            let (this, message) = (this.clone(), input.clone());
            alert
                .title("Stash all changes")
                .description(
                    "Staged, unstaged and untracked changes move to a new stash and the working tree is left clean.",
                )
                .child(div().pt_2().child(Input::new(&input).small()))
                .ok_text("Stash")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let message = message.read(cx).value().to_string();
                    let _ = this.update(cx, |this, cx| {
                        this.git_op(move |top| git::stash_push(top, &message), cx)
                    });
                    true
                })
        });
    }

    // ── Commit box ──────────────────────────────────────────────────────────

    fn summary_text(&self, cx: &App) -> String {
        self.summary.read(cx).value().trim().to_string()
    }

    /// ⌘↵ and the main button: write a message first if there is none.
    fn primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.summary_text(cx).is_empty() && !self.amend {
            self.generate(window, cx);
        } else {
            self.commit(false, window, cx);
        }
    }

    fn commit(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(toplevel) = self.toplevel() else {
            return;
        };
        let summary = self.summary_text(cx);
        if summary.is_empty() && !self.amend {
            self.error = Some("Write a summary, or let Gentle Shell write it".into());
            cx.notify();
            return;
        }
        let description = self.description.read(cx).value().to_string();
        let amend = self.amend;
        self.busy = Some(if push {
            "Committing and pushing…"
        } else {
            "Committing…"
        });
        self.error = None;
        cx.notify();
        self._commit = cx.spawn_in(window, async move |this, cx| {
            let (committed, result, tree) = cx
                .background_executor()
                .spawn(async move {
                    let mut committed = false;
                    let result =
                        git::commit(&toplevel, &summary, &description, amend).and_then(|()| {
                            committed = true;
                            if push {
                                git::push(&toplevel)
                                    .map_err(|e| format!("Committed, but the push failed: {e}"))
                            } else {
                                Ok(())
                            }
                        });
                    let tree = git::work_tree(&toplevel).map(|tree| Repo { toplevel, tree });
                    (committed, result, tree)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                this.error = result.err();
                if committed {
                    this.amend = false;
                    this.summary
                        .update(cx, |state, cx| state.set_value("", window, cx));
                    this.description
                        .update(cx, |state, cx| state.set_value("", window, cx));
                }
                this.apply(tree, cx);
                cx.notify();
            });
        });
    }

    fn generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.generating {
            return;
        }
        let Some(toplevel) = self.toplevel() else {
            return;
        };
        self.generating = true;
        self.error = None;
        cx.notify();
        self._generate = cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { generate_message(&toplevel) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.generating = false;
                match result {
                    Ok((summary, description)) => {
                        this.summary
                            .update(cx, |state, cx| state.set_value(summary, window, cx));
                        this.description
                            .update(cx, |state, cx| state.set_value(description, window, cx));
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        });
    }

    /// Amending with empty fields starts from the last commit's message.
    fn toggle_amend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.amend = !self.amend;
        cx.notify();
        let empty =
            self.summary_text(cx).is_empty() && self.description.read(cx).value().trim().is_empty();
        let Some(toplevel) = self.toplevel().filter(|_| self.amend && empty) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let message = cx
                .background_executor()
                .spawn(async move { git::last_commit_message(&toplevel) })
                .await;
            let Some((summary, description)) = message else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.amend && this.summary_text(cx).is_empty() {
                    this.summary
                        .update(cx, |state, cx| state.set_value(summary, window, cx));
                    this.description
                        .update(cx, |state, cx| state.set_value(description, window, cx));
                }
            });
        })
        .detach();
    }

    fn on_commit_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "enter" && event.keystroke.modifiers.secondary(cx) {
            self.primary(window, cx);
            cx.stop_propagation();
        }
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    fn render_header(&self, cx: &mut Context<Self>) -> Div {
        let has_changes = self.change_count() > 0;
        let mode = |id: &'static str, icon: IconName, tree: bool, tip: &'static str| {
            let active = self.tree_mode == tree;
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .when(active, |d| d.bg(ui::raised()))
                .when(!active, |d| d.hover(|s| s.bg(ui::surface_hover())))
                .child(Icon::new(icon).xsmall().text_color(if active {
                    ui::fg()
                } else {
                    ui::muted()
                }))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.tree_mode != tree {
                        this.tree_mode = tree;
                        this.relayout();
                        cx.notify();
                    }
                }))
        };
        h_flex()
            .flex_none()
            .h(px(36.))
            .pl_3()
            .pr_1p5()
            .gap_2()
            .child(Icon::new(IconName::Diff).small().text_color(ui::muted()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::fg())
                    .child("Working tree"),
            )
            .child(
                h_flex()
                    .flex_none()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded_lg()
                    .bg(ui::well())
                    .child(mode("changes-mode-list", IconName::List, false, "List"))
                    .child(mode(
                        "changes-mode-tree",
                        IconName::ListTree,
                        true,
                        "Group by folder",
                    )),
            )
            .child(
                Button::new("changes-stash")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Archive))
                    .tooltip("Stash all changes")
                    .disabled(!has_changes)
                    .on_click(cx.listener(|this, _, window, cx| this.open_stash(window, cx))),
            )
    }

    fn render_section_header(&self, side: Side, count: usize, cx: &mut Context<Self>) -> Div {
        let (label, action, id) = match side {
            Side::Unstaged => ("Unstaged", "Stage all", "changes-stage-all"),
            Side::Staged => ("Staged", "Unstage all", "changes-unstage-all"),
        };
        let all = move |this: &mut Self, cx: &mut Context<Self>| match side {
            Side::Unstaged => this.git_op(git::stage_all, cx),
            Side::Staged => this.git_op(git::unstage_all, cx),
        };
        let weak = cx.entity().downgrade();
        h_flex()
            .h(px(ROW_HEIGHT))
            .px_3()
            .gap_2()
            .child(
                Checkbox::new(SharedString::from(format!("{id}-check")))
                    .small()
                    .checked(side == Side::Staged)
                    .tooltip(action)
                    .on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| all(this, cx));
                    }),
            )
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::fg())
                    .child(label),
            )
            .child(
                div()
                    .px_1p5()
                    .rounded_md()
                    .bg(ui::well())
                    .font_family(ui::MONO)
                    .text_size(px(10.5))
                    .text_color(ui::muted())
                    .child(count.to_string()),
            )
            .child(div().flex_1())
            .child(
                Button::new(id)
                    .outline()
                    .xsmall()
                    .label(action)
                    .on_click(cx.listener(move |this, _, _, cx| all(this, cx))),
            )
    }

    fn render_untracked(count: usize) -> Div {
        h_flex()
            .h(px(ROW_HEIGHT))
            .px_3()
            .gap_1()
            .text_size(px(12.))
            .child(
                div()
                    .font_family(ui::MONO)
                    .text_color(ui::success())
                    .child(count.to_string()),
            )
            .child(div().text_color(ui::muted()).child("untracked"))
    }

    fn render_line(&self, line: &Line, cx: &mut Context<Self>) -> AnyElement {
        match line {
            Line::Header { side, count } => self
                .render_section_header(*side, *count, cx)
                .into_any_element(),
            Line::Untracked(count) => Self::render_untracked(*count).into_any_element(),
            Line::Dir {
                side,
                dir,
                count,
                collapsed,
            } => self
                .render_dir(*side, dir, *count, *collapsed, cx)
                .into_any_element(),
            Line::Entry {
                side,
                entry,
                badge,
                nested,
            } => match self.tree().and_then(|tree| tree.entries.get(*entry)) {
                Some(status) => {
                    let indent = if *nested { 18. } else { 0. };
                    self.render_entry(status, *side, *badge, indent, cx)
                }
                None => div().h(px(ROW_HEIGHT)).into_any_element(),
            },
        }
    }

    fn render_dir(
        &self,
        side: Side,
        dir: &str,
        count: usize,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key = (side, dir.to_string());
        h_flex()
            .id(SharedString::from(format!("changes-dir-{side:?}-{dir}")))
            .h(px(ROW_HEIGHT))
            .mx_1p5()
            .px_1p5()
            .gap_1p5()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(ui::surface_hover()))
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .xsmall()
                .text_color(ui::faint()),
            )
            .child(
                Icon::new(if collapsed {
                    IconName::Folder
                } else {
                    IconName::FolderOpen
                })
                .small()
                .text_color(ui::muted()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(ui::muted())
                    .child(dir.to_string()),
            )
            .child(
                div()
                    .font_family(ui::MONO)
                    .text_size(px(10.5))
                    .text_color(ui::faint())
                    .child(count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed.remove(&key) {
                    this.collapsed.insert(key.clone());
                }
                this.relayout();
                cx.notify();
            }))
    }

    fn render_entry(
        &self,
        entry: &StatusEntry,
        side: Side,
        badge: char,
        indent: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|(s, path)| *s == side && *path == entry.path);
        let (dir, name) = match entry.path.rsplit_once('/') {
            Some((dir, name)) if indent == 0. => (format!("{dir}/"), name.to_string()),
            Some((_, name)) => (String::new(), name.to_string()),
            None => (String::new(), entry.path.clone()),
        };
        let deleted = badge == 'D';
        let id = format!("change-{side:?}-{}", entry.path);
        let weak = cx.entity().downgrade();
        let check = {
            let (weak, entry) = (weak.clone(), entry.clone());
            div()
                .id(SharedString::from(format!("{id}-check-wrap")))
                .flex_none()
                // Toggling the checkbox must not also open the diff.
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    Checkbox::new(SharedString::from(format!("{id}-check")))
                        .small()
                        .checked(side == Side::Staged)
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |this, cx| this.toggle(&entry, side, cx));
                        }),
                )
        };
        let path = entry.path.clone();
        let menu_entry = entry.clone();
        h_flex()
            .id(SharedString::from(id))
            .h(px(ROW_HEIGHT))
            .mx_1p5()
            .pl(px(6. + indent))
            .pr_1p5()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(ui::raised()))
            .when(!selected, |d| d.hover(|s| s.bg(ui::surface_hover())))
            .child(check)
            .child(status_badge(badge))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .when(!dir.is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(ui::faint())
                                .child(dir),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .max_w_full()
                            .truncate()
                            .text_color(if deleted { ui::muted() } else { ui::fg() })
                            .when(deleted, |d| d.line_through())
                            .child(name),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.click_entry(&path, side, cx)))
            .context_menu(move |menu, _, _| {
                let entry = menu_entry.clone();
                let deleted = side == Side::Unstaged && entry.worktree == 'D'
                    || side == Side::Staged && entry.index == 'D';
                let open = {
                    let (weak, path) = (weak.clone(), entry.path.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = weak.update(cx, |this, cx| {
                            if let Some(top) = this.toplevel() {
                                cx.emit(ChangesEvent::OpenFile(top.join(&path)));
                            }
                        });
                    }
                };
                let discard = {
                    let (weak, entry) = (weak.clone(), entry.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let entry = entry.clone();
                        let _ = weak
                            .update(cx, |this, cx| this.confirm_discard(entry, side, window, cx));
                    }
                };
                let toggle = {
                    let (weak, entry) = (weak.clone(), entry.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        let _ = weak.update(cx, |this, cx| this.toggle(&entry, side, cx));
                    }
                };
                menu.item(
                    PopupMenuItem::new("Open file")
                        .icon(Icon::new(IconName::File))
                        .disabled(deleted)
                        .on_click(open),
                )
                .item(
                    PopupMenuItem::new(match side {
                        Side::Unstaged => "Stage",
                        Side::Staged => "Unstage",
                    })
                    .icon(Icon::new(match side {
                        Side::Unstaged => IconName::Plus,
                        Side::Staged => IconName::Minus,
                    }))
                    .on_click(toggle),
                )
                .separator()
                .item(
                    PopupMenuItem::new("Discard changes…")
                        .icon(Icon::new(IconName::Undo2))
                        .disabled(entry.kind == EntryKind::Conflict)
                        .on_click(discard),
                )
            })
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let tree = match &self.repo {
            None => return empty_state(IconName::RefreshCw, "Reading git status…"),
            Some(None) => return empty_state(IconName::GitBranch, "Not a git repository"),
            Some(Some(repo)) => &repo.tree,
        };
        if tree.entries.is_empty() {
            return empty_state(IconName::CircleCheck, "Working tree clean");
        }
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "changes-list",
                    self.lines.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let lines = this.lines.clone();
                        lines[range]
                            .iter()
                            .map(|line| this.render_line(line, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .py_1()
                .track_scroll(&self.scroll),
            )
            .vertical_scrollbar(&self.scroll)
            .into_any_element()
    }

    fn render_commit(&self, cx: &mut Context<Self>) -> Div {
        let ready = matches!(self.repo, Some(Some(_)));
        let staged = self
            .tree()
            .is_some_and(|t| t.entries.iter().any(|e| e.staged().is_some()));
        let writes = self.summary_text(cx).is_empty() && !self.amend;
        let primary_label = if writes { "Generate message" } else { "Commit" };
        let working = self.busy.is_some() || self.generating;
        let weak = cx.entity().downgrade();
        v_flex()
            .flex_none()
            .mx_2()
            .mb_2()
            .mt_1()
            .px_3()
            .py_2()
            .gap_1()
            .rounded_xl()
            .bg(ui::surface())
            .capture_key_down(cx.listener(Self::on_commit_key))
            .child(
                div()
                    .text_size(px(13.5))
                    .child(Input::new(&self.summary).appearance(false)),
            )
            .child(
                div()
                    .min_h(px(48.))
                    .text_size(px(13.))
                    .text_color(ui::muted())
                    .child(Textarea::new(&self.description).appearance(false)),
            )
            .child(
                h_flex()
                    .pt_1()
                    .gap_1()
                    .child(
                        Button::new("changes-write")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Sparkles))
                            .label("Write it")
                            .loading(self.generating)
                            .disabled(!ready || working || !staged)
                            .tooltip("Write the message from the staged diff with Gentle Shell")
                            .on_click(cx.listener(|this, _, window, cx| this.generate(window, cx))),
                    )
                    .child(
                        Button::new("changes-amend")
                            .ghost()
                            .xsmall()
                            .label("Amend")
                            .selected(self.amend)
                            .disabled(!ready || working)
                            .tooltip("Rewrite the last commit instead of making a new one")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_amend(window, cx)),
                            ),
                    )
                    .child(div().flex_1())
                    .when_some(self.busy, |d, busy| {
                        d.child(
                            h_flex()
                                .gap_1p5()
                                .text_size(px(11.5))
                                .text_color(ui::muted())
                                .child(Spinner::new().xsmall().color(ui::accent()))
                                .child(busy),
                        )
                    })
                    .when(self.generating, |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(ui::muted())
                                .child("Writing…"),
                        )
                    }),
            )
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("changes-error")
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .text_size(px(11.5))
                        .text_color(ui::danger())
                        .child(error),
                )
            })
            .child(
                DropdownButton::new("changes-commit")
                    .primary()
                    .small()
                    .w_full()
                    .disabled(!ready || working)
                    .button(
                        Button::new("changes-commit-main")
                            .flex_1()
                            .label(primary_label)
                            .child(ui::kbd("⌘↵"))
                            .on_click(cx.listener(|this, _, window, cx| this.primary(window, cx))),
                    )
                    .dropdown_menu(move |menu, _, _| {
                        let item = |label: &'static str, icon: IconName, push: Option<bool>| {
                            let weak = weak.clone();
                            PopupMenuItem::new(label).icon(Icon::new(icon)).on_click(
                                move |_, window, cx| {
                                    let _ = weak.update(cx, |this, cx| match push {
                                        Some(push) => this.commit(push, window, cx),
                                        None => this.generate(window, cx),
                                    });
                                },
                            )
                        };
                        menu.item(item("Commit", IconName::GitCommitHorizontal, Some(false)))
                            .item(item("Commit & Push", IconName::Upload, Some(true)))
                            .separator()
                            .item(item("Generate message", IconName::Sparkles, None))
                    }),
            )
    }
}

impl Render for ChangesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The sidebar slot paints the glass; no fill here.
        v_flex()
            .size_full()
            .min_h_0()
            .child(self.render_header(cx))
            .child(self.render_list(cx))
            .when(matches!(self.repo, Some(Some(_))), |d| {
                d.child(self.render_commit(cx))
            })
    }
}

/// The boxed status letter: A, M, D, R, `?` untracked, `U` conflict.
fn status_badge(badge: char) -> Div {
    let color = if badge == 'U' {
        ui::danger()
    } else {
        status_color(badge)
    };
    div()
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(color.opacity(0.5))
        .bg(color.opacity(0.1))
        .font_family(ui::MONO)
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .text_color(color)
        .child(badge.to_string())
}

fn empty_state(icon: IconName, text: &'static str) -> AnyElement {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .p_6()
        .child(Icon::new(icon).text_color(ui::faint()))
        .child(div().text_size(px(12.)).text_color(ui::muted()).child(text))
        .into_any_element()
}

// ── Commit messages from Gentle Shell ──────────────────────────────────────

fn commit_prompt(diff: &str) -> String {
    format!(
        "Write a git commit message for the staged changes below, following Conventional \
         Commits (`type(scope): summary`, types: feat, fix, refactor, perf, docs, test, build, \
         ci, chore, style). Reply with the message only, no code fences, quotes or commentary: \
         a summary line of at most 72 characters in the imperative mood, then a blank line, \
         then a short body wrapped at 72 columns explaining what changed and why. Omit the body \
         for trivial changes.\n\n<diff>\n{diff}</diff>\n"
    )
}

/// Run the user's Gentle Shell once, in print mode without a session or
/// tools, on the staged diff. Blocking; call off the UI thread.
fn generate_message(toplevel: &Path) -> Result<(String, String), String> {
    let diff = git::staged_diff(toplevel, DIFF_LIMIT)?;
    if diff.trim().is_empty() {
        return Err("Stage some changes first; the message is written from the staged diff".into());
    }
    let spec = pi_rpc::resolve_backend(toplevel).map_err(|e| format!("{e:#}"))?;
    let mut command = pi_rpc::command(&spec.program);
    // Launcher options (home selectors) come before Pi's own arguments.
    command
        .args(spec.args.iter().take_while(|a| *a != "--mode"))
        .args(["--print", "--no-session", "--no-tools"])
        .arg(commit_prompt(&diff))
        .current_dir(toplevel)
        .stdin(Stdio::null());
    for (key, value) in &spec.env {
        // Interactive-host dialogs make no sense for a one-shot print run.
        if key != "GENTLE_SHELL_INTERACTIVE_HOST" {
            command.env(key, value);
        }
    }
    let out = command
        .output()
        .map_err(|e| format!("Could not start gentle-shell: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        let (tail, _) = ui::tail(detail.trim(), 6);
        return Err(format!("gentle-shell failed: {tail}"));
    }
    parse_commit_message(&stdout).ok_or_else(|| "gentle-shell returned no message".into())
}

/// `(summary, description)` from a model reply: code fences, a leading
/// "Commit message:" label and wrapping quotes are dropped.
fn parse_commit_message(reply: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = reply
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    let start = lines.iter().position(|line| {
        let line = line.trim();
        !line.is_empty()
            && !line
                .trim_end_matches(':')
                .eq_ignore_ascii_case("commit message")
    })?;
    let summary = lines[start].trim();
    let summary = summary
        .strip_prefix("Summary:")
        .unwrap_or(summary)
        .trim()
        .trim_matches(|c| c == '"' || c == '`' || c == '\'')
        .trim()
        .to_string();
    let description = lines[start + 1..].join("\n").trim().to_string();
    (!summary.is_empty()).then_some((summary, description))
}

#[cfg(test)]
mod tests {
    use super::parse_commit_message;

    #[test]
    fn commit_message_summary_and_body() {
        let reply = "feat(files): add changes tab\n\nSplit the working tree into staged\nand unstaged sections.\n";
        assert_eq!(
            parse_commit_message(reply),
            Some((
                "feat(files): add changes tab".into(),
                "Split the working tree into staged\nand unstaged sections.".into()
            ))
        );
    }

    #[test]
    fn commit_message_strips_fences_labels_and_quotes() {
        let reply = "Commit message:\n```\n\"fix: handle detached HEAD\"\n```\n";
        assert_eq!(
            parse_commit_message(reply),
            Some(("fix: handle detached HEAD".into(), String::new()))
        );
        assert_eq!(parse_commit_message("\n```\n```\n"), None);
    }
}
