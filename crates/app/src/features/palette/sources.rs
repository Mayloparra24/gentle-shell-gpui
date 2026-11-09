//! What the switcher reads from disk: files under the workspace root and
//! local git branches; plus `git switch` for the branch rows.

use std::path::Path;
use std::process::{Command, Stdio};

/// Files under `root`, relative, honoring `.gitignore`; at most `cap`.
pub(super) fn walk_files(root: &Path, cap: usize) -> Vec<String> {
    ignore::WalkBuilder::new(root)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|entry| {
            entry
                .path()
                .strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .take(cap)
        .collect()
}

fn git_command(root: &Path) -> Command {
    let mut command = pi_rpc::command("git");
    command
        .arg("-C")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Local branches, `(name, is_current)`; empty outside a repository.
pub(super) fn list_branches(root: &Path) -> Vec<(String, bool)> {
    let Ok(out) = git_command(root)
        .args(["branch", "--format=%(HEAD)%(refname:short)"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_branches(&String::from_utf8_lossy(&out.stdout))
}

fn parse_branches(raw: &str) -> Vec<(String, bool)> {
    raw.lines()
        .filter_map(|line| {
            let (head, name) = line.split_at_checked(1)?;
            let name = name.trim();
            (!name.is_empty() && !name.starts_with('(')).then(|| (name.to_string(), head == "*"))
        })
        .collect()
}

/// `git switch <branch>` in `root`; the error is git's own message.
pub fn switch_branch(root: &Path, branch: &str) -> Result<(), String> {
    let out = git_command(root)
        .args(["switch", branch])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_branches;

    #[test]
    fn branches_parse_with_the_current_one_marked() {
        let raw = "*main\n feature/x\n (HEAD detached at 1234)\n\n";
        assert_eq!(
            parse_branches(raw),
            vec![("main".to_string(), true), ("feature/x".to_string(), false)]
        );
    }
}
