//! Emulators and simulators: what the device hub lists, and the shell
//! commands in an agent's tool calls that start or drive one. Pure data and
//! parsers; the app runs the hub and shows the devices.

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

/// The device hub's `GET /api/devices` (`{simulators, emulators, errors}`),
/// plus the AVDs `emulator -list-avds` knows that the hub doesn't list (it
/// only lists emulators that are running or were used before). Physical
/// devices are left out. Booted devices are `Ready`: the hub reports no boot
/// progress. The second list is what the hub couldn't read (no Xcode, …).
pub fn hub_devices(json: &str, avds: &[String]) -> Result<(Vec<Device>, Vec<String>), String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("The device hub's list is unreadable: {e}"))?;
    let mut devices = Vec::new();
    for (key, platform) in [
        ("simulators", Platform::Ios),
        ("emulators", Platform::Android),
    ] {
        for device in value[key].as_array().into_iter().flatten() {
            if device["physical"] == true {
                continue;
            }
            let (Some(id), Some(name)) = (device["id"].as_str(), device["name"].as_str()) else {
                continue;
            };
            devices.push(Device {
                platform,
                id: id.to_string(),
                name: name.to_string(),
                state: if device["booted"] == true {
                    State::Ready
                } else {
                    State::Shutdown
                },
                os: device["version"]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(str::to_string),
            });
        }
    }
    for avd in avds {
        let listed = devices
            .iter()
            .any(|d| d.platform == Platform::Android && same_avd(&d.name, avd));
        if !listed {
            devices.push(Device {
                platform: Platform::Android,
                id: avd.clone(),
                name: avd.clone(),
                state: State::Shutdown,
                os: None,
            });
        }
    }
    sort_devices(&mut devices);
    let errors = value["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|error| {
            error
                .as_str()
                .or_else(|| error["message"].as_str())
                .map(str::to_string)
        })
        .collect();
    Ok((devices, errors))
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
        if is(at, "emulator")
            && let Some(avd) = emulator_avd(&segment[at + 1..])
        {
            return Some(LaunchIntent {
                platform: Platform::Android,
                id: resolve_shell_value(&avd, &assignments),
            });
        }
        // PowerShell: `Start-Process [-FilePath] emulator -ArgumentList '-avd','Pixel_7'`.
        if token(at).is_some_and(|word| word.eq_ignore_ascii_case("Start-Process")) {
            let rest = &segment[at + 1..];
            let file = match rest
                .iter()
                .position(|w| w.eq_ignore_ascii_case("-FilePath"))
            {
                Some(found) => rest.get(found + 1),
                None => rest.iter().find(|word| !word.starts_with('-')),
            };
            if file.is_some_and(|file| program_name(file) == "emulator") {
                // Arguments come as one string or a list: split both.
                let args: Vec<String> = rest
                    .iter()
                    .flat_map(|word| word.split([',', ' ']))
                    // `@('-avd','Pixel_7')` lists; a bare `@Pixel_7` stays.
                    .map(|word| {
                        word.trim_start_matches("@(")
                            .trim_matches(|c| matches!(c, '\'' | '"' | '(' | ')'))
                    })
                    .filter(|word| !word.is_empty())
                    .map(str::to_string)
                    .collect();
                if let Some(avd) = emulator_avd(&args) {
                    return Some(LaunchIntent {
                        platform: Platform::Android,
                        id: resolve_shell_value(&avd, &assignments),
                    });
                }
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

/// The AVD in an `emulator` launch's arguments: `-avd <name>` or `@name`.
/// A shell variable we cannot resolve later still tells us the platform;
/// the card waits for whichever device appears.
fn emulator_avd(args: &[String]) -> Option<String> {
    if let Some(found) = args.iter().position(|word| word == "-avd")
        && let Some(avd) = args.get(found + 1).map(|avd| avd.trim())
        && !avd.is_empty()
        && !avd.starts_with('-')
    {
        return Some(avd.to_string());
    }
    // `emulator @Pixel_7` is the other legal spelling.
    args.iter().find_map(|word| {
        word.strip_prefix('@')
            .map(str::trim)
            .filter(|avd| !avd.is_empty())
            .map(str::to_string)
    })
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
    while token(at).is_some_and(|word| {
        matches!(word, "env" | "sudo" | "nohup" | "exec" | "command")
            // Windows: `cmd /c …`, `start "" …` (a quoted window title).
            || word.eq_ignore_ascii_case("cmd")
            || word.eq_ignore_ascii_case("start")
    }) {
        at += 1;
        while token(at).is_some_and(|word| {
            word.starts_with('-') || is_cmd_flag(word) || word.is_empty() || is_assignment(word)
        }) {
            at += 1;
        }
    }
    at
}

/// A `cmd` / `start` switch (`/c`, `/b`, `/min`), not a Unix path.
fn is_cmd_flag(word: &str) -> bool {
    word.strip_prefix('/').is_some_and(|rest| {
        !rest.is_empty() && rest.len() <= 8 && rest.chars().all(|c| c.is_ascii_alphabetic())
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_list_adds_unlisted_avds_and_skips_physical_devices() {
        let json = r#"{"simulators": [
              {"id": "U1", "name": "iPhone 17", "version": "iOS 27.2", "platform": "ios", "booted": true, "physical": false},
              {"id": "U2", "name": "iPad Air", "version": "iOS 27.2", "platform": "ios", "booted": false, "physical": false}],
            "emulators": [
              {"id": "emulator-5554", "name": "Pixel_7", "version": "Android 15.0", "platform": "android", "booted": true, "physical": false},
              {"id": "R58M", "name": "Galaxy", "platform": "android", "booted": true, "physical": true}],
            "errors": [{"message": "xcrun: unable to find simctl"}]}"#;
        let (devices, errors) =
            hub_devices(json, &["Pixel 7".into(), "Tablet_API_35".into()]).unwrap();
        let rows: Vec<(&str, &str, State)> = devices
            .iter()
            .map(|d| (d.id.as_str(), d.name.as_str(), d.state))
            .collect();
        assert_eq!(
            rows,
            vec![
                // Android first, running before stopped; the running Pixel_7
                // isn't listed twice under its `-list-avds` spelling.
                ("emulator-5554", "Pixel_7", State::Ready),
                ("Tablet_API_35", "Tablet_API_35", State::Shutdown),
                ("U1", "iPhone 17", State::Ready),
                ("U2", "iPad Air", State::Shutdown),
            ]
        );
        assert_eq!(devices[2].os.as_deref(), Some("iOS 27.2"));
        assert_eq!(errors, vec!["xcrun: unable to find simctl".to_string()]);
        assert!(hub_devices("<html>", &[]).is_err());
    }

    #[test]
    fn avds_are_trimmed_and_blank_lines_dropped() {
        assert_eq!(
            parse_avd_list(" Pixel_7 \n\nPixel_Tablet\r\n# comment\nINFO    | Storing crashdata\n"),
            vec!["Pixel_7", "Pixel_Tablet"]
        );
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
    fn windows_launch_forms_are_detected() {
        let pixel = Some(LaunchIntent {
            platform: Platform::Android,
            id: Some("Pixel_7".into()),
        });
        for command in [
            r#"start "" "C:\Users\me\AppData\Local\Android\Sdk\emulator\emulator.exe" -avd Pixel_7"#,
            "cmd /c start /b emulator -avd Pixel_7",
            "Start-Process emulator -ArgumentList '-avd','Pixel_7','-no-window'",
            r#"Start-Process -FilePath "$env:LOCALAPPDATA\Android\Sdk\emulator\emulator.exe" -ArgumentList "-avd Pixel_7 -no-audio""#,
            r#"& "C:\Android\Sdk\emulator\emulator.exe" -avd Pixel_7"#,
        ] {
            assert_eq!(launch_intent(command), pixel, "{command}");
        }
        assert_eq!(
            launch_intent("Start-Process notepad -ArgumentList '-avd x'"),
            None
        );
        // A Unix path after a wrapper is still the program, not a `cmd` switch.
        assert_eq!(
            launch_intent("nohup /opt/sdk/emulator/emulator -avd Pixel_7 &"),
            pixel
        );
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
}
