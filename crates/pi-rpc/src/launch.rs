use anyhow::{Result, bail};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

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
/// Order: `$GENTLE_SHELL_BIN`, then `gentle-shell` on the PATH. GUI launches
/// (Finder, a desktop entry) inherit a minimal PATH without the user's Node
/// version manager, so on Unix the login shell's PATH is merged in first; the
/// child gets the same PATH so the launcher's `#!/usr/bin/env node` resolves.
pub fn resolve_backend(cwd: &Path) -> Result<LaunchSpec> {
    let path = search_path();
    let program = match std::env::var_os("GENTLE_SHELL_BIN") {
        Some(bin) if !bin.is_empty() => PathBuf::from(bin),
        _ => match which::which_in("gentle-shell", Some(&path), cwd) {
            Ok(program) => program,
            Err(_) => bail!(
                "gentle-shell was not found on your PATH.\n\
                 Install it with `npm i -g gentle-pi`, or set GENTLE_SHELL_BIN to the launcher."
            ),
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

/// The merged search PATH handed to every child.
pub fn child_path() -> OsString {
    search_path()
}

/// The launcher's own version line (`gentle-shell 4.6.0` → `4.6.0`).
pub fn backend_version(spec: &LaunchSpec) -> Option<String> {
    let mut command = std::process::Command::new(&spec.program);
    command
        .arg("--version")
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let out = command.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.starts_with("gentle-shell "))?;
    Some(line.trim_start_matches("gentle-shell ").trim().to_string())
}

/// The login shell's PATH merged ahead of the inherited one, computed once.
static SEARCH_PATH: LazyLock<OsString> = LazyLock::new(|| {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(login) = login_shell_path() {
        dirs.extend(std::env::split_paths(&login));
    }
    if let Some(current) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&current) {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
});

fn search_path() -> OsString {
    SEARCH_PATH.clone()
}

#[cfg(unix)]
fn login_shell_path() -> Option<OsString> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const START: &str = "__GENTLE_PATH_START__";
    const END: &str = "__GENTLE_PATH_END__";
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    // Interactive login so rc files that set up nvm/fnm/mise run; markers fence
    // the value from anything those files print.
    let mut child = Command::new(&shell)
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
