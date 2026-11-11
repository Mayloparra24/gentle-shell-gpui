//! Finding and running the emulator tools: adb, the Android emulator and
//! Apple's `simctl`. Everything here blocks, so callers run it on the
//! background executor. Children get `pi_rpc::child_path()` so a GUI launch
//! finds the same SDKs a terminal would.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gentle_core::devices::{
    Device, Platform, State, android_devices, android_launch_args, android_stop_args,
    boot_completed, emulator_port, ios_boot_args, ios_bootstatus_args, ios_stop_args,
    next_emulator_port, parse_adb_devices, parse_avd_list, parse_avd_name, parse_simctl_devices,
    serial_for_port,
};

/// How long a query (`adb devices`, `simctl list`, a screenshot) may take.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an emulator may take to finish booting.
const BOOT_TIMEOUT: Duration = Duration::from_secs(180);
/// The stop command is best effort; never hold a quit for it.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The tools this machine has, resolved once per discovery.
#[derive(Clone, Debug, Default)]
pub struct Tools {
    adb: Option<PathBuf>,
    emulator: Option<PathBuf>,
    simctl: Option<PathBuf>,
    sdk: Option<PathBuf>,
}

/// What discovery found, including the reasons some platform is missing.
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub devices: Vec<Device>,
    pub notes: Vec<String>,
}

/// An emulator process this panel started, kept so it can be killed on quit.
pub struct Launched {
    pub device: Device,
    pub child: Option<Child>,
}

impl Tools {
    /// Find the tools on the PATH and under the usual SDK install folders.
    pub fn detect() -> Self {
        let sdk = find_sdk();
        let from_sdk = |folder: &str, name: &str| {
            sdk.as_ref()
                .map(|sdk| sdk.join(folder).join(exe(name)))
                .filter(|path| path.is_file())
        };
        Self {
            adb: pi_rpc::find_program("adb").or_else(|| from_sdk("platform-tools", "adb")),
            emulator: pi_rpc::find_program("emulator").or_else(|| from_sdk("emulator", "emulator")),
            simctl: simctl_path(),
            sdk,
        }
    }

    pub fn has_android(&self) -> bool {
        self.adb.is_some() || self.emulator.is_some()
    }

    pub fn has_ios(&self) -> bool {
        self.simctl.is_some()
    }

    /// Every AVD and simulator, plus every running emulator. One platform's
    /// failure does not hide the other's devices.
    pub fn discover(&self) -> Discovery {
        let mut discovery = Discovery::default();
        match self.android() {
            Ok((devices, note)) => {
                discovery.devices.extend(devices);
                discovery.notes.extend(note);
            }
            Err(note) => discovery.notes.push(note),
        }
        match self.ios() {
            Ok(devices) => discovery.devices.extend(devices),
            Err(note) => discovery.notes.push(note),
        }
        gentle_core::devices::sort_devices(&mut discovery.devices);
        discovery
    }

    fn android(&self) -> Result<(Vec<Device>, Option<String>), String> {
        if !self.has_android() {
            return Err(
                "No Android SDK found (install Android Studio or the emulator tools).".to_string(),
            );
        }
        let avds = match &self.emulator {
            Some(emulator) => {
                let output = self.run_program(emulator, &["-list-avds"], QUERY_TIMEOUT)?;
                parse_avd_list(&String::from_utf8_lossy(&output))
            }
            None => Vec::new(),
        };
        let Some(adb) = &self.adb else {
            return Ok((
                android_devices(&avds, &[], &[], &[]),
                Some("adb was not found; running emulators cannot be listed.".into()),
            ));
        };
        let output = self.run_program(adb, &["devices", "-l"], QUERY_TIMEOUT)?;
        let adb_devices = parse_adb_devices(&String::from_utf8_lossy(&output));
        let mut names = Vec::new();
        let mut booted = Vec::new();
        for device in adb_devices
            .iter()
            .filter(|device| device.state == "device" && is_emulator(&device.serial))
        {
            if let Ok(output) =
                self.adb_bytes(&["-s", &device.serial, "emu", "avd", "name"], QUERY_TIMEOUT)
                && let Some(name) = parse_avd_name(&String::from_utf8_lossy(&output))
            {
                names.push((device.serial.clone(), name));
            }
            if let Ok(output) = self.adb_bytes(
                &[
                    "-s",
                    &device.serial,
                    "shell",
                    "getprop",
                    "sys.boot_completed",
                ],
                QUERY_TIMEOUT,
            ) {
                booted.push((
                    device.serial.clone(),
                    boot_completed(&String::from_utf8_lossy(&output)),
                ));
            }
        }
        Ok((android_devices(&avds, &adb_devices, &names, &booted), None))
    }

    fn ios(&self) -> Result<Vec<Device>, String> {
        if !cfg!(target_os = "macos") {
            return Ok(Vec::new());
        }
        let Some(simctl) = &self.simctl else {
            return Err("iOS simulators need Xcode (simctl was not found).".to_string());
        };
        let output = self.run_program(
            simctl,
            &["list", "devices", "available", "--json"],
            QUERY_TIMEOUT,
        )?;
        parse_simctl_devices(&String::from_utf8_lossy(&output))
    }

    /// Boot `target` with the cheapest flags it supports. Android returns as
    /// soon as the emulator process starts, with the adb serial it will use;
    /// `wait_android` waits for the system itself.
    pub fn launch(
        &self,
        target: &Device,
        used_ports: &[u16],
        cancel: &AtomicBool,
    ) -> Result<Launched, String> {
        match target.platform {
            Platform::Android => {
                let emulator = self
                    .emulator
                    .as_ref()
                    .ok_or("The Android emulator was not found.")?;
                // Without adb the panel could not tell when boot finished or
                // stop the process later, so do not start one it cannot see.
                if self.adb.is_none() {
                    return Err("adb is needed to observe the emulator.".to_string());
                }
                let mut used = used_ports.to_vec();
                let mut port = next_emulator_port(&used);
                // Ports from a previous run may still be in TIME_WAIT or held
                // by another process that adb does not report.
                for _ in 0..32 {
                    if port_free(port) {
                        break;
                    }
                    used.push(port);
                    port = next_emulator_port(&used);
                }
                let serial = serial_for_port(port);
                let args = android_launch_args(&target.name, port);
                let log = std::env::temp_dir().join(format!("gentle-emulator-{serial}.log"));
                let stderr = std::fs::File::create(&log).ok();
                let mut command = self.command(emulator);
                command
                    .args(&args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null());
                match &stderr {
                    Some(file) => {
                        command.stderr(Stdio::from(file.try_clone().map_err(|error| {
                            format!("Could not open the emulator log: {error}")
                        })?));
                    }
                    None => {
                        command.stderr(Stdio::null());
                    }
                }
                let mut child = command
                    .spawn()
                    .map_err(|error| format!("The Android emulator could not start: {error}"))?;
                // A bad AVD or a missing system image exits immediately.
                std::thread::sleep(Duration::from_millis(250));
                if let Some(status) = child
                    .try_wait()
                    .map_err(|error| format!("The Android emulator failed: {error}"))?
                {
                    let detail = std::fs::read_to_string(&log).unwrap_or_default();
                    let detail = detail.trim();
                    return Err(if detail.is_empty() {
                        format!("The Android emulator exited ({status}).")
                    } else {
                        format!("The Android emulator exited ({status}): {detail}")
                    });
                }
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    return Err("The launch was cancelled.".to_string());
                }
                Ok(Launched {
                    device: Device {
                        platform: Platform::Android,
                        id: serial,
                        name: target.name.clone(),
                        state: State::Booting,
                        os: target.os.clone(),
                    },
                    child: Some(child),
                })
            }
            Platform::Ios => {
                let simctl = self
                    .simctl
                    .as_ref()
                    .ok_or("iOS simulators need Xcode (simctl was not found).")?;
                // `boot` on a booted device is not an error for our purposes.
                match self.run_owned(simctl, &ios_boot_args(&target.id), QUERY_TIMEOUT) {
                    Ok(_) => {}
                    Err(error) if error.contains("current state: Booted") => {}
                    Err(error) => return Err(error),
                }
                if cancel.load(Ordering::Relaxed) {
                    if let Some(simctl) = self.simctl.as_ref() {
                        let _ = self.run_owned(simctl, &ios_stop_args(&target.id), STOP_TIMEOUT);
                    }
                    return Err("The launch was cancelled.".to_string());
                }
                Ok(Launched {
                    device: Device {
                        platform: Platform::Ios,
                        id: target.id.clone(),
                        name: target.name.clone(),
                        state: State::Booting,
                        os: target.os.clone(),
                    },
                    child: None,
                })
            }
        }
    }

    /// Wait until Android's framework finished booting. Returns the process
    /// so the panel keeps the handle; `cancel` is checked between polls and
    /// on timeout the emulator is killed, not left consuming resources.
    pub fn wait_android(
        &self,
        serial: &str,
        mut child: Child,
        cancel: &AtomicBool,
    ) -> Result<Child, String> {
        let Some(adb) = self.adb.as_ref() else {
            return Err("adb was not found; the emulator cannot be observed.".into());
        };
        let deadline = Instant::now() + BOOT_TIMEOUT;
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                return Err("The launch was cancelled.".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(format!("The Android emulator exited ({status})."));
                }
                Err(error) => {
                    let _ = child.kill();
                    return Err(format!("The Android emulator failed: {error}"));
                }
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                return Err("The Android emulator did not finish booting.".into());
            }
            if let Ok(output) = self.run_program(
                adb,
                &["-s", serial, "shell", "getprop", "sys.boot_completed"],
                Duration::from_secs(5),
            ) && boot_completed(&String::from_utf8_lossy(&output))
            {
                return Ok(child);
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    }

    /// `simctl bootstatus -b` blocks until the simulator is usable.
    pub fn wait_ios(&self, udid: &str) -> Result<(), String> {
        let simctl = self
            .simctl
            .as_ref()
            .ok_or("iOS simulators need Xcode (simctl was not found).")?;
        self.run_owned(simctl, &ios_bootstatus_args(udid), BOOT_TIMEOUT)?;
        Ok(())
    }

    /// Stop a device by id. Android goes through adb's console so the
    /// emulator shuts down cleanly and releases its port.
    pub fn stop(&self, device: &Device) -> Result<(), String> {
        match device.platform {
            Platform::Android => {
                let adb = self.adb.as_ref().ok_or("adb was not found.")?;
                self.run_owned(adb, &android_stop_args(&device.id), STOP_TIMEOUT)?;
            }
            Platform::Ios => {
                let simctl = self
                    .simctl
                    .as_ref()
                    .ok_or("iOS simulators need Xcode (simctl was not found).")?;
                self.run_owned(simctl, &ios_stop_args(&device.id), STOP_TIMEOUT)?;
            }
        }
        Ok(())
    }

    /// Stop commands spawned during shutdown, without waiting: quit must not
    /// hang on an emulator that is slow to answer.
    pub fn request_stop(&self, device: &Device) {
        let (program, args) = match device.platform {
            Platform::Android => {
                let Some(adb) = &self.adb else { return };
                (adb, android_stop_args(&device.id))
            }
            Platform::Ios => {
                let Some(simctl) = &self.simctl else { return };
                (simctl, ios_stop_args(&device.id))
            }
        };
        let mut command = self.command(program);
        let _ = command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }

    pub fn adb_bytes(&self, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
        let adb = self.adb.as_ref().ok_or("adb was not found.")?;
        self.run_program(adb, args, timeout)
    }

    pub fn simctl_run(&self, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
        let simctl = self
            .simctl
            .as_ref()
            .ok_or("iOS simulators need Xcode (simctl was not found).")?;
        self.run_program(simctl, args, timeout)
    }

    fn run_owned(
        &self,
        program: &Path,
        args: &[String],
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_program(program, &refs, timeout)
    }

    fn run_program(
        &self,
        program: &Path,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let output = self.output(program, args, timeout)?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        Err(if stderr.is_empty() {
            format!(
                "{} exited ({})",
                program
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| program.display().to_string()),
                output.status
            )
        } else {
            stderr.to_string()
        })
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = pi_rpc::command(program);
        command.env("PATH", pi_rpc::child_path());
        if let Some(sdk) = &self.sdk {
            command
                .env("ANDROID_HOME", sdk)
                .env("ANDROID_SDK_ROOT", sdk);
        }
        command
    }

    /// Run a child with a timeout, reading both pipes on their own threads so
    /// a large screencap can never fill a pipe and deadlock the wait.
    fn output(&self, program: &Path, args: &[&str], timeout: Duration) -> Result<Output, String> {
        let mut child = self
            .command(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                format!(
                    "Could not run {}: {error}",
                    program.file_name().unwrap_or_default().to_string_lossy()
                )
            })?;
        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        let out = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(pipe) = &mut stdout {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        });
        let err = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(pipe) = &mut stderr {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out.join();
                    let _ = err.join();
                    return Err(format!(
                        "{} timed out after {}s",
                        program.file_name().unwrap_or_default().to_string_lossy(),
                        timeout.as_secs()
                    ));
                }
                Err(error) => {
                    let _ = child.kill();
                    return Err(format!("Could not wait for the device tool: {error}"));
                }
            }
        };
        let stdout = out.join().unwrap_or_default();
        let stderr = err.join().unwrap_or_default();
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn is_emulator(serial: &str) -> bool {
    emulator_port(serial).is_some()
}

/// The console port and the adb port next to it must both be free.
fn port_free(port: u16) -> bool {
    let console = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();
    let adb = std::net::TcpListener::bind(("127.0.0.1", port + 1)).is_ok();
    console && adb
}

/// The first Android SDK folder that has adb or the emulator.
fn find_sdk() -> Option<PathBuf> {
    sdk_roots().into_iter().find(|dir| {
        dir.join("platform-tools").join(exe("adb")).is_file()
            || dir.join("emulator").join(exe("emulator")).is_file()
    })
}

/// The SDK's `emulator` and `platform-tools` folders, for the agent's PATH:
/// Android Studio installs them (on Windows under `%LOCALAPPDATA%\Android\Sdk`)
/// without putting either on the PATH, so an agent asked to start an
/// emulator wouldn't find `emulator` or `adb`.
pub(crate) fn android_tool_dirs() -> Vec<PathBuf> {
    find_sdk()
        .map(|sdk| vec![sdk.join("emulator"), sdk.join("platform-tools")])
        .unwrap_or_default()
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

/// Xcode's `simctl`. `xcode-select` may point at the Command Line Tools,
/// which have no simctl, so developer folders are tried directly.
pub(crate) fn simctl_path() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // Xcode does not usually move while the app runs; resolve once so the
    // 2 s discovery while waiting for an agent does not spawn `xcode-select`
    // every time.
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

/// The AVD names currently booted, for choosing a free emulator port.
pub fn used_ports(devices: &[Device]) -> Vec<u16> {
    devices
        .iter()
        .filter(|device| device.platform == Platform::Android)
        .filter_map(|device| emulator_port(&device.id))
        .collect()
}
