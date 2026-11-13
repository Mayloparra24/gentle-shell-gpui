//! Gentle Shell slash commands whose UI only exists in the terminal: over RPC
//! `ctx.ui.custom()` returns nothing, so they either show nothing or warn
//! "requires TUI mode". The app routes the ones it has a native screen for and
//! hides the rest from the palette and the composer's `/` menu.

/// A native screen a slash command opens instead of reaching the backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Usage, Subscription tab.
    Usage,
    /// Usage, Local stats tab.
    Stats,
    /// The session's Agents window, on its Agents tab.
    Agents,
    /// The session's Agents window, on its Jobs tab (background jobs).
    Jobs,
    Profiles,
    Palette,
    /// Files sidebar › Changes tab.
    Changes,
}

/// The native screen for a typed or picked slash command. `text` is what
/// follows the `/` (a leading `/` is tolerated): the name plus any arguments.
/// Arguments matter only for `gentle:profiles`, whose subcommands (`export`,
/// `import`, …) work over RPC and so are not routed.
pub fn route(text: &str) -> Option<Screen> {
    let text = text.trim().trim_start_matches('/');
    let (name, args) = match text.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (text, ""),
    };
    Some(match name {
        "gentle:usage" => Screen::Usage,
        "gentle:stats" => Screen::Stats,
        "gentle:agents" => Screen::Agents,
        "gentle:jobs" => Screen::Jobs,
        "gentle:profiles" if args.is_empty() => Screen::Profiles,
        "gentle:commands" => Screen::Palette,
        "gentle:changes" => Screen::Changes,
        _ => return None,
    })
}

/// Commands that do nothing useful over RPC and have no native screen yet:
/// hidden from the palette and the composer's `/` menu. `name` may carry a
/// leading `/`; arguments are ignored.
pub fn hidden(name: &str) -> bool {
    let name = name.trim().trim_start_matches('/');
    let name = name.split_whitespace().next().unwrap_or_default();
    matches!(
        name,
        "gentle:customize"
            | "gentle:guide"
            | "gentle:voice"
            | "gentle:music"
            | "gentle:panel"
            | "history"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_terminal_only_commands_to_native_screens() {
        assert_eq!(route("gentle:usage"), Some(Screen::Usage));
        assert_eq!(route("gentle:stats"), Some(Screen::Stats));
        assert_eq!(route("gentle:agents"), Some(Screen::Agents));
        assert_eq!(route("gentle:jobs"), Some(Screen::Jobs));
        assert_eq!(route("gentle:profiles"), Some(Screen::Profiles));
        assert_eq!(route("gentle:commands"), Some(Screen::Palette));
        assert_eq!(route("gentle:changes"), Some(Screen::Changes));
    }

    #[test]
    fn tolerates_leading_slash_and_whitespace() {
        assert_eq!(route("/gentle:usage"), Some(Screen::Usage));
        assert_eq!(route("  /gentle:changes  "), Some(Screen::Changes));
        assert_eq!(route("/gentle:profiles   "), Some(Screen::Profiles));
        assert!(hidden("/gentle:guide"));
        assert!(hidden(" history "));
    }

    #[test]
    fn profiles_with_arguments_reach_the_backend() {
        assert_eq!(route("gentle:profiles export work"), None);
        assert_eq!(route("/gentle:profiles clear"), None);
        assert_eq!(route("gentle:profiles\texport"), None);
    }

    #[test]
    fn other_routes_ignore_arguments() {
        assert_eq!(route("gentle:usage now"), Some(Screen::Usage));
        assert_eq!(route("gentle:jobs list"), Some(Screen::Jobs));
    }

    #[test]
    fn everything_else_is_not_routed() {
        for text in [
            "",
            "/",
            "gentle:doctor",
            "gentle:usages",
            "usage",
            "compact",
            "skill:x",
        ] {
            assert_eq!(route(text), None, "{text}");
        }
    }

    #[test]
    fn hides_dead_commands_only() {
        for name in [
            "gentle:customize",
            "gentle:guide",
            "gentle:voice",
            "gentle:music",
            "gentle:music next",
            "gentle:panel",
            "history",
        ] {
            assert!(hidden(name), "{name}");
        }
        for name in [
            "gentle:usage",
            "gentle:doctor",
            "gentle:history",
            "historyx",
            "",
        ] {
            assert!(!hidden(name), "{name}");
        }
    }

    #[test]
    fn routed_and_hidden_never_overlap() {
        for name in [
            "gentle:usage",
            "gentle:stats",
            "gentle:agents",
            "gentle:jobs",
            "gentle:profiles",
            "gentle:commands",
            "gentle:changes",
        ] {
            assert!(!hidden(name), "{name}");
        }
        for name in [
            "gentle:customize",
            "gentle:guide",
            "gentle:voice",
            "gentle:music",
            "gentle:panel",
            "history",
        ] {
            assert_eq!(route(name), None, "{name}");
        }
    }
}
