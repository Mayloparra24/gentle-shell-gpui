//! The file operations behind the tree's context menu. The path helpers are
//! pure; the rest is blocking I/O: call off the UI thread.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// What a paste does with the clipboard entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipMode {
    Copy,
    Cut,
}

/// `path` relative to `root` with `/` separators: `.` for the root itself,
/// the full path for anything outside it.
pub fn relative_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

/// The folder a paste on `target` lands in: the folder itself, or a file's
/// parent. A folder pasted onto itself lands beside itself.
pub fn paste_dir(target: &Path, target_is_dir: bool, source: &Path) -> PathBuf {
    if target_is_dir && target != source {
        target.to_path_buf()
    } else {
        target.parent().unwrap_or(target).to_path_buf()
    }
}

/// `name copy.ext`, then `name copy 2.ext`, … beside `path`: the first name
/// `exists` reports free. Folders and dotfiles (`.env`) keep their whole name
/// as the stem.
pub fn copy_name(path: &Path, is_dir: bool, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new(""));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if !is_dir && dot > 0 => name.split_at(dot),
        _ => (name.as_str(), ""),
    };
    (1u32..)
        .map(|n| match n {
            1 => parent.join(format!("{stem} copy{ext}")),
            n => parent.join(format!("{stem} copy {n}{ext}")),
        })
        .find(|candidate| !exists(candidate))
        .expect("an unused copy name")
}

/// Where pasting `source` into `dir` puts it. A copy never overwrites: a
/// taken name (pasting beside the original included) gets a ` copy` suffix.
/// A move refuses a taken name and a folder moving into itself; moving to
/// where it already is gives `Ok(None)`.
pub fn paste_destination(
    source: &Path,
    source_is_dir: bool,
    dir: &Path,
    mode: ClipMode,
    exists: impl Fn(&Path) -> bool,
) -> Result<Option<PathBuf>, String> {
    let name = source
        .file_name()
        .ok_or_else(|| format!("Can't paste {}", source.display()))?;
    let shown = name.to_string_lossy();
    let dest = dir.join(name);
    if source_is_dir && dir.starts_with(source) {
        return Err(format!("Can't paste “{shown}” into itself"));
    }
    match mode {
        ClipMode::Cut if dest == source => Ok(None),
        ClipMode::Cut if exists(&dest) => Err(format!(
            "“{shown}” already exists in {}",
            dir.file_name()
                .map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into())
        )),
        ClipMode::Cut => Ok(Some(dest)),
        ClipMode::Copy if exists(&dest) => Ok(Some(copy_name(&dest, source_is_dir, exists))),
        ClipMode::Copy => Ok(Some(dest)),
    }
}

/// `name` as a new single entry beside others: not empty, not `.`/`..`, no
/// separators.
pub fn check_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name is required".into());
    }
    if name == "." || name == ".." || name.contains('/') || name.contains(std::path::MAIN_SEPARATOR)
    {
        return Err(format!("“{name}” is not a valid name"));
    }
    Ok(name)
}

/// A new entry typed into `dir`: `a/b/c.rs` creates the folders on the way.
pub fn new_entry_path(dir: &Path, typed: &str) -> Result<PathBuf, String> {
    let typed = typed.trim().trim_end_matches('/');
    if typed.is_empty() {
        return Err("A name is required".into());
    }
    let rel = Path::new(typed);
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!("“{typed}” is not a valid name"));
    }
    Ok(dir.join(rel))
}

/// A new empty file or folder; never replaces an existing entry.
pub fn create(path: &Path, is_dir: bool) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if is_dir {
        fs::create_dir(path)
    } else {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(drop)
    }
}

/// Rename in place, refusing to replace another entry. A case-only rename on
/// a case-insensitive disk is allowed: both names are the same entry.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    if to != from && fs::symlink_metadata(to).is_ok() && !same_entry(from, to) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "an entry with that name already exists",
        ));
    }
    fs::rename(from, to)
}

/// Move `from` to `to` (which must not exist), copying across disks.
pub fn move_entry(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_all(from, to)?;
            remove_all(from)
        }
        other => other,
    }
}

/// Copy a file, or a folder with everything in it, to `to`. Symlinks are
/// copied as links on Unix.
pub fn copy_all(from: &Path, to: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(from)?;
    #[cfg(unix)]
    if meta.file_type().is_symlink() {
        return std::os::unix::fs::symlink(fs::read_link(from)?, to);
    }
    if meta.is_dir() {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(drop)
    }
}

/// Into the platform trash (the macOS Trash, the freedesktop trash, the
/// Recycle Bin).
pub fn trash(path: &Path) -> Result<(), String> {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::new();
    // NSFileManager needs no Finder automation permission (and plays no sound).
    #[cfg(target_os = "macos")]
    trash::macos::TrashContextExtMacos::set_delete_method(
        &mut context,
        trash::macos::DeleteMethod::NsFileManager,
    );
    context.delete(path).map_err(|e| e.to_string())
}

fn remove_all(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(unix)]
fn same_entry(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_entry(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn taken(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |p| set.contains(p)
    }

    #[test]
    fn copy_names_count_up_past_taken_ones() {
        let none = taken(&[]);
        assert_eq!(
            copy_name(Path::new("/p/main.rs"), false, &none),
            PathBuf::from("/p/main copy.rs")
        );
        assert_eq!(
            copy_name(Path::new("/p/a.tar.gz"), false, &none),
            PathBuf::from("/p/a.tar copy.gz")
        );
        assert_eq!(
            copy_name(Path::new("/p/.env"), false, &none),
            PathBuf::from("/p/.env copy")
        );
        assert_eq!(
            copy_name(Path::new("/p/v1.2"), true, &none),
            PathBuf::from("/p/v1.2 copy")
        );
        let some = taken(&["/p/main copy.rs", "/p/main copy 2.rs"]);
        assert_eq!(
            copy_name(Path::new("/p/main.rs"), false, some),
            PathBuf::from("/p/main copy 3.rs")
        );
    }

    #[test]
    fn relative_paths_use_slashes_and_fall_back_to_absolute() {
        let root = Path::new("/repo");
        assert_eq!(relative_path(root, Path::new("/repo")), ".");
        assert_eq!(
            relative_path(root, Path::new("/repo/src/main.rs")),
            "src/main.rs"
        );
        assert_eq!(
            relative_path(root, Path::new("/elsewhere/x")),
            "/elsewhere/x"
        );
        // A sibling sharing the prefix is not inside.
        assert_eq!(
            relative_path(root, Path::new("/repository/x")),
            "/repository/x"
        );
    }

    #[test]
    fn pastes_land_in_the_folder_or_the_files_parent() {
        let elsewhere = Path::new("/x/y");
        assert_eq!(
            paste_dir(Path::new("/r/src"), true, elsewhere),
            PathBuf::from("/r/src")
        );
        assert_eq!(
            paste_dir(Path::new("/r/src/a.rs"), false, elsewhere),
            PathBuf::from("/r/src")
        );
        assert_eq!(
            paste_dir(Path::new("/r/src"), true, Path::new("/r/src")),
            PathBuf::from("/r")
        );

        let free = taken(&[]);
        assert_eq!(
            paste_destination(
                Path::new("/r/a.rs"),
                false,
                Path::new("/r/src"),
                ClipMode::Cut,
                &free
            ),
            Ok(Some(PathBuf::from("/r/src/a.rs")))
        );
        // Moving onto itself does nothing; copying beside itself makes a copy.
        let original = taken(&["/r/a.rs"]);
        assert_eq!(
            paste_destination(
                Path::new("/r/a.rs"),
                false,
                Path::new("/r"),
                ClipMode::Cut,
                &original
            ),
            Ok(None)
        );
        assert_eq!(
            paste_destination(
                Path::new("/r/a.rs"),
                false,
                Path::new("/r"),
                ClipMode::Copy,
                &original
            ),
            Ok(Some(PathBuf::from("/r/a copy.rs")))
        );
        // A move never replaces an entry.
        let clash = taken(&["/r/src/a.rs"]);
        assert!(
            paste_destination(
                Path::new("/r/a.rs"),
                false,
                Path::new("/r/src"),
                ClipMode::Cut,
                clash
            )
            .is_err()
        );
    }

    #[test]
    fn folders_cannot_be_pasted_into_their_own_subtree() {
        let free = taken(&[]);
        for mode in [ClipMode::Cut, ClipMode::Copy] {
            assert!(
                paste_destination(
                    Path::new("/r/src"),
                    true,
                    Path::new("/r/src/deep"),
                    mode,
                    &free
                )
                .is_err()
            );
            assert!(
                paste_destination(Path::new("/r/src"), true, Path::new("/r/src"), mode, &free)
                    .is_err()
            );
        }
        // `/r/srcs` only shares a prefix.
        assert_eq!(
            paste_destination(
                Path::new("/r/src"),
                true,
                Path::new("/r/srcs"),
                ClipMode::Cut,
                &free
            ),
            Ok(Some(PathBuf::from("/r/srcs/src")))
        );
    }

    #[test]
    fn names_are_single_entries_and_new_paths_stay_inside() {
        assert_eq!(check_name("  lib.rs "), Ok("lib.rs"));
        assert!(check_name("").is_err());
        assert!(check_name("..").is_err());
        assert!(check_name("a/b").is_err());
        assert_eq!(
            new_entry_path(Path::new("/r"), "a/b.rs"),
            Ok(PathBuf::from("/r/a/b.rs"))
        );
        assert_eq!(
            new_entry_path(Path::new("/r"), "dir/"),
            Ok(PathBuf::from("/r/dir"))
        );
        assert!(new_entry_path(Path::new("/r"), "../x").is_err());
        assert!(new_entry_path(Path::new("/r"), "/etc/x").is_err());
    }

    #[test]
    fn copies_folders_and_refuses_to_rename_over_entries() {
        let dir = std::env::temp_dir().join(format!("gentle-ops-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src/inner")).unwrap();
        fs::write(dir.join("src/inner/a.rs"), "a").unwrap();
        fs::write(dir.join("b.rs"), "b").unwrap();

        copy_all(&dir.join("src"), &dir.join("src copy")).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("src copy/inner/a.rs")).unwrap(),
            "a"
        );

        assert!(rename(&dir.join("b.rs"), &dir.join("src")).is_err());
        rename(&dir.join("b.rs"), &dir.join("c.rs")).unwrap();
        assert!(dir.join("c.rs").is_file());

        create(&dir.join("new/deep.rs"), false).unwrap();
        assert!(create(&dir.join("new/deep.rs"), false).is_err());
        move_entry(&dir.join("new"), &dir.join("src/new")).unwrap();
        assert!(dir.join("src/new/deep.rs").is_file());
        fs::remove_dir_all(&dir).unwrap();
    }
}
