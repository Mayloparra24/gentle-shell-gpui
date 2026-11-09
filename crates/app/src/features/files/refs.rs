//! The sidebar's Branches tab, after Tern: a filter over the repository's
//! branches, remotes, tags, stashes, linked worktrees and submodules, with
//! HEAD's branch, upstream and ahead/behind counts pinned at the bottom.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::features::git::{self, RefEntry, RefsSnapshot, StashAction};
use crate::shared::ui;

const POLL: Duration = Duration::from_secs(5);
const ROW_HEIGHT: f32 = 26.;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefsEvent {
    /// HEAD or the refs changed (checkout, stash pop, …).
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Section {
    Branches,
    Remotes,
    Tags,
    Stashes,
    Worktrees,
    Submodules,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Self::Branches => "Branches",
            Self::Remotes => "Remotes",
            Self::Tags => "Tags",
            Self::Stashes => "Stashes",
            Self::Worktrees => "Worktrees",
            Self::Submodules => "Submodules",
        }
    }
}

#[derive(Clone, PartialEq)]
struct Repo {
    toplevel: PathBuf,
    refs: RefsSnapshot,
}

/// One row of the virtualized list, [`ROW_HEIGHT`] tall: a section header,
/// or a ref by its index in the snapshot's list for that section.
#[derive(Clone, Copy)]
enum Line {
    Header {
        section: Section,
        count: usize,
        collapsed: bool,
    },
    Ref(Section, usize),
}

/// Indices of the `items` that pass `hit`.
fn matching<T>(items: &[T], hit: impl Fn(&T) -> bool) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| hit(item))
        .map(|(i, _)| i)
        .collect()
}

pub struct RefsView {
    root: PathBuf,
    /// `None` until the first read, `Some(None)` outside a git work tree.
    repo: Option<Option<Repo>>,
    filter: Entity<InputState>,
    collapsed: HashSet<Section>,
    /// The list's rows, rebuilt when the refs, filter or collapsed sections change.
    lines: Rc<Vec<Line>>,
    scroll: UniformListScrollHandle,
    busy: bool,
    error: Option<String>,
    visible: bool,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<RefsEvent> for RefsView {}

impl RefsView {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter refs"));
        let subscriptions = vec![cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.relayout(cx);
                cx.notify();
            }
        })];
        let mut this = Self {
            root,
            repo: None,
            filter,
            collapsed: HashSet::new(),
            lines: Rc::new(Vec::new()),
            scroll: UniformListScrollHandle::new(),
            busy: false,
            error: None,
            visible: true,
            _poll: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this._poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(due) = this.update_in(cx, |this, window, _| {
                    (this.visible && window.is_window_active()).then(|| this.refs_job())
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

    /// Pause polling while hidden; showing the tab re-reads the refs.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        let shown = visible && !self.visible;
        self.visible = visible;
        if shown {
            self.refresh(cx);
        }
    }

    fn snapshot(&self) -> Option<&Repo> {
        self.repo.as_ref()?.as_ref()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let job = self.refs_job();
        cx.spawn(async move |this, cx| {
            let repo = cx.background_executor().spawn(job).await;
            let _ = this.update(cx, |this, cx| this.apply(repo, cx));
        })
        .detach();
    }

    fn refs_job(&self) -> impl Future<Output = Option<Repo>> + Send + 'static + use<> {
        let root = self.root.clone();
        let known = self.snapshot().map(|repo| repo.toplevel.clone());
        async move {
            let toplevel = known.or_else(|| git::toplevel(&root))?;
            let refs = git::refs_snapshot(&toplevel)?;
            Some(Repo { toplevel, refs })
        }
    }

    fn apply(&mut self, repo: Option<Repo>, cx: &mut Context<Self>) {
        if self.repo.as_ref() == Some(&repo) {
            return;
        }
        let first = self.repo.is_none();
        self.repo = Some(repo);
        self.relayout(cx);
        if !first {
            cx.emit(RefsEvent::Changed);
        }
        cx.notify();
    }

    /// Rebuild the flat row model: each section with a match gets a header
    /// (with its match count) and, unless collapsed, its matching refs.
    fn relayout(&mut self, cx: &App) {
        let mut lines = Vec::new();
        if let Some(repo) = self.snapshot() {
            let query = self.filter.read(cx).value().trim().to_lowercase();
            let hit = |text: &str| query.is_empty() || text.to_lowercase().contains(&query);
            let refs = &repo.refs;
            let sections = [
                (
                    Section::Branches,
                    matching(&refs.branches, |b| hit(&b.name)),
                ),
                (Section::Remotes, matching(&refs.remotes, |r| hit(&r.name))),
                (Section::Tags, matching(&refs.tags, |t| hit(&t.name))),
                (
                    Section::Stashes,
                    matching(&refs.stashes, |s| hit(&s.message) || hit(&s.name)),
                ),
                (
                    Section::Worktrees,
                    matching(&refs.worktrees, |w| {
                        hit(&w.path.to_string_lossy()) || w.branch.as_deref().is_some_and(hit)
                    }),
                ),
                (
                    Section::Submodules,
                    matching(&refs.submodules, |s| hit(&s.path)),
                ),
            ];
            for (section, items) in sections {
                if items.is_empty() {
                    continue;
                }
                let collapsed = self.collapsed.contains(&section);
                lines.push(Line::Header {
                    section,
                    count: items.len(),
                    collapsed,
                });
                if !collapsed {
                    lines.extend(items.into_iter().map(|ix| Line::Ref(section, ix)));
                }
            }
        }
        self.lines = Rc::new(lines);
    }

    /// Run a git operation in the background, then re-read the refs.
    fn git_op(
        &mut self,
        job: impl FnOnce(&Path) -> Result<(), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(toplevel) = self.snapshot().map(|repo| repo.toplevel.clone()) else {
            return;
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (result, repo) = cx
                .background_executor()
                .spawn(async move {
                    let result = job(&toplevel);
                    let repo = git::refs_snapshot(&toplevel).map(|refs| Repo { toplevel, refs });
                    (result, repo)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.error = result.err();
                this.apply(repo, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn checkout(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.snapshot() else {
            return;
        };
        if repo.refs.head.branch.as_deref() == Some(branch.as_str()) {
            return;
        }
        if !repo.refs.head.dirty {
            self.git_op(
                move |top| crate::features::palette::switch_branch(top, &branch),
                cx,
            );
            return;
        }
        let title = format!("Switch to \u{201c}{branch}\u{201d}?");
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, branch) = (this.clone(), branch.clone());
            alert
                .title(title.clone())
                .description(
                    "You have uncommitted changes. They come along to the other branch; \
                     git refuses the switch if they conflict.",
                )
                .ok_text("Switch")
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let branch = branch.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.git_op(
                            move |top| crate::features::palette::switch_branch(top, &branch),
                            cx,
                        )
                    });
                    true
                })
        });
    }

    fn confirm_drop(
        &mut self,
        name: String,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, name) = (this.clone(), name.clone());
            alert
                .title(format!("Drop {name}?"))
                .description(format!(
                    "\u{201c}{message}\u{201d} is deleted. This can't be undone."
                ))
                .ok_text("Drop")
                .ok_variant(ButtonVariant::Danger)
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    let name = name.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.git_op(move |top| git::stash(top, StashAction::Drop, &name), cx)
                    });
                    true
                })
        });
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    fn render_section_header(
        &self,
        section: Section,
        count: usize,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id(SharedString::from(format!("refs-section-{section:?}")))
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
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::muted())
                    .child(section.label()),
            )
            .child(
                div()
                    .font_family(ui::MONO)
                    .text_size(px(10.5))
                    .text_color(ui::faint())
                    .child(count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.collapsed.remove(&section) {
                    this.collapsed.insert(section);
                }
                this.relayout(cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_line(&self, line: Line, cx: &mut Context<Self>) -> AnyElement {
        let (section, ix) = match line {
            Line::Header {
                section,
                count,
                collapsed,
            } => return self.render_section_header(section, count, collapsed, cx),
            Line::Ref(section, ix) => (section, ix),
        };
        let rendered = self.snapshot().and_then(|repo| {
            let refs = &repo.refs;
            Some(match section {
                Section::Branches => self.render_branch(refs.branches.get(ix)?, cx),
                Section::Remotes => self.render_remote(&refs.remotes.get(ix)?.name, cx),
                Section::Tags => {
                    let t = refs.tags.get(ix)?;
                    row(
                        SharedString::from(format!("refs-tag-{}", t.name)),
                        IconName::Tag,
                        ui::muted(),
                        t.name.clone(),
                        ui::fg(),
                        Some(detail(t.subject.clone())),
                    )
                    .into_any_element()
                }
                Section::Stashes => {
                    let s = refs.stashes.get(ix)?;
                    self.render_stash(&s.name, &s.message, cx)
                }
                Section::Worktrees => {
                    let w = refs.worktrees.get(ix)?;
                    let title = match (&w.branch, &w.head) {
                        (Some(branch), _) => branch.clone(),
                        (None, Some(head)) => {
                            format!("detached at {}", &head[..head.len().min(7)])
                        }
                        (None, None) => "bare".to_string(),
                    };
                    let flag = if w.prunable {
                        Some(ui::chip("prunable", ui::warning()))
                    } else if w.locked {
                        Some(ui::chip("locked", ui::muted()))
                    } else {
                        None
                    };
                    let (prefix, leaf) = ui::split_path(&w.path);
                    row(
                        SharedString::from(format!("refs-worktree-{}", w.path.display())),
                        IconName::FolderGit,
                        ui::muted(),
                        title,
                        ui::fg(),
                        Some(
                            h_flex()
                                .min_w_0()
                                .gap_1()
                                .child(detail(format!("{prefix}{leaf}")))
                                .children(flag)
                                .into_any_element(),
                        ),
                    )
                    .into_any_element()
                }
                Section::Submodules => {
                    let s = refs.submodules.get(ix)?;
                    let state = match s.state {
                        '-' => Some(ui::chip("not initialized", ui::muted())),
                        '+' => Some(ui::chip("moved", ui::warning())),
                        'U' => Some(ui::chip("conflict", ui::danger())),
                        _ => None,
                    };
                    row(
                        SharedString::from(format!("refs-submodule-{}", s.path)),
                        IconName::Package,
                        ui::muted(),
                        s.path.clone(),
                        ui::fg(),
                        state.map(IntoElement::into_any_element),
                    )
                    .into_any_element()
                }
            })
        });
        rendered.unwrap_or_else(|| div().h(px(ROW_HEIGHT)).into_any_element())
    }

    fn render_remote(&self, remote: &str, cx: &mut Context<Self>) -> AnyElement {
        let name = remote.to_string();
        let weak = cx.entity().downgrade();
        row(
            SharedString::from(format!("refs-remote-{remote}")),
            IconName::Cloud,
            ui::muted(),
            remote.to_string(),
            ui::fg(),
            None,
        )
        .context_menu(move |menu, _, _| {
            let (weak, name) = (weak.clone(), name.clone());
            menu.item(
                PopupMenuItem::new("Check out as local branch")
                    .icon(Icon::new(IconName::GitBranch))
                    .on_click(move |_, _, cx| {
                        let name = name.clone();
                        let _ = weak.update(cx, |this, cx| {
                            this.git_op(move |top| git::checkout_remote(top, &name), cx)
                        });
                    }),
            )
        })
        .into_any_element()
    }

    fn render_stash(&self, stash: &str, stash_message: &str, cx: &mut Context<Self>) -> AnyElement {
        let (name, message) = (stash.to_string(), stash_message.to_string());
        let weak = cx.entity().downgrade();
        row(
            SharedString::from(format!("refs-stash-{stash}")),
            IconName::Archive,
            ui::muted(),
            stash_message.to_string(),
            ui::fg(),
            Some(
                div()
                    .flex_none()
                    .font_family(ui::MONO)
                    .text_size(px(10.5))
                    .text_color(ui::faint())
                    .child(stash.trim_start_matches("stash").to_string())
                    .into_any_element(),
            ),
        )
        .context_menu(move |menu, _, _| {
            let action = |label: &'static str, icon: IconName, action: StashAction| {
                let (weak, name) = (weak.clone(), name.clone());
                PopupMenuItem::new(label)
                    .icon(Icon::new(icon))
                    .on_click(move |_, _, cx| {
                        let name = name.clone();
                        let _ = weak.update(cx, |this, cx| {
                            this.git_op(move |top| git::stash(top, action, &name), cx)
                        });
                    })
            };
            let drop = {
                let (weak, name, message) = (weak.clone(), name.clone(), message.clone());
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let (name, message) = (name.clone(), message.clone());
                    let _ =
                        weak.update(cx, |this, cx| this.confirm_drop(name, message, window, cx));
                }
            };
            menu.item(action("Apply", IconName::Check, StashAction::Apply))
                .item(action("Pop", IconName::ArrowUp, StashAction::Pop))
                .separator()
                .item(
                    PopupMenuItem::new("Drop…")
                        .icon(Icon::new(IconName::Trash))
                        .on_click(drop),
                )
        })
        .into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.repo {
            None => return note("Reading refs…").into_any_element(),
            Some(None) => return note("Not a git repository").into_any_element(),
            Some(Some(_)) => {}
        }
        if self.lines.is_empty() {
            let value = self.filter.read(cx).value();
            let query = value.trim();
            return if query.is_empty() {
                note("No branches, remotes, tags, stashes, worktrees or submodules")
            } else {
                note(format!("No refs match \u{201c}{query}\u{201d}"))
            }
            .into_any_element();
        }
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "refs-list",
                    self.lines.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        let lines = this.lines.clone();
                        lines[range]
                            .iter()
                            .map(|line| this.render_line(*line, cx))
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

    fn render_branch(&self, branch: &RefEntry, cx: &mut Context<Self>) -> AnyElement {
        let trailing = if branch.gone {
            Some(ui::chip("gone", ui::warning()).into_any_element())
        } else if branch.ahead > 0 || branch.behind > 0 {
            Some(ahead_behind(branch.ahead, branch.behind).into_any_element())
        } else if branch.current {
            Some(
                Icon::new(IconName::Check)
                    .xsmall()
                    .text_color(ui::accent())
                    .into_any_element(),
            )
        } else {
            None
        };
        let name = branch.name.clone();
        let menu_name = branch.name.clone();
        let current = branch.current;
        let weak = cx.entity().downgrade();
        row(
            SharedString::from(format!("refs-branch-{}", branch.name)),
            IconName::GitBranch,
            if current { ui::accent() } else { ui::muted() },
            branch.name.clone(),
            if current {
                ui::fg()
            } else {
                ui::fg().opacity(0.85)
            },
            trailing,
        )
        .when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
        .when(!current, |d| d.cursor_pointer())
        .on_click(cx.listener(move |this, _, window, cx| this.checkout(name.clone(), window, cx)))
        .context_menu(move |menu, _, _| {
            let (weak, name) = (weak.clone(), menu_name.clone());
            menu.item(
                PopupMenuItem::new("Check out")
                    .icon(Icon::new(IconName::GitBranch))
                    .disabled(current)
                    .on_click(move |_, window, cx| {
                        let name = name.clone();
                        let _ = weak.update(cx, |this, cx| this.checkout(name, window, cx));
                    }),
            )
        })
        .into_any_element()
    }

    fn render_footer(&self) -> Option<Div> {
        let head = &self.snapshot()?.refs.head;
        let title = match (&head.branch, &head.sha) {
            (Some(branch), _) => branch.clone(),
            (None, Some(sha)) => format!("Detached at {sha}"),
            (None, None) => "No commits yet".to_string(),
        };
        let upstream = head
            .upstream
            .clone()
            .unwrap_or_else(|| "No upstream".into());
        let commits = format!(
            "{} {}",
            head.commits,
            if head.commits == 1 {
                "commit"
            } else {
                "commits"
            }
        );
        Some(
            v_flex()
                .flex_none()
                .px_3()
                .py_2p5()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::GitBranch)
                                .small()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui::fg())
                                .child(title),
                        )
                        .child(ahead_behind(head.ahead, head.behind)),
                )
                .child(
                    div()
                        .pl(px(22.))
                        .truncate()
                        .text_size(px(12.))
                        .text_color(ui::muted())
                        .child(format!("{upstream} \u{b7} {commits}")),
                ),
        )
    }
}

impl Render for RefsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                div().flex_none().mx_2().mt_1().mb_1().child(
                    ui::field()
                        .h(px(30.))
                        .child(Icon::new(IconName::Funnel).small().text_color(ui::faint()))
                        .child(Input::new(&self.filter).appearance(false).small())
                        .when(self.busy, |d| {
                            d.child(Spinner::new().xsmall().color(ui::accent()))
                        }),
                ),
            )
            .child(self.render_body(cx))
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .id("refs-error")
                        .flex_none()
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .mx_3()
                        .mb_1()
                        .text_size(px(11.5))
                        .text_color(ui::danger())
                        .child(error),
                )
            })
            .children(self.render_footer())
    }
}

/// One ref: icon, name, and an optional trailing detail.
fn row(
    id: SharedString,
    icon: IconName,
    icon_color: Hsla,
    title: String,
    title_color: Hsla,
    trailing: Option<AnyElement>,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(ROW_HEIGHT))
        .mx_1p5()
        .pl(px(24.))
        .pr_1p5()
        .gap_2()
        .rounded_md()
        .hover(|s| s.bg(ui::surface_hover()))
        .child(Icon::new(icon).xsmall().text_color(icon_color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .text_color(title_color)
                .child(title),
        )
        .children(trailing)
}

fn detail(text: String) -> AnyElement {
    div()
        .min_w_0()
        .max_w(px(160.))
        .truncate()
        .text_size(px(11.5))
        .text_color(ui::faint())
        .child(text)
        .into_any_element()
}

/// `↑2 ↓0`, as in the footer and on branches that track an upstream.
fn ahead_behind(ahead: u32, behind: u32) -> Div {
    h_flex()
        .flex_none()
        .gap_1p5()
        .font_family(ui::MONO)
        .text_size(px(11.))
        .child(
            div()
                .text_color(if ahead > 0 { ui::fg() } else { ui::muted() })
                .child(format!("\u{2191}{ahead}")),
        )
        .child(
            div()
                .text_color(if behind > 0 { ui::fg() } else { ui::muted() })
                .child(format!("\u{2193}{behind}")),
        )
}

/// Tern's empty and status text: plain, under the filter. Fills the body
/// so the footer stays at the bottom.
fn note(text: impl Into<SharedString>) -> Div {
    div().flex_1().min_h_0().child(
        div()
            .px_4()
            .py_2()
            .text_size(px(12.5))
            .text_color(ui::faint())
            .child(text.into()),
    )
}
