//! Named workspaces (Tern calls them sessions): each has a root folder and its
//! own tabs. The list, with every tab's pane layout, is kept as JSON in
//! `<data dir>/gentle-shell-gpui/workspaces.json` and restored on launch.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::pane::{PaneKind, PaneTree};

/// One pane as saved: enough to build it again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPane {
    pub kind: PaneKind,
    pub cwd: PathBuf,
    /// The file a file pane shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The conversation an agent pane had, resumed when it still exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTab {
    pub layout: PaneTree<SavedPane>,
    /// Index of the focused pane among the layout's leaves.
    #[serde(default)]
    pub focused: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWorkspace {
    pub name: String,
    pub root: PathBuf,
    #[serde(default)]
    pub tabs: Vec<SavedTab>,
    #[serde(default)]
    pub active_tab: usize,
    /// Unix seconds.
    #[serde(default)]
    pub last_active: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedState {
    #[serde(default)]
    pub version: u32,
    /// Index of the workspace the window showed last.
    #[serde(default)]
    pub current: usize,
    #[serde(default)]
    pub workspaces: Vec<SavedWorkspace>,
    /// The files sidebar (⌘B) was showing.
    #[serde(default = "shown")]
    pub sidebar: bool,
}

fn shown() -> bool {
    true
}

/// A first launch shows the sidebar, as a file from before it was saved does.
impl Default for SavedState {
    fn default() -> Self {
        Self {
            version: 0,
            current: 0,
            workspaces: Vec::new(),
            sidebar: true,
        }
    }
}

pub const VERSION: u32 = 1;

pub fn file_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("gentle-shell-gpui").join("workspaces.json"))
}

/// The saved list; empty when there is none or it can't be read.
pub fn load() -> SavedState {
    file_path()
        .and_then(|path| load_from(&path))
        .unwrap_or_default()
}

pub fn load_from(path: &Path) -> Option<SavedState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(state: &SavedState) -> std::io::Result<()> {
    match file_path() {
        Some(path) => save_to(&path, state),
        None => Ok(()),
    }
}

/// Written to a sibling and renamed over, so a crash mid-write keeps the old list.
pub fn save_to(path: &Path, state: &SavedState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `base`, or `base 2`, `base 3`… when taken.
pub fn unique_name<'a>(taken: impl IntoIterator<Item = &'a str> + Clone, base: &str) -> String {
    let base = base.trim();
    let base = if base.is_empty() { "workspace" } else { base };
    let free = |name: &str| {
        !taken
            .clone()
            .into_iter()
            .any(|t| t.eq_ignore_ascii_case(name))
    };
    if free(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| free(name))
        .expect("an unused name")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        SavedPane, SavedState, SavedTab, SavedWorkspace, VERSION, load_from, save_to, unique_name,
    };
    use crate::pane::{PaneKind, PaneTree, SplitAxis};

    fn sample() -> SavedState {
        let agent = SavedPane {
            kind: PaneKind::Agent,
            cwd: PathBuf::from("/tmp/project"),
            path: None,
            session: Some(PathBuf::from("/tmp/sessions/a.jsonl")),
        };
        let terminal = SavedPane {
            kind: PaneKind::Terminal,
            cwd: PathBuf::from("/tmp/project"),
            path: None,
            session: None,
        };
        let file = SavedPane {
            kind: PaneKind::File,
            cwd: PathBuf::from("/tmp/project"),
            path: Some(PathBuf::from("/tmp/project/README.md")),
            session: None,
        };
        SavedState {
            version: VERSION,
            current: 1,
            workspaces: vec![
                SavedWorkspace {
                    name: "Default".into(),
                    root: PathBuf::from("/tmp"),
                    tabs: vec![SavedTab {
                        layout: PaneTree::Leaf(agent.clone()),
                        focused: 0,
                    }],
                    active_tab: 0,
                    last_active: 10,
                },
                SavedWorkspace {
                    name: "project".into(),
                    root: PathBuf::from("/tmp/project"),
                    tabs: vec![SavedTab {
                        layout: PaneTree::Split {
                            axis: SplitAxis::Horizontal,
                            ratio: 0.4,
                            first: Box::new(PaneTree::Leaf(agent)),
                            second: Box::new(PaneTree::Split {
                                axis: SplitAxis::Vertical,
                                ratio: 0.5,
                                first: Box::new(PaneTree::Leaf(terminal)),
                                second: Box::new(PaneTree::Leaf(file)),
                            }),
                        },
                        focused: 2,
                    }],
                    active_tab: 0,
                    last_active: 20,
                },
            ],
            sidebar: false,
        }
    }

    #[test]
    fn json_round_trip() {
        let state = sample();
        let json = serde_json::to_string(&state).unwrap();
        let back: SavedState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
        // Optional fields stay out of the file and default back in.
        assert!(!json.contains("\"path\":null"));
        let minimal: SavedState =
            serde_json::from_str(r#"{"workspaces":[{"name":"x","root":"/x"}]}"#).unwrap();
        assert_eq!(minimal.workspaces[0].tabs.len(), 0);
        assert_eq!(minimal.current, 0);
        // A file from before the sidebar was saved keeps it showing.
        assert!(minimal.sidebar);
        assert_eq!(SavedState::default().sidebar, minimal.sidebar);
    }

    #[test]
    fn file_round_trip() {
        let dir = std::env::temp_dir().join(format!("gentle-ws-test-{}", std::process::id()));
        let path = dir.join("nested").join("workspaces.json");
        let state = sample();
        save_to(&path, &state).unwrap();
        assert_eq!(load_from(&path), Some(state));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_from(&path), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn names_stay_unique() {
        let taken = ["Default", "app", "app 2"];
        assert_eq!(unique_name(taken, "new"), "new");
        assert_eq!(unique_name(taken, "App"), "App 3");
        assert_eq!(unique_name(taken, "  "), "workspace");
    }
}
