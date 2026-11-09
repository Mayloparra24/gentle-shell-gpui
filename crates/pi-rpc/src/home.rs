//! Where Gentle Shell keeps its state, resolved the way the launcher does
//! (gentle-pi `lib/gentle-shell-launcher.ts` `resolveHome`), plus the Pi
//! package the launcher runs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::file_cache::Stamp;
use crate::launch::{LaunchSpec, extra_args};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Homes {
    /// The Pi agent dir: `auth.json`, `models.json`, `settings.json`, sessions.
    pub agent: PathBuf,
    /// Gentle AI config: `profiles.json`, `permission-mode.json`, …
    pub config: PathBuf,
}

pub(crate) fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn user_pi_home() -> PathBuf {
    std::env::var_os("GENTLE_SHELL_USER_PI_HOME")
        .or_else(|| std::env::var_os("PI_CODING_AGENT_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".pi").join("agent"))
}

/// `~/.gentle-shell/config.json`, where `gentle-shell home` persists the mode.
pub(crate) fn launcher_config_path() -> PathBuf {
    home_dir().join(".gentle-shell").join("config.json")
}

/// The last resolution and the config file stamp it read; everything else
/// it depends on is this process's environment.
static HOMES: Mutex<Option<(Option<Stamp>, Homes)>> = Mutex::new(None);

/// Order: `--link`, `--isolated`, `--home <dir>` (from `GENTLE_SHELL_ARGS`),
/// then `~/.gentle-shell/config.json`, then the isolated default. Cached
/// until that config file changes.
pub fn resolve_homes() -> Homes {
    memo_by_stamp(&HOMES, &launcher_config_path(), resolve_homes_uncached)
}

/// `slot`'s value while `path` keeps the stamp it had when `compute` last ran.
fn memo_by_stamp<T: Clone>(
    slot: &Mutex<Option<(Option<Stamp>, T)>>,
    path: &Path,
    compute: impl FnOnce() -> T,
) -> T {
    let stamp = Stamp::of(path);
    let mut cached = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, value)) = cached.as_ref()
        && *at == stamp
    {
        return value.clone();
    }
    let value = compute();
    *cached = Some((stamp, value.clone()));
    value
}

fn resolve_homes_uncached() -> Homes {
    let args = extra_args();
    let isolated = || {
        std::env::var_os("GENTLE_SHELL_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".gentle-shell").join("agent"))
    };
    // Only the isolated mode keeps a Gentle Shell config folder of its own.
    let (agent, is_isolated) = if args.iter().any(|a| a == "--link") {
        (user_pi_home(), false)
    } else if args.iter().any(|a| a == "--isolated") {
        (isolated(), true)
    } else if let Some(i) = args.iter().position(|a| a == "--home")
        && let Some(dir) = args.get(i + 1)
    {
        (PathBuf::from(dir), false)
    } else {
        match read_config_mode() {
            Some(ConfigMode::Link) => (user_pi_home(), false),
            Some(ConfigMode::Path(dir)) => (dir, false),
            Some(ConfigMode::Isolated) | None => (isolated(), true),
        }
    };
    Homes {
        config: config_home(&agent, is_isolated),
        agent,
    }
}

/// gentle-pi `resolveGentlePiConfigHome` / `standaloneConfigHome`:
/// `GENTLE_PI_CONFIG_HOME` wins; a self-contained install (the launcher the
/// app runs, [`crate::launch::bundled_launcher`]) keeps it in
/// `<prefix>/config/gentle-ai` (`bundledPiConfigDir`); an isolated
/// `~/.gentle-shell/agent` keeps its config beside it; `--link` and custom
/// homes share `~/.pi/gentle-ai`.
fn config_home(agent: &Path, isolated: bool) -> PathBuf {
    if let Some(dir) = std::env::var_os("GENTLE_PI_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let bundled = std::env::var_os("GENTLE_SHELL_BIN")
        .is_none_or(|bin| bin.is_empty())
        .then(crate::launch::bundled_launcher)
        .flatten();
    if let Some(dir) = bundled.as_deref().and_then(bundled_config_home) {
        return dir;
    }
    let parent = agent.parent();
    let beside = agent.file_name().is_some_and(|n| n == "agent")
        && parent
            .and_then(Path::file_name)
            .is_some_and(|n| n == ".gentle-shell");
    match parent {
        Some(parent) if isolated && beside => parent.join("gentle-ai"),
        _ => home_dir().join(".pi").join("gentle-ai"),
    }
}

/// `<prefix>/config/gentle-ai` for a self-contained install's launcher at
/// `<prefix>/bin/gentle-shell[.cmd]`.
fn bundled_config_home(launcher: &Path) -> Option<PathBuf> {
    let prefix = launcher.parent()?.parent()?;
    Some(prefix.join("config").join("gentle-ai"))
}

/// The marker a finished adoption leaves in the new config folder.
const ADOPTED_MARKER: &str = ".adopted-standalone-config";

/// Gentle Shell's self-contained install (4.7) keeps its config in
/// `<prefix>/config/gentle-ai` and never reads the folder an older launcher
/// kept beside `~/.gentle-shell/agent` (`~/.gentle-shell/gentle-ai`): after
/// upgrading, profiles, model routing and permission defaults silently stop
/// applying. Copy over once what the new folder lacks; nothing is overwritten
/// and the old folder is left as it was. Returns what was copied.
pub fn adopt_standalone_config() -> std::io::Result<Vec<PathBuf>> {
    let homes = resolve_homes();
    let legacy = homes.agent.parent().map(|parent| parent.join("gentle-ai"));
    match legacy {
        Some(legacy) if legacy != homes.config && legacy.is_dir() => {
            adopt_into(&legacy, &homes.config)
        }
        _ => Ok(Vec::new()),
    }
}

/// Copy into `target` every file of `legacy` it doesn't have yet, recursively
/// (lock and temporary files skipped), once: a marker in `target` keeps a
/// file the user later deletes from coming back.
fn adopt_into(legacy: &Path, target: &Path) -> std::io::Result<Vec<PathBuf>> {
    if target.join(ADOPTED_MARKER).exists() {
        return Ok(Vec::new());
    }
    let mut copied = Vec::new();
    copy_missing(legacy, target, &mut copied)?;
    std::fs::write(
        target.join(ADOPTED_MARKER),
        format!("copied from {}\n", legacy.display()),
    )?;
    Ok(copied)
}

fn copy_missing(from: &Path, to: &Path, copied: &mut Vec<PathBuf>) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let text = name.to_string_lossy();
        if text.ends_with(".lock") || text.ends_with(".tmp") || text == ADOPTED_MARKER {
            continue;
        }
        let (source, dest) = (entry.path(), to.join(&name));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_missing(&source, &dest, copied)?;
        } else if kind.is_file() && !dest.exists() {
            // `fs::copy` keeps the permission bits (0600 policies stay private).
            std::fs::copy(&source, &dest)?;
            copied.push(dest);
        }
    }
    Ok(())
}

enum ConfigMode {
    Link,
    Isolated,
    Path(PathBuf),
}

fn read_config_mode() -> Option<ConfigMode> {
    let raw = std::fs::read_to_string(launcher_config_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    match value["mode"].as_str()? {
        "link" => Some(ConfigMode::Link),
        "isolated" => Some(ConfigMode::Isolated),
        "path" => value["dir"]
            .as_str()
            .map(|d| ConfigMode::Path(PathBuf::from(d))),
        _ => None,
    }
}

/// The `@earendil-works/pi-coding-agent` package next to the resolved
/// launcher (`<gentle-pi>/bin/gentle-shell.mjs`), nested or hoisted.
pub fn pi_package(spec: &LaunchSpec) -> Option<PathBuf> {
    let launcher = dunce::canonicalize(&spec.program).unwrap_or_else(|_| spec.program.clone());
    let mut roots: Vec<PathBuf> = Vec::new();
    // Unix: the npm bin is a symlink into <gentle-pi>/bin/.
    if let Some(root) = launcher.parent().and_then(Path::parent) {
        roots.push(root.to_path_buf());
        // Self-contained install (4.7): `<prefix>/bin/gentle-shell[.cmd]` is a
        // generated script; the active version lives in `<prefix>/current`.
        roots.push(root.join("current").join("node_modules").join("gentle-pi"));
    }
    // Windows: `<prefix>/gentle-shell.cmd` beside `<prefix>/node_modules/gentle-pi`.
    if let Some(dir) = spec.program.parent() {
        roots.push(dir.join("node_modules").join("gentle-pi"));
        roots.push(
            dir.join("..")
                .join("lib")
                .join("node_modules")
                .join("gentle-pi"),
        );
    }
    for root in roots {
        let nested = root
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-coding-agent");
        let hoisted = root
            .join("..")
            .join("@earendil-works")
            .join("pi-coding-agent");
        for candidate in [nested, hoisted] {
            if candidate.join("dist").join("index.js").is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::{Duration, SystemTime};

    #[test]
    fn standalone_config_is_adopted_once_without_overwriting() {
        let root = std::env::temp_dir().join(format!("gs-adopt-{}", std::process::id()));
        let (legacy, target) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(legacy.join("backups")).unwrap();
        std::fs::write(legacy.join("profiles.json"), "old profiles").unwrap();
        std::fs::write(legacy.join("star-prompt.json"), "old star").unwrap();
        std::fs::write(legacy.join("state.json.lock"), "").unwrap();
        std::fs::write(legacy.join("backups").join("a.json"), "b").unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("star-prompt.json"), "new star").unwrap();

        let copied = adopt_into(&legacy, &target).unwrap();
        assert_eq!(copied.len(), 2, "{copied:?}");
        assert_eq!(
            std::fs::read_to_string(target.join("profiles.json")).unwrap(),
            "old profiles"
        );
        // What the new folder already had wins; locks never move.
        assert_eq!(
            std::fs::read_to_string(target.join("star-prompt.json")).unwrap(),
            "new star"
        );
        assert!(!target.join("state.json.lock").exists());
        assert!(target.join("backups").join("a.json").exists());
        // Only once: a file deleted afterwards stays deleted.
        std::fs::remove_file(target.join("profiles.json")).unwrap();
        assert!(adopt_into(&legacy, &target).unwrap().is_empty());
        assert!(!target.join("profiles.json").exists());
        // The old folder is untouched.
        assert!(legacy.join("profiles.json").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bundled_install_keeps_its_config_under_its_prefix() {
        assert_eq!(
            bundled_config_home(Path::new("/home/me/.gentle-shell/bin/gentle-shell")),
            Some(PathBuf::from("/home/me/.gentle-shell/config/gentle-ai"))
        );
        assert_eq!(
            bundled_config_home(Path::new("/Local/gentle-shell/bin/gentle-shell.cmd")),
            Some(PathBuf::from("/Local/gentle-shell/config/gentle-ai"))
        );
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn memo_recomputes_only_when_the_config_changes() {
        let path = std::env::temp_dir().join(format!("pi-rpc-homes-{}.json", std::process::id()));
        let slot = Mutex::new(None);
        let read = || std::fs::read_to_string(&path).ok();
        assert_eq!(memo_by_stamp(&slot, &path, read), None);

        std::fs::write(&path, "link").unwrap();
        set_mtime(&path, 2_000_000);
        assert_eq!(memo_by_stamp(&slot, &path, read).as_deref(), Some("link"));

        // Same mtime and length: the cached resolution stands.
        std::fs::write(&path, "path").unwrap();
        set_mtime(&path, 2_000_000);
        assert_eq!(memo_by_stamp(&slot, &path, read).as_deref(), Some("link"));

        // `gentle-shell home` rewrote it.
        set_mtime(&path, 2_000_001);
        assert_eq!(memo_by_stamp(&slot, &path, read).as_deref(), Some("path"));

        std::fs::remove_file(&path).unwrap();
        assert_eq!(memo_by_stamp(&slot, &path, read), None);
    }

    #[test]
    fn pi_package_is_found_in_a_self_contained_install() {
        let prefix = std::env::temp_dir().join(format!("pi-rpc-bundled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&prefix);
        let launcher = prefix.join("bin").join("gentle-shell");
        let pi = prefix
            .join("current")
            .join("node_modules")
            .join("@earendil-works")
            .join("pi-coding-agent");
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::create_dir_all(pi.join("dist")).unwrap();
        std::fs::create_dir_all(prefix.join("current/node_modules/gentle-pi/bin")).unwrap();
        std::fs::write(&launcher, "#!/bin/sh\n").unwrap();
        std::fs::write(pi.join("dist").join("index.js"), "").unwrap();
        let spec = LaunchSpec {
            program: launcher,
            args: Vec::new(),
            cwd: prefix.clone(),
            env: Vec::new(),
        };
        let found = pi_package(&spec).map(|p| dunce::canonicalize(p).unwrap());
        assert_eq!(found, Some(dunce::canonicalize(&pi).unwrap()));
        std::fs::remove_dir_all(&prefix).unwrap();
    }
}
