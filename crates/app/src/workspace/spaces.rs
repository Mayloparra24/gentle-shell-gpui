//! Named workspaces: create, switch, rename, delete, branch switching.

use super::*;

impl Workspace {
    pub(super) fn space(&self) -> &Space {
        &self.spaces[self.current]
    }

    pub(super) fn space_mut(&mut self) -> &mut Space {
        &mut self.spaces[self.current]
    }

    pub(super) fn push_space(&mut self, name: Option<String>, root: PathBuf) -> usize {
        let base = name.unwrap_or_else(|| ui::split_path(&root).1);
        let name = workspaces::unique_name(self.spaces.iter().map(|s| s.name.as_str()), &base);
        self.spaces.push(Space::new(name, root));
        self.spaces.len() - 1
    }

    /// Build a restored workspace's panes; with `fill`, give an empty one an
    /// agent tab.
    pub(super) fn ensure_open(
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

    pub(super) fn switch_workspace(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn create_workspace(
        &mut self,
        name: Option<String>,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = self.push_space(name.filter(|n| !n.trim().is_empty()), root);
        self.switch_workspace(index, window, cx);
    }

    pub(super) fn pick_workspace_folder(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn rename_workspace(&mut self, index: usize, name: String, cx: &mut Context<Self>) {
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

    pub(super) fn confirm_delete_workspace(
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

    pub(super) fn delete_workspace(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn confirm_switch_branch(
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
}
