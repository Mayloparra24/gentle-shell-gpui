//! First-run onboarding engine: checks every dependency Gentle Shell needs on
//! this OS and installs what's missing without sudo. Anything that needs a
//! password or an interactive login is handed to the user as a terminal
//! command (`Fix::Terminal`).
//!
//! Toolchains it installs live in `<data dir>/gentle-shell-gpui/toolchain/`
//! (`node/`, `npm-global/`, `bin/`); `apply_toolchain_path` puts them first
//! on pi-rpc's search PATH, so sessions find them on every launch.

mod extras;
mod fetch;
mod gentle;
mod probe;

use std::path::{Path, PathBuf};

use serde_json::json;

pub use probe::platform_label;
use probe::{Version, capture, parse_version};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckId {
    Node,
    Go,
    GentleShell,
    Setup,
    Provider,
    Git,
    Engram,
    ClaudeBridge,
    ClaudeLogin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Required,
    Recommended,
    Optional,
}

/// The text says what was found, or why not.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Ok(String),
    Missing(String),
    Outdated(String),
    Failed(String),
}

/// `Auto`: `install` does it unattended. `Terminal`: needs the user (sudo or
/// an interactive login); run the command in a terminal. `SignIn`: Providers.
#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    None,
    Auto,
    Terminal(String),
    SignIn,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub id: CheckId,
    pub title: &'static str,
    pub summary: &'static str,
    pub need: Need,
    pub status: Status,
    pub fix: Fix,
}

/// Progress and tool output, one line at a time.
type Log<'a> = dyn Fn(String) + Send + Sync + 'a;

/// gentle-pi `engines.node`.
const MIN_NODE: Version = (22, 19, 0);
/// gentle-pi's Windows postinstall builds Gentle AI with this Go.
const MIN_GO: Version = (1, 25, 10);
/// The launcher refuses an older Pi.
const MIN_PI: Version = (1, 2, 0);
/// gentle-engram accepts a stable Engram core from 3.0.0; with an older
/// server it can't verify the server's identity and memory stays offline.
const MIN_ENGRAM: Version = (3, 0, 0);

// ── Toolchain folders and PATH ─────────────────────────────────────────────

fn user_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(std::env::temp_dir)
}

/// `<data dir>/gentle-shell-gpui/toolchain`.
fn toolchain_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("gentle-shell-gpui").join("toolchain"))
}

/// Where an npm prefix (or a Node install) keeps its executables.
fn prefix_bin(prefix: &Path, windows: bool) -> PathBuf {
    if windows {
        prefix.to_path_buf()
    } else {
        prefix.join("bin")
    }
}

/// The app-owned toolchain folders that exist: managed Node, the user npm
/// prefix, and loose binaries (Engram).
fn managed_dirs() -> Vec<PathBuf> {
    let Some(toolchain) = toolchain_dir() else {
        return Vec::new();
    };
    let windows = cfg!(windows);
    [
        prefix_bin(&toolchain.join("node"), windows),
        prefix_bin(&toolchain.join("npm-global"), windows),
        toolchain.join("bin"),
    ]
    .into_iter()
    .filter(|dir| dir.is_dir())
    .collect()
}

/// Put the app's toolchain folders first on pi-rpc's search PATH (and, on
/// Windows, the install folders of tools winget added since this app
/// started, when they aren't found otherwise). Cheap: called at startup and
/// before every check run.
pub fn apply_toolchain_path() {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut dirs = managed_dirs();
    #[cfg(windows)]
    {
        // The managed folders first, so fallbacks only fill real gaps.
        pi_rpc::set_extra_path(dirs.clone());
        for (program, dir) in extras::windows_fallback_dirs() {
            if dir.is_dir() && !dirs.contains(&dir) && pi_rpc::find_program(program).is_none() {
                dirs.push(dir);
            }
        }
    }
    pi_rpc::set_extra_path(dirs);
}

/// After an install: new folders on the PATH, and pi-rpc's cached login
/// PATH and `--version` answers dropped.
fn refresh_paths() {
    pi_rpc::forget_cached();
    apply_toolchain_path();
}

// ── Checks ─────────────────────────────────────────────────────────────────

fn meta(id: CheckId) -> (&'static str, &'static str, Need) {
    match id {
        CheckId::Node => (
            "Node.js",
            "Runs Gentle Shell and its packages (22.19 or newer, with npm).",
            Need::Required,
        ),
        CheckId::Go => (
            "Go",
            "Builds the Gentle AI companion on Windows (1.25.10 or newer).",
            Need::Required,
        ),
        CheckId::GentleShell => (
            "Gentle Shell",
            "The gentle-pi launcher and the Pi agent it bundles.",
            Need::Required,
        ),
        CheckId::Setup => (
            "Companion setup",
            "Gentle AI's packages, skills and memory in your Gentle Shell home.",
            Need::Required,
        ),
        CheckId::Provider => (
            "AI provider",
            "Sign in to at least one model provider to chat.",
            Need::Required,
        ),
        CheckId::Git => (
            "Git",
            "Reviews, worktrees, changes and branches.",
            Need::Recommended,
        ),
        CheckId::ClaudeBridge => (
            "Claude bridge",
            "Use a Claude Pro/Max subscription through Claude Code.",
            Need::Optional,
        ),
        CheckId::ClaudeLogin => (
            "Claude Code sign-in",
            "The Claude bridge uses Claude Code's own login.",
            Need::Optional,
        ),
        CheckId::Engram => (
            "Engram",
            "Long-term memory across sessions.",
            Need::Optional,
        ),
    }
}

/// The checks shown on `os` (`std::env::consts::OS`), in display order.
fn check_order(os: &str) -> Vec<CheckId> {
    use CheckId::*;
    let mut ids = vec![Node];
    if os == "windows" {
        ids.push(Go);
    }
    ids.extend([
        GentleShell,
        Setup,
        Provider,
        Git,
        ClaudeBridge,
        ClaudeLogin,
        Engram,
    ]);
    ids
}

/// Blocking: probes every dependency relevant to this OS.
pub fn run_checks() -> Vec<Check> {
    apply_toolchain_path();
    let launcher = gentle::launcher();
    let agent = pi_rpc::resolve_homes().agent;
    let claude = pi_rpc::find_program("claude");
    let signed_in = claude.as_deref().and_then(gentle::claude_signed_in);
    check_order(std::env::consts::OS)
        .into_iter()
        .map(|id| {
            let (status, fix) = match id {
                CheckId::Node => extras::check_node(),
                CheckId::Go => extras::check_go(),
                CheckId::GentleShell => gentle::check_gentle_shell(&launcher),
                CheckId::Setup => gentle::check_setup(&launcher),
                CheckId::Provider => extras::check_provider(&agent, signed_in),
                CheckId::Git => extras::check_git(),
                CheckId::ClaudeBridge => gentle::check_bridge(&agent),
                CheckId::ClaudeLogin => {
                    extras::check_claude_login(&agent, claude.as_deref(), signed_in)
                }
                CheckId::Engram => extras::check_engram(),
            };
            let (title, summary, need) = meta(id);
            Check {
                id,
                title,
                summary,
                need,
                status,
                fix,
            }
        })
        .collect()
}

/// Blocking: installs `id` unattended, streaming progress lines to `log`.
pub fn install(id: CheckId, log: &(dyn Fn(String) + Send + Sync)) -> Result<(), String> {
    let result = match id {
        CheckId::Node => fetch::install_node(log),
        CheckId::Go if cfg!(windows) => extras::install_go(log),
        CheckId::Go => Err("Go is only needed on Windows.".into()),
        CheckId::GentleShell => gentle::install_gentle_shell(log),
        CheckId::Setup => gentle::run_setup(log),
        CheckId::Provider => Err("Sign in from the Providers screen.".into()),
        CheckId::Git => extras::install_git(log),
        CheckId::ClaudeBridge => gentle::install_bridge(log),
        CheckId::ClaudeLogin => {
            Err("Sign in to Claude Code in a terminal: `claude auth login`.".into())
        }
        CheckId::Engram => extras::install_engram(log),
    };
    refresh_paths();
    crate::settings::forget_claude_auth();
    result
}

// ── Completion marker ──────────────────────────────────────────────────────

/// Onboarding was never completed on this machine.
pub fn first_run() -> bool {
    let Some(path) = crate::themes::store_path() else {
        return true;
    };
    let done = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|settings| settings["onboarding"]["done"] == true);
    !done
}

/// Persist completion in the app's `settings.json` (`onboarding` key).
pub fn mark_done() {
    let Some(path) = crate::themes::store_path() else {
        return;
    };
    let value = json!({ "done": true, "version": env!("CARGO_PKG_VERSION") });
    if let Err(err) = crate::themes::write_key(&path, "onboarding", value) {
        eprintln!("gentle-shell-gpui: could not save onboarding state: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CheckId::*;

    #[test]
    fn checks_per_os_in_display_order() {
        let unix = vec![
            Node,
            GentleShell,
            Setup,
            Provider,
            Git,
            ClaudeBridge,
            ClaudeLogin,
            Engram,
        ];
        assert_eq!(check_order("macos"), unix);
        assert_eq!(check_order("linux"), unix);
        let windows = check_order("windows");
        assert_eq!(&windows[..3], &[Node, Go, GentleShell]);
        assert_eq!(windows.len(), unix.len() + 1);
    }

    #[test]
    fn needs_per_check() {
        assert_eq!(meta(Node).2, Need::Required);
        assert_eq!(meta(Provider).2, Need::Required);
        assert_eq!(meta(Git).2, Need::Recommended);
        assert_eq!(meta(Engram).2, Need::Optional);
        assert_eq!(meta(ClaudeLogin).2, Need::Optional);
    }

    #[test]
    fn npm_prefix_bin_per_os() {
        let prefix = Path::new("/data/toolchain/npm-global");
        assert_eq!(prefix_bin(prefix, false), prefix.join("bin"));
        assert_eq!(prefix_bin(prefix, true), prefix.to_path_buf());
    }
}
