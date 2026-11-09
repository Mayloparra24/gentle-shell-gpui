//! Running tools (captured with a deadline, or streamed into the log),
//! version parsing, and the platform label.

use std::io::{BufRead as _, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use super::Log;

/// `(major, minor, patch)`.
pub type Version = (u64, u64, u64);

/// `v22.19.0`, `22.19`, `go1.25.10`, `1.25rc1` → numbers; missing parts are 0.
pub fn parse_version(text: &str) -> Option<Version> {
    let text = text.trim();
    let text = text.strip_prefix("go").unwrap_or(text);
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = [0u64; 3];
    let mut found = 0;
    for (i, part) in text.split('.').take(3).enumerate() {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        parts[i] = digits.parse().ok()?;
        found = i + 1;
        if digits.len() != part.len() {
            break;
        }
    }
    (found > 0).then_some((parts[0], parts[1], parts[2]))
}

pub fn show_version(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

/// `go version go1.25.10 windows/amd64` → `go1.25.10`'s version.
pub fn go_version(output: &str) -> Option<Version> {
    output
        .split_whitespace()
        .find(|w| w.starts_with("go") && w[2..].starts_with(|c: char| c.is_ascii_digit()))
        .and_then(parse_version)
}

fn hide_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// A tool's stdout and whether it succeeded, run on the merged PATH; `None`
/// when it can't start or outlives the deadline (it is killed then).
pub fn capture(program: &Path, args: &[&str]) -> Option<(bool, String)> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("PATH", pi_rpc::child_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_window(&mut command);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    Some((status.success(), reader.join().ok()?))
}

/// The trimmed stdout of a successful run.
pub fn output_of(program: &Path, args: &[&str]) -> Option<String> {
    capture(program, args)
        .filter(|(ok, _)| *ok)
        .map(|(_, text)| text.trim().to_string())
}

static ANSI: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)").unwrap()
});

/// Every output line, without colors; a line redrawn with `\r` (progress
/// bars, spinners) only reports its final state.
fn pump(stream: impl Read, log: &Log) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let last = text
                    .split('\r')
                    .map(|part| ANSI.replace_all(part, "").trim_end().to_string())
                    .rfind(|part| part.trim().chars().count() > 1);
                if let Some(line) = last {
                    log(line);
                }
            }
        }
    }
}

/// Run `command` (stdin closed, merged PATH), streaming stdout and stderr
/// lines into `log`; an error names `what` and the exit code.
pub fn run_logged(mut command: Command, what: &str, log: &Log) -> Result<(), String> {
    command
        .env("PATH", pi_rpc::child_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_window(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| format!("Couldn't start {what}: {e}"))?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    std::thread::scope(|scope| {
        if let Some(out) = stdout {
            scope.spawn(move || pump(out, log));
        }
        if let Some(err) = stderr {
            scope.spawn(move || pump(err, log));
        }
    });
    let status = child
        .wait()
        .map_err(|e| format!("{what} didn't finish: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(match status.code() {
            Some(code) => format!("{what} failed (exit code {code})."),
            None => format!("{what} was stopped."),
        })
    }
}

// ── Platform ───────────────────────────────────────────────────────────────

fn arch_label(arch: &str) -> &str {
    match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// A `KEY=value` from `/etc/os-release`, unquoted.
pub fn os_release_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.strip_prefix(key)?.strip_prefix('=')?.trim();
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// `Ubuntu 24.04` from `/etc/os-release`.
pub fn linux_name(os_release: &str) -> String {
    match (
        os_release_value(os_release, "NAME"),
        os_release_value(os_release, "VERSION_ID"),
    ) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        _ => os_release_value(os_release, "PRETTY_NAME")
            .or_else(|| os_release_value(os_release, "NAME"))
            .unwrap_or_else(|| "Linux".into()),
    }
}

/// `Microsoft Windows [Version 10.0.22631.4602]` → `Windows 11`.
pub fn windows_name(ver: &str) -> String {
    let numbers = ver
        .split("Version")
        .nth(1)
        .map(|rest| rest.trim_matches(|c: char| c == ' ' || c == ']' || c == '\r' || c == '\n'))
        .map(|v| {
            v.split('.')
                .map(|p| p.parse::<u64>().ok())
                .collect::<Vec<_>>()
        });
    match numbers.as_deref() {
        Some([Some(10), Some(0), Some(build), ..]) if *build >= 22000 => "Windows 11".into(),
        Some([Some(10), Some(0), ..]) => "Windows 10".into(),
        _ => "Windows".into(),
    }
}

pub fn platform_label() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => {
            let version = output_of(Path::new("/usr/bin/sw_vers"), &["-productVersion"])
                .filter(|v| !v.is_empty())
                .map(|v| format!(" {v}"))
                .unwrap_or_default();
            let chip = if arch == "aarch64" {
                "Apple silicon"
            } else {
                "Intel"
            };
            format!("macOS{version} · {chip}")
        }
        "windows" => {
            let ver = output_of(Path::new("cmd"), &["/c", "ver"]).unwrap_or_default();
            format!("{} · {}", windows_name(&ver), arch_label(arch))
        }
        "linux" => {
            let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
            format!("{} · {}", linux_name(&release), arch_label(arch))
        }
        other => format!("{other} · {}", arch_label(arch)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_compares_versions() {
        assert_eq!(parse_version("v22.19.0"), Some((22, 19, 0)));
        assert_eq!(parse_version("22.19"), Some((22, 19, 0)));
        assert_eq!(parse_version("go1.25.10"), Some((1, 25, 10)));
        assert_eq!(parse_version("1.25rc1"), Some((1, 25, 0)));
        assert_eq!(parse_version("not found"), None);
        assert!(parse_version("v22.18.9") < parse_version("22.19"));
        assert!(parse_version("v22.21.1") >= parse_version("v22.19.0"));
        assert!(parse_version("go1.25.9") < parse_version("go1.25.10"));
        assert_eq!(
            go_version("go version go1.25.10 windows/amd64"),
            Some((1, 25, 10))
        );
        assert_eq!(go_version("nothing"), None);
        assert_eq!(show_version((22, 19, 0)), "22.19.0");
    }

    #[test]
    fn names_platforms() {
        let ubuntu = "PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\n";
        assert_eq!(linux_name(ubuntu), "Ubuntu 24.04");
        assert_eq!(
            linux_name("PRETTY_NAME=\"Arch Linux\"\nNAME=\"Arch Linux\"\n"),
            "Arch Linux"
        );
        assert_eq!(linux_name(""), "Linux");
        assert_eq!(
            windows_name("\r\nMicrosoft Windows [Version 10.0.22631.4602]\r\n"),
            "Windows 11"
        );
        assert_eq!(
            windows_name("Microsoft Windows [Version 10.0.19045.5247]"),
            "Windows 10"
        );
        assert_eq!(windows_name(""), "Windows");
    }
}
