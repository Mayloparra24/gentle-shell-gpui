//! One directory level of the project tree, honoring `.gitignore` unless
//! ignored entries are asked for.
//! Blocking I/O: call off the UI thread.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Dotfiles, drawn dimmed.
    pub hidden: bool,
    /// Matched by an ignore file; only listed when asked for, drawn dimmed.
    pub ignored: bool,
}

/// The direct children of `dir`: folders first, then files, each
/// case-insensitively by name. Entries ignored by `.gitignore` (including
/// ignore files in parent directories), `.git/info/exclude`, the global git
/// excludes and `.ignore` are left out unless `show_ignored`, which lists
/// them marked [`Entry::ignored`]. The `.git` folder never is. Hidden files
/// are listed.
pub fn list_dir(dir: &Path, show_ignored: bool) -> Vec<Entry> {
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        // Honor `.gitignore` even before `git init`.
        .require_git(false)
        .follow_links(false)
        .build();
    // Symlinks to folders expand like folders.
    let kept = walker
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .map(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir())
                || (e.path_is_symlink() && e.path().is_dir());
            (e.into_path(), is_dir)
        });
    let mut entries: Vec<Entry> = if show_ignored {
        let kept: std::collections::HashSet<PathBuf> = kept.map(|(path, _)| path).collect();
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|e| {
                let path = e.path();
                let is_dir = e
                    .file_type()
                    .is_ok_and(|t| t.is_dir() || (t.is_symlink() && path.is_dir()));
                let ignored = !kept.contains(&path);
                entry(path, is_dir, ignored)
            })
            .collect()
    } else {
        kept.filter_map(|(path, is_dir)| entry(path, is_dir, false))
            .collect()
    };
    entries.sort_by(compare);
    entries
}

fn entry(path: PathBuf, is_dir: bool, ignored: bool) -> Option<Entry> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if name == ".git" {
        return None;
    }
    Some(Entry {
        hidden: name.starts_with('.'),
        path,
        name,
        is_dir,
        ignored,
    })
}

fn compare(a: &Entry, b: &Entry) -> Ordering {
    b.is_dir
        .cmp(&a.is_dir)
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}

#[cfg(test)]
mod tests {
    use super::list_dir;
    use std::fs;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gentle-files-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &std::path::Path) -> Vec<(String, bool, bool)> {
        list_dir(dir, false)
            .into_iter()
            .map(|e| (e.name, e.is_dir, e.hidden))
            .collect()
    }

    #[test]
    fn folders_first_then_case_insensitive_names() {
        let dir = temp_dir("order");
        for f in ["b.rs", "A.md", "c.toml", ".env"] {
            fs::write(dir.join(f), "x").unwrap();
        }
        for d in ["zeta", "Alpha", ".github"] {
            fs::create_dir(dir.join(d)).unwrap();
        }
        assert_eq!(
            names(&dir),
            vec![
                (".github".into(), true, true),
                ("Alpha".into(), true, false),
                ("zeta".into(), true, false),
                (".env".into(), false, true),
                ("A.md".into(), false, false),
                ("b.rs".into(), false, false),
                ("c.toml".into(), false, false),
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gitignore_applies_here_and_in_subfolders() {
        let dir = temp_dir("ignore");
        fs::write(dir.join(".gitignore"), "target/\n*.log\n/build\n").unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::create_dir_all(dir.join("target/debug")).unwrap();
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::create_dir_all(dir.join("src/build")).unwrap();
        fs::write(dir.join("debug.log"), "x").unwrap();
        fs::write(dir.join("main.rs"), "x").unwrap();
        fs::write(dir.join("src/lib.rs"), "x").unwrap();
        fs::write(dir.join("src/trace.log"), "x").unwrap();

        assert_eq!(
            names(&dir),
            vec![
                ("src".into(), true, false),
                (".gitignore".into(), false, true),
                ("main.rs".into(), false, false),
            ]
        );
        // A parent's `.gitignore` still applies when listing a subfolder lazily;
        // the anchored `/build` only matches at the root.
        assert_eq!(
            names(&dir.join("src")),
            vec![
                ("build".into(), true, false),
                ("lib.rs".into(), false, false)
            ]
        );
        // Asked for, ignored entries come back marked; `.git` never does.
        let all: Vec<(String, bool)> = list_dir(&dir, true)
            .into_iter()
            .map(|e| (e.name, e.ignored))
            .collect();
        assert_eq!(
            all,
            vec![
                ("build".into(), true),
                ("src".into(), false),
                ("target".into(), true),
                (".gitignore".into(), false),
                ("debug.log".into(), true),
                ("main.rs".into(), false),
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
