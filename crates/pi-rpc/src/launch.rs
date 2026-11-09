use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::file_cache::Stamp;

/// A `Command` for a background child of this GUI app.
///
/// On Windows it sets `CREATE_NO_WINDOW`: a release build has no console, so
/// every console child (git, node, shells, the backend) would flash a console
/// window of its own, and a debug build's children would attach to the
/// launching terminal. Elsewhere it is plain `Command::new`.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = std::process::Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    std::process::Command::new(program)
}

/// How to start one RPC backend process.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, OsString)>,
}

/// Resolve `gentle-shell` and build the RPC launch for `cwd`.
///
/// Order: `$GENTLE_SHELL_BIN`, then Gentle Shell's self-contained install
/// ([`bundled_launcher`]), then `gentle-shell` on the PATH. GUI launches
/// (Finder, a desktop entry) inherit a minimal PATH without the user's Node
/// version manager, so on Unix the login shell's PATH is merged in first; the
/// child gets the same PATH so the launcher's `#!/usr/bin/env node` resolves.
pub fn resolve_backend(cwd: &Path) -> Result<LaunchSpec> {
    let path = search_path();
    let program = match std::env::var_os("GENTLE_SHELL_BIN") {
        Some(bin) if !bin.is_empty() => PathBuf::from(bin),
        _ => match bundled_launcher() {
            Some(program) => program,
            None => match which::which_in("gentle-shell", Some(&path), cwd) {
                Ok(program) => program,
                Err(_) => bail!(
                    "gentle-shell was not found on your PATH.\n\
                     Install it with `npm i -g gentle-pi`, or set GENTLE_SHELL_BIN to the launcher."
                ),
            },
        },
    };
    // Launcher options such as `--link` or `--home <dir>` go before the Pi args.
    let mut args = extra_args();
    args.extend(["--mode".to_string(), "rpc".to_string()]);
    Ok(LaunchSpec {
        program,
        args,
        cwd: cwd.to_path_buf(),
        env: vec![
            // Turns on ask_user_question dialogs and live helper activity for
            // interactive RPC hosts (gentle-shell docs: "Interactive RPC hosts").
            ("GENTLE_SHELL_INTERACTIVE_HOST".into(), "1".into()),
            ("PATH".into(), path),
        ],
    })
}

/// Gentle Shell's self-contained install, when present: what its installer
/// and `gentle-shell upgrade` (since 4.7) set up, with its own Node and Pi.
/// It wins over a global npm `gentle-pi` found on the PATH, which upgrading
/// leaves behind and which may come first there.
pub fn bundled_launcher() -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    bundled_candidates(
        &crate::home::home_dir(),
        local_app_data.as_deref(),
        cfg!(windows),
    )
    .into_iter()
    .find(|launcher| launcher.is_file())
}

/// Where the self-contained install keeps its launcher: `~/.gentle-shell/bin`
/// on macOS and Linux; on Windows `%LOCALAPPDATA%\gentle-shell\bin`, or
/// `%USERPROFILE%\.gentle-shell-bundle\bin` when the first folder was refused.
fn bundled_candidates(home: &Path, local_app_data: Option<&Path>, windows: bool) -> Vec<PathBuf> {
    if windows {
        let launcher = Path::new("bin").join("gentle-shell.cmd");
        local_app_data
            .map(|dir| dir.join("gentle-shell").join(&launcher))
            .into_iter()
            .chain([home.join(".gentle-shell-bundle").join(&launcher)])
            .collect()
    } else {
        vec![home.join(".gentle-shell").join("bin").join("gentle-shell")]
    }
}

/// Launcher options from `GENTLE_SHELL_ARGS`.
pub(crate) fn extra_args() -> Vec<String> {
    std::env::var("GENTLE_SHELL_ARGS")
        .map(|extra| extra.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// `node` on the merged search PATH (Gentle Shell requires it).
pub fn resolve_node(cwd: &Path) -> Result<PathBuf> {
    which::which_in("node", Some(search_path()), cwd)
        .map_err(|_| anyhow::anyhow!("node was not found on your PATH"))
}

/// `name` on the merged search PATH (with the app's extra dirs first).
pub fn find_program(name: &str) -> Option<PathBuf> {
    let cwd = std::env::temp_dir();
    which::which_in(name, Some(search_path()), cwd).ok()
}

/// The merged search PATH handed to every child.
pub fn child_path() -> OsString {
    search_path()
}

/// Directories searched before everything else (toolchains the app installed
/// into folders it owns). Replaces the previous list.
pub fn set_extra_path(dirs: Vec<PathBuf>) {
    *EXTRA_PATH.lock().unwrap_or_else(|e| e.into_inner()) = dirs;
}

/// Forget everything resolved from the environment: cached `--version`
/// output and the login shell's PATH (both recomputed on next use), after
/// something was installed.
pub fn forget_cached() {
    forget_versions();
    *BASE_PATH.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The launcher's own version line (`gentle-shell 4.6.0` → `4.6.0`).
pub fn backend_version(spec: &LaunchSpec) -> Option<String> {
    let text = version_output(spec)?;
    let line = text.lines().find(|l| l.starts_with("gentle-shell "))?;
    Some(line.trim_start_matches("gentle-shell ").trim().to_string())
}

/// Which launcher build, and which home config, a cached `--version` ran against.
#[derive(Clone, Copy, PartialEq, Eq)]
struct VersionKey {
    program: Stamp,
    /// `~/.gentle-shell/config.json`: its `home` line follows `gentle-shell home`.
    config: Option<Stamp>,
}

static VERSIONS: Mutex<BTreeMap<PathBuf, (VersionKey, String)>> = Mutex::new(BTreeMap::new());
/// Held while a probe runs, so concurrent tab starts share one run.
static VERSION_RUN: Mutex<()> = Mutex::new(());

/// Everything `<launcher> --version` prints on stdout, cached per launcher
/// until the launcher file (an upgrade) or the home config changes.
pub fn version_output(spec: &LaunchSpec) -> Option<String> {
    let program = dunce::canonicalize(&spec.program).unwrap_or_else(|_| spec.program.clone());
    let key = || {
        Some(VersionKey {
            program: Stamp::of(&program)?,
            config: Stamp::of(&crate::home::launcher_config_path()),
        })
    };
    let cached = |key: VersionKey| {
        let versions = VERSIONS.lock().unwrap_or_else(|e| e.into_inner());
        versions
            .get(&program)
            .filter(|(k, _)| *k == key)
            .map(|(_, text)| text.clone())
    };
    if let Some(text) = key().and_then(cached) {
        return Some(text);
    }
    let _run = VERSION_RUN.lock().unwrap_or_else(|e| e.into_inner());
    // Stat before running, so a change mid-run only costs another run later.
    let key = key();
    if let Some(text) = key.and_then(cached) {
        return Some(text);
    }
    let text = run_version(spec)?;
    if let Some(key) = key {
        VERSIONS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(program, (key, text.clone()));
    }
    Some(text)
}

/// Drop every cached `--version` (after `gentle-shell upgrade`).
pub fn forget_versions() {
    VERSIONS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn run_version(spec: &LaunchSpec) -> Option<String> {
    let mut command = crate::command(&spec.program);
    command
        .arg("--version")
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    let out = command.output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

static EXTRA_PATH: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The login shell's PATH merged ahead of the inherited one, computed on
/// first use and again after `forget_cached`.
static BASE_PATH: Mutex<Option<OsString>> = Mutex::new(None);

fn base_path() -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(login) = login_shell_path() {
        dirs.extend(std::env::split_paths(&login));
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    merge_paths(dirs, &current).unwrap_or(current)
}

/// `first`, then every dir of `rest` not already in it.
fn merge_paths(first: Vec<PathBuf>, rest: &OsString) -> Option<OsString> {
    let mut dirs = first;
    for dir in std::env::split_paths(rest) {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).ok()
}

fn search_path() -> OsString {
    // Held while the login shell runs, so concurrent callers share one run.
    let base = BASE_PATH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(base_path)
        .clone();
    let extra = EXTRA_PATH.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if extra.is_empty() {
        return base;
    }
    merge_paths(extra, &base).unwrap_or(base)
}

#[cfg(unix)]
fn login_shell_path() -> Option<OsString> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    const START: &str = "__GENTLE_PATH_START__";
    const END: &str = "__GENTLE_PATH_END__";
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    // Interactive login so rc files that set up nvm/fnm/mise run; markers fence
    // the value from anything those files print.
    let mut child = crate::command(&shell)
        .args([
            "-l",
            "-i",
            "-c",
            &format!("printf '{START}%s{END}' \"$PATH\""),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    let start = out.find(START)? + START.len();
    let end = out[start..].find(END)? + start;
    Some(OsString::from(&out[start..end]))
}

#[cfg(not(unix))]
fn login_shell_path() -> Option<OsString> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs::File;
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, SystemTime};

    #[test]
    fn bundled_install_lives_in_gentle_shells_own_folder() {
        let home = Path::new("/home/me");
        assert_eq!(
            bundled_candidates(home, None, false),
            [PathBuf::from("/home/me/.gentle-shell/bin/gentle-shell")]
        );
        // Windows: LOCALAPPDATA first, the fallback folder after it.
        let local = Path::new("/home/me/AppData/Local");
        assert_eq!(
            bundled_candidates(home, Some(local), true),
            [
                local
                    .join("gentle-shell")
                    .join("bin")
                    .join("gentle-shell.cmd"),
                home.join(".gentle-shell-bundle")
                    .join("bin")
                    .join("gentle-shell.cmd"),
            ]
        );
        assert_eq!(bundled_candidates(home, None, true).len(), 1);
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn extra_dirs_come_first_without_duplicates() {
        let base = OsString::from("/usr/bin:/opt/x/bin:/bin");
        let merged = merge_paths(vec!["/opt/x/bin".into(), "/tc/node/bin".into()], &base).unwrap();
        assert_eq!(
            merged,
            OsString::from("/opt/x/bin:/tc/node/bin:/usr/bin:/bin")
        );
    }

    #[test]
    fn version_is_cached_until_the_launcher_changes() {
        let dir = std::env::temp_dir().join(format!("pi-rpc-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let runs = dir.join("runs");
        let program = dir.join("gentle-shell");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho x >> '{}'\necho 'gentle-shell 1.2.3'\necho 'home isolated /h'\n",
                runs.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        set_mtime(&program, 3_000_000);
        let spec = LaunchSpec {
            program: program.clone(),
            args: Vec::new(),
            cwd: dir.clone(),
            env: Vec::new(),
        };
        let run_count = || std::fs::read_to_string(&runs).map_or(0, |s| s.lines().count());

        assert_eq!(backend_version(&spec).as_deref(), Some("1.2.3"));
        assert_eq!(
            version_output(&spec).as_deref(),
            Some("gentle-shell 1.2.3\nhome isolated /h\n")
        );
        assert_eq!(run_count(), 1);

        // An upgrade rewrites the launcher.
        set_mtime(&program, 3_000_001);
        assert_eq!(backend_version(&spec).as_deref(), Some("1.2.3"));
        assert_eq!(run_count(), 2);

        forget_versions();
        assert_eq!(backend_version(&spec).as_deref(), Some("1.2.3"));
        assert_eq!(run_count(), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
