//! What submitting composer text does before it would reach Pi as a prompt:
//! Pi's own built-in commands (terminal-only over RPC, so the app runs them
//! through RPC calls), Gentle Shell commands the app shows natively, and the
//! window that gathers an extension command's `notify` output.

use std::time::{Duration, Instant};

use crate::commands::Screen;
use crate::transcript::NoticeLevel;

/// Pi built-ins the app runs itself, for the `/` menu and the palette.
pub(super) const BUILTINS: &[(&str, &str)] = &[
    (
        "compact",
        "Compact the conversation context (optional: what to keep)",
    ),
    ("name", "Rename this session: /name <title>"),
    ("copy", "Copy the last answer to the clipboard"),
    ("export", "Export the conversation as an HTML file"),
    ("reload", "Restart Gentle Shell, keeping the conversation"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Builtin {
    /// `/compact [instructions]`.
    Compact(Option<String>),
    /// `/name <title>`; empty shows the current name.
    Name(String),
    Copy,
    /// `/export [path]`; no path asks with a save dialog.
    Export(Option<String>),
    Reload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Submit {
    Builtin(Builtin),
    /// A Gentle Shell command whose UI is terminal-only: open the app's screen.
    Open(Screen),
    /// Send to Pi as typed.
    Prompt,
}

/// `/name args` → (`name`, `args` trimmed); `None` unless `text` is a slash command.
fn split_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim().strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    (!name.is_empty()).then_some((name, args.trim()))
}

pub(super) fn parse_builtin(text: &str) -> Option<Builtin> {
    let (name, args) = split_command(text)?;
    let args_opt = (!args.is_empty()).then(|| args.to_string());
    Some(match name {
        "compact" => Builtin::Compact(args_opt),
        "name" => Builtin::Name(args.to_string()),
        "copy" => Builtin::Copy,
        "export" => Builtin::Export(args_opt),
        "reload" => Builtin::Reload,
        _ => return None,
    })
}

/// Built-ins first (as in Pi's terminal), then natively shown commands,
/// then everything else goes out as typed.
pub(super) fn classify(text: &str) -> Submit {
    if let Some(builtin) = parse_builtin(text) {
        return Submit::Builtin(builtin);
    }
    match text
        .trim()
        .strip_prefix('/')
        .and_then(crate::commands::route)
    {
        Some(screen) => Submit::Open(screen),
        None => Submit::Prompt,
    }
}

/// How long after Pi answers a command its notices still count as its output
/// (extensions often notify right after their handler returns).
pub(super) const OUTPUT_GRACE: Duration = Duration::from_millis(1500);

/// The `notify` output of one extension command, gathered from sending it
/// until [`OUTPUT_GRACE`] after Pi reported it `handled`.
#[derive(Debug)]
pub(super) struct Capture {
    /// Matches Pi's reply to the request that started it.
    pub id: u64,
    /// The command as typed, the result card's title.
    pub command: String,
    pub notices: Vec<(NoticeLevel, String)>,
    replied: Option<Instant>,
}

impl Capture {
    pub(super) fn new(id: u64, command: &str) -> Self {
        Self {
            id,
            command: command.trim().to_string(),
            notices: Vec::new(),
            replied: None,
        }
    }

    /// Whether a notice arriving at `now` belongs to this command.
    fn accepts(&self, now: Instant) -> bool {
        self.replied
            .is_none_or(|at| now.saturating_duration_since(at) < OUTPUT_GRACE)
    }

    /// Keep `message` if it arrived inside the window; false otherwise.
    pub(super) fn offer(&mut self, level: NoticeLevel, message: &str, now: Instant) -> bool {
        if !self.accepts(now) {
            return false;
        }
        self.notices.push((level, message.to_string()));
        true
    }

    /// Pi answered the command at `now`: the grace window starts.
    pub(super) fn replied(&mut self, now: Instant) {
        self.replied.get_or_insert(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_name_and_arguments() {
        assert_eq!(
            parse_builtin("/name foo bar"),
            Some(Builtin::Name("foo bar".into()))
        );
        assert_eq!(parse_builtin("/name"), Some(Builtin::Name(String::new())));
        assert_eq!(parse_builtin("/compact"), Some(Builtin::Compact(None)));
        assert_eq!(
            parse_builtin("  /compact   keep the API notes "),
            Some(Builtin::Compact(Some("keep the API notes".into())))
        );
        assert_eq!(
            parse_builtin("/compact\nkeep the API notes"),
            Some(Builtin::Compact(Some("keep the API notes".into())))
        );
        assert_eq!(parse_builtin("/copy"), Some(Builtin::Copy));
        assert_eq!(parse_builtin("/export"), Some(Builtin::Export(None)));
        assert_eq!(
            parse_builtin("/export ~/a.html"),
            Some(Builtin::Export(Some("~/a.html".into())))
        );
        assert_eq!(parse_builtin("/reload"), Some(Builtin::Reload));
        // Not built-ins: other names, prefixes of them, plain text.
        assert_eq!(parse_builtin("/compacting"), None);
        assert_eq!(parse_builtin("/gentle:doctor"), None);
        assert_eq!(parse_builtin("compact"), None);
        assert_eq!(parse_builtin("/"), None);
    }

    #[test]
    fn submits_try_builtins_then_native_screens_then_pi() {
        assert_eq!(
            classify("/compact"),
            Submit::Builtin(Builtin::Compact(None))
        );
        assert_eq!(classify("/gentle:usage"), Submit::Open(Screen::Usage));
        assert_eq!(classify("/gentle:profiles"), Submit::Open(Screen::Profiles));
        // With arguments the profiles command works over RPC.
        assert_eq!(classify("/gentle:profiles export mine"), Submit::Prompt);
        assert_eq!(classify("/gentle:doctor"), Submit::Prompt);
        assert_eq!(classify("/unknown-thing"), Submit::Prompt);
        assert_eq!(classify("hello /compact"), Submit::Prompt);
    }

    #[test]
    fn capture_keeps_notices_until_the_grace_after_the_reply() {
        let t0 = Instant::now();
        let mut capture = Capture::new(1, " /gentle:doctor ");
        assert_eq!(capture.command, "/gentle:doctor");
        // Before the reply, however long it takes.
        assert!(capture.offer(NoticeLevel::Info, "a", t0 + Duration::from_secs(30)));
        let replied = t0 + Duration::from_secs(31);
        capture.replied(replied);
        // A second reply does not extend the window.
        capture.replied(replied + Duration::from_secs(1));
        assert!(capture.offer(
            NoticeLevel::Warning,
            "b",
            replied + OUTPUT_GRACE - Duration::from_millis(1)
        ));
        assert!(!capture.offer(NoticeLevel::Info, "late", replied + OUTPUT_GRACE));
        assert_eq!(
            capture.notices,
            vec![
                (NoticeLevel::Info, "a".to_string()),
                (NoticeLevel::Warning, "b".to_string())
            ]
        );
    }
}
