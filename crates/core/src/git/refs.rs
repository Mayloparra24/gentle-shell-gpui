//! Refs for the Branches tab: branches, remotes, tags, stashes, worktrees and
//! submodules, ahead/behind counts, and the HEAD footer and snapshot models.

use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
