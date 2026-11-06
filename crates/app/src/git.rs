//! Git for the right rail and the sidebar's Changes and Branches tabs. Runs
//! `git`; call everything here off the UI thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// Porcelain XY code collapsed to one letter: M, A, D, R, ? (untracked).
    pub status: char,
    /// `None` for binary files.
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSnapshot {
    pub branch: Option<String>,
    pub files: Vec<FileChange>,
}

/// `None` when `cwd` is not inside a git work tree.
pub fn snapshot(cwd: &Path) -> Option<GitSnapshot> {
    let status = git(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    // `symbolic-ref` also names the branch of a repository without commits;
    // it fails only on a detached HEAD.
    let branch = git(cwd, &["symbolic-ref", "--short", "-q", "HEAD"])
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty());
    // Staged + unstaged against HEAD; a repository without commits has no HEAD.
    let numstat = git(cwd, &["diff", "--numstat", "-z", "HEAD"])
        .or_else(|| git(cwd, &["diff", "--numstat", "-z"]))
        .unwrap_or_default();
    let mut files = parse_status(&status);
    let stats = parse_numstat(&numstat);
    for file in &mut files {
        if let Some(&(added, removed)) = stats.get(&file.path) {
            (file.added, file.removed) = (added, removed);
        } else if file.status == '?' {
            file.added = count_lines(&cwd.join(&file.path));
            file.removed = Some(0);
        }
    }
    Some(GitSnapshot { branch, files })
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let out = command.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `git status --porcelain=v1 -z`. Renames carry their source as an
/// extra NUL-separated field, which is skipped.
pub fn parse_status(raw: &str) -> Vec<FileChange> {
    let mut files = Vec::new();
    let mut fields = raw.split('\0').filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        let code: Vec<char> = code.chars().take(2).collect();
        let status = match (code[0], code[1]) {
            ('?', _) => '?',
            ('R', _) | (_, 'R') => {
                fields.next();
                'R'
            }
            ('A', _) => 'A',
            ('D', _) | (_, 'D') => 'D',
            _ => 'M',
        };
        files.push(FileChange {
            path: path.to_string(),
            status,
            added: None,
            removed: None,
        });
    }
    files
}

/// Parse `git diff --numstat -z`: `added\tremoved\tpath\0`, or for renames
/// `added\tremoved\t\0old\0new\0`. Binary files report `-`.
pub fn parse_numstat(raw: &str) -> BTreeMap<String, (Option<u64>, Option<u64>)> {
    let mut map = BTreeMap::new();
    let mut fields = raw.split('\0');
    while let Some(entry) = fields.next() {
        let mut parts = entry.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            fields.next();
            fields.next().unwrap_or_default()
        } else {
            path
        };
        map.insert(path.to_string(), (added.parse().ok(), removed.parse().ok()));
    }
    map
}

fn count_lines(path: &Path) -> Option<u64> {
    const LIMIT: u64 = 4 * 1024 * 1024;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > LIMIT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    Some(newlines + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n")))
}

// ── Working tree: staged and unstaged ──────────────────────────────────────

/// `git status --porcelain=v2 --branch`: the branch line and every entry
/// with its staged (index) and unstaged (worktree) columns apart.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkTree {
    pub branch: BranchStatus,
    pub entries: Vec<StatusEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchStatus {
    /// `None` before the first commit.
    pub oid: Option<String>,
    /// `None` on a detached HEAD.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Tracked,
    /// Unmerged: both sides changed during a merge, rebase or cherry-pick.
    Conflict,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Relative to the repository's top level.
    pub path: String,
    /// The source of a staged rename or copy.
    pub orig: Option<String>,
    /// Porcelain `X` (index) and `Y` (worktree) codes; `.` is unchanged.
    pub index: char,
    pub worktree: char,
    pub kind: EntryKind,
}

impl StatusEntry {
    /// The Staged section's badge (A, M, D, R); `None` when nothing is staged.
    pub fn staged(&self) -> Option<char> {
        match self.kind {
            EntryKind::Tracked => badge(self.index),
            EntryKind::Conflict | EntryKind::Untracked => None,
        }
    }

    /// The Unstaged section's badge: A, M, D, `?` untracked or `U` conflict.
    pub fn unstaged(&self) -> Option<char> {
        match self.kind {
            EntryKind::Tracked => badge(self.worktree),
            EntryKind::Conflict => Some('U'),
            EntryKind::Untracked => Some('?'),
        }
    }
}

fn badge(code: char) -> Option<char> {
    match code {
        '.' | ' ' => None,
        'A' | 'C' => Some('A'),
        'D' => Some('D'),
        'R' => Some('R'),
        _ => Some('M'),
    }
}

/// Parse `git status --porcelain=v2 --branch -z`. Renames carry their source
/// as the next NUL-separated field; paths may contain spaces.
pub fn parse_status_v2(raw: &str) -> WorkTree {
    let mut tree = WorkTree::default();
    let mut fields = raw.split('\0');
    while let Some(record) = fields.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            let branch = &mut tree.branch;
            match key {
                "branch.oid" => branch.oid = (value != "(initial)").then(|| value.to_string()),
                "branch.head" => branch.head = (value != "(detached)").then(|| value.to_string()),
                "branch.upstream" => branch.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            branch.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            branch.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some((tag, rest)) = record.split_at_checked(2) else {
            continue;
        };
        // Fields before the path: XY plus 6 (ordinary), 7 (rename), 8 (unmerged).
        let (before, kind) = match tag {
            "1 " => (7, EntryKind::Tracked),
            "2 " => (8, EntryKind::Tracked),
            "u " => (9, EntryKind::Conflict),
            "? " => {
                tree.entries.push(StatusEntry {
                    path: rest.to_string(),
                    orig: None,
                    index: '?',
                    worktree: '?',
                    kind: EntryKind::Untracked,
                });
                continue;
            }
            _ => continue,
        };
        let parts: Vec<&str> = rest.splitn(before + 1, ' ').collect();
        let (Some(xy), Some(path)) = (parts.first(), parts.get(before)) else {
            continue;
        };
        let mut xy = xy.chars();
        let (index, worktree) = (xy.next().unwrap_or('.'), xy.next().unwrap_or('.'));
        let orig = (tag == "2 ")
            .then(|| fields.next().map(str::to_string))
            .flatten();
        tree.entries.push(StatusEntry {
            path: path.to_string(),
            orig,
            index,
            worktree,
            kind,
        });
    }
    tree
}

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

/// The repository's top level for any path inside it.
pub fn toplevel(cwd: &Path) -> Option<PathBuf> {
    let top = git(cwd, &["rev-parse", "--show-toplevel"])?;
    let top = top.trim();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

/// Run `git` for its effect. The error is git's own message; credentials
/// are never prompted for (there is no terminal to answer).
pub fn run(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
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

pub fn truncate_diff(diff: &str, max: usize) -> String {
    if diff.len() <= max {
        return diff.to_string();
    }
    let mut end = max;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    let cut = diff[..end].rfind('\n').map_or(end, |i| i + 1);
    format!(
        "{}[diff truncated: {} more bytes]\n",
        &diff[..cut],
        diff.len() - cut
    )
}

// ── Refs: branches, remotes, tags, stashes, worktrees, submodules ─────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Branch,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEntry {
    pub kind: RefKind,
    /// Short name: `main`, `origin/main`, `v1.0`.
    pub name: String,
    /// The checked-out branch.
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// The upstream was deleted on the remote.
    pub gone: bool,
    pub sha: String,
    pub subject: String,
}

/// The `git for-each-ref --format` [`parse_refs`] reads.
pub const REF_FORMAT: &str = "--format=%(HEAD)%00%(refname)%00%(upstream:short)%00%(upstream:track)%00%(objectname:short)%00%(subject)";

/// Parse `git for-each-ref` with [`REF_FORMAT`]. A remote's symbolic `HEAD`
/// (`origin/HEAD`) is skipped.
pub fn parse_refs(raw: &str) -> Vec<RefEntry> {
    raw.lines()
        .filter_map(|line| {
            let mut f = line.split('\0');
            let head = f.next()?;
            let refname = f.next()?;
            let upstream = f.next().unwrap_or_default();
            let track = f.next().unwrap_or_default();
            let sha = f.next().unwrap_or_default();
            let subject = f.next().unwrap_or_default();
            let (kind, name) = if let Some(name) = refname.strip_prefix("refs/heads/") {
                (RefKind::Branch, name)
            } else if let Some(name) = refname.strip_prefix("refs/remotes/") {
                if name.ends_with("/HEAD") {
                    return None;
                }
                (RefKind::Remote, name)
            } else {
                (RefKind::Tag, refname.strip_prefix("refs/tags/")?)
            };
            let (ahead, behind, gone) = parse_track(track);
            Some(RefEntry {
                kind,
                name: name.to_string(),
                current: head == "*",
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                ahead,
                behind,
                gone,
                sha: sha.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect()
}

/// `%(upstream:track)`: `[ahead 2, behind 1]`, `[behind 3]`, `[gone]` or empty.
fn parse_track(track: &str) -> (u32, u32, bool) {
    let inner = track.trim().trim_start_matches('[').trim_end_matches(']');
    let (mut ahead, mut behind) = (0, 0);
    for part in inner.split(',').map(str::trim) {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    (ahead, behind, inner == "gone")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// `stash@{0}`: what apply/pop/drop take.
    pub name: String,
    /// The branch it was made on, when git recorded one.
    pub branch: Option<String>,
    pub message: String,
}

/// Parse `git stash list --format=%gd%x00%gs`: `stash@{0}\0On main: msg`
/// or `stash@{1}\0WIP on main: abc1234 subject`.
pub fn parse_stashes(raw: &str) -> Vec<Stash> {
    raw.lines()
        .filter_map(|line| {
            let (name, subject) = line.split_once('\0')?;
            let (branch, message) = subject
                .strip_prefix("WIP on ")
                .or_else(|| subject.strip_prefix("On "))
                .and_then(|rest| rest.split_once(": "))
                .map_or((None, subject), |(b, m)| (Some(b.to_string()), m));
            Some(Stash {
                name: name.to_string(),
                branch,
                message: message.to_string(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Option<String>,
    /// Short branch name; `None` when detached or bare.
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Parse `git worktree list --porcelain`: blank-line separated blocks of
/// `worktree <path>`, `HEAD <sha>`, `branch <ref>`/`detached`, flags.
pub fn parse_worktrees(raw: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    for line in raw.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "worktree" {
            out.push(Worktree {
                path: PathBuf::from(value),
                ..Worktree::default()
            });
            continue;
        }
        let Some(tree) = out.last_mut() else {
            continue;
        };
        match key {
            "HEAD" => tree.head = Some(value.to_string()),
            "branch" => {
                tree.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_string(),
                )
            }
            "detached" => tree.detached = true,
            "bare" => tree.bare = true,
            "locked" => tree.locked = true,
            "prunable" => tree.prunable = true,
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub path: String,
    pub sha: String,
    /// ` ` in sync, `-` not initialized, `+` checked out at another commit,
    /// `U` merge conflicts.
    pub state: char,
    pub describe: Option<String>,
}

/// Parse `git submodule status`: `<state><sha> <path>[ (<describe>)]`.
pub fn parse_submodules(raw: &str) -> Vec<Submodule> {
    raw.lines()
        .filter_map(|line| {
            let mut chars = line.chars();
            let state = chars.next()?;
            let (sha, rest) = chars.as_str().split_once(' ')?;
            let (path, describe) = match rest.strip_suffix(')').and_then(|r| r.rsplit_once(" (")) {
                Some((path, describe)) => (path, Some(describe.to_string())),
                None => (rest, None),
            };
            Some(Submodule {
                path: path.to_string(),
                sha: sha.to_string(),
                state,
                describe,
            })
        })
        .collect()
}

/// Parse `git rev-list --left-right --count A...B`: `(left only, right only)`.
pub fn parse_left_right(raw: &str) -> Option<(u32, u32)> {
    let mut parts = raw.split_whitespace();
    let left = parts.next()?.parse().ok()?;
    let right = parts.next()?.parse().ok()?;
    Some((left, right))
}

/// HEAD as the Branches footer shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadInfo {
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Short commit id; `None` before the first commit.
    pub sha: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub commits: u64,
    /// Tracked files have uncommitted changes (a checkout may refuse).
    pub dirty: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefsSnapshot {
    pub head: HeadInfo,
    pub branches: Vec<RefEntry>,
    pub remotes: Vec<RefEntry>,
    pub tags: Vec<RefEntry>,
    pub stashes: Vec<Stash>,
    /// Linked worktrees; the main one is left out.
    pub worktrees: Vec<Worktree>,
    pub submodules: Vec<Submodule>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes_and_renames() {
        let raw = " M src/a.rs\0R  new.rs\0old.rs\0?? notes.md\0 D gone.txt\0A  added.rs\0";
        let files = parse_status(raw);
        let summary: Vec<(char, &str)> =
            files.iter().map(|f| (f.status, f.path.as_str())).collect();
        assert_eq!(
            summary,
            vec![
                ('M', "src/a.rs"),
                ('R', "new.rs"),
                ('?', "notes.md"),
                ('D', "gone.txt"),
                ('A', "added.rs")
            ]
        );
    }

    #[test]
    fn numstat_handles_binary_and_renames() {
        let raw = "3\t1\tsrc/a.rs\0-\t-\tlogo.png\0\x32\t0\t\0old.rs\0new.rs\0";
        let map = parse_numstat(raw);
        assert_eq!(map["src/a.rs"], (Some(3), Some(1)));
        assert_eq!(map["logo.png"], (None, None));
        assert_eq!(map["new.rs"], (Some(2), Some(0)));
        assert!(!map.contains_key("old.rs"));
    }

    #[test]
    fn status_v2_branch_columns_renames_and_spaces() {
        let raw = concat!(
            "# branch.oid 1f2e3d4c5b6a79881f2e3d4c5b6a79881f2e3d4c\0",
            "# branch.head main\0",
            "# branch.upstream origin/main\0",
            "# branch.ab +2 -1\0",
            "1 .M N... 100644 100644 100644 aaaa bbbb src/main.rs\0",
            "1 MM N... 100644 100644 100644 aaaa bbbb docs/read me.md\0",
            "1 A. N... 000000 100644 100644 0000 cccc new file.txt\0",
            "1 .D N... 100644 100644 000000 aaaa aaaa gone.rs\0",
            "2 R. N... 100644 100644 100644 aaaa aaaa R100 src/new name.rs\0src/old name.rs\0",
            "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc conflict.rs\0",
            "? notes/todo list.md\0",
        );
        let tree = parse_status_v2(raw);
        assert_eq!(
            tree.branch,
            BranchStatus {
                oid: Some("1f2e3d4c5b6a79881f2e3d4c5b6a79881f2e3d4c".into()),
                head: Some("main".into()),
                upstream: Some("origin/main".into()),
                ahead: 2,
                behind: 1,
            }
        );
        let sides: Vec<(&str, Option<char>, Option<char>)> = tree
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.staged(), e.unstaged()))
            .collect();
        assert_eq!(
            sides,
            vec![
                ("src/main.rs", None, Some('M')),
                ("docs/read me.md", Some('M'), Some('M')),
                ("new file.txt", Some('A'), None),
                ("gone.rs", None, Some('D')),
                ("src/new name.rs", Some('R'), None),
                ("conflict.rs", None, Some('U')),
                ("notes/todo list.md", None, Some('?')),
            ]
        );
        assert_eq!(tree.entries[4].orig.as_deref(), Some("src/old name.rs"));
        assert_eq!(tree.entries[5].kind, EntryKind::Conflict);
    }

    #[test]
    fn status_v2_detached_initial_and_no_upstream() {
        let tree = parse_status_v2("# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!(tree.branch, BranchStatus::default());
        assert!(tree.entries.is_empty());
        let tree = parse_status_v2("# branch.oid abc\0# branch.head feature/x\0");
        assert_eq!(tree.branch.head.as_deref(), Some("feature/x"));
        assert_eq!(tree.branch.upstream, None);
        assert_eq!((tree.branch.ahead, tree.branch.behind), (0, 0));
    }

    #[test]
    fn refs_kinds_tracking_and_remote_head() {
        let raw = concat!(
            "*\0refs/heads/main\0origin/main\0[ahead 2, behind 1]\0abc1234\0feat: add x\n",
            " \0refs/heads/feature/old\0origin/feature/old\0[gone]\0def5678\0wip: thing\n",
            " \0refs/heads/local only\0\0\0beef123\0chore: y\n",
            " \0refs/remotes/origin/HEAD\0\0\0abc1234\0feat: add x\n",
            " \0refs/remotes/origin/main\0\0\0abc1234\0feat: add x\n",
            " \0refs/tags/v1.0.0\0\0\0fff0000\0Release 1.0.0\n",
        );
        let refs = parse_refs(raw);
        assert_eq!(refs.len(), 5);
        let main = &refs[0];
        assert!(main.current && main.kind == RefKind::Branch);
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind, main.gone), (2, 1, false));
        assert!(refs[1].gone && !refs[1].current);
        assert_eq!(refs[1].name, "feature/old");
        assert_eq!(refs[2].name, "local only");
        assert_eq!(
            (refs[2].upstream.clone(), refs[2].sha.as_str()),
            (None, "beef123")
        );
        assert_eq!(
            (refs[3].kind, refs[3].name.as_str()),
            (RefKind::Remote, "origin/main")
        );
        assert_eq!(
            (refs[4].kind, refs[4].subject.as_str()),
            (RefKind::Tag, "Release 1.0.0")
        );
    }

    #[test]
    fn stash_list_messages_and_branches() {
        let raw = "stash@{0}\0On main: half-done refactor\nstash@{1}\0WIP on feature/x: abc1234 feat: thing\nstash@{2}\0autostash\n";
        let stashes = parse_stashes(raw);
        assert_eq!(stashes.len(), 3);
        assert_eq!(stashes[0].name, "stash@{0}");
        assert_eq!(stashes[0].branch.as_deref(), Some("main"));
        assert_eq!(stashes[0].message, "half-done refactor");
        assert_eq!(stashes[1].branch.as_deref(), Some("feature/x"));
        assert_eq!(stashes[1].message, "abc1234 feat: thing");
        assert_eq!(
            (stashes[2].branch.clone(), stashes[2].message.as_str()),
            (None, "autostash")
        );
    }

    #[test]
    fn worktree_porcelain_blocks() {
        let raw = concat!(
            "worktree /Users/me/repo\nHEAD 1111111111111111111111111111111111111111\nbranch refs/heads/main\n\n",
            "worktree /Users/me/repo wt/feature\nHEAD 2222222222222222222222222222222222222222\nbranch refs/heads/feature/a\nlocked\n\n",
            "worktree /tmp/detached\nHEAD 3333333333333333333333333333333333333333\ndetached\nprunable gitdir file points to non-existent location\n\n",
        );
        let trees = parse_worktrees(raw);
        assert_eq!(trees.len(), 3);
        assert_eq!(trees[0].branch.as_deref(), Some("main"));
        assert_eq!(trees[1].path, PathBuf::from("/Users/me/repo wt/feature"));
        assert_eq!(trees[1].branch.as_deref(), Some("feature/a"));
        assert!(trees[1].locked);
        assert!(trees[2].detached && trees[2].prunable && trees[2].branch.is_none());
    }

    #[test]
    fn submodule_status_lines() {
        let raw = " 1234567890abcdef1234567890abcdef12345678 vendor/lib (v1.2.0)\n-abcdefabcdefabcdefabcdefabcdefabcdefabcd third party/tool\n+fedcbafedcbafedcbafedcbafedcbafedcbafedc docs (heads/main)\n";
        let subs = parse_submodules(raw);
        assert_eq!(subs.len(), 3);
        assert_eq!((subs[0].state, subs[0].path.as_str()), (' ', "vendor/lib"));
        assert_eq!(subs[0].describe.as_deref(), Some("v1.2.0"));
        assert_eq!(
            (
                subs[1].state,
                subs[1].path.as_str(),
                subs[1].describe.clone()
            ),
            ('-', "third party/tool", None)
        );
        assert_eq!(subs[2].state, '+');
    }

    #[test]
    fn left_right_counts() {
        assert_eq!(parse_left_right("3\t5\n"), Some((3, 5)));
        assert_eq!(parse_left_right("0\t0"), Some((0, 0)));
        assert_eq!(parse_left_right(""), None);
    }

    #[test]
    fn truncate_diff_cuts_on_line_break() {
        let diff = "line one\nline two\nline three\n";
        assert_eq!(truncate_diff(diff, 100), diff);
        let cut = truncate_diff(diff, 12);
        assert!(cut.starts_with("line one\n[diff truncated: "));
        // Never splits a multi-byte character.
        assert!(truncate_diff("ééééé\n", 3).starts_with("é"));
    }

    /// The real git commands against a throwaway repository.
    #[test]
    fn smoke_against_temp_repo() {
        if Command::new("git").arg("--version").output().is_err() {
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
