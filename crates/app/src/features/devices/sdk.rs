//! The platform tools behind the device hub: the Android SDK (emulator, adb)
//! and Xcode's `simctl`. The hub drives them; the app only finds them, puts
//! the SDK on the agents' PATH and lists AVDs the hub hasn't seen yet.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a listed set of AVDs is reused: the list is read on every poll,
/// and listing spawns the emulator binary.
const AVDS_TTL: Duration = Duration::from_secs(60);

static AVDS: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn sdk_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(dir) = std::env::var_os(key).filter(|dir| !dir.is_empty()) {
            roots.push(PathBuf::from(dir));
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    roots.push(home.join("Library").join("Android").join("sdk"));
    roots.push(home.join("Android").join("Sdk"));
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Android").join("Sdk"));
    }
    roots.push(PathBuf::from("/usr/local/share/android-sdk"));
    roots.push(PathBuf::from(
        "/opt/homebrew/share/android-commandlinetools",
    ));
    roots.dedup();
    roots
}

/// The first Android SDK folder that has adb or the emulator.
pub(crate) fn android_sdk() -> Option<PathBuf> {
    sdk_roots().into_iter().find(|dir| {
        dir.join("platform-tools").join(exe("adb")).is_file()
            || dir.join("emulator").join(exe("emulator")).is_file()
    })
}

/// The SDK's `emulator` and `platform-tools` folders, for the agents' (and
/// the hub's) PATH: Android Studio installs them (on Windows under
/// `%LOCALAPPDATA%\Android\Sdk`) without putting either on the PATH.
pub(crate) fn android_tool_dirs() -> Vec<PathBuf> {
    android_sdk()
        .map(|sdk| vec![sdk.join("emulator"), sdk.join("platform-tools")])
        .unwrap_or_default()
}

/// Every AVD `emulator -list-avds` knows; empty without the SDK. Reused for
/// [`AVDS_TTL`] unless [`forget_avds`] runs.
pub(super) fn avds() -> Vec<String> {
    let cached = AVDS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, avds)) = cached.as_ref()
        && at.elapsed() < AVDS_TTL
    {
        return avds.clone();
    }
    drop(cached);
    let avds = list_avds();
    *AVDS.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), avds.clone()));
    avds
}

/// The next [`avds`] lists again (an explicit refresh, a boot).
pub(super) fn forget_avds() {
    *AVDS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn list_avds() -> Vec<String> {
    let Some(emulator) = android_sdk()
        .map(|sdk| sdk.join("emulator").join(exe("emulator")))
        .filter(|path| path.is_file())
        .or_else(|| pi_rpc::find_program("emulator"))
    else {
        return Vec::new();
    };
    let Ok(mut child) = pi_rpc::command(&emulator)
        .arg("-list-avds")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read as _;
        let _ = stdout.read_to_string(&mut out);
    }
    gentle_core::devices::parse_avd_list(&out)
}

/// Xcode's `simctl`. `xcode-select` may point at the Command Line Tools,
/// which have no simctl, so developer folders are tried directly.
pub(crate) fn simctl_path() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // Xcode does not usually move while the app runs; resolve it once.
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(resolve_simctl).clone()
}

fn resolve_simctl() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("DEVELOPER_DIR").filter(|dir| !dir.is_empty()) {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from("/Applications/Xcode.app/Contents/Developer"));
    // `xcode-select -p` is the last resort because it runs a process.
    if let Ok(output) = Command::new("xcode-select").arg("-p").output()
        && output.status.success()
    {
        let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !dir.is_empty() {
            roots.push(PathBuf::from(dir));
        }
    }
    roots
        .into_iter()
        .map(|dir| dir.join("usr").join("bin").join("simctl"))
        .find(|path| path.is_file())
        // A PATH that already has simctl (Homebrew, a wrapper) is fine too.
        .or_else(|| pi_rpc::find_program("simctl"))
}

/// Xcode's developer folder when `xcode-select` points at the Command Line
/// Tools (which have no simctl) while Xcode is installed: tools that call
/// `xcrun simctl` (Maestro, the hub) need it as `DEVELOPER_DIR` to see the
/// simulators.
pub(crate) fn xcode_developer_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let found = Command::new("xcrun")
        .args(["--find", "simctl"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if found {
        return None;
    }
    // `<Developer>/usr/bin/simctl`
    simctl_path()?
        .ancestors()
        .nth(3)
        .filter(|dir| dir.ends_with("Developer"))
        .map(PathBuf::from)
}
