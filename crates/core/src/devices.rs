//! Emulators and simulators: what the machine has, how to boot one with the
//! least resources, how to tell whether it finished booting, and how to read
//! a raw Android frame. Pure data and parsers; the app resolves the tools,
//! spawns the processes and draws the preview.

use std::collections::HashMap;

/// Which family of device a row belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Platform {
    Android,
    Ios,
}

impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Self::Android => "Android",
            Self::Ios => "iOS",
        }
    }
}

/// What a device is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Shutdown,
    /// The system is starting (Android boots its UI long after adb answers).
    Booting,
    /// The system finished booting and the screen can be used.
    Ready,
}

impl State {
    pub fn is_running(self) -> bool {
        !matches!(self, Self::Shutdown)
    }
}

/// One emulator or simulator the panel can show.
///
/// Android AVDs that are not running have the AVD name as `id` (there is no
/// serial yet); once booted the same AVD appears with the adb serial as `id`
/// and the AVD name as `name`. iOS simulators keep their UDID throughout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub platform: Platform,
    pub id: String,
    pub name: String,
    pub state: State,
    /// The OS version, when the source reports one (iOS runtimes do).
    pub os: Option<String>,
}

impl Device {
    /// A stable key for the selection: the AVD name or the simulator UDID.
    pub fn key(&self) -> String {
        match self.platform {
            Platform::Android => self.name.clone(),
            Platform::Ios => self.id.clone(),
        }
    }

    pub fn subtitle(&self) -> String {
        self.os
            .clone()
            .unwrap_or_else(|| self.platform.label().to_string())
    }
}

/// A line of `adb devices -l`, reduced to what the panel needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    /// `device`, `offline`, `unauthorized`, …
    pub state: String,
    pub model: Option<String>,
}

impl AdbDevice {
    fn usable(&self) -> bool {
        self.state == "device"
    }
}

/// `emulator -list-avds`: one AVD name per line.
pub fn parse_avd_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                // `emulator` writes warnings with this shape before the list.
                && !line.contains('|')
        })
        .map(str::to_string)
        .collect()
}

/// `adb devices -l`. The header line and blank lines are ignored.
pub fn parse_adb_devices(text: &str) -> Vec<AdbDevice> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("List of devices") {
                return None;
            }
            let mut fields = line.split_whitespace();
            let serial = fields.next()?.to_string();
            let state = fields.next()?.to_string();
            let model = fields
                .find_map(|field| field.strip_prefix("model:"))
                .map(|model| model.replace('_', " "));
            Some(AdbDevice {
                serial,
                state,
                model,
            })
        })
        .collect()
}

/// Running Android emulators as devices, plus every AVD that is not running
/// as a shutdown target.
///
/// `avd_names` maps a serial to `adb -s <serial> emu avd name`; a serial
/// missing from it falls back to the model or the serial. `booted` maps a
/// serial to the answer of `getprop sys.boot_completed`; a serial missing
/// from it is still booting.
pub fn android_devices(
    avds: &[String],
    adb: &[AdbDevice],
    avd_names: &[(String, String)],
    booted: &[(String, bool)],
) -> Vec<Device> {
    let names: HashMap<&str, &str> = avd_names
        .iter()
        .map(|(serial, name)| (serial.as_str(), name.as_str()))
        .collect();
    let booted: HashMap<&str, bool> = booted
        .iter()
        .map(|(serial, done)| (serial.as_str(), *done))
        .collect();
    let mut out = Vec::new();
    for device in adb.iter().filter(|d| is_emulator_serial(&d.serial)) {
        let name = names
            .get(device.serial.as_str())
            .map(|name| (*name).to_string())
            .or_else(|| device.model.clone())
            .unwrap_or_else(|| device.serial.clone());
        let state = if !device.usable() {
            State::Booting
        } else if booted.get(device.serial.as_str()).copied().unwrap_or(false) {
            State::Ready
        } else {
            State::Booting
        };
        out.push(Device {
            platform: Platform::Android,
            id: device.serial.clone(),
            name,
            state,
            os: None,
        });
    }
    for avd in avds {
        if !out.iter().any(|device| same_avd(&device.name, avd)) {
            out.push(Device {
                platform: Platform::Android,
                id: avd.clone(),
                name: avd.clone(),
                state: State::Shutdown,
                os: None,
            });
        }
    }
    sort_devices(&mut out);
    out
}

/// AVD names use underscores where the emulator model uses spaces
/// (`Pixel_7` vs `Pixel 7`); compare them that way when deciding whether an
/// AVD is already listed as running.
fn same_avd(left: &str, right: &str) -> bool {
    let normalize = |name: &str| {
        name.chars()
            .filter(|c| !matches!(c, ' ' | '_' | '-'))
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    normalize(left) == normalize(right)
}

/// `xcrun simctl list devices available --json`. Only available devices are
/// listed; unavailable runtimes are skipped instead of shown as broken rows.
pub fn parse_simctl_devices(json: &str) -> Result<Vec<Device>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("simctl: {error}"))?;
    let Some(devices) = value.get("devices").and_then(|d| d.as_object()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (runtime, list) in devices {
        let os = runtime_label(runtime);
        let Some(list) = list.as_array() else {
            continue;
        };
        for device in list {
            if device.get("isAvailable").and_then(|b| b.as_bool()) == Some(false) {
                continue;
            }
            let (Some(id), Some(name)) = (
                device.get("udid").and_then(|v| v.as_str()),
                device.get("name").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let state = match device.get("state").and_then(|v| v.as_str()) {
                Some("Booted") => State::Ready,
                Some("Shutdown") => State::Shutdown,
                _ => State::Booting,
            };
            out.push(Device {
                platform: Platform::Ios,
                id: id.to_string(),
                name: name.to_string(),
                state,
                os: os.clone(),
            });
        }
    }
    sort_devices(&mut out);
    Ok(out)
}

/// `com.apple.CoreSimulator.SimRuntime.iOS-27-2` → `iOS 27.2`.
fn runtime_label(runtime: &str) -> Option<String> {
    let id = runtime.rsplit('.').next()?;
    let (os, version) = id.split_once('-')?;
    if version.is_empty() {
        return None;
    }
    Some(format!("{os} {}", version.replace('-', ".")))
}

pub fn sort_devices(devices: &mut [Device]) {
    devices.sort_by(|a, b| {
        a.platform
            .cmp(&b.platform)
            // Running first, then by name; a stable order keeps rows still.
            .then_with(|| b.state.is_running().cmp(&a.state.is_running()))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// The serial Android gives an emulator listening on `port`.
pub fn serial_for_port(port: u16) -> String {
    format!("emulator-{port}")
}

pub fn is_emulator_serial(serial: &str) -> bool {
    serial.starts_with("emulator-")
}

/// The even console port of an adb serial, if it is an emulator.
pub fn emulator_port(serial: &str) -> Option<u16> {
    serial
        .strip_prefix("emulator-")?
        .parse::<u16>()
        .ok()
        .filter(|port| *port >= 5554)
}

/// The first free console port, skipping the ones already in use. Android
/// uses an even console port and the next odd one for adb.
pub fn next_emulator_port(used: &[u16]) -> u16 {
    let used: Vec<u16> = used.to_vec();
    (0..=64)
        .map(|step| 5554 + step * 2)
        .find(|port| !used.contains(port))
        .unwrap_or(5554)
}

/// `emulator` arguments that boot headless and cheap: no window, no sound,
/// no cameras, no boot animation and no snapshot write.
pub fn android_launch_args(avd: &str, port: u16) -> Vec<String> {
    [
        "-avd",
        avd,
        "-port",
        &port.to_string(),
        "-no-window",
        "-no-audio",
        "-no-boot-anim",
        "-no-snapshot-save",
        "-gpu",
        "swiftshader_indirect",
        "-camera-back",
        "none",
        "-camera-front",
        "none",
        "-no-metrics",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// `simctl bootstatus -b` waits for the boot to finish; the app bounds it.
pub fn ios_boot_args(udid: &str) -> Vec<String> {
    vec!["boot".into(), udid.into()]
}

pub fn ios_bootstatus_args(udid: &str) -> Vec<String> {
    vec!["bootstatus".into(), udid.into(), "-b".into()]
}

pub fn android_stop_args(serial: &str) -> Vec<String> {
    vec!["-s".into(), serial.into(), "emu".into(), "kill".into()]
}

pub fn ios_stop_args(udid: &str) -> Vec<String> {
    vec!["shutdown".into(), udid.into()]
}

/// `getprop sys.boot_completed` prints `1` once the framework is up.
pub fn boot_completed(stdout: &str) -> bool {
    stdout.trim() == "1"
}

/// `adb -s <serial> emu avd name` prints the AVD name, then `OK`. The name can
/// itself contain spaces, so the first line is kept whole.
pub fn parse_avd_name(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && *line != "OK")
        .map(str::to_string)
}

/// What a shell command in a tool call asked the machine to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchIntent {
    pub platform: Platform,
    /// The AVD name or simulator UDID, when the command names one.
    pub id: Option<String>,
}

/// Detect an emulator or simulator launch in a shell command, so the window
/// can show the device the agent is about to use. Only the launch forms are
/// recognized; `adb devices`, `simctl list` and quoted examples are not.
pub fn launch_intent(command: &str) -> Option<LaunchIntent> {
    let assignments = shell_assignments(command);
    for segment in shell_segments(command) {
        let token = |at: usize| segment.get(at).map(String::as_str);
        let is = |at: usize, name: &str| token(at).is_some_and(|word| program_name(word) == name);
        let at = program_start(&segment);
        if is(at, "emulator") {
            if let Some(found) = segment.iter().position(|word| word == "-avd")
                && let Some(avd) = segment.get(found + 1).map(|avd| avd.trim())
                && !avd.is_empty()
                && !avd.starts_with('-')
            {
                // A shell variable we cannot resolve still tells us the
                // platform; the card waits for whichever device appears.
                return Some(LaunchIntent {
                    platform: Platform::Android,
                    id: resolve_shell_value(avd, &assignments),
                });
            }
            // `emulator @Pixel_7` is the other legal spelling.
            if let Some(avd) = segment.iter().skip(at + 1).find_map(|word| {
                word.strip_prefix('@')
                    .map(str::trim)
                    .filter(|avd| !avd.is_empty())
            }) {
                return Some(LaunchIntent {
                    platform: Platform::Android,
                    id: resolve_shell_value(avd, &assignments),
                });
            }
        }
        let simctl = (is(at, "xcrun") && is(at + 1, "simctl")) || is(at, "simctl");
        if simctl {
            let base = if is(at, "xcrun") { at + 2 } else { at + 1 };
            let boot = segment[base.min(segment.len())..]
                .iter()
                .position(|word| word == "boot")
                .map(|found| base + found);
            if let Some(boot) = boot {
                let id = segment.get(boot + 1).map(String::as_str);
                return Some(LaunchIntent {
                    platform: Platform::Ios,
                    id: match id {
                        Some("booted") | None => None,
                        Some(id) if id.starts_with('-') => None,
                        Some(id) => resolve_shell_value(id, &assignments),
                    },
                });
            }
        }
    }
    None
}

/// A shell command that drives a device, as the window's instance tray shows
/// it next to the device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCommand {
    /// The platform the tool targets; `None` for tools that drive either
    /// (Maestro, Appium, Detox).
    pub platform: Option<Platform>,
    /// The segment that touches the device, without leading assignments,
    /// wrappers or the program's folder: `adb shell input tap 540 1200`.
    pub summary: String,
}

/// Detect a command that drives an emulator or simulator (`adb`, `emulator`,
/// `[xcrun] simctl`, `maestro`, `appium`, `detox`), so the tray can say what
/// the agent is doing with it. Quoted examples (`echo 'adb …'`) are not.
pub fn device_command(command: &str) -> Option<DeviceCommand> {
    for segment in shell_segments(command) {
        let at = program_start(&segment);
        let Some(program) = segment.get(at).map(|word| program_name(word)) else {
            continue;
        };
        let (platform, start) = match program {
            "adb" | "emulator" => (Some(Platform::Android), at),
            "simctl" => (Some(Platform::Ios), at),
            "xcrun"
                if segment
                    .get(at + 1)
                    .is_some_and(|word| program_name(word) == "simctl") =>
            {
                (Some(Platform::Ios), at + 1)
            }
            "maestro" | "appium" | "detox" => (None, at),
            _ => continue,
        };
        let mut words = vec![program_name(&segment[start]).to_string()];
        words.extend(segment[start + 1..].iter().map(|word| {
            if word.chars().any(char::is_whitespace) {
                format!("\"{word}\"")
            } else {
                word.clone()
            }
        }));
        return Some(DeviceCommand {
            platform,
            summary: words.join(" "),
        });
    }
    None
}

/// The program a word runs, without its folder or `.exe`.
fn program_name(word: &str) -> &str {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// Where the program starts in a segment: past leading environment
/// assignments (`DEVELOPER_DIR=… xcrun …`) and wrappers that run another
/// program (`env FOO=1 emulator …`, `sudo emulator …`, `nohup …`).
fn program_start(segment: &[String]) -> usize {
    let token = |at: usize| segment.get(at).map(String::as_str);
    let mut at = 0;
    while token(at).is_some_and(is_assignment) {
        at += 1;
    }
    while matches!(
        token(at),
        Some("env" | "sudo" | "nohup" | "exec" | "command")
    ) {
        at += 1;
        while token(at).is_some_and(|word| word.starts_with('-') || is_assignment(word)) {
            at += 1;
        }
    }
    at
}

/// Simple `NAME=value` assignments anywhere in the command, so a launch
/// written as `simctl boot "$UDID"` after `UDID=…` names the right device.
fn shell_assignments(command: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for segment in shell_segments(command) {
        for token in segment {
            let Some((name, value)) = token.split_once('=') else {
                continue;
            };
            if !is_assignment(&token)
                || value.is_empty()
                || value.contains('$')
                || value.contains('`')
                || value.contains('(')
            {
                continue;
            }
            out.insert(name.to_string(), value.to_string());
        }
    }
    out
}

/// The value of a device id word: a literal, or a simple variable we saw
/// assigned earlier in the same command. `None` means "a device of this
/// platform is about to start, but its name is not literal".
fn resolve_shell_value(word: &str, assignments: &HashMap<String, String>) -> Option<String> {
    let word = word.trim();
    if let Some(rest) = word.strip_prefix('$') {
        let name = rest.trim_matches(|c| c == '{' || c == '}');
        return assignments.get(name).cloned();
    }
    if word.contains('$') || word.contains('`') || word.contains('(') {
        return None;
    }
    (!word.is_empty()).then(|| word.to_string())
}

/// `NAME=value` before the program (`DEVELOPER_DIR=…`, `AVD=Pixel_7`).
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Split a shell command into segments (`;`, `&&`, `||`, `|`) and each one
/// into words, keeping quoted strings together. Enough for launch commands
/// (`emulator -avd "Pixel 7"`); not a full shell parser.
fn shell_segments(command: &str) -> Vec<Vec<String>> {
    let mut segments = Vec::new();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let finish_word = |tokens: &mut Vec<String>, current: &mut String, started: &mut bool| {
        if *started {
            tokens.push(std::mem::take(current));
            *started = false;
        }
    };
    let finish_segment = |segments: &mut Vec<Vec<String>>, tokens: &mut Vec<String>| {
        if !tokens.is_empty() {
            segments.push(std::mem::take(tokens));
        }
    };
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c == '\n' || c == ';' || c == '|' || c == '&' => {
                finish_word(&mut tokens, &mut current, &mut started);
                finish_segment(&mut segments, &mut tokens);
            }
            None if c.is_whitespace() => {
                finish_word(&mut tokens, &mut current, &mut started);
            }
            None => {
                current.push(c);
                started = true;
            }
        }
    }
    finish_word(&mut tokens, &mut current, &mut started);
    finish_segment(&mut segments, &mut tokens);
    segments
}

/// A raw `screencap` frame: width, height, pixel format and the pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screencap<'a> {
    pub width: usize,
    pub height: usize,
    /// `1` RGBA_8888, `2` RGBX_8888, `3` RGB_888, `4` RGB_565, `5` BGRA_8888.
    pub format: u32,
    pub pixels: &'a [u8],
}

impl Screencap<'_> {
    pub fn bytes_per_pixel(&self) -> usize {
        match self.format {
            3 => 3,
            4 => 2,
            _ => 4,
        }
    }
}

/// `adb exec-out screencap` writes a little-endian header (three words, or
/// four with the color space) followed by the pixels. The color-space word
/// only exists on newer Androids, so both header sizes are tried and the one
/// that matches the payload length wins.
pub fn parse_screencap(bytes: &[u8]) -> Option<Screencap<'_>> {
    // `screencap -p` writes a PNG with no raw header; this parser is only for
    // the raw form.
    if bytes.starts_with(b"\x89PNG") {
        return None;
    }
    let word = |at: usize| -> Option<u32> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    for header in [16usize, 12] {
        let (Some(width), Some(height), Some(format)) = (word(0), word(4), word(8)) else {
            continue;
        };
        let width = width as usize;
        let height = height as usize;
        if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
            continue;
        }
        let bpp = match format {
            1 | 2 | 5 => 4,
            3 => 3,
            4 => 2,
            _ => continue,
        };
        let len = header + width * height * bpp;
        if bytes.len() < len {
            continue;
        }
        return Some(Screencap {
            width,
            height,
            format,
            pixels: &bytes[header..len],
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avds_are_trimmed_and_blank_lines_dropped() {
        assert_eq!(
            parse_avd_list(" Pixel_7 \n\nPixel_Tablet\r\n# comment\nINFO    | Storing crashdata\n"),
            vec!["Pixel_7", "Pixel_Tablet"]
        );
    }

    #[test]
    fn adb_devices_reads_serial_state_and_model() {
        let devices = parse_adb_devices(
            "List of devices attached\n\
             emulator-5554\tdevice product:sdk model:Pixel_7 device:emu64a transport_id:1\n\
             emulator-5556\toffline\n",
        );
        assert_eq!(
            devices,
            vec![
                AdbDevice {
                    serial: "emulator-5554".into(),
                    state: "device".into(),
                    model: Some("Pixel 7".into())
                },
                AdbDevice {
                    serial: "emulator-5556".into(),
                    state: "offline".into(),
                    model: None
                },
            ]
        );
    }

    #[test]
    fn android_running_and_stopped_avds_merge_without_duplicates() {
        let avds = vec!["Pixel_7".into(), "Pixel_Tablet".into()];
        let adb = parse_adb_devices(
            "List of devices attached\n\
             emulator-5554\tdevice model:Pixel_7\n",
        );
        let devices = android_devices(
            &avds,
            &adb,
            &[("emulator-5554".into(), "Pixel_7".into())],
            &[("emulator-5554".into(), true)],
        );
        assert_eq!(
            devices,
            vec![
                Device {
                    platform: Platform::Android,
                    id: "emulator-5554".into(),
                    name: "Pixel_7".into(),
                    state: State::Ready,
                    os: None
                },
                Device {
                    platform: Platform::Android,
                    id: "Pixel_Tablet".into(),
                    name: "Pixel_Tablet".into(),
                    state: State::Shutdown,
                    os: None
                },
            ]
        );
    }

    #[test]
    fn a_running_model_name_does_not_duplicate_its_avd() {
        let adb = parse_adb_devices("emulator-5554\tdevice model:Pixel_7\n");
        let devices = android_devices(
            &["Pixel_7".into()],
            &adb,
            &[],
            &[("emulator-5554".into(), true)],
        );
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Pixel 7");
        assert_eq!(devices[0].state, State::Ready);
    }

    #[test]
    fn an_adb_device_that_has_not_booted_is_booting() {
        let adb = parse_adb_devices("emulator-5554\tdevice\n");
        let devices = android_devices(&[], &adb, &[], &[]);
        assert_eq!(devices[0].state, State::Booting);
        assert_eq!(devices[0].name, "emulator-5554");
    }

    #[test]
    fn simctl_json_lists_available_devices_with_their_runtime() {
        let json = r#"{
            "devices": {
                "com.apple.CoreSimulator.SimRuntime.iOS-27-2": [
                    {"udid": "A", "name": "iPhone 18", "state": "Booted", "isAvailable": true},
                    {"udid": "B", "name": "iPhone 17", "state": "Shutdown", "isAvailable": true},
                    {"udid": "C", "name": "Old", "state": "Shutdown", "isAvailable": false}
                ],
                "com.apple.CoreSimulator.SimRuntime.tvOS-27-0": [
                    {"udid": "D", "name": "Apple TV", "state": "Shutdown", "isAvailable": true}
                ]
            }
        }"#;
        let devices = parse_simctl_devices(json).unwrap();
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].id, "A");
        assert_eq!(devices[0].state, State::Ready);
        assert_eq!(devices[0].os.as_deref(), Some("iOS 27.2"));
        assert!(
            devices
                .iter()
                .any(|d| d.id == "D" && d.os.as_deref() == Some("tvOS 27.0"))
        );
        assert!(!devices.iter().any(|d| d.id == "C"));
    }

    #[test]
    fn simctl_json_rejects_garbage() {
        assert!(parse_simctl_devices("not json").is_err());
        assert!(parse_simctl_devices("{}").unwrap().is_empty());
    }

    #[test]
    fn launch_plans_are_headless_and_cheap() {
        let args = android_launch_args("Pixel_7", 5554);
        assert!(args.contains(&"-no-window".into()));
        assert!(args.contains(&"-no-audio".into()));
        assert!(args.contains(&"-no-boot-anim".into()));
        assert!(args.contains(&"-no-snapshot-save".into()));
        assert!(args.contains(&"5554".into()));
        assert_eq!(ios_boot_args("U"), vec!["boot", "U"]);
        assert_eq!(ios_bootstatus_args("U"), vec!["bootstatus", "U", "-b"]);
        assert_eq!(
            android_stop_args("emulator-5554"),
            vec!["-s", "emulator-5554", "emu", "kill"]
        );
        assert_eq!(ios_stop_args("U"), vec!["shutdown", "U"]);
    }

    #[test]
    fn emulator_ports_skip_the_ones_in_use() {
        assert_eq!(next_emulator_port(&[]), 5554);
        assert_eq!(next_emulator_port(&[5554, 5556]), 5558);
        assert_eq!(serial_for_port(5558), "emulator-5558");
        assert_eq!(emulator_port("emulator-5558"), Some(5558));
        assert_eq!(emulator_port("R58M"), None);
        assert_eq!(emulator_port("emulator-123"), None);
    }

    #[test]
    fn boot_completed_and_avd_name_parse() {
        assert!(boot_completed("1\r\n"));
        assert!(!boot_completed("0"));
        assert!(!boot_completed(""));
        assert_eq!(parse_avd_name("Pixel 7\nOK\n"), Some("Pixel 7".into()));
        assert_eq!(parse_avd_name("OK\n"), None);
    }

    #[test]
    fn launch_commands_are_detected_but_queries_are_not() {
        assert_eq!(
            launch_intent("emulator -avd Pixel_7 -no-window"),
            Some(LaunchIntent {
                platform: Platform::Android,
                id: Some("Pixel_7".into())
            })
        );
        assert_eq!(
            launch_intent("$HOME/Library/Android/sdk/emulator/emulator -avd \"Pixel 7\""),
            Some(LaunchIntent {
                platform: Platform::Android,
                id: Some("Pixel 7".into())
            })
        );
        assert_eq!(
            launch_intent("xcrun simctl boot B9354562-B316-40F9"),
            Some(LaunchIntent {
                platform: Platform::Ios,
                id: Some("B9354562-B316-40F9".into())
            })
        );
        assert_eq!(
            launch_intent("emulator @Pixel_7"),
            Some(LaunchIntent {
                platform: Platform::Android,
                id: Some("Pixel_7".into())
            })
        );
        assert_eq!(
            launch_intent(
                "DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcrun simctl boot U"
            ),
            Some(LaunchIntent {
                platform: Platform::Ios,
                id: Some("U".into())
            })
        );
        assert_eq!(
            launch_intent(
                "export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer\n\
                 UDID=5FED63DF-30FE-4950-B5E3-EC2B349ABE5E\n\
                 xcrun simctl boot \"$UDID\""
            ),
            Some(LaunchIntent {
                platform: Platform::Ios,
                id: Some("5FED63DF-30FE-4950-B5E3-EC2B349ABE5E".into())
            })
        );
        assert_eq!(
            launch_intent("xcrun simctl boot \"$UNKNOWN\""),
            Some(LaunchIntent {
                platform: Platform::Ios,
                id: None
            })
        );
        assert_eq!(
            launch_intent("AVD=Pixel_7\nemulator -avd \"$AVD\" -no-window"),
            Some(LaunchIntent {
                platform: Platform::Android,
                id: Some("Pixel_7".into())
            })
        );
        assert_eq!(
            launch_intent("emulator -avd \"$AVD\" -no-window"),
            Some(LaunchIntent {
                platform: Platform::Android,
                id: None
            })
        );
        assert_eq!(
            launch_intent("xcrun simctl boot booted"),
            Some(LaunchIntent {
                platform: Platform::Ios,
                id: None
            })
        );
        assert_eq!(launch_intent("adb devices -l"), None);
        assert_eq!(launch_intent("xcrun simctl list devices"), None);
        assert_eq!(launch_intent("grep -avd x file"), None);
        assert_eq!(launch_intent("echo 'emulator -avd Pixel'"), None);
    }

    #[test]
    fn device_commands_name_their_platform_and_skip_wrappers() {
        assert_eq!(
            device_command("cd app && adb -s emulator-5554 shell input tap 540 1200"),
            Some(DeviceCommand {
                platform: Some(Platform::Android),
                summary: "adb -s emulator-5554 shell input tap 540 1200".into()
            })
        );
        assert_eq!(
            device_command("DEVELOPER_DIR=/x /usr/bin/xcrun simctl openurl booted \"my app://\""),
            Some(DeviceCommand {
                platform: Some(Platform::Ios),
                summary: "simctl openurl booted \"my app://\"".into()
            })
        );
        assert_eq!(
            device_command("nohup maestro test flow.yaml"),
            Some(DeviceCommand {
                platform: None,
                summary: "maestro test flow.yaml".into()
            })
        );
        assert_eq!(device_command("xcrun clang main.c"), None);
        assert_eq!(device_command("echo 'adb shell ls'"), None);
        assert_eq!(device_command("npm run build"), None);
    }

    #[test]
    fn screencap_reads_three_and_four_word_headers() {
        let mut bytes = Vec::new();
        for word in [2u32, 1, 1] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let frame = parse_screencap(&bytes).unwrap();
        assert_eq!((frame.width, frame.height, frame.format), (2, 1, 1));
        assert_eq!(frame.pixels, &[1, 2, 3, 4, 5, 6, 7, 8]);

        let mut bytes = Vec::new();
        for word in [1u32, 1, 4] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        // The 12-byte header matches exactly; a 16-byte one would not.
        assert!(parse_screencap(&bytes).is_some());

        let mut bytes = Vec::new();
        for word in [1u32, 1, 1, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        let frame = parse_screencap(&bytes).unwrap();
        assert_eq!(frame.pixels.len(), 4);

        assert!(parse_screencap(b"\x89PNG\r\n\x1a\n").is_none());
        assert!(parse_screencap(&[]).is_none());
    }
}
