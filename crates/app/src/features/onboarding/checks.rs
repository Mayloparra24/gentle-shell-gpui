//! Reading the checks: readiness, the install plan, row tone and the
//! install log's tail. No GPUI here.

use std::collections::VecDeque;

use crate::features::setup::{Check, CheckId, Fix, Need, Status};

/// Install output kept per run (the failure view shows the tail).
const LOG_KEEP: usize = 200;

pub(super) fn is_ok(check: &Check) -> bool {
    matches!(check.status, Status::Ok(_))
}

/// Every Required check passes.
pub(super) fn required_ok(checks: &[Check]) -> bool {
    checks
        .iter()
        .filter(|c| c.need == Need::Required)
        .all(is_ok)
}

/// Everything but the optional extras passes.
pub(super) fn all_set(checks: &[Check]) -> bool {
    checks
        .iter()
        .filter(|c| c.need != Need::Optional)
        .all(is_ok)
}

/// gentle-shell can start: the launcher and what it runs on are in place
/// (setup and sign-in can still be pending; the launcher handles those).
pub(super) fn usable(checks: &[Check]) -> bool {
    checks
        .iter()
        .filter(|c| matches!(c.id, CheckId::Node | CheckId::Go | CheckId::GentleShell))
        .all(is_ok)
}

/// What "Install everything" runs, in screen order: every Required or
/// Recommended check an unattended install fixes that isn't done.
pub(super) fn install_plan(checks: &[Check]) -> VecDeque<CheckId> {
    checks
        .iter()
        .filter(|c| c.need != Need::Optional && c.fix == Fix::Auto && !is_ok(c))
        .map(|c| c.id)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Good,
    Warn,
    Bad,
}

/// Green when done; red for a missing requirement or a failure; amber for
/// anything outdated or missing that isn't required.
pub(super) fn tone(check: &Check) -> Tone {
    match &check.status {
        Status::Ok(_) => Tone::Good,
        Status::Failed(_) => Tone::Bad,
        Status::Missing(_) if check.need == Need::Required => Tone::Bad,
        Status::Missing(_) | Status::Outdated(_) => Tone::Warn,
    }
}

pub(super) fn status_text(status: &Status) -> &str {
    match status {
        Status::Ok(s) | Status::Missing(s) | Status::Outdated(s) | Status::Failed(s) => s,
    }
}

/// Append install output, one entry per non-empty line, keeping the tail.
pub(super) fn push_lines(log: &mut Vec<String>, chunk: &str) {
    log.extend(
        chunk
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string),
    );
    if log.len() > LOG_KEEP {
        log.drain(..log.len() - LOG_KEEP);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{
        Check, CheckId, Fix, LOG_KEEP, Need, Status, Tone, all_set, install_plan, push_lines,
        required_ok, tone, usable,
    };

    fn check(id: CheckId, need: Need, status: Status, fix: Fix) -> Check {
        Check {
            id,
            title: "t",
            summary: "s",
            need,
            status,
            fix,
        }
    }

    fn ok(id: CheckId, need: Need) -> Check {
        check(id, need, Status::Ok("1".into()), Fix::None)
    }

    #[test]
    fn plan_runs_needed_unattended_fixes_in_order() {
        let checks = vec![
            ok(CheckId::Node, Need::Required),
            check(
                CheckId::GentleShell,
                Need::Required,
                Status::Missing("not found".into()),
                Fix::Auto,
            ),
            check(
                CheckId::Setup,
                Need::Required,
                Status::Failed("exit 1".into()),
                Fix::Auto,
            ),
            check(
                CheckId::Provider,
                Need::Required,
                Status::Missing("none".into()),
                Fix::SignIn,
            ),
            check(
                CheckId::Git,
                Need::Recommended,
                Status::Missing("none".into()),
                Fix::Terminal("sudo apt install git".into()),
            ),
            check(
                CheckId::Engram,
                Need::Optional,
                Status::Missing("none".into()),
                Fix::Auto,
            ),
        ];
        assert_eq!(
            install_plan(&checks),
            VecDeque::from([CheckId::GentleShell, CheckId::Setup])
        );
    }

    #[test]
    fn readiness_ignores_extras() {
        let mut checks = vec![
            ok(CheckId::Node, Need::Required),
            ok(CheckId::GentleShell, Need::Required),
            check(
                CheckId::Engram,
                Need::Optional,
                Status::Missing("none".into()),
                Fix::Auto,
            ),
        ];
        assert!(required_ok(&checks));
        assert!(all_set(&checks));
        assert!(usable(&checks));
        checks.push(check(
            CheckId::Git,
            Need::Recommended,
            Status::Outdated("2.0".into()),
            Fix::None,
        ));
        assert!(required_ok(&checks));
        assert!(!all_set(&checks));
        checks.push(check(
            CheckId::Provider,
            Need::Required,
            Status::Missing("none".into()),
            Fix::SignIn,
        ));
        assert!(!required_ok(&checks));
        // No sign-in yet, but gentle-shell itself can start.
        assert!(usable(&checks));
    }

    #[test]
    fn tones_follow_need_and_status() {
        let missing = |need| check(CheckId::Git, need, Status::Missing("x".into()), Fix::None);
        assert_eq!(tone(&missing(Need::Required)), Tone::Bad);
        assert_eq!(tone(&missing(Need::Recommended)), Tone::Warn);
        assert_eq!(tone(&missing(Need::Optional)), Tone::Warn);
        assert_eq!(
            tone(&check(
                CheckId::Node,
                Need::Required,
                Status::Outdated("v20".into()),
                Fix::Auto
            )),
            Tone::Warn
        );
        assert_eq!(
            tone(&check(
                CheckId::Engram,
                Need::Optional,
                Status::Failed("x".into()),
                Fix::Auto
            )),
            Tone::Bad
        );
        assert_eq!(tone(&ok(CheckId::Node, Need::Required)), Tone::Good);
    }

    #[test]
    fn log_keeps_non_empty_lines_and_the_tail() {
        let mut log = Vec::new();
        push_lines(&mut log, "a\n\n  \nb  \r\nc");
        assert_eq!(log, ["a", "b", "c"]);
        for i in 0..LOG_KEEP {
            push_lines(&mut log, &i.to_string());
        }
        assert_eq!(log.len(), LOG_KEEP);
        assert_eq!(log.last().map(String::as_str), Some("199"));
    }
}
