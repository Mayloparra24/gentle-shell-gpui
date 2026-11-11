//! Devices: Expo's device hub (live, interactive iOS simulators and Android
//! emulators in the app) and Callstack's `agent-device` (how agents tap,
//! type and check them), the versions T3 Code pins, installed with npm into
//! the app's own toolchain. Plus what they need from the machine (Node 22.12,
//! Xcode, the Android SDK, ffmpeg for emulator video) and the two switches
//! in Configuration → Devices. The hub process itself is `features::devices`.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::probe::{capture, parse_version, run_logged};
use super::{Log, refresh_paths, toolchain_dir};

/// An npm package the app installs at a pinned version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
    /// Its `bin` script, run with Node.
    entry: &'static str,
}

/// Serves each device's live screen and input over loopback.
pub const HUB: Package = Package {
    name: "expo-device-hub",
    version: "0.22.1",
    entry: "dist/server/cli.mjs",
};

/// The CLI agents drive a device with (accessibility snapshots, taps, text).
pub const AGENT: Package = Package {
    name: "agent-device",
    version: "0.21.36",
    entry: "bin/agent-device.mjs",
};

/// agent-device's `engines.node`; the hub needs 20, so this covers both.
const MIN_NODE: (u64, u64, u64) = (22, 12, 0);

impl Package {
    /// `<toolchain>/devices/<name>-<version>`, an npm prefix of its own.
    fn prefix(&self) -> Option<PathBuf> {
        toolchain_dir().map(|t| {
            t.join("devices")
                .join(format!("{}-{}", self.name, self.version))
        })
    }

    /// The script to run with Node, once installed.
    pub fn entry(&self) -> Option<PathBuf> {
        self.prefix()
            .map(|p| p.join("node_modules").join(self.name).join(self.entry))
            .filter(|path| path.is_file())
    }
}

/// `node_modules/.bin` of agent-device's prefix, for the agents' PATH.
pub fn agent_bin_dir() -> Option<PathBuf> {
    AGENT
        .entry()
        .and(AGENT.prefix())
        .map(|p| p.join("node_modules").join(".bin"))
}

fn node() -> Option<PathBuf> {
    pi_rpc::find_program("node")
}

/// The Node the hub and agent-device run on, and its version.
pub fn node_version() -> Option<(PathBuf, (u64, u64, u64))> {
    let node = node()?;
    let (ok, out) = capture(&node, &["--version"])?;
    let version = ok.then(|| parse_version(out.trim().trim_start_matches('v')))??;
    Some((node, version))
}

/// The two switches in Configuration → Devices, in the app's settings file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    /// Show simulators and emulators in the device panel (runs the hub).
    pub enabled: bool,
    /// New agent sessions get `gentle-device`, `agent-device` and the
    /// instructions to use them.
    pub agents: bool,
}

pub fn prefs() -> Prefs {
    let Some(value) = crate::shared::themes::store_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return Prefs::default();
    };
    let devices = &value["devices"];
    Prefs {
        enabled: devices["enabled"] == true,
        agents: devices["agents"] == true,
    }
}

pub fn save_prefs(prefs: Prefs) -> Result<(), String> {
    let path = crate::shared::themes::store_path().ok_or("This system has no app data folder.")?;
    crate::shared::themes::write_key(
        &path,
        "devices",
        json!({"enabled": prefs.enabled, "agents": prefs.agents}),
    )
    .map_err(|e| format!("Couldn't save the device settings: {e}"))
}

/// What Configuration → Devices shows.
#[derive(Clone, Debug, PartialEq)]
pub struct DevicesStatus {
    pub node: Option<(PathBuf, (u64, u64, u64))>,
    pub hub: Option<PathBuf>,
    pub agent: Option<PathBuf>,
    /// iOS simulators can stream: Apple silicon with Xcode's simctl.
    pub ios: Result<(), String>,
    /// The Android SDK's folder, when one is installed.
    pub android_sdk: Option<PathBuf>,
    /// Emulator video is encoded with ffmpeg (libx264).
    pub ffmpeg: Option<PathBuf>,
}

impl DevicesStatus {
    pub fn node_ok(&self) -> bool {
        self.node.as_ref().is_some_and(|(_, v)| *v >= MIN_NODE)
    }
}

/// Blocking: looks at the machine.
pub fn status() -> DevicesStatus {
    let ios = if !cfg!(target_os = "macos") {
        Err("iOS simulators need a Mac.".to_string())
    } else if std::env::consts::ARCH != "aarch64" {
        Err("Streaming iOS simulators needs Apple silicon.".to_string())
    } else if crate::features::devices::simctl_path().is_none() {
        Err("Install Xcode for iOS simulators.".to_string())
    } else {
        Ok(())
    };
    DevicesStatus {
        node: node_version(),
        hub: HUB.entry(),
        agent: AGENT.entry(),
        ios,
        android_sdk: crate::features::devices::android_sdk(),
        ffmpeg: pi_rpc::find_program("ffmpeg"),
    }
}

/// How to install ffmpeg here, to run in a terminal tab.
pub fn ffmpeg_command() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew install ffmpeg"
    } else if cfg!(windows) {
        "winget install --id Gyan.FFmpeg -e"
    } else {
        "sudo apt install ffmpeg"
    }
}

fn install_package(package: Package, log: &Log) -> Result<(), String> {
    if package.entry().is_some() {
        return Ok(());
    }
    let (_, version) = node_version().ok_or("Node.js isn't installed (Setup & requirements).")?;
    if version < MIN_NODE {
        return Err(format!(
            "{} needs Node.js {}.{} or newer; this one is {}.{}.{}.",
            package.name, MIN_NODE.0, MIN_NODE.1, version.0, version.1, version.2
        ));
    }
    let npm = pi_rpc::find_program("npm").ok_or("npm isn't installed (Setup & requirements).")?;
    let prefix = package
        .prefix()
        .ok_or("This system has no app data folder.")?;
    std::fs::create_dir_all(&prefix)
        .map_err(|e| format!("Couldn't create {}: {e}", prefix.display()))?;
    log(format!("Installing {} {}…", package.name, package.version));
    let mut command = pi_rpc::command(&npm);
    command
        .arg("install")
        .arg("--prefix")
        .arg(&prefix)
        .arg(format!("{}@{}", package.name, package.version))
        .args(["--no-audit", "--no-fund", "--omit=dev"]);
    run_logged(command, &format!("npm install {}", package.name), log)?;
    package.entry().map(|_| ()).ok_or_else(|| {
        format!(
            "npm finished, but {} isn't where it should be.",
            package.name
        )
    })
}

/// The device hub, for "Use devices".
pub fn install_hub(log: &Log) -> Result<(), String> {
    install_package(HUB, log)
}

/// agent-device, for "Agents can use devices"; on the agents' PATH after.
pub fn install_agent(log: &Log) -> Result<(), String> {
    install_package(AGENT, log)?;
    refresh_paths();
    Ok(())
}

/// The command an agent runs a device with: Node on agent-device's script,
/// pinned to `platform` and the device's UDID or adb serial.
pub fn agent_device_prefix(ios: bool, id: &str) -> String {
    let flag = if ios { "--udid" } else { "--serial" };
    format!(
        "agent-device --platform {} {flag} {id}",
        if ios { "ios" } else { "android" }
    )
}

/// The node binary and script that run the hub.
pub fn hub_command() -> Option<(PathBuf, PathBuf)> {
    Some((node()?, HUB.entry()?))
}
