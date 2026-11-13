//! The Branches tab: every ref, stash, worktree and submodule with HEAD's
//! footer numbers, and the stash and remote-checkout actions.

use std::path::{Path, PathBuf};

use gentle_core::git::refs::{
    HeadInfo, REF_FORMAT, RefKind, RefsSnapshot, parse_left_right, parse_refs, parse_stashes,
    parse_submodules, parse_worktrees, worktree_path,
};
use gentle_core::git::status::parse_status_v2;

use super::{git, run};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StashAction {
    Apply,
    Pop,
    Drop,
}

pub fn stash(toplevel: &Path, action: StashAction, name: &str) -> Result<(), String> {
    let verb = match action {
        StashAction::Apply => "apply",
        StashAction::Pop => "pop",
        StashAction::Drop => "drop",
    };
    run(toplevel, &["stash", verb, "-q", name]).map(drop)
}

/// Everything the Branches tab lists; `None` outside a repository.
pub fn refs_snapshot(toplevel: &Path) -> Option<RefsSnapshot> {
    let status = git(
        toplevel,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=no",
        ],
    )?;
    let status = parse_status_v2(&status);
    let refs = git(
        toplevel,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            REF_FORMAT,
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )
    .map(|raw| parse_refs(&raw))
    .unwrap_or_default();
    let (mut branches, mut remotes, mut tags) = (Vec::new(), Vec::new(), Vec::new());
    for entry in refs {
        match entry.kind {
            RefKind::Branch => branches.push(entry),
            RefKind::Remote => remotes.push(entry),
            RefKind::Tag => tags.push(entry),
        }
    }
    let (ahead, behind) = status
        .branch
        .upstream
        .as_ref()
        .and_then(|_| {
            git(
                toplevel,
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
        })
        .and_then(|raw| parse_left_right(&raw))
        .unwrap_or((0, 0));
    let commits = status
        .branch
        .oid
        .as_ref()
        .and_then(|_| git(toplevel, &["rev-list", "--count", "HEAD"]))
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(0);
    let head = HeadInfo {
        sha: status
            .branch
            .oid
            .as_ref()
            .map(|oid| oid.chars().take(7).collect()),
        branch: status.branch.head,
        upstream: status.branch.upstream,
        ahead,
        behind,
        commits,
        dirty: !status.entries.is_empty(),
    };
    let stashes = git(toplevel, &["stash", "list", "--format=%gd%x00%gs"])
        .map(|raw| parse_stashes(&raw))
        .unwrap_or_default();
    let worktrees = git(toplevel, &["worktree", "list", "--porcelain"])
        .map(|raw| parse_worktrees(&raw).into_iter().skip(1).collect())
        .unwrap_or_default();
    let submodules = if toplevel.join(".gitmodules").exists() {
        git(toplevel, &["submodule", "status"])
            .map(|raw| parse_submodules(&raw))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(RefsSnapshot {
        head,
        branches,
        remotes,
        tags,
        stashes,
        worktrees,
        submodules,
    })
}

/// Check out a remote branch as a new local branch tracking it.
pub fn checkout_remote(toplevel: &Path, remote_branch: &str) -> Result<(), String> {
    run(toplevel, &["switch", "--track", remote_branch]).map(drop)
}

/// A linked worktree for `branch`, beside the main one
/// ([`worktree_path`]); `new` creates the branch there from HEAD. Returns the
/// new worktree's folder.
pub fn add_worktree(toplevel: &Path, branch: &str, new: bool) -> Result<PathBuf, String> {
    // From a linked worktree too, new ones go beside the main worktree
    // (`git worktree list` names it first).
    let main = git(toplevel, &["worktree", "list", "--porcelain"])
        .and_then(|raw| parse_worktrees(&raw).into_iter().next())
        .map_or_else(|| toplevel.to_path_buf(), |main| main.path);
    let path = worktree_path(&main, branch, |p| p.exists());
    let folder = path.to_string_lossy();
    let args: Vec<&str> = if new {
        vec!["worktree", "add", "-b", branch, "--", &folder]
    } else {
        vec!["worktree", "add", "--", &folder, branch]
    };
    run(toplevel, &args)?;
    Ok(path)
}

/// Remove the linked worktree at `path`: its folder goes, its branch stays.
/// git refuses while it has changes of its own.
pub fn remove_worktree(toplevel: &Path, path: &Path) -> Result<(), String> {
    run(
        toplevel,
        &["worktree", "remove", "--", &path.to_string_lossy()],
    )
    .map(drop)
}

/// Forget worktrees whose folders are gone.
pub fn prune_worktrees(toplevel: &Path) -> Result<(), String> {
    run(toplevel, &["worktree", "prune"]).map(drop)
}
