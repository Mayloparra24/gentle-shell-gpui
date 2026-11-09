//! Reading and writing `workspaces.json` under the user's data dir.

use std::path::{Path, PathBuf};

use super::SavedState;

fn file_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("gentle-shell-gpui").join("workspaces.json"))
}

/// The saved list; empty when there is none or it can't be read.
pub fn load() -> SavedState {
    file_path()
        .and_then(|path| load_from(&path))
        .unwrap_or_default()
}

pub(super) fn load_from(path: &Path) -> Option<SavedState> {
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
pub(super) fn save_to(path: &Path, state: &SavedState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}
