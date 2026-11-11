//! Maestro: UI automation the agent drives as a tool. It taps, types, swipes,
//! asserts and takes screenshots on Android emulators, iOS simulators and web
//! pages (a managed Chromium), and runs Maestro's YAML flows. The app puts a
//! Java runtime (when there is no Java 17+) and the Maestro CLI in its own
//! toolchain folder, no sudo, then registers `maestro mcp` with Pi through
//! `gentle-shell mcp add`, so Pi writes its own `mcp.json`. Settings' UI
//! testing section drives all of it.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::gentle::launcher_command;
use super::probe::{output_of, parse_version, run_logged};
use super::{Log, fetch, refresh_paths, toolchain_dir, user_home};

/// Maestro runs on Java 17 or newer.
const MIN_JAVA: u64 = 17;
/// The MCP server's name, so its tools are `mcp__maestro__…`.
const SERVER: &str = "maestro";
const DESCRIPTION: &str = "UI testing on Android emulators, iOS simulators and web pages \
(Chromium): list devices, take screenshots, inspect the screen, and run Maestro commands and \
flows (launch apps, tap, type, swipe, assert).";

fn script() -> &'static str {
    if cfg!(windows) {
        "maestro.bat"
    } else {
        "maestro"
    }
}

fn java_exe() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}

/// Where the app installs the CLI's launcher scripts.
pub fn toolchain_bin(toolchain: &Path) -> PathBuf {
    toolchain.join("maestro").join("bin")
}

/// The `bin` of the Java runtime the app installs.
pub fn toolchain_java_bin(toolchain: &Path) -> PathBuf {
    java_home_in(toolchain.join("java")).join("bin")
}

/// A Java runtime's home inside its unpacked folder (macOS bundles keep it
/// in `Contents/Home`).
fn java_home_in(root: PathBuf) -> PathBuf {
    let bundle = root.join("Contents").join("Home");
    if bundle.is_dir() { bundle } else { root }
}

/// The major version in a JDK/JRE `release` file (`JAVA_VERSION="21.0.5"`,
/// or `"1.8.0_402"` for Java 8).
pub fn java_release_major(text: &str) -> Option<u64> {
    let version = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("JAVA_VERSION="))?
        .trim_matches('"');
    let (major, minor, _) = parse_version(version)?;
    Some(if major == 1 { minor } else { major })
}

fn java_major(home: &Path) -> Option<u64> {
    java_release_major(&std::fs::read_to_string(home.join("release")).ok()?)
}

/// A Java 17+ home: the app's own, `JAVA_HOME`, the one macOS's
/// `java_home` picks, or the one behind the `java` on the PATH.
fn java_home() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(toolchain) = toolchain_dir() {
        candidates.push(java_home_in(toolchain.join("java")));
    }
    if let Some(home) = std::env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        candidates.push(PathBuf::from(home));
    }
    if cfg!(target_os = "macos")
        && let Some(home) = output_of(Path::new("/usr/libexec/java_home"), &["-v", "17+"])
    {
        candidates.push(PathBuf::from(home.trim()));
    }
    if let Some(home) = pi_rpc::find_program("java")
        .and_then(|java| java.canonicalize().ok())
        .and_then(|java| Some(java.parent()?.parent()?.to_path_buf()))
    {
        candidates.push(home);
    }
    candidates.into_iter().find(|home| {
        home.join("bin").join(java_exe()).is_file()
            && java_major(home).is_some_and(|major| major >= MIN_JAVA)
    })
}

/// The Maestro CLI: the app's own, the one on the PATH, or the official
/// installer's `~/.maestro/bin`.
fn maestro() -> Option<PathBuf> {
    app_cli()
        .or_else(|| pi_rpc::find_program("maestro"))
        .or_else(|| {
            Some(user_home().join(".maestro").join("bin").join(script())).filter(|p| p.is_file())
        })
}

/// The CLI the app installed, if any.
fn app_cli() -> Option<PathBuf> {
    toolchain_dir()
        .map(|toolchain| toolchain_bin(&toolchain).join(script()))
        .filter(|path| path.is_file())
}

/// Pi's global `mcp.json` has an enabled `maestro` server.
pub fn registered(mcp_json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(mcp_json) else {
        return false;
    };
    let server = &value["mcpServers"][SERVER];
    server.is_object() && server["enabled"] != false
}

fn is_registered(agent: &Path) -> bool {
    std::fs::read_to_string(agent.join("mcp.json")).is_ok_and(|text| registered(&text))
}

/// What Settings shows about Maestro.
#[derive(Clone, Debug, PartialEq)]
pub struct MaestroStatus {
    pub cli: Option<PathBuf>,
    /// The CLI is the app's own, so Remove can delete it.
    pub app_owned: bool,
    /// A Java 17+ home and its major version.
    pub java: Option<(PathBuf, u64)>,
    /// Pi's `mcp.json` has the `maestro` server.
    pub connected: bool,
}

/// Blocking: looks for the CLI, Java and the registration.
pub fn status() -> MaestroStatus {
    let cli = maestro();
    MaestroStatus {
        app_owned: cli.is_some() && cli == app_cli(),
        cli,
        java: java_home().and_then(|home| java_major(&home).map(|major| (home, major))),
        connected: is_registered(&pi_rpc::resolve_homes().agent),
    }
}

/// `maestro: connected, 10 tools (…)` from `gentle-shell mcp list`.
pub fn server_line(list: &str) -> Option<String> {
    list.lines()
        .map(str::trim)
        .find(|line| line.starts_with(&format!("{SERVER}:")))
        .map(str::to_string)
}

fn launcher() -> Result<pi_rpc::LaunchSpec, String> {
    pi_rpc::resolve_backend(&user_home()).map_err(|_| "Install Gentle Shell first.".to_string())
}

/// Blocking: starts the server the way the agent would and reports what Pi
/// sees (`maestro: connected, 10 tools …`, or the error).
pub fn test_connection() -> Result<String, String> {
    let output = launcher_command(&launcher()?, &["mcp", "list"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("gentle-shell didn't start: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    server_line(&text).ok_or_else(|| "Pi lists no maestro server.".to_string())
}

/// `gentle-shell mcp remove maestro`: the agent loses the tools; the CLI
/// stays installed.
pub fn disconnect(log: &Log) -> Result<(), String> {
    log("Disconnecting Maestro from the agent (gentle-shell mcp remove maestro)…".into());
    run_logged(
        launcher_command(&launcher()?, &["mcp", "remove", SERVER]),
        "gentle-shell mcp remove",
        log,
    )
}

/// Disconnect, then delete the CLI and the Java runtime the app installed.
/// A Maestro or Java you installed yourself is left alone.
pub fn remove(log: &Log) -> Result<(), String> {
    if is_registered(&pi_rpc::resolve_homes().agent) {
        disconnect(log)?;
    }
    let toolchain = toolchain_dir().ok_or("This system has no app data folder.")?;
    for (dir, what) in [
        (toolchain.join("maestro"), "Maestro"),
        (toolchain.join("java"), "Java runtime"),
    ] {
        if dir.exists() {
            log(format!("Deleting the {what} in {}…", dir.display()));
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("Couldn't delete {}: {e}", dir.display()))?;
        }
    }
    refresh_paths();
    Ok(())
}

/// Java (when missing), the CLI (when missing), then the MCP server.
pub fn install(log: &Log) -> Result<(), String> {
    let spec = launcher()?;
    let java = match java_home() {
        Some(home) => {
            log(format!("Using the Java in {}.", home.display()));
            home
        }
        None => {
            fetch::install_java_runtime(log)?;
            java_home().ok_or("Java was installed, but it doesn't run.")?
        }
    };
    let maestro = match maestro() {
        Some(maestro) => {
            log(format!("Using the Maestro CLI in {}.", maestro.display()));
            maestro
        }
        None => {
            fetch::install_maestro_release(log)?;
            refresh_paths();
            maestro().ok_or("Maestro was installed, but this app can't find it.")?
        }
    };
    let java_env = format!("JAVA_HOME={}", java.display());
    let developer_env = crate::features::devices::xcode_developer_dir()
        .map(|dir| format!("DEVELOPER_DIR={}", dir.display()));
    let maestro = maestro.to_string_lossy();
    let mut args = vec!["mcp", "add", SERVER, "--env", &java_env];
    if let Some(developer_env) = &developer_env {
        log(format!("iOS simulators through Xcode ({developer_env})."));
        args.extend(["--env", developer_env]);
    }
    args.extend(["--description", DESCRIPTION, "--"]);
    // A batch file only starts through cmd.exe.
    if cfg!(windows) {
        args.extend(["cmd", "/c"]);
    }
    args.extend([maestro.as_ref(), "mcp"]);
    log("Connecting Maestro to the agent (gentle-shell mcp add maestro)…".into());
    run_logged(launcher_command(&spec, &args), "gentle-shell mcp add", log)?;
    refresh_paths();
    if is_registered(&pi_rpc::resolve_homes().agent) {
        log("Done. New sessions (or /reload in an open one) have the maestro tools.".into());
        Ok(())
    } else {
        Err("gentle-shell added Maestro, but it isn't in this home's mcp.json.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_release_majors() {
        assert_eq!(
            java_release_major("IMPLEMENTOR=\"Eclipse\"\nJAVA_VERSION=\"21.0.5\"\n"),
            Some(21)
        );
        assert_eq!(java_release_major("JAVA_VERSION=\"17\""), Some(17));
        assert_eq!(java_release_major("JAVA_VERSION=\"1.8.0_402\""), Some(8));
        assert_eq!(java_release_major("OS_NAME=Darwin"), None);
    }

    #[test]
    fn server_line_from_mcp_list() {
        let list = "other: failed (spawn ENOENT)\nmaestro: connected, 10 tools (codemode, global)\n  /x/maestro mcp\n";
        assert_eq!(
            server_line(list).as_deref(),
            Some("maestro: connected, 10 tools (codemode, global)")
        );
        assert_eq!(server_line("No MCP servers configured."), None);
    }

    #[test]
    fn registration_reads_pi_mcp_json() {
        assert!(registered(
            r#"{"mcpServers":{"maestro":{"command":"maestro","args":["mcp"]}}}"#
        ));
        assert!(!registered(
            r#"{"mcpServers":{"maestro":{"command":"maestro","enabled":false}}}"#
        ));
        assert!(!registered(r#"{"mcpServers":{"other":{"command":"x"}}}"#));
        assert!(!registered("not json"));
    }
}
