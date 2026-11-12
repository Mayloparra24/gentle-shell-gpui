//! Git output: parsers for what the app's `git` commands print and the models
//! of the right rail and the sidebar's Changes and Branches tabs. The app
//! runs `git`; this module only reads its output. `status` covers porcelain
//! v1/v2 and numstat, `refs` branches, remotes, tags, stashes, worktrees and
//! submodules.

pub mod refs;
pub mod repo;
pub mod status;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_diff_cuts_on_line_break() {
        let diff = "line one\nline two\nline three\n";
        assert_eq!(truncate_diff(diff, 100), diff);
        let cut = truncate_diff(diff, 12);
        assert!(cut.starts_with("line one\n[diff truncated: "));
        // Never splits a multi-byte character.
        assert!(truncate_diff("ééééé\n", 3).starts_with("é"));
    }
}
