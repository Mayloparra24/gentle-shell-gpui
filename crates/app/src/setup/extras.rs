//! Node, Go, Git, Engram, the provider and the Claude Code sign-in.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::gentle::{bridge_version, install_prefix};
use super::probe::{
    go_version, os_release_value, output_of, parse_version, run_logged, show_version,
};
use super::{
    Fix, Log, MIN_ENGRAM, MIN_GO, MIN_NODE, Status, fetch, managed_dirs, prefix_bin, refresh_paths,
};

// ── Node.js ────────────────────────────────────────────────────────────────

pub fn check_node() -> (Status, Fix) {
    let fix =
        if fetch::node_asset("v22.0.0", std::env::consts::OS, std::env::consts::ARCH).is_some() {
            Fix::Auto
        } else {
            Fix::None
        };
    let Some(node) = pi_rpc::find_program("node") else {
        return (Status::Missing("Node.js isn't installed".into()), fix);
    };
    let Some(text) = output_of(&node, &["--version"]) else {
        return (
            Status::Failed(format!("{} doesn't run", node.display())),
            fix,
        );
    };
    let Some(version) = parse_version(&text) else {
        return (
            Status::Failed(format!("Unexpected `node --version`: {text}")),
            fix,
        );
    };
    if version < MIN_NODE {
        return (
            Status::Outdated(format!(
                "Node {} — needs {} or newer",
                show_version(version),
                show_version(MIN_NODE)
            )),
            fix,
        );
    }
    match pi_rpc::find_program("npm").and_then(|npm| output_of(&npm, &["--version"])) {
        Some(npm) => (
            Status::Ok(format!("Node {} · npm {npm}", show_version(version))),
            Fix::None,
        ),
        None => (
            Status::Missing(format!("Node {} found, but not npm", show_version(version))),
            fix,
        ),
    }
}

// ── winget (Windows) ───────────────────────────────────────────────────────

fn winget() -> Option<PathBuf> {
    pi_rpc::find_program("winget")
}

/// A per-machine package via winget; Windows may show its own UAC prompt.
fn winget_install(id: &str, what: &str, log: &Log) -> Result<(), String> {
    let winget = winget().ok_or("winget isn't available on this system.")?;
    let mut command = pi_rpc::command(winget);
    command.args([
        "install",
        "--id",
        id,
        "-e",
        "--silent",
        "--accept-source-agreements",
        "--accept-package-agreements",
    ]);
    log(format!(
        "Installing {what} with winget (Windows may ask for permission)…"
    ));
    run_logged(command, "winget install", log)
}

/// Install folders of tools a winget install puts on the machine PATH, which
/// this already-running app doesn't see: `(program, dir)`.
#[cfg(windows)]
pub fn windows_fallback_dirs() -> Vec<(&'static str, PathBuf)> {
    let env = |key: &str| std::env::var_os(key).map(PathBuf::from);
    let program_files = env("ProgramFiles").unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let mut dirs = vec![
        ("go", program_files.join("Go").join("bin")),
        ("git", program_files.join("Git").join("cmd")),
        ("node", program_files.join("nodejs")),
    ];
    if let Some(local) = env("LOCALAPPDATA") {
        dirs.push(("git", local.join("Programs").join("Git").join("cmd")));
    }
    if let Some(roaming) = env("APPDATA") {
        dirs.push(("gentle-shell", roaming.join("npm")));
    }
    dirs
}

// ── Go (Windows) ───────────────────────────────────────────────────────────

fn go_fix() -> Fix {
    if winget().is_some() {
        Fix::Auto
    } else {
        Fix::Terminal("start https://go.dev/dl/".into())
    }
}

pub fn check_go() -> (Status, Fix) {
    let Some(go) = pi_rpc::find_program("go") else {
        return (Status::Missing("Go isn't installed".into()), go_fix());
    };
    match output_of(&go, &["version"]).as_deref().and_then(go_version) {
        Some(v) if v >= MIN_GO => (Status::Ok(format!("Go {}", show_version(v))), Fix::None),
        Some(v) => (
            Status::Outdated(format!(
                "Go {} — needs {} or newer",
                show_version(v),
                show_version(MIN_GO)
            )),
            go_fix(),
        ),
        None => (
            Status::Failed(format!("{} doesn't run", go.display())),
            go_fix(),
        ),
    }
}

pub fn install_go(log: &Log) -> Result<(), String> {
    let result = winget_install("GoLang.Go", "Go", log);
    refresh_paths();
    // winget exits non-zero when it's already installed; the probe decides.
    match check_go() {
        (Status::Ok(_), _) => Ok(()),
        _ => result.and(Err(
            "Go was installed, but this app can't find it yet.".into()
        )),
    }
}

// ── Git ────────────────────────────────────────────────────────────────────

/// The distro's package manager command for `package`.
pub fn linux_install_command(
    package: &str,
    os_release: &str,
    has: impl Fn(&str) -> bool,
) -> String {
    let ids: Vec<String> = ["ID", "ID_LIKE"]
        .iter()
        .filter_map(|key| os_release_value(os_release, key))
        .flat_map(|v| v.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .collect();
    let is = |names: &[&str]| ids.iter().any(|id| names.contains(&id.as_str()));
    let manager = if is(&["debian", "ubuntu"]) {
        "apt"
    } else if is(&["fedora", "rhel", "centos"]) {
        "dnf"
    } else if is(&["arch"]) {
        "pacman"
    } else if is(&["suse", "opensuse"]) {
        "zypper"
    } else if is(&["alpine"]) {
        "apk"
    } else {
        ["apt", "dnf", "pacman", "zypper", "apk"]
            .into_iter()
            .find(|m| has(m))
            .unwrap_or("apt")
    };
    match manager {
        "pacman" => format!("sudo pacman -S {package}"),
        "apk" => format!("sudo apk add {package}"),
        other => format!("sudo {other} install {package}"),
    }
}

fn git_fix() -> Fix {
    match std::env::consts::OS {
        "windows" if winget().is_some() => Fix::Auto,
        "windows" => Fix::Terminal("start https://git-scm.com/download/win".into()),
        "macos" => Fix::Terminal("xcode-select --install".into()),
        _ => {
            let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
            Fix::Terminal(linux_install_command("git", &release, |m| {
                pi_rpc::find_program(m).is_some()
            }))
        }
    }
}

pub fn check_git() -> (Status, Fix) {
    let mut git = pi_rpc::find_program("git");
    // macOS's /usr/bin/git is a stub that pops the Command Line Tools
    // installer until they are installed.
    if cfg!(target_os = "macos")
        && git.as_deref() == Some(Path::new("/usr/bin/git"))
        && output_of(Path::new("/usr/bin/xcode-select"), &["-p"]).is_none()
    {
        git = None;
    }
    match git.and_then(|git| output_of(&git, &["--version"])) {
        Some(text) => (
            Status::Ok(format!(
                "Git {}",
                text.trim_start_matches("git version ").trim()
            )),
            Fix::None,
        ),
        None => (Status::Missing("Git isn't installed".into()), git_fix()),
    }
}

pub fn install_git(log: &Log) -> Result<(), String> {
    if !cfg!(windows) {
        return match git_fix() {
            Fix::Terminal(command) => Err(format!(
                "Git needs your password to install: run `{command}`."
            )),
            _ => Err("Install Git from your system's package manager.".into()),
        };
    }
    let result = winget_install("Git.Git", "Git", log);
    refresh_paths();
    match check_git() {
        (Status::Ok(_), _) => Ok(()),
        _ => result.and(Err(
            "Git was installed, but this app can't find it yet.".into()
        )),
    }
}

// ── Engram ─────────────────────────────────────────────────────────────────

fn engram() -> Option<PathBuf> {
    std::env::var_os("ENGRAM_BIN")
        .filter(|b| !b.is_empty())
        .map(PathBuf::from)
        .or_else(|| pi_rpc::find_program("engram"))
}

pub fn check_engram() -> (Status, Fix) {
    match engram().and_then(|e| output_of(&e, &["version"])) {
        Some(text) => engram_status(&text),
        None => (Status::Missing("Not installed".into()), Fix::Auto),
    }
}

/// From `engram version` (`engram 1.18.0`, then an update notice). A version
/// it can't read stays OK, as before.
fn engram_status(text: &str) -> (Status, Fix) {
    let line = text.lines().next().unwrap_or("installed").trim();
    match line.split_whitespace().last().and_then(parse_version) {
        Some(version) if version < MIN_ENGRAM => (
            Status::Outdated(format!(
                "Engram {} — needs {} or newer",
                show_version(version),
                show_version(MIN_ENGRAM)
            )),
            Fix::Auto,
        ),
        _ => (Status::Ok(line.to_string()), Fix::None),
    }
}

fn brew() -> Option<PathBuf> {
    pi_rpc::find_program("brew").or_else(|| {
        [
            "/opt/homebrew/bin/brew",
            "/usr/local/bin/brew",
            "/home/linuxbrew/.linuxbrew/bin/brew",
        ]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    })
}

/// Homebrew's tap when Homebrew is there, else the GitHub release binary.
pub fn install_engram(log: &Log) -> Result<(), String> {
    if !cfg!(windows)
        && let Some(brew) = brew()
    {
        // Newer Homebrew wants third-party taps trusted; older ones lack the command.
        let _ = output_of(&brew, &["trust", "gentleman-programming/tap"]);
        let mut command = pi_rpc::command(&brew);
        command.args(["install", "gentleman-programming/tap/engram"]);
        log("Installing Engram with Homebrew…".into());
        match run_logged(command, "brew install", log) {
            Ok(()) => return Ok(()),
            Err(e) => log(format!("{e} Downloading the release instead.")),
        }
    }
    fetch::install_engram_release(log)?;
    refresh_paths();
    match check_engram() {
        (Status::Ok(_), _) => Ok(()),
        // An `ENGRAM_BIN` or an older install earlier on the PATH still wins.
        (Status::Outdated(why), _) => {
            Err(format!("Engram was installed, but another one runs: {why}"))
        }
        _ => Err("Engram was installed, but it doesn't run.".into()),
    }
}

// ── Provider and Claude Code ───────────────────────────────────────────────

fn json_keys(path: &Path, field: Option<&str>) -> Vec<String> {
    let Some(value) = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return Vec::new();
    };
    let value = field.map_or(&value, |f| &value[f]);
    value
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

const ENV_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "OPENROUTER_API_KEY",
    "GROQ_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
];

/// `claude_login`: Claude Code's sign-in, when the bridge can use it.
pub fn check_provider(agent: &Path, claude_login: Option<bool>) -> (Status, Fix) {
    let mut found = json_keys(&agent.join("auth.json"), None);
    found.extend(json_keys(&agent.join("models.json"), Some("providers")));
    if !found.is_empty() {
        return (
            Status::Ok(format!("Signed in: {}", found.join(", "))),
            Fix::None,
        );
    }
    if let Some(key) = ENV_KEYS
        .iter()
        .find(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
    {
        return (Status::Ok(format!("{key} is set")), Fix::None);
    }
    if bridge_version(agent).is_some_and(|v| v.is_some()) && claude_login == Some(true) {
        return (
            Status::Ok("Claude through your Claude Code sign-in".into()),
            Fix::None,
        );
    }
    (Status::Missing("No provider signed in".into()), Fix::SignIn)
}

/// `command` for the user's terminal, with the app's own toolchain dirs (and
/// `also`) on its PATH: the terminal doesn't inherit them. Unix spells the
/// whole PATH out (no `$PATH`), so sh, bash, zsh and fish all accept it.
fn in_terminal(command: &str, also: Option<PathBuf>) -> String {
    let mut dirs = managed_dirs();
    dirs.extend(also);
    if dirs.is_empty() {
        return command.to_string();
    }
    if cfg!(windows) {
        let joined: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
        return format!("set \"PATH={};%PATH%\" && {command}", joined.join(";"));
    }
    for dir in std::env::split_paths(&pi_rpc::child_path()) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    let path = std::env::join_paths(dirs).unwrap_or_default();
    format!("export PATH='{}' && {command}", path.to_string_lossy())
}

/// The sign-in only matters with the bridge; without it, it is shown as
/// waiting on the bridge (no fix) unless Claude Code is already signed in.
pub fn check_claude_login(
    agent: &Path,
    claude: Option<&Path>,
    signed_in: Option<bool>,
) -> (Status, Fix) {
    if signed_in == Some(true) {
        return (Status::Ok("Signed in".into()), Fix::None);
    }
    if bridge_version(agent).is_none() {
        return (
            Status::Missing("Install the Claude bridge first".into()),
            Fix::None,
        );
    }
    match (claude, signed_in) {
        (Some(_), Some(false)) => (
            Status::Missing("Not signed in".into()),
            Fix::Terminal(in_terminal("claude auth login", None)),
        ),
        (Some(claude), None) => (
            Status::Failed(format!("{} doesn't answer `auth status`", claude.display())),
            Fix::Terminal(in_terminal("claude auth login", None)),
        ),
        _ => {
            let prefix = pi_rpc::find_program("npm").and_then(|npm| install_prefix(&npm));
            let install = match &prefix {
                Some(p) if cfg!(windows) => format!(
                    "npm i -g @anthropic-ai/claude-code --prefix \"{}\"",
                    p.display()
                ),
                Some(p) => format!(
                    "npm i -g @anthropic-ai/claude-code --prefix '{}'",
                    p.display().to_string().replace('\'', "'\\''")
                ),
                None => "npm i -g @anthropic-ai/claude-code".to_string(),
            };
            let bin = prefix.map(|p| prefix_bin(&p, cfg!(windows)));
            (
                Status::Missing("Claude Code isn't installed".into()),
                Fix::Terminal(in_terminal(&format!("{install} && claude auth login"), bin)),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_git_command_per_distro() {
        let none = |_: &str| false;
        assert_eq!(
            linux_install_command("git", "ID=ubuntu\nID_LIKE=debian\n", none),
            "sudo apt install git"
        );
        assert_eq!(
            linux_install_command(
                "git",
                "ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n",
                none
            ),
            "sudo dnf install git"
        );
        assert_eq!(
            linux_install_command("git", "ID=arch\n", none),
            "sudo pacman -S git"
        );
        assert_eq!(
            linux_install_command(
                "git",
                "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n",
                none
            ),
            "sudo zypper install git"
        );
        assert_eq!(
            linux_install_command("git", "ID=unknown\n", |m| m == "pacman"),
            "sudo pacman -S git"
        );
        assert_eq!(
            linux_install_command("git", "", none),
            "sudo apt install git"
        );
    }

    #[test]
    fn engram_older_than_three_is_outdated() {
        let status = |text: &str| engram_status(text).0;
        assert_eq!(
            status("engram 1.18.0\nUpdate available: 1.18.0 -> 3.7.0\n"),
            Status::Outdated("Engram 1.18.0 — needs 3.0.0 or newer".into())
        );
        assert!(matches!(status("engram 2.9.9"), Status::Outdated(_)));
        assert_eq!(status("engram 3.0.0"), Status::Ok("engram 3.0.0".into()));
        assert_eq!(status("engram v3.7.0"), Status::Ok("engram v3.7.0".into()));
        assert_eq!(status("engram dev"), Status::Ok("engram dev".into()));
    }
}
