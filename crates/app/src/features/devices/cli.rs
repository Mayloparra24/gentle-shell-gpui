//! `gentle-device`: how agents (and the user's terminal) use simulators and
//! emulators inside the app. The shim runs `gentle-shell-gpui --device …`;
//! this process hands the request to the running app (which starts the hub,
//! boots the device and shows its tab), then reads the hub directly to wait
//! for the device and print how to drive it with `agent-device`.

use std::time::{Duration, Instant};

use gentle_core::devices::{Device, Platform, State};

use super::{find_device, hub};
use crate::features::setup::devices as setup;
use crate::platform::instance;

const USAGE: &str = "\
gentle-device: simulators and emulators in Gentle Shell's device panel

  gentle-device list               every iOS simulator and Android emulator
  gentle-device open <name|id>     boot it (if needed) and show it in the app
  gentle-device close <name|id> [--shutdown]
                                   close its tab; --shutdown also turns it off

Drive an open device with the agent-device command `open` prints.";

/// What a request asks the app's device panel to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Start the hub (for `list`), without showing anything.
    Ensure,
    Open(String),
    Close {
        query: String,
        shutdown: bool,
    },
}

/// The request in `gentle-device`'s arguments.
pub fn parse(args: &[String]) -> Result<Request, String> {
    let rest = |from: usize| {
        args[from..]
            .iter()
            .filter(|a| !a.starts_with("--"))
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    };
    match args.first().map(String::as_str) {
        Some("list") => Ok(Request::Ensure),
        Some("open") if !rest(1).is_empty() => Ok(Request::Open(rest(1))),
        Some("close") if !rest(1).is_empty() => Ok(Request::Close {
            query: rest(1),
            shutdown: args.iter().any(|a| a == "--shutdown"),
        }),
        _ => Err(USAGE.to_string()),
    }
}

/// `gentle-shell-gpui --device <args>`: the exit code.
pub fn run(args: Vec<String>) -> i32 {
    let request = match parse(&args) {
        Ok(request) => request,
        Err(usage) => {
            eprintln!("{usage}");
            return 2;
        }
    };
    let sent = instance::send(&instance::OpenRequest {
        device: Some(args.clone()),
        ..Default::default()
    });
    if !sent {
        eprintln!("gentle-device: the Gentle Shell app isn't running. Open it, then try again.");
        return 1;
    }
    match request {
        Request::Ensure => list(args.iter().any(|a| a == "--json")),
        Request::Open(query) => open(&query),
        Request::Close { .. } => 0,
    }
}

/// The hub's port, once the app started it.
fn wait_for_hub() -> Result<u16, String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(port) = hub::running_port() {
            return Ok(port);
        }
        if Instant::now() > deadline {
            return Err(
                "The device hub didn't start. In the app: Configuration → Devices → Use devices."
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn list(json: bool) -> i32 {
    let listed = wait_for_hub().and_then(hub::list);
    let (devices, errors) = match listed {
        Ok(listed) => listed,
        Err(error) => {
            eprintln!("gentle-device: {error}");
            return 1;
        }
    };
    if json {
        let rows: Vec<serde_json::Value> = devices
            .iter()
            .map(|d| {
                serde_json::json!({
                    "name": d.name,
                    "id": d.id,
                    "platform": platform_name(d.platform),
                    "os": d.os,
                    "running": d.state.is_running(),
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(rows));
    } else {
        for d in &devices {
            println!(
                "{:<28} {:<8} {:<14} {:<8} {}",
                d.name,
                platform_name(d.platform),
                d.os.as_deref().unwrap_or("-"),
                if d.state.is_running() {
                    "running"
                } else {
                    "off"
                },
                d.id
            );
        }
        if devices.is_empty() {
            println!(
                "No simulators or emulators. Install Xcode (iOS) or Android Studio and create one."
            );
        }
    }
    for error in errors {
        eprintln!("gentle-device: {error}");
    }
    0
}

fn open(query: &str) -> i32 {
    let port = match wait_for_hub() {
        Ok(port) => port,
        Err(error) => {
            eprintln!("gentle-device: {error}");
            return 1;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut said_booting = false;
    let device = loop {
        let devices = hub::list(port).map(|(d, _)| d).unwrap_or_default();
        match find_device(&devices, query) {
            Some(device) if device.state == State::Ready => break device.clone(),
            Some(device) if !said_booting => {
                eprintln!("Starting {} in the app's device panel…", device.name);
                said_booting = true;
            }
            Some(_) => {}
            None if !devices.is_empty() => {
                eprintln!(
                    "gentle-device: no simulator or emulator named \"{query}\". Run `gentle-device list`."
                );
                return 1;
            }
            None => {}
        }
        if Instant::now() > deadline {
            eprintln!("gentle-device: \"{query}\" didn't finish starting in time.");
            return 1;
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    print!("{}", quick_start(&device, setup::AGENT.entry().is_some()));
    0
}

fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Ios => "ios",
        Platform::Android => "android",
    }
}

/// What `open` prints: the device, and the agent-device calls pinned to it.
fn quick_start(device: &Device, agent_device: bool) -> String {
    let os = device.os.as_deref().unwrap_or(device.platform.label());
    let mut out = format!(
        "{} ({os}) is running in the app's device panel, where the user watches it.\n",
        device.name
    );
    if !agent_device {
        out.push_str(
            "agent-device isn't installed: in the app, Configuration → Devices → Agents can use devices.\n",
        );
        return out;
    }
    let ios = device.platform == Platform::Ios;
    let run = setup::agent_device_prefix(ios, &device.id);
    let app = if ios {
        "com.apple.mobilesafari"
    } else {
        "com.android.chrome"
    };
    out.push_str(&format!(
        "Drive it only with agent-device, pinned to it:\n\
         \x20 {run} open {app}            # launch an app (bundle or package id)\n\
         \x20 {run} snapshot -i           # what's on screen, with @refs\n\
         \x20 {run} press @e3 --settle    # tap an element\n\
         \x20 {run} fill @e5 \"text\" --settle\n\
         \x20 {run} screenshot /tmp/shot.png\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::{Request, parse};

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn requests_from_arguments() {
        assert_eq!(parse(&args("list --json")), Ok(Request::Ensure));
        assert_eq!(
            parse(&args("open iPhone 17")),
            Ok(Request::Open("iPhone 17".into()))
        );
        assert_eq!(
            parse(&args("close Pixel_7 --shutdown")),
            Ok(Request::Close {
                query: "Pixel_7".into(),
                shutdown: true
            })
        );
        assert!(parse(&args("open")).is_err());
        assert!(parse(&args("boot x")).is_err());
        assert!(parse(&[]).is_err());
    }
}
