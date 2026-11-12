//! Working-tree status: porcelain v1 and numstat for the right rail's change
//! list, porcelain v2 with staged and unstaged columns for the Changes tab.

use std::collections::BTreeMap;

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

/// Parse `git status --porcelain=v1 -z`. Renames and copies carry their
/// source as an extra NUL-separated field, which is skipped.
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
            // A copy's destination is a new file, as in porcelain v2.
            ('C', _) | (_, 'C') => {
                fields.next();
                'A'
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
    fn copy_source_is_not_an_extra_change() {
        let files = parse_status("C  copied.txt\0source.txt\0M  source.txt\0");
        let summary: Vec<(char, &str)> =
            files.iter().map(|f| (f.status, f.path.as_str())).collect();
        assert_eq!(summary, vec![('A', "copied.txt"), ('M', "source.txt")]);
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
}
