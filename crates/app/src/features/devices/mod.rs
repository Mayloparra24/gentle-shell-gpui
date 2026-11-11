//! Devices: iOS simulators and Android emulators, live and interactive inside
//! the app. Expo's device hub ([`hub`]) boots them headless and serves each
//! one's screen with touch, keyboard and hardware buttons; the window's
//! [`DevicePanel`] shows them as tabs, docked at the right or floating.
//! Agents open devices with `gentle-device` (the app boots it and shows a
//! tab) and drive them with `agent-device`, so nothing opens Simulator.app or
//! an emulator window outside the app.
//!
//! [`Hub`] is app-wide: one hub process, started on first use while
//! Configuration → Devices has "Use devices" on, and the device list, polled
//! only while a panel shows or a device is booting. Devices the app booted
//! are shut down when it quits.

pub mod cli;
mod hub;
mod instructions;
mod panel;
mod sdk;

use std::time::Duration;

use gentle_core::devices::{Device, State};
use gpui_kit::*;

use crate::features::setup::devices as setup;
pub use instructions::agent_instructions;
pub use panel::{DevicePanel, DevicePanelEvent, Mode};
pub(crate) use sdk::{android_sdk, android_tool_dirs, simctl_path, xcode_developer_dir};

/// What the hub is doing.
#[derive(Clone, Debug, PartialEq)]
pub enum HubStatus {
    /// Not running ("Use devices" is off, or nothing asked for a device yet).
    Off,
    Starting,
    Running(u16),
    Failed(String),
}

pub struct Hub {
    /// Configuration → Devices' switches, kept here so renders don't read
    /// the settings file.
    prefs: setup::Prefs,
    status: HubStatus,
    process: Option<hub::HubProcess>,
    devices: Vec<Device>,
    /// What the hub couldn't list (no Xcode, no SDK, …).
    errors: Vec<String>,
    /// Devices this app booted: quitting shuts them down.
    booted_here: Vec<Device>,
    /// Keys of devices booting; the list is polled fast until they're ready.
    booting: Vec<String>,
    /// Device panels showing; the list is polled while any is.
    watchers: usize,
    last_error: Option<String>,
    _start: Task<()>,
    _poll: Task<()>,
    _quit: Subscription,
}

struct HubGlobal(Entity<Hub>);

impl Global for HubGlobal {}

/// The app's hub model, created on first use.
pub fn hub(cx: &mut App) -> Entity<Hub> {
    if let Some(global) = cx.try_global::<HubGlobal>() {
        return global.0.clone();
    }
    let entity = cx.new(Hub::new);
    cx.set_global(HubGlobal(entity.clone()));
    entity
}

impl Hub {
    fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok((wait, port)) = this.read_with(cx, |hub, _| hub.next_poll()) else {
                    return;
                };
                cx.background_executor().timer(wait).await;
                let Some(port) = port else { continue };
                let listed = cx
                    .background_executor()
                    .spawn(async move { hub::list(port) })
                    .await;
                if this
                    .update(cx, |hub, cx| hub.apply_list(listed, cx))
                    .is_err()
                {
                    return;
                }
            }
        });
        Self {
            prefs: setup::prefs(),
            status: HubStatus::Off,
            process: None,
            devices: Vec::new(),
            errors: Vec::new(),
            booted_here: Vec::new(),
            booting: Vec::new(),
            watchers: 0,
            last_error: None,
            _start: Task::ready(()),
            _poll: poll,
            _quit: cx.on_app_quit(|hub, _| {
                hub.quit();
                async {}
            }),
        }
    }

    pub fn prefs(&self) -> setup::Prefs {
        self.prefs
    }

    /// Configuration changed the switches: stop the hub when "Use devices"
    /// went off.
    pub fn set_prefs(&mut self, prefs: setup::Prefs, cx: &mut Context<Self>) {
        self.prefs = prefs;
        if !prefs.enabled {
            self.stop(cx);
        }
        cx.notify();
    }

    pub fn status(&self) -> &HubStatus {
        &self.status
    }

    pub fn port(&self) -> Option<u16> {
        match self.status {
            HubStatus::Running(port) => Some(port),
            _ => None,
        }
    }

    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The last boot or shutdown that failed.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn is_booting(&self, device: &Device) -> bool {
        self.booting.contains(&device.key())
    }

    fn next_poll(&self) -> (Duration, Option<u16>) {
        let port = self.port();
        if !self.booting.is_empty() {
            (Duration::from_secs(1), port)
        } else if self.watchers > 0 {
            (Duration::from_secs(3), port)
        } else {
            (Duration::from_secs(5), None)
        }
    }

    fn apply_list(
        &mut self,
        listed: Result<(Vec<Device>, Vec<String>), String>,
        cx: &mut Context<Self>,
    ) {
        match listed {
            Ok((devices, errors)) => {
                self.booting.retain(|key| {
                    !devices
                        .iter()
                        .any(|d| &d.key() == key && d.state == State::Ready)
                });
                if self.devices != devices || self.errors != errors {
                    self.devices = devices;
                    self.errors = errors;
                    cx.notify();
                }
            }
            // The hub went away (crashed, killed): start it again on demand.
            Err(error) if self.port().is_some_and(|port| !hub::ready(port)) => {
                self.process = None;
                self.status = HubStatus::Failed(error);
                cx.notify();
            }
            Err(_) => {}
        }
    }

    /// A panel started or stopped showing.
    pub fn watch(&mut self, showing: bool, cx: &mut Context<Self>) {
        if showing {
            self.watchers += 1;
            self.ensure(cx);
            self.refresh(cx);
        } else {
            self.watchers = self.watchers.saturating_sub(1);
        }
    }

    /// Start the hub when "Use devices" is on and it isn't running.
    pub fn ensure(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, HubStatus::Starting | HubStatus::Running(_)) || !self.prefs.enabled
        {
            return;
        }
        self.status = HubStatus::Starting;
        cx.notify();
        self._start = cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async { hub::HubProcess::start() })
                .await;
            let _ = this.update(cx, |hub, cx| {
                match started {
                    Ok(process) => {
                        hub.status = HubStatus::Running(process.port());
                        hub.process = Some(process);
                        hub.refresh(cx);
                    }
                    Err(error) => hub.status = HubStatus::Failed(error),
                }
                cx.notify();
            });
        });
    }

    /// "Use devices" turned off: stop the hub (devices keep running).
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(process) = self.process.take() {
            // Stopping waits for the hub's helpers; not on the main thread.
            cx.background_executor()
                .spawn(async move { drop(process) })
                .detach();
        }
        self.status = HubStatus::Off;
        self.devices.clear();
        cx.notify();
    }

    /// Read the list now.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(port) = self.port() else { return };
        cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move { hub::list(port) })
                .await;
            let _ = this.update(cx, |hub, cx| hub.apply_list(listed, cx));
        })
        .detach();
    }

    /// Boot `device` headless; the list follows it until it's ready.
    pub fn boot(&mut self, device: Device, cx: &mut Context<Self>) {
        let Some(port) = self.port() else {
            self.ensure(cx);
            return;
        };
        if device.state.is_running() || self.is_booting(&device) {
            return;
        }
        self.booting.push(device.key());
        self.last_error = None;
        if !self.booted_here.iter().any(|d| d.key() == device.key()) {
            self.booted_here.push(device.clone());
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = device.clone();
            let result = cx
                .background_executor()
                .spawn(async move { hub::boot(port, &target) })
                .await;
            let _ = this.update(cx, |hub, cx| {
                if let Err(error) = result {
                    hub.booting.retain(|key| *key != device.key());
                    hub.booted_here.retain(|d| d.key() != device.key());
                    hub.last_error = Some(error);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn shutdown(&mut self, device: Device, cx: &mut Context<Self>) {
        let Some(port) = self.port() else { return };
        self.booted_here.retain(|d| d.key() != device.key());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { hub::shutdown(port, &device) })
                .await;
            let _ = this.update(cx, |hub, cx| {
                hub.last_error = result.err();
                hub.refresh(cx);
            });
        })
        .detach();
    }

    /// The app is quitting: shut down what it booted, stop agent-device's
    /// daemon (when agents can drive devices), then stop the hub.
    fn quit(&mut self) {
        if self.prefs.enabled && self.prefs.agents {
            hub::stop_agent_daemon();
        }
        if let Some(port) = self.port() {
            for device in self.booted_here.drain(..) {
                let current = self
                    .devices
                    .iter()
                    .find(|d| d.key() == device.key())
                    .cloned()
                    .unwrap_or(device);
                let _ = hub::shutdown(port, &current);
            }
        }
        self.process = None;
    }

    /// The device `query` names; see [`find_device`].
    pub fn find(&self, query: &str) -> Option<&Device> {
        find_device(&self.devices, query)
    }
}

/// The device `query` names: a UDID, an adb serial, or a name (case, `_`,
/// `-` and spaces don't matter).
pub fn find_device<'a>(devices: &'a [Device], query: &str) -> Option<&'a Device> {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| !matches!(c, ' ' | '_' | '-'))
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let wanted = norm(query);
    devices
        .iter()
        .find(|d| d.id.eq_ignore_ascii_case(query))
        .or_else(|| devices.iter().find(|d| norm(&d.name) == wanted))
}
