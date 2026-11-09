//! The Changes tab: the work tree with staged and unstaged sides apart, and
//! what acts on it (stage, unstage, discard, stash, commit, push), plus the
//! last commit message and the staged diff.

use std::path::Path;

use gentle_core::git::status::{EntryKind, StatusEntry, WorkTree, parse_status_v2};
use gentle_core::git::truncate_diff;

use super::{git, run};

/// The work tree of the repository at `toplevel`; `None` outside one.
pub fn work_tree(toplevel: &Path) -> Option<WorkTree> {
    git(
        toplevel,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )
    .map(|raw| parse_status_v2(&raw))
}

fn run_with(cwd: &Path, args: &[&str], paths: &[String]) -> Result<(), String> {
    let mut all: Vec<&str> = args.to_vec();
    all.push("--");
    all.extend(paths.iter().map(String::as_str));
    run(cwd, &all).map(drop)
}

fn has_head(toplevel: &Path) -> bool {
    git(toplevel, &["rev-parse", "--verify", "-q", "HEAD"]).is_some()
}

pub fn stage(toplevel: &Path, paths: &[String]) -> Result<(), String> {
    run_with(toplevel, &["add", "-A"], paths)
}

pub fn unstage(toplevel: &Path, paths: &[String]) -> Result<(), String> {
    if has_head(toplevel) {
        run_with(toplevel, &["restore", "--staged"], paths)
    } else {
        run_with(toplevel, &["rm", "--cached", "-r", "-q"], paths)
    }
}

pub fn stage_all(toplevel: &Path) -> Result<(), String> {
    run(toplevel, &["add", "-A"]).map(drop)
}

pub fn unstage_all(toplevel: &Path) -> Result<(), String> {
    if has_head(toplevel) {
        run(toplevel, &["reset", "-q"]).map(drop)
    } else {
        run(toplevel, &["rm", "--cached", "-r", "-q", "--", "."]).map(drop)
    }
}

/// Throw away one side of an entry. Unstaged: the worktree goes back to the
/// index (untracked files are deleted). Staged: index and worktree go back
/// to HEAD (a staged new file is deleted).
pub fn discard(toplevel: &Path, entry: &StatusEntry, staged: bool) -> Result<(), String> {
    let path = vec![entry.path.clone()];
    if !staged {
        return match entry.kind {
            EntryKind::Untracked => run_with(toplevel, &["clean", "-f", "-q"], &path),
            _ => run_with(toplevel, &["restore", "--worktree"], &path),
        };
    }
    match (entry.index, &entry.orig) {
        ('A' | 'C', _) => run_with(toplevel, &["rm", "-f", "-q"], &path),
        ('R', Some(orig)) => {
            run_with(toplevel, &["rm", "-f", "-q"], &path)?;
            let orig = vec![orig.clone()];
            run_with(
                toplevel,
                &["restore", "--source=HEAD", "--staged", "--worktree"],
                &orig,
            )
        }
        _ => run_with(
            toplevel,
            &["restore", "--source=HEAD", "--staged", "--worktree"],
            &path,
        ),
    }
}

/// Stash every change, untracked files included.
pub fn stash_push(toplevel: &Path, message: &str) -> Result<(), String> {
    let mut args = vec!["stash", "push", "--include-untracked"];
    if !message.trim().is_empty() {
        args.extend(["-m", message.trim()]);
    }
    run(toplevel, &args).map(drop)
}

/// `git commit`; an amend with an empty summary keeps the old message.
pub fn commit(
    toplevel: &Path,
    summary: &str,
    description: &str,
    amend: bool,
) -> Result<(), String> {
    let (summary, description) = (summary.trim(), description.trim());
    let mut args = vec!["commit"];
    if amend {
        args.push("--amend");
    }
    if summary.is_empty() {
        if !amend {
            return Err("Write a summary first".into());
        }
        args.push("--no-edit");
    } else {
        args.extend(["-m", summary]);
        if !description.is_empty() {
            args.extend(["-m", description]);
        }
    }
    run(toplevel, &args).map(drop)
}

/// `git push`, publishing the branch to a remote first when it has no
/// upstream (`origin` if present, else the only/first remote).
pub fn push(toplevel: &Path) -> Result<(), String> {
    if git(toplevel, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_some() {
        return run(toplevel, &["push"]).map(drop);
    }
    let remotes = run(toplevel, &["remote"])?;
    let remotes: Vec<&str> = remotes
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect();
    let remote = remotes
        .iter()
        .find(|r| **r == "origin")
        .or(remotes.first())
        .ok_or("This repository has no remote to push to")?;
    run(toplevel, &["push", "--set-upstream", *remote, "HEAD"]).map(drop)
}

/// The last commit's message as `(summary, description)`.
pub fn last_commit_message(toplevel: &Path) -> Option<(String, String)> {
    let message = git(toplevel, &["log", "-1", "--format=%B"])?;
    let message = message.trim();
    let (summary, description) = message.split_once('\n').unwrap_or((message, ""));
    Some((summary.trim().to_string(), description.trim().to_string()))
}

/// The staged diff with a stat header, cut to `max` bytes on a line break.
pub fn staged_diff(toplevel: &Path, max: usize) -> Result<String, String> {
    let diff = run(
        toplevel,
        &[
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--stat",
            "--patch",
        ],
    )?;
    Ok(truncate_diff(&diff, max))
}
