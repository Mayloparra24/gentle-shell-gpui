//! Git for the right rail and the sidebar's Changes and Branches tabs. Runs
//! `git`; call everything here off the UI thread.
//!
//! The output parsers and their models are `gentle_core::git`. Here, the
//! parent runs `git` and finds the top level; `snapshot` builds the right
//! rail's change list, `changes` reads and acts on the work tree for the
//! Changes tab, and `refs` lists and acts on the Branches tab's refs.

use std::path::{Path, PathBuf};
use std::process::Stdio;

mod changes;
mod refs;
mod snapshot;

pub use changes::{
    commit, discard, last_commit_message, push, stage, stage_all, staged_diff, stash_push, unstage,
    unstage_all, work_tree,
};
pub use gentle_core::git::refs::{RefEntry, RefsSnapshot};
pub use gentle_core::git::status::{EntryKind, GitSnapshot, StatusEntry, WorkTree};
pub use refs::{StashAction, checkout_remote, refs_snapshot, stash};
pub use snapshot::snapshot;

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let mut command = pi_rpc::command("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let out = command.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The repository's top level for any path inside it.
pub fn toplevel(cwd: &Path) -> Option<PathBuf> {
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?;
    let top = top.trim();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

/// `Repository: <path>` when `root` is a folder inside a larger repository
/// (its top level is above it); `None` when `root` is the top level.
pub fn repository_line(root: &Path, toplevel: &Path) -> Option<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let home = dirs::home_dir();
    gentle_core::git::repo::nested_repository(
        &root.to_string_lossy(),
        &toplevel.to_string_lossy(),
        home.as_deref().map(|h| h.to_string_lossy()).as_deref(),
        cfg!(any(windows, target_os = "macos")),
    )
    .map(|path| format!("Repository: {path}"))
}

/// Run `git` for its effect. The error is git's own message; credentials
/// are never prompted for (there is no terminal to answer).
pub fn run(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = pi_rpc::command("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    let out = command.output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let message = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        Err(message.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real git commands against a throwaway repository.
    #[test]
    fn smoke_against_temp_repo() {
        if pi_rpc::command("git").arg("--version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "gentle-git-smoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |args: &[&str]| run(&dir, args).unwrap();
        sh(&["init", "-q", "-b", "main"]);
        sh(&["config", "user.email", "t@example.com"]);
        sh(&["config", "user.name", "T"]);
        sh(&["config", "commit.gpgsign", "false"]);
        let top = toplevel(&dir).unwrap();

        // Before the first commit: no HEAD, nothing to unstage against.
        std::fs::write(dir.join("a file.txt"), "one\n").unwrap();
        let tree = work_tree(&top).unwrap();
        assert_eq!(tree.branch.head.as_deref(), Some("main"));
        assert_eq!(tree.branch.oid, None);
        assert_eq!(tree.entries[0].unstaged(), Some('?'));
        stage(&top, &["a file.txt".into()]).unwrap();
        assert_eq!(work_tree(&top).unwrap().entries[0].staged(), Some('A'));
        unstage_all(&top).unwrap();
        assert_eq!(work_tree(&top).unwrap().entries[0].unstaged(), Some('?'));
        stage_all(&top).unwrap();
        assert!(commit(&top, "", "", false).is_err());
        commit(&top, "feat: first", "Body line.", false).unwrap();
        assert_eq!(
            last_commit_message(&top),
            Some(("feat: first".into(), "Body line.".into()))
        );

        // Staged and unstaged halves of one file, then a rename.
        std::fs::write(dir.join("a file.txt"), "two\n").unwrap();
        stage(&top, &["a file.txt".into()]).unwrap();
        std::fs::write(dir.join("a file.txt"), "three\n").unwrap();
        let entry = work_tree(&top).unwrap().entries.remove(0);
        assert_eq!((entry.staged(), entry.unstaged()), (Some('M'), Some('M')));
        assert!(staged_diff(&top, 10_000).unwrap().contains("+two"));
        discard(&top, &entry, false).unwrap();
        let entry = work_tree(&top).unwrap().entries.remove(0);
        assert_eq!((entry.staged(), entry.unstaged()), (Some('M'), None));
        unstage(&top, &["a file.txt".into()]).unwrap();
        discard(&top, &work_tree(&top).unwrap().entries[0], false).unwrap();
        assert!(work_tree(&top).unwrap().entries.is_empty());
        sh(&["mv", "a file.txt", "b file.txt"]);
        let entry = work_tree(&top).unwrap().entries.remove(0);
        assert_eq!(entry.staged(), Some('R'));
        assert_eq!(entry.orig.as_deref(), Some("a file.txt"));
        discard(&top, &entry, true).unwrap();
        assert!(work_tree(&top).unwrap().entries.is_empty());
        commit(&top, "", "", true).unwrap();

        // Stash, refs and the footer numbers.
        std::fs::write(dir.join("new.txt"), "x\n").unwrap();
        stash_push(&top, "parked").unwrap();
        sh(&["branch", "feature/x"]);
        sh(&["tag", "v0.1"]);
        let refs = refs_snapshot(&top).unwrap();
        assert_eq!(refs.head.branch.as_deref(), Some("main"));
        assert_eq!(refs.head.upstream, None);
        assert_eq!(refs.head.commits, 1);
        assert!(!refs.head.dirty);
        assert_eq!(refs.branches.len(), 2);
        assert!(refs.branches.iter().any(|b| b.current && b.name == "main"));
        assert_eq!(refs.tags[0].name, "v0.1");
        assert_eq!(refs.stashes[0].message, "parked");
        assert!(refs.worktrees.is_empty());
        stash(&top, StashAction::Pop, &refs.stashes[0].name).unwrap();
        assert_eq!(work_tree(&top).unwrap().entries[0].path, "new.txt");
        assert!(push(&top).unwrap_err().contains("no remote"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
