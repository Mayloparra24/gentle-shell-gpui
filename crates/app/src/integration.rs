//! Registers the terminal and file-viewer panes with the tiling layer, and
//! wires the file sidebar to the workspace.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use crate::files::FileView;
use crate::pane::{PaneFactories, PaneItem, PaneKind};
use crate::terminal::TerminalView;

/// The git work tree containing `path` (FileView::diff runs git there).
pub fn repo_root(path: &std::path::Path) -> PathBuf {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    pi_rpc::command("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or_else(|| dir.to_path_buf())
}

pub fn register(cx: &mut App) {
    crate::files::init(cx);
    let terminal: crate::pane::PaneFactory =
        Rc::new(|cwd: PathBuf, window: &mut Window, cx: &mut App| {
            let view = cx.new(|cx| TerminalView::new(cwd, window, cx));
            let focus = view.focus_handle(cx);
            let title_view = view.clone();
            PaneItem {
                kind: PaneKind::Terminal,
                view: view.into(),
                focus,
                title: Rc::new(move |cx: &App| title_view.read(cx).title()),
                unsaved: None,
            }
        });
    let file: crate::pane::PaneFactory =
        Rc::new(|path: PathBuf, window: &mut Window, cx: &mut App| {
            let view = cx.new(|cx| FileView::open(path, window, cx));
            let focus = view.focus_handle(cx);
            let title_view = view.clone();
            PaneItem {
                kind: PaneKind::File,
                view: view.into(),
                focus,
                unsaved: {
                    let view = title_view.clone();
                    Some(Rc::new(move |cx: &App| view.read(cx).has_unsaved()))
                },
                title: Rc::new(move |cx: &App| title_view.read(cx).title()),
            }
        });
    let diff: crate::pane::PaneFactory =
        Rc::new(|path: PathBuf, window: &mut Window, cx: &mut App| {
            let root = repo_root(&path);
            let view = cx.new(|cx| FileView::diff(root, path, window, cx));
            let focus = view.focus_handle(cx);
            let title_view = view.clone();
            PaneItem {
                kind: PaneKind::File,
                view: view.into(),
                focus,
                title: Rc::new(move |cx: &App| title_view.read(cx).title()),
                unsaved: None,
            }
        });
    cx.set_global(PaneFactories {
        terminal: Some(terminal),
        file: Some(file),
        diff: Some(diff),
    });
}

/// The Files | Changes | Branches sidebar for `workspace`, re-rooted when the shown
/// workspace changes. The subscriptions live as long as the window.
pub fn attach_sidebar(
    workspace: &Entity<crate::workspace::Workspace>,
    window: &mut Window,
    cx: &mut App,
) {
    use crate::files::{FileTreeEvent, FileTreeView};
    use crate::workspace::WorkspaceEvent;

    fn mount(
        workspace: &Entity<crate::workspace::Workspace>,
        root: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) {
        let tree = cx.new(|cx| FileTreeView::new(root, window, cx));
        let shown = workspace.read(cx).sidebar_visible();
        tree.update(cx, |tree, cx| tree.set_visible(shown, cx));
        let target = workspace.downgrade();
        window
            .subscribe(&tree, cx, move |_, event: &FileTreeEvent, window, cx| {
                let _ = target.update(cx, |workspace, cx| match event {
                    FileTreeEvent::Open(path) => workspace.open_file(path.clone(), window, cx),
                    FileTreeEvent::OpenToSide(path) => {
                        workspace.open_file_beside(path.clone(), window, cx)
                    }
                    FileTreeEvent::OpenInNewTab(path) => {
                        workspace.open_file_in_tab(path.clone(), window, cx)
                    }
                    FileTreeEvent::OpenDiff(path) => workspace.open_diff(path.clone(), window, cx),
                    FileTreeEvent::InsertPath(text) => {
                        if !workspace.insert_into_composer(text, window, cx) {
                            window.push_notification(
                                Notification::info(
                                    "Open an agent to insert paths into its prompt.",
                                ),
                                cx,
                            );
                        }
                    }
                    FileTreeEvent::Collapse => workspace.toggle_sidebar(cx),
                });
            })
            .detach();
        let visible = tree.downgrade();
        window
            .subscribe(workspace, cx, move |_, event: &WorkspaceEvent, _, cx| {
                if let WorkspaceEvent::SidebarToggled(shown) = event {
                    let _ = visible.update(cx, |tree, cx| tree.set_visible(*shown, cx));
                }
            })
            .detach();
        workspace.update(cx, |workspace, cx| workspace.set_sidebar(tree.into(), cx));
    }

    let root = workspace.read(cx).root().to_path_buf();
    mount(workspace, root, window, cx);
    let owner = workspace.clone();
    window
        .subscribe(
            workspace,
            cx,
            move |_, event: &WorkspaceEvent, window, cx| match event {
                WorkspaceEvent::RootChanged(root) => mount(&owner, root.clone(), window, cx),
                WorkspaceEvent::SidebarToggled(_) => {}
            },
        )
        .detach();
}
