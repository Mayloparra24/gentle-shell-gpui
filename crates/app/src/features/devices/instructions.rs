//! What the app tells every agent it starts (appended to the system prompt
//! with `--append-system-prompt`): pages open in the app's browser with
//! `gentle-open`, and, with "Agents can use devices" on, simulators and
//! emulators open in the device panel with `gentle-device` and are driven
//! with `agent-device`. The point is that the user watches everything inside
//! the app; nothing opens a separate browser, Simulator or emulator window.

use std::path::PathBuf;

use crate::features::setup::devices::Prefs;

const PAGES: &str = "\
## Gentle Shell app: pages and devices

You run inside the Gentle Shell desktop app, where the user watches what you open.

- Web pages (a dev server, a preview, docs): run `gentle-open <url>`; the page opens in the app's \
browser. Never use `open`, `xdg-open`, `start` or launch a browser yourself.
";

const DEVICES: &str = "\
- iOS simulators and Android emulators: run `gentle-device list`, then `gentle-device open \"<name>\"`. \
It boots the device without a separate window, shows it in the app's device panel and prints the \
`agent-device` command pinned to it.
- Do every app launch, tap, text entry, screenshot and check on that device with that command: \
`open <bundle-or-package-id>`, `snapshot -i` (what's on screen, with @refs), `press @e3 --settle`, \
`fill @e5 \"text\" --settle`, `install <app>`, `screenshot <path>`.
- Never start or drive a device yourself: no `xcrun simctl boot`, `open -a Simulator`, \
`emulator -avd`, `adb shell input`, `idb`, or scripts that wrap them. Build with the project's \
usual tools, then install and launch through `agent-device`.
- `gentle-device close \"<name>\" --shutdown` turns a device off when you're done with it.
";

/// The instructions for `prefs`, written to the app's data folder; the
/// file to append, or `None` when it can't be written.
pub fn agent_instructions(prefs: Prefs) -> Option<PathBuf> {
    let path = dirs::data_dir()?
        .join("gentle-shell-gpui")
        .join("agent-instructions.md");
    let mut text = PAGES.to_string();
    if prefs.enabled && prefs.agents {
        text.push_str(DEVICES);
    }
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, text).ok()?;
    }
    Some(path)
}
