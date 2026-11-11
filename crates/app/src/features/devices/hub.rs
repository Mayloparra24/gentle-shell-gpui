//! The device hub process (Expo's `expo-device-hub`: serve-sim for iOS
//! simulators, serve-emu for Android emulators) and its loopback API. One per
//! app, started when something needs a device. Its own web page shows one
//! device live, with touch, keyboard and hardware buttons; each tab of the
//! device panel loads that page pinned to its device.
//!
//! The hub's port is recorded in `<data>/gentle-shell-gpui/devices/hub.json`
//! so `gentle-device` (the agents' command) can reach it. Blocking; callers
//! run it off the main thread.

use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use gentle_core::devices::{Device, Platform};
use serde_json::json;

use crate::features::setup::devices as setup;

/// The hub answers `/readyz` within a few seconds; Node's first run can take
/// longer on a cold disk.
const START_TIMEOUT: Duration = Duration::from_secs(60);

fn state_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("gentle-shell-gpui").join("devices"))
}

fn state_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("hub.json"))
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .build()
        .into()
}

fn base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The hub on `port` answers.
pub fn ready(port: u16) -> bool {
    agent(Duration::from_secs(1))
        .get(format!("{}/readyz", base(port)))
        .call()
        .is_ok_and(|response| response.status().as_u16() == 200)
}

/// The port of the hub this app runs, when it answers.
pub fn running_port() -> Option<u16> {
    let text = std::fs::read_to_string(state_path()?).ok()?;
    let port = serde_json::from_str::<serde_json::Value>(&text).ok()?["port"].as_u64()? as u16;
    ready(port).then_some(port)
}

/// The running hub process; dropping it stops the hub.
pub struct HubProcess {
    child: Child,
    port: u16,
}

impl HubProcess {
    /// Start the hub on a free loopback port and wait until it answers.
    pub fn start() -> Result<Self, String> {
        let (node, entry) =
            setup::hub_command().ok_or("Turn on Use devices in Configuration → Devices first.")?;
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .map(|addr| addr.port())
            .map_err(|e| format!("No free port for the device hub: {e}"))?;
        let dir = state_dir().ok_or("This system has no app data folder.")?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
        let log = std::fs::File::create(dir.join("hub.log"))
            .map_err(|e| format!("Couldn't open the hub's log: {e}"))?;
        let mut command = pi_rpc::command(&node);
        command
            .arg(&entry)
            .args(["--port", &port.to_string(), "--host", "127.0.0.1"])
            .args(["--hide-sidebar", "--hide-boot-device"])
            .env("PATH", pi_rpc::child_path())
            .env("FORCE_COLOR", "0")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log));
        // serve-sim (iOS) is Apple-silicon macOS only; elsewhere the hub
        // shows Android emulators alone.
        if !(cfg!(target_os = "macos") && std::env::consts::ARCH == "aarch64") {
            command.args(["--platform", "android"]);
        }
        if let Some(sdk) = super::sdk::android_sdk() {
            command
                .env("ANDROID_HOME", &sdk)
                .env("ANDROID_SDK_ROOT", &sdk);
        }
        if let Some(developer) = super::sdk::xcode_developer_dir() {
            command.env("DEVELOPER_DIR", developer);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("The device hub didn't start: {e}"))?;
        let mut hub = Self { child, port };
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if ready(port) {
                break;
            }
            if let Ok(Some(status)) = hub.child.try_wait() {
                let tail = std::fs::read_to_string(dir.join("hub.log")).unwrap_or_default();
                let last = tail
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("");
                return Err(format!("The device hub stopped ({status}). {last}"));
            }
            if Instant::now() > deadline {
                return Err("The device hub didn't answer in time; see devices/hub.log.".into());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        let record = json!({"port": port, "pid": hub.child.id()}).to_string();
        let _ = std::fs::write(dir.join("hub.json"), record);
        Ok(hub)
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Stop agent-device's daemon and everything under it (the iOS runner's
/// xcodebuild, the runner app on the simulator). An agent that never closed
/// its session keeps them alive after the app quits; they idle-stop only
/// once no session is open.
pub fn stop_agent_daemon() {
    let Some((node, entry)) = setup::agent_command() else {
        return;
    };
    let _ = pi_rpc::command(&node)
        .arg(&entry)
        .args(["daemon", "stop", "--clean"])
        .env("PATH", pi_rpc::child_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

impl Drop for HubProcess {
    fn drop(&mut self) {
        if let Some(path) = state_path() {
            let _ = std::fs::remove_file(path);
        }
        // SIGTERM lets the hub stop its helpers (serve-sim, scrcpy) first.
        #[cfg(unix)]
        {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = self.child.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every simulator and emulator, and what the hub couldn't list.
pub fn list(port: u16) -> Result<(Vec<Device>, Vec<String>), String> {
    let mut response = agent(Duration::from_secs(20))
        .get(format!("{}/api/devices", base(port)))
        .call()
        .map_err(|e| format!("The device hub didn't answer: {e}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("The device hub's answer is unreadable: {e}"))?;
    let (devices, mut errors) = gentle_core::devices::hub_devices(&text, &super::sdk::avds())?;
    // Without an Android SDK the hub's Android errors only say so; the
    // panel and Configuration already do.
    if super::sdk::android_sdk().is_none() {
        errors.retain(|error| {
            let error = error.to_lowercase();
            !["android", "avdmanager", "adb", "emulator"]
                .iter()
                .any(|word| error.contains(word))
        });
    }
    Ok((devices, errors))
}

fn action(port: u16, route: &str, device: &Device) -> Result<(), String> {
    let body = json!({
        "platform": match device.platform {
            Platform::Ios => "ios",
            Platform::Android => "android",
        },
        "id": device.id,
        "name": device.name,
    });
    let mut response = agent(Duration::from_secs(180))
        .post(format!("{}/api/devices/{route}", base(port)))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("The device hub didn't answer: {e}"))?;
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if value["ok"] == true {
        Ok(())
    } else {
        Err(value["error"]
            .as_str()
            .or_else(|| value["errors"][0].as_str())
            .unwrap_or("The device hub refused.")
            .to_string())
    }
}

/// Start a device headless (no Simulator.app, no emulator window).
pub fn boot(port: u16, device: &Device) -> Result<(), String> {
    action(port, "boot", device)
}

pub fn shutdown(port: u16, device: &Device) -> Result<(), String> {
    action(port, "shutdown", device)
}

/// The hub's viewer page.
pub fn viewer_url(port: u16) -> String {
    format!("{}/", base(port))
}

/// Pins the viewer to one device, as the hub's `--single-device` does.
pub fn viewer_script(device: &Device) -> String {
    format!(
        "window.__EXPO_DEVICE_HUB_SINGLE_DEVICE__ = {};",
        serde_json::Value::String(device.id.clone())
    )
}
