//! Gentle Shell itself: the launcher (`npm install -g gentle-pi`), its
//! one-time `setup`, and the npm prefix decision that keeps sudo out of it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::probe::{
    Version, capture, go_version, output_of, parse_version, run_logged, show_version,
};
use super::{
    Fix, Log, MIN_GO, MIN_PI, Status, prefix_bin, refresh_paths, toolchain_dir, user_home,
};

/// What `gentle-shell --version` reported.
#[derive(Debug, Clone, PartialEq)]
pub struct LauncherVersion {
    pub gentle_shell: String,
    pub pi: Option<String>,
    pub home_mode: Option<String>,
    pub home_dir: Option<PathBuf>,
}

/// `gentle-shell 4.6.0` / `pi 1.5.0|not found` / `home <mode> <dir>`.
pub fn parse_launcher_version(text: &str) -> Option<LauncherVersion> {
    let mut version = LauncherVersion {
        gentle_shell: String::new(),
        pi: None,
        home_mode: None,
        home_dir: None,
    };
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("gentle-shell ") {
            version.gentle_shell = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("pi ") {
            version.pi = Some(v.trim().to_string()).filter(|v| v != "not found");
        } else if let Some(rest) = line.strip_prefix("home ") {
            let (mode, dir) = rest.split_once(' ').unwrap_or((rest, ""));
            version.home_mode = Some(mode.to_string());
            version.home_dir =
                Some(PathBuf::from(dir.trim())).filter(|d| !d.as_os_str().is_empty());
        }
    }
    (!version.gentle_shell.is_empty()).then_some(version)
}

/// The resolved launcher and what it reports, probed once per check run.
pub struct Launcher {
    pub spec: pi_rpc::LaunchSpec,
    pub version: Option<LauncherVersion>,
}

pub fn launcher() -> Result<Launcher, String> {
    let spec = pi_rpc::resolve_backend(&user_home()).map_err(|e| format!("{e:#}"))?;
    let version = pi_rpc::version_output(&spec)
        .as_deref()
        .and_then(parse_launcher_version);
    Ok(Launcher { spec, version })
}

/// Launcher options (home selectors from `GENTLE_SHELL_ARGS`) that precede a subcommand.
fn launcher_command(spec: &pi_rpc::LaunchSpec, sub: &[&str]) -> Command {
    let mut command = Command::new(&spec.program);
    command
        .args(spec.args.iter().take_while(|a| *a != "--mode"))
        .args(sub)
        .current_dir(&spec.cwd);
    command
}

pub fn check_gentle_shell(launcher: &Result<Launcher, String>) -> (Status, Fix) {
    let launcher = match launcher {
        Ok(launcher) => launcher,
        Err(_) => {
            return (
                Status::Missing("gentle-shell isn't installed".into()),
                Fix::Auto,
            );
        }
    };
    let Some(version) = &launcher.version else {
        return (
            Status::Failed(format!(
                "{} didn't report its version (Node.js or Pi may be missing)",
                launcher.spec.program.display()
            )),
            Fix::Auto,
        );
    };
    let Some(pi) = &version.pi else {
        return (
            Status::Failed("Pi wasn't found next to gentle-shell".into()),
            Fix::Auto,
        );
    };
    match parse_version(pi) {
        Some(v) if v >= MIN_PI => (
            Status::Ok(format!("gentle-shell {} · pi {pi}", version.gentle_shell)),
            Fix::None,
        ),
        _ => (
            Status::Outdated(format!("pi {pi} — needs {} or newer", show_version(MIN_PI))),
            Fix::Auto,
        ),
    }
}

// ── npm prefix ─────────────────────────────────────────────────────────────

/// Where `npm install -g` should go: `None` for npm's own global prefix (the
/// managed Node's is always writable), else the app's user prefix.
pub fn choose_prefix(
    npm: &Path,
    managed_node: Option<&Path>,
    global_writable: impl FnOnce() -> bool,
    user_prefix: &Path,
) -> Option<PathBuf> {
    if managed_node.is_some_and(|dir| npm.starts_with(dir)) || global_writable() {
        None
    } else {
        Some(user_prefix.to_path_buf())
    }
}

fn writable(dir: &Path) -> bool {
    // The nearest existing folder decides: npm creates the rest.
    let Some(dir) = dir.ancestors().find(|d| d.is_dir()) else {
        return false;
    };
    let probe = dir.join(format!(".gentle-shell-write-test-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Whether `npm install -g` can write npm's global prefix without sudo.
fn global_prefix_writable(npm: &Path) -> bool {
    let Some(prefix) = output_of(npm, &["prefix", "-g"]).map(PathBuf::from) else {
        return false;
    };
    let dirs = if cfg!(windows) {
        vec![prefix.join("node_modules"), prefix]
    } else {
        vec![prefix.join("lib").join("node_modules"), prefix.join("bin")]
    };
    dirs.iter().all(|d| writable(d))
}

/// The prefix a global npm install should use (see `choose_prefix`).
pub fn install_prefix(npm: &Path) -> Option<PathBuf> {
    let toolchain = toolchain_dir()?;
    choose_prefix(
        npm,
        Some(&toolchain.join("node")),
        || global_prefix_writable(npm),
        &toolchain.join("npm-global"),
    )
}

pub fn install_gentle_shell(log: &Log) -> Result<(), String> {
    if cfg!(windows) {
        let go = pi_rpc::find_program("go")
            .and_then(|go| output_of(&go, &["version"]))
            .and_then(|out| go_version(&out));
        if go.is_none_or(|v: Version| v < MIN_GO) {
            return Err(format!(
                "On Windows gentle-pi builds its Gentle AI companion with Go {} or newer. Install Go first, then try again.",
                show_version(MIN_GO)
            ));
        }
    }
    let npm = pi_rpc::find_program("npm").ok_or("npm wasn't found. Install Node.js first.")?;
    let mut command = Command::new(&npm);
    command.args([
        "install",
        "-g",
        "gentle-pi",
        "--allow-scripts=gentle-pi",
        "--foreground-scripts",
    ]);
    if let Some(prefix) = install_prefix(&npm) {
        log(format!(
            "npm's global folder needs admin rights; installing into {} instead.",
            prefix.display()
        ));
        std::fs::create_dir_all(prefix_bin(&prefix, cfg!(windows)))
            .map_err(|e| format!("Couldn't create {}: {e}", prefix.display()))?;
        command.arg("--prefix").arg(&prefix);
    }
    log("Installing gentle-pi with npm…".into());
    run_logged(command, "npm install -g gentle-pi", log)?;
    refresh_paths();
    let spec = pi_rpc::resolve_backend(&user_home()).map_err(|_| {
        "gentle-pi was installed, but gentle-shell still isn't on the PATH.".to_string()
    })?;
    log(format!("gentle-shell is at {}.", spec.program.display()));
    Ok(())
}

// ── Setup marker ───────────────────────────────────────────────────────────

/// How `~/.gentle-shell/config.json` `provisioned[<home>]` compares with the
/// installed gentle-pi (launcher `needsProvisioning`).
#[derive(Debug, Clone, PartialEq)]
pub enum Marker {
    Match { at: String },
    Missing,
    StalePin { recorded: String },
    StaleGentlePi { recorded: Option<String> },
}

/// `pin` is `None` when it can't be read; then only gentle-pi is compared.
pub fn marker_state(config: &Value, home_key: &str, pin: Option<&str>, gentle_pi: &str) -> Marker {
    let entry = &config["provisioned"][home_key];
    let (Some(gentle_ai), Some(at)) = (entry["gentleAi"].as_str(), entry["at"].as_str()) else {
        return Marker::Missing;
    };
    if pin.is_some_and(|pin| pin != gentle_ai) {
        return Marker::StalePin {
            recorded: gentle_ai.to_string(),
        };
    }
    match entry["gentlePi"].as_str() {
        Some(v) if v == gentle_pi => Marker::Match { at: at.to_string() },
        recorded => Marker::StaleGentlePi {
            recorded: recorded.map(str::to_string),
        },
    }
}

/// `export const INSTALLER_VERSION = "4.3.1";` → `4.3.1`.
pub fn installer_pin(source: &str) -> Option<String> {
    let re = regex::Regex::new(r#"INSTALLER_VERSION\s*=\s*["']([^"']+)["']"#).ok()?;
    Some(re.captures(source)?[1].to_string())
}

/// The gentle-ai pin of the installed gentle-pi (launcher `resolveSetupGentleAiPin`).
fn gentle_ai_pin(root: Option<&Path>) -> Option<String> {
    if let Ok(pin) = std::env::var("GENTLE_SHELL_GENTLE_AI_PIN")
        && !pin.is_empty()
    {
        return Some(pin);
    }
    let root = root?;
    if let Ok(source) =
        std::fs::read_to_string(root.join("scripts").join("gentle-ai-installer.mjs"))
        && let Some(pin) = installer_pin(&source)
    {
        return Some(pin);
    }
    std::fs::read_dir(root.join(".gentle-ai"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_prefix('v')
                .map(str::to_string)
        })
        .max_by_key(|v| parse_version(v))
}

/// Node's `realpath` of the home (no `\\?\` prefix on Windows).
fn home_key(dir: &Path) -> String {
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let text = real.to_string_lossy().into_owned();
    text.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(text)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `settings.json` `packages` lists `name` (string or `{source}` entries).
pub fn lists_package(agent: &Path, name: &str) -> bool {
    read_json(&agent.join("settings.json")).is_some_and(|settings| {
        settings["packages"].as_array().is_some_and(|packages| {
            packages
                .iter()
                .filter_map(|p| p.as_str().or(p["source"].as_str()))
                .any(|source| crate::settings::npm_name(source) == Some(name))
        })
    })
}

pub fn check_setup(launcher: &Result<Launcher, String>) -> (Status, Fix) {
    let Some((launcher, version)) = launcher
        .as_ref()
        .ok()
        .and_then(|l| Some((l, l.version.as_ref()?)))
    else {
        return (
            Status::Missing("Needs a working Gentle Shell first".into()),
            Fix::Auto,
        );
    };
    let home = version
        .home_dir
        .clone()
        .unwrap_or_else(|| pi_rpc::resolve_homes().agent);
    // `--link` homes are the user's own Pi home: setup runs but records nothing.
    if version.home_mode.as_deref() == Some("link") {
        return if lists_package(&home, "gentle-engram") {
            (
                Status::Ok(format!("Linked to {}", home.display())),
                Fix::None,
            )
        } else {
            (
                Status::Missing(format!("{} isn't set up yet", home.display())),
                Fix::Auto,
            )
        };
    }
    let config_path = std::env::var_os("GENTLE_SHELL_CONFIG")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home().join(".gentle-shell").join("config.json"));
    let config = read_json(&config_path).unwrap_or(Value::Null);
    let root = crate::profiles::gentle_pi_root(&launcher.spec.program);
    let pin = gentle_ai_pin(root.as_deref());
    match marker_state(
        &config,
        &home_key(&home),
        pin.as_deref(),
        &version.gentle_shell,
    ) {
        Marker::Match { at } => (
            Status::Ok(format!(
                "Gentle AI {} · set up {}",
                pin.as_deref().unwrap_or("companion"),
                at.get(..10).unwrap_or(&at)
            )),
            Fix::None,
        ),
        Marker::Missing => (
            Status::Missing("Not set up yet — the first chat would do it silently".into()),
            Fix::Auto,
        ),
        Marker::StalePin { recorded } => (
            Status::Outdated(format!(
                "Set up with Gentle AI {recorded}; gentle-pi now pins {}",
                pin.unwrap_or_default()
            )),
            Fix::Auto,
        ),
        Marker::StaleGentlePi { recorded } => (
            Status::Outdated(format!(
                "Set up with gentle-pi {}; {} is installed",
                recorded.as_deref().unwrap_or("an older version"),
                version.gentle_shell
            )),
            Fix::Auto,
        ),
    }
}

/// `gentle-shell setup` (non-interactive: stdin is closed); done once the
/// launcher recorded it.
pub fn run_setup(log: &Log) -> Result<(), String> {
    let spec = pi_rpc::resolve_backend(&user_home()).map_err(|e| format!("{e:#}"))?;
    log("Running gentle-shell setup; this can take a few minutes…".into());
    run_logged(
        launcher_command(&spec, &["setup"]),
        "gentle-shell setup",
        log,
    )?;
    refresh_paths();
    match check_setup(&launcher()) {
        (Status::Ok(_), _) => Ok(()),
        (Status::Missing(why) | Status::Outdated(why) | Status::Failed(why), _) => Err(format!(
            "Setup finished, but it isn't recorded as done: {why}"
        )),
    }
}

// ── Claude bridge ──────────────────────────────────────────────────────────

pub const BRIDGE: &str = "pi-claude-bridge";

/// The bridge's version when it is listed and installed in the home;
/// `Some(None)` when listed but its package is missing.
pub fn bridge_version(agent: &Path) -> Option<Option<String>> {
    if !lists_package(agent, BRIDGE) {
        return None;
    }
    let package = agent
        .join("npm")
        .join("node_modules")
        .join(BRIDGE)
        .join("package.json");
    Some(read_json(&package).map(|p| p["version"].as_str().unwrap_or("?").to_string()))
}

pub fn check_bridge(agent: &Path) -> (Status, Fix) {
    match bridge_version(agent) {
        Some(Some(version)) => (Status::Ok(format!("{BRIDGE} {version}")), Fix::None),
        Some(None) => (
            Status::Failed("Listed in settings, but its package is missing".into()),
            Fix::Auto,
        ),
        None => (Status::Missing("Not installed".into()), Fix::Auto),
    }
}

/// `gentle-shell install npm:pi-claude-bridge` against the resolved home.
pub fn install_bridge(log: &Log) -> Result<(), String> {
    let spec = pi_rpc::resolve_backend(&user_home())
        .map_err(|_| "Install Gentle Shell first.".to_string())?;
    let source = format!("npm:{BRIDGE}");
    log(format!("Installing {source}…"));
    run_logged(
        launcher_command(&spec, &["install", &source]),
        "gentle-shell install",
        log,
    )?;
    crate::settings::forget_claude_auth();
    Ok(())
}

/// `claude auth status` → signed in?; `None` when Claude Code isn't installed
/// or doesn't answer.
pub fn claude_signed_in(claude: &Path) -> Option<bool> {
    let (_, out) = capture(claude, &["auth", "status"])?;
    let value: Value = serde_json::from_str(out.trim()).ok()?;
    Some(value["loggedIn"] == true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_launcher_versions() {
        let v = parse_launcher_version(
            "gentle-shell 4.6.0\npi 1.5.0\nhome isolated /Users/me/.gentle-shell/agent\n",
        )
        .unwrap();
        assert_eq!(v.gentle_shell, "4.6.0");
        assert_eq!(v.pi.as_deref(), Some("1.5.0"));
        assert_eq!(v.home_mode.as_deref(), Some("isolated"));
        assert_eq!(
            v.home_dir,
            Some(PathBuf::from("/Users/me/.gentle-shell/agent"))
        );
        let none = parse_launcher_version("gentle-shell 4.6.0\npi not found\n").unwrap();
        assert_eq!(none.pi, None);
        assert_eq!(parse_launcher_version(""), None);
    }

    #[test]
    fn matches_the_provisioned_marker() {
        let config = json!({"provisioned": {"/h": {"gentleAi": "4.3.1", "gentlePi": "4.6.0", "at": "2026-11-06T17:08:31.806Z"}}});
        assert_eq!(
            marker_state(&config, "/h", Some("4.3.1"), "4.6.0"),
            Marker::Match {
                at: "2026-11-06T17:08:31.806Z".into()
            }
        );
        assert_eq!(
            marker_state(&config, "/h", Some("4.3.1"), "4.7.0"),
            Marker::StaleGentlePi {
                recorded: Some("4.6.0".into())
            }
        );
        assert_eq!(
            marker_state(&config, "/h", Some("4.4.0"), "4.6.0"),
            Marker::StalePin {
                recorded: "4.3.1".into()
            }
        );
        assert_eq!(
            marker_state(&config, "/other", Some("4.3.1"), "4.6.0"),
            Marker::Missing
        );
        assert_eq!(
            marker_state(&Value::Null, "/h", Some("4.3.1"), "4.6.0"),
            Marker::Missing
        );
        let old = json!({"provisioned": {"/h": {"gentleAi": "4.3.1", "at": "x"}}});
        assert_eq!(
            marker_state(&old, "/h", Some("4.3.1"), "4.6.0"),
            Marker::StaleGentlePi { recorded: None }
        );
        assert!(matches!(
            marker_state(&config, "/h", None, "4.6.0"),
            Marker::Match { .. }
        ));
    }

    #[test]
    fn reads_the_installer_pin() {
        assert_eq!(
            installer_pin("// x\nexport const INSTALLER_VERSION = \"4.3.1\";\n").as_deref(),
            Some("4.3.1")
        );
        assert_eq!(installer_pin("const OTHER = '1';"), None);
    }

    #[test]
    fn decides_the_npm_prefix() {
        let user = Path::new("/data/toolchain/npm-global");
        let managed = Path::new("/data/toolchain/node");
        // The managed Node's own prefix is always used, without probing.
        assert_eq!(
            choose_prefix(
                &managed.join("bin/npm"),
                Some(managed),
                || panic!("not probed"),
                user
            ),
            None
        );
        assert_eq!(
            choose_prefix(
                Path::new("/opt/homebrew/bin/npm"),
                Some(managed),
                || true,
                user
            ),
            None
        );
        assert_eq!(
            choose_prefix(Path::new("/usr/bin/npm"), Some(managed), || false, user),
            Some(user.to_path_buf())
        );
    }
}
