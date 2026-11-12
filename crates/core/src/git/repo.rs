//! Where a folder sits inside its git repository: the path to show when the
//! folder is nested in a larger one. Paths are compared as text so the same
//! rules (and tests) hold on every operating system.

/// The repository's top level, shortened with `~` under `home`, when `root`
/// is a folder strictly inside it; `None` when `root` is the top level itself
/// or the two do not line up (never a guess).
///
/// `git rev-parse --show-toplevel` prints forward slashes; the folder may
/// come with backslashes or as a `\\?\` path. `ignore_case` is for file
/// systems that do not tell `Repo` from `repo` apart (ASCII letters only: a
/// name that differs in the case of another letter shows no notice).
///
/// Names are plain text: spaces and accents need no special handling.
pub fn nested_repository(
    root: &str,
    toplevel: &str,
    home: Option<&str>,
    ignore_case: bool,
) -> Option<String> {
    let root = normalize(root);
    let top = normalize(toplevel);
    strip_dir(&root, &top, ignore_case)?;
    if let Some(home) = home.map(normalize) {
        if same(&top, &home, ignore_case) {
            return Some("~".to_string());
        }
        if let Some(rest) = strip_dir(&top, &home, ignore_case) {
            return Some(format!("~/{rest}"));
        }
    }
    // `/` and a drive root lose their slash when trimmed; give it back.
    let root_only = top.is_empty() || (top.len() == 2 && top.ends_with(':'));
    Some(if root_only { format!("{top}/") } else { top })
}

/// Forward slashes, no `\\?\` prefix, no trailing slash (`/` becomes empty).
fn normalize(path: &str) -> String {
    // A backslash separates folders only in a Windows path (a drive letter or
    // `\\`); on Unix it is part of a name.
    let windows = path.starts_with("\\\\")
        || matches!(path.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic());
    let path = if windows {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    let path = match path.strip_prefix("//?/UNC/") {
        Some(share) => format!("//{share}"),
        None => path.strip_prefix("//?/").unwrap_or(&path).to_string(),
    };
    path.trim_end_matches('/').to_string()
}

fn same(a: &str, b: &str, ignore_case: bool) -> bool {
    if ignore_case {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// What follows `dir/` in `path`, when `path` is strictly inside `dir`.
fn strip_dir<'a>(path: &'a str, dir: &str, ignore_case: bool) -> Option<&'a str> {
    let head = path.get(..dir.len())?;
    let rest = path[dir.len()..].strip_prefix('/')?;
    (same(head, dir, ignore_case) && !rest.is_empty()).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::nested_repository;

    fn nested(root: &str, top: &str) -> Option<String> {
        nested_repository(root, top, Some("/home/me"), false)
    }

    #[test]
    fn the_top_level_itself_shows_nothing() {
        assert_eq!(nested("/work/repo", "/work/repo"), None);
        assert_eq!(nested("/work/repo/", "/work/repo"), None);
    }

    #[test]
    fn a_nested_folder_names_the_repository() {
        assert_eq!(
            nested("/work/repo/app/ui", "/work/repo"),
            Some("/work/repo".to_string())
        );
        assert_eq!(nested("/a", "/"), Some("/".to_string()));
    }

    #[test]
    fn a_folder_that_only_shares_a_name_prefix_is_not_inside() {
        assert_eq!(nested("/work/repo-two", "/work/repo"), None);
        assert_eq!(nested("/other/repo/sub", "/work/repo"), None);
    }

    #[test]
    fn the_home_folder_is_shortened() {
        assert_eq!(
            nested("/home/me/code/repo/sub", "/home/me/code/repo"),
            Some("~/code/repo".to_string())
        );
        assert_eq!(nested("/home/me/sub", "/home/me"), Some("~".to_string()));
        assert_eq!(
            nested("/home/meow/x/sub", "/home/meow/x"),
            Some("/home/meow/x".to_string())
        );
        assert_eq!(
            nested_repository("/work/repo/sub", "/work/repo", None, false),
            Some("/work/repo".to_string())
        );
    }

    #[test]
    fn windows_paths_compare_across_separators_and_verbatim_prefixes() {
        let home = Some("C:\\Users\\me");
        assert_eq!(
            nested_repository("C:\\Users\\me\\repo\\sub", "C:/Users/me/repo", home, true),
            Some("~/repo".to_string())
        );
        assert_eq!(
            nested_repository(
                "\\\\?\\C:\\Users\\me\\repo\\sub",
                "C:/Users/me/repo",
                None,
                true
            ),
            Some("C:/Users/me/repo".to_string())
        );
        assert_eq!(
            nested_repository("\\\\?\\C:\\Users\\me\\repo", "C:/Users/me/repo", None, true),
            None
        );
    }

    #[test]
    fn letter_case_only_matters_on_case_sensitive_file_systems() {
        let (root, top) = ("c:/users/ME/repo/sub", "C:/Users/me/repo");
        assert_eq!(
            nested_repository(root, top, None, true),
            Some("C:/Users/me/repo".to_string())
        );
        assert_eq!(nested_repository(root, top, None, false), None);
    }

    #[test]
    fn extended_unc_names_the_repository() {
        assert_eq!(
            nested_repository(
                r"\\?\UNC\server\share\repo\sub",
                "//server/share/repo",
                None,
                true
            ),
            Some("//server/share/repo".to_string())
        );
    }

    #[test]
    fn a_drive_root_stays_absolute() {
        assert_eq!(
            nested_repository("C:/sub", "C:/", None, true),
            Some("C:/".to_string())
        );
    }

    #[test]
    fn a_backslash_in_a_unix_name_is_part_of_the_name() {
        assert_eq!(
            nested_repository(r"/tmp/repo\name/sub", r"/tmp/repo\name", None, false),
            Some(r"/tmp/repo\name".to_string())
        );
        assert_eq!(nested(r"/tmp/a\b", "/tmp/a/b"), None);
    }

    #[test]
    fn spaces_and_accents_are_ordinary_characters() {
        assert_eq!(
            nested(
                "/home/me/Mi música/repo ñ/sub dir",
                "/home/me/Mi música/repo ñ"
            ),
            Some("~/Mi música/repo ñ".to_string())
        );
        assert_eq!(
            nested_repository(
                "C:\\Users\\José Pérez\\Documentos\\repo\\sub",
                "C:/Users/José Pérez/Documentos/repo",
                Some("C:\\Users\\José Pérez"),
                true
            ),
            Some("~/Documentos/repo".to_string())
        );
        // A name that only differs in a non-ASCII letter's case is not matched
        // (no notice rather than a wrong one), and never panics mid-character.
        assert_eq!(
            nested_repository("/x/Ñandú/sub", "/x/ñandú", None, true),
            None
        );
        assert_eq!(nested("/x/é", "/x/a"), None);
    }
}
