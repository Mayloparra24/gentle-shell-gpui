//! Configuration: everything the user's Gentle Shell is already set up with,
//! read from the same homes the launcher uses, plus package management
//! (`gentle-shell install|remove`) and the Claude bridge's plan.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gentle_core::notifications;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::file_cache::Stamp;
use serde_json::Value;

use crate::{alerts, app_update, themes, ui};

pub enum SettingsEvent {
    Close,
    /// Packages or sign-ins changed: tabs should reload their backend.
    Changed,
    /// Run this Gentle Shell slash command in the active agent pane and
    /// report back through [`SettingsView::shell_done`].
    ShellCommand(String),
    /// `gentle-shell upgrade` finished: agent tabs restart on the new version.
    BackendUpdated,
    /// An action from the Setup & requirements checklist (sign in, a
    /// terminal command, an install that finished) for the window to carry out.
    Setup(crate::onboarding::OnboardingEvent),
}

/// Recommended packages, with why. `pi-claude-bridge` is the provider Gentle
/// Shell's docs reference for Claude subscriptions (priced turns, usage headers).
pub const RECOMMENDED: &[(&str, &str)] = &[
    (
        "npm:pi-claude-bridge",
        "Claude Pro/Max through Claude Code (claude-bridge/… models)",
    ),
    (
        "npm:pi-claude-bridge-askclaude",
        "AskClaude tool: delegate to Claude Code from another provider",
    ),
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub home_mode: String,
    pub agent: PathBuf,
    pub config: PathBuf,
    pub launcher: Option<PathBuf>,
    pub versions: Vec<String>,
    pub default_model: Option<String>,
    pub default_thinking: Option<String>,
    pub enabled_models: Vec<String>,
    pub theme: Option<String>,
    pub packages: Vec<Package>,
    pub extensions: Vec<String>,
    pub sign_ins: Vec<(String, String)>,
    pub custom_providers: Vec<String>,
    pub mcp_servers: Vec<String>,
    pub permission_mode: Option<String>,
    pub persona: Option<String>,
    pub active_profile: Option<String>,
    pub profiles: Vec<String>,
    pub background_subagents: BackgroundPolicy,
    pub banner: Option<String>,
    pub bridge_plan: Option<String>,
    pub claude_code: Option<String>,
    /// A newer Gentle Shell release, checked as the launcher's startup does.
    pub update: Option<crate::updates::UpdateOffer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub source: String,
    pub version: Option<String>,
}

/// Which source decided the background-subagents policy, in Gentle Shell's
/// order: project file, then global file, then environment, then off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PolicySource {
    ProjectFile,
    GlobalFile,
    Environment,
    #[default]
    Default,
}

/// The effective background-subagents policy, resolved as
/// `lib/background-subagents-policy.ts` does so the app and the shell agree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackgroundPolicy {
    pub on: bool,
    pub source: PolicySource,
    /// The deciding file exists but fails the strict decode (reads as off).
    pub malformed: bool,
    pub project_file: PathBuf,
}

pub const BACKGROUND_SUBAGENTS_FILE: &str = "background-subagents.json";
const BACKGROUND_SUBAGENTS_SCHEMA: &str = "gentle-pi.background-subagents/v1";

/// Strict decode: exactly `{"schema": …/v1, "policy": "on"|"off"}`.
fn parse_policy_file(raw: &str) -> Option<bool> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if object.len() != 2 || object.get("schema")?.as_str()? != BACKGROUND_SUBAGENTS_SCHEMA {
        return None;
    }
    match object.get("policy")?.as_str()? {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

/// First hit wins; a present but malformed file fails closed to off and keeps
/// the decision (it does not fall through), exactly like the shell.
pub fn resolve_background_policy(cwd: &Path, config: &Path, env: Option<&str>) -> BackgroundPolicy {
    let project_file = cwd
        .join(".pi")
        .join("gentle-ai")
        .join(BACKGROUND_SUBAGENTS_FILE);
    let global_file = config.join(BACKGROUND_SUBAGENTS_FILE);
    for (source, path) in [
        (PolicySource::ProjectFile, &project_file),
        (PolicySource::GlobalFile, &global_file),
    ] {
        if !path.exists() {
            continue;
        }
        let decoded = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| parse_policy_file(&raw));
        return BackgroundPolicy {
            on: decoded == Some(true),
            source,
            malformed: decoded.is_none(),
            project_file: project_file.clone(),
        };
    }
    match env {
        Some(value @ ("on" | "off")) => BackgroundPolicy {
            on: value == "on",
            source: PolicySource::Environment,
            malformed: false,
            project_file,
        },
        _ => BackgroundPolicy {
            project_file,
            ..BackgroundPolicy::default()
        },
    }
}

impl BackgroundPolicy {
    /// `on · global file`, `off · default`, `off · project file (malformed)`.
    pub fn describe(&self) -> String {
        let source = match self.source {
            PolicySource::ProjectFile => "project file",
            PolicySource::GlobalFile => "global file",
            PolicySource::Environment => "GENTLE_PI_BACKGROUND_SUBAGENTS",
            PolicySource::Default => "default",
        };
        let state = if self.on { "on" } else { "off" };
        if self.malformed {
            format!("{state} · {source} (malformed)")
        } else {
            format!("{state} · {source}")
        }
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `npm:@scope/name@1.2` → `@scope/name`.
pub fn npm_name(source: &str) -> Option<&str> {
    let spec = source.strip_prefix("npm:")?;
    let cut = if let Some(rest) = spec.strip_prefix('@') {
        rest.find('@').map(|i| i + 1)
    } else {
        spec.find('@')
    };
    Some(match cut {
        Some(i) => &spec[..i],
        None => spec,
    })
}

/// Everything readable without running anything but `--version`.
pub fn read_snapshot(agent: &Path, config: &Path, cwd: &Path) -> Snapshot {
    let settings = read_json(&agent.join("settings.json")).unwrap_or(Value::Null);
    let str_of = |v: &Value| v.as_str().map(str::to_string);
    let packages = settings["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().or(p["source"].as_str()))
        .map(|source| Package {
            source: source.to_string(),
            version: npm_name(source).and_then(|name| {
                read_json(
                    &agent
                        .join("npm")
                        .join("node_modules")
                        .join(name)
                        .join("package.json"),
                )
                .and_then(|p| p["version"].as_str().map(str::to_string))
            }),
        })
        .collect();
    let sign_ins = read_json(&agent.join("auth.json"))
        .and_then(|v| v.as_object().cloned())
        .map(|o| {
            o.into_iter()
                .map(|(k, v)| (k, v["type"].as_str().unwrap_or("?").to_string()))
                .collect()
        })
        .unwrap_or_default();
    let keys = |v: Option<Value>, field: &str| -> Vec<String> {
        v.and_then(|v| v[field].as_object().map(|o| o.keys().cloned().collect()))
            .unwrap_or_default()
    };
    let profiles_file = read_json(&config.join("profiles.json"));
    let persona = read_json(&cwd.join(".pi").join("gentle-ai").join("persona.json"))
        .or_else(|| read_json(&config.join("persona.json")))
        .and_then(|v| str_of(&v["mode"]));
    let background = resolve_background_policy(
        cwd,
        config,
        std::env::var("GENTLE_PI_BACKGROUND_SUBAGENTS")
            .ok()
            .as_deref(),
    );
    let banner = read_json(&config.join("banner.json")).map(|b| {
        format!(
            "{} · rose {} · text {}",
            b["color"].as_str().unwrap_or("pink"),
            if b["showRose"] == false { "off" } else { "on" },
            if b["showTextLogo"] == false {
                "off"
            } else {
                "on"
            }
        )
    });
    Snapshot {
        home_mode: String::new(),
        agent: agent.to_path_buf(),
        config: config.to_path_buf(),
        launcher: None,
        versions: Vec::new(),
        default_model: match (
            str_of(&settings["defaultProvider"]),
            str_of(&settings["defaultModel"]),
        ) {
            (Some(p), Some(m)) => Some(format!("{p}/{m}")),
            (None, Some(m)) => Some(m),
            _ => None,
        },
        default_thinking: str_of(&settings["defaultThinkingLevel"]),
        enabled_models: settings["enabledModels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(&str_of)
            .collect(),
        theme: str_of(&settings["theme"]),
        packages,
        extensions: settings["extensions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(&str_of)
            .collect(),
        sign_ins,
        custom_providers: keys(read_json(&agent.join("models.json")), "providers"),
        mcp_servers: keys(read_json(&agent.join("mcp.json")), "mcpServers"),
        permission_mode: read_json(&config.join("permission-mode.json"))
            .and_then(|v| str_of(&v["mode"])),
        persona,
        active_profile: profiles_file.as_ref().and_then(|v| str_of(&v["active"])),
        profiles: keys(profiles_file, "profiles"),
        background_subagents: background,
        banner,
        bridge_plan: read_json(&agent.join("claude-bridge.json"))
            .and_then(|v| str_of(&v["provider"]["plan"])),
        claude_code: None,
        update: None,
    }
}

/// Runs the launcher's own read-only probes: `--version` and the home mode.
fn probe(spec: &pi_rpc::LaunchSpec, snapshot: &mut Snapshot) {
    snapshot.launcher = Some(spec.program.clone());
    if let Some(text) = pi_rpc::version_output(spec) {
        for line in text.lines() {
            if let Some(mode) = line.strip_prefix("home ") {
                snapshot.home_mode = mode.to_string();
            } else if !line.trim().is_empty() {
                snapshot.versions.push(line.trim().to_string());
            }
        }
    }
    snapshot.claude_code = claude_auth_status();
}

/// One probe result and what it was valid for.
struct Probe<T> {
    at: Instant,
    stamp: Option<Stamp>,
    value: T,
}

/// `claude auth status`, reused briefly: reopening Configuration should not
/// start Node every time.
static CLAUDE_AUTH: Mutex<Option<Probe<Option<String>>>> = Mutex::new(None);
const CLAUDE_AUTH_TTL: Duration = Duration::from_secs(60);

/// Run `claude auth status` again next time (after anything that may change it).
pub fn forget_claude_auth() {
    *CLAUDE_AUTH.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The bridge rides on Claude Code's own login. Also re-run when Claude
/// Code's `~/.claude.json` (it records the signed-in account) changes.
fn claude_auth_status() -> Option<String> {
    let stamp = dirs::home_dir().and_then(|home| Stamp::of(&home.join(".claude.json")));
    reuse_or_run(&CLAUDE_AUTH, Instant::now(), CLAUDE_AUTH_TTL, stamp, || {
        let mut claude = pi_rpc::command("claude");
        claude
            .args(["auth", "status"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        claude.env("PATH", pi_rpc::child_path());
        match claude.output() {
            Ok(out) => serde_json::from_slice::<Value>(&out.stdout).ok().map(|v| {
                if v["loggedIn"] == true {
                    format!(
                        "signed in ({})",
                        v["authMethod"].as_str().unwrap_or("claude.ai")
                    )
                } else {
                    "not signed in — run `claude auth login`".to_string()
                }
            }),
            Err(_) => Some("Claude Code is not installed".into()),
        }
    })
}

/// `slot`'s value if younger than `ttl` and taken at `stamp`, else `run()`.
/// The lock is held while running so concurrent loads share one run.
fn reuse_or_run<T: Clone>(
    slot: &Mutex<Option<Probe<T>>>,
    now: Instant,
    ttl: Duration,
    stamp: Option<Stamp>,
    run: impl FnOnce() -> T,
) -> T {
    let mut cached = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(probe) = cached.as_ref()
        && probe.stamp == stamp
        && now.saturating_duration_since(probe.at) < ttl
    {
        return probe.value.clone();
    }
    let value = run();
    *cached = Some(Probe {
        at: now,
        stamp,
        value: value.clone(),
    });
    value
}

/// Set `provider.plan` in `claude-bridge.json`, keeping every other key.
pub fn save_bridge_plan(agent: &Path, plan: &str) -> std::io::Result<()> {
    let path = agent.join("claude-bridge.json");
    let mut value = read_json(&path)
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    if !value["provider"].is_object() {
        value["provider"] = serde_json::json!({});
    }
    value["provider"]["plan"] = Value::String(plan.to_string());
    let temp = agent.join(format!(".claude-bridge.json.{}.tmp", std::process::id()));
    std::fs::write(
        &temp,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        ),
    )?;
    std::fs::rename(temp, path)
}

pub struct SettingsView {
    cwd: PathBuf,
    snapshot: Option<Snapshot>,
    package_input: Entity<InputState>,
    running: Option<String>,
    log: Vec<String>,
    _load: Task<()>,
    _job: Task<()>,
    _subscription: Subscription,
    /// Window transparency, 0–100 %.
    transparency: Entity<SliderState>,
    _transparency: Subscription,
    /// A `/gentle:background-subagents` change sent to the shell, not yet answered.
    background_pending: bool,
    /// The shell's answer to the last change: `(text, failed)`.
    background_note: Option<(String, bool)>,
    /// `gentle-shell upgrade` running.
    upgrading: bool,
    /// How the last `gentle-shell upgrade` ended, with its last output lines.
    upgrade: Option<UpgradeResult>,
    /// "Skip this version" couldn't be saved.
    skip_error: Option<String>,
    /// Re-render as this app's own update moves along.
    _app_update: Subscription,
    /// A failed notifications.json write.
    notifications_note: Option<String>,
    /// Setup & requirements: the onboarding checklist, embedded.
    setup: Entity<crate::onboarding::OnboardingView>,
    _setup: Subscription,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let package_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("npm:package-name, git:github.com/…, or a local path")
        });
        let subscription = cx.subscribe_in(
            &package_input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let source = input.read(cx).value().trim().to_string();
                    if !source.is_empty() {
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        this.run_package("install", source, cx);
                    }
                }
            },
        );
        let percent = (cx.global::<themes::Themes>().transparency * 100.).round();
        let transparency = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(percent)
        });
        // Drag re-tints live; the file is written once, on release.
        let transparency_sub = cx.subscribe(&transparency, |_, _, event: &SliderEvent, cx| {
            let (value, persist) = match event {
                SliderEvent::Change(value) => (value.start(), false),
                SliderEvent::Release(value) => (value.start(), true),
            };
            themes::set_transparency(value / 100., persist, cx);
        });
        let setup = cx.new(crate::onboarding::OnboardingView::embedded);
        let setup_sub = cx.subscribe(&setup, |_, _, event, cx| {
            cx.emit(SettingsEvent::Setup(event.clone()));
        });
        let mut this = Self {
            cwd,
            snapshot: None,
            package_input,
            running: None,
            log: Vec::new(),
            _load: Task::ready(()),
            _job: Task::ready(()),
            _subscription: subscription,
            transparency,
            _transparency: transparency_sub,
            background_pending: false,
            background_note: None,
            upgrading: false,
            upgrade: None,
            skip_error: None,
            _app_update: cx.observe_global::<app_update::AppUpdate>(|_, cx| cx.notify()),
            notifications_note: None,
            setup,
            _setup: setup_sub,
        };
        this.reload(cx);
        this
    }

    /// gentle-shell can start, as of the Setup & requirements checklist.
    pub fn setup_usable(&self, cx: &App) -> bool {
        self.setup.read(cx).usable()
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        self._load = cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    let homes = pi_rpc::resolve_homes();
                    let mut snapshot = read_snapshot(&homes.agent, &homes.config, &cwd);
                    if let Ok(spec) = pi_rpc::resolve_backend(&cwd) {
                        probe(&spec, &mut snapshot);
                    }
                    // Cached for a day in the launcher config, so reopening
                    // Configuration stays off the network.
                    snapshot.update = crate::updates::current_from_versions(&snapshot.versions)
                        .and_then(crate::updates::check);
                    snapshot
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.snapshot = Some(snapshot);
                cx.notify();
            });
        });
    }

    /// `gentle-shell install|remove <source>`: Pi's own package manager
    /// against the resolved home.
    fn run_package(&mut self, verb: &'static str, source: String, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        self.running = Some(format!("{verb} {source}"));
        self.log.clear();
        let cwd = self.cwd.clone();
        self._job = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let spec = pi_rpc::resolve_backend(&cwd).map_err(|e| format!("{e:#}"))?;
                    let mut command = pi_rpc::command(&spec.program);
                    // Launcher options (home selectors) precede the subcommand.
                    command
                        .args(spec.args.iter().take_while(|a| *a != "--mode"))
                        .args([verb, &source])
                        .current_dir(&spec.cwd)
                        .stdin(std::process::Stdio::null());
                    for (k, v) in &spec.env {
                        command.env(k, v);
                    }
                    let out = command.output().map_err(|e| e.to_string())?;
                    let text = format!(
                        "{}{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                    if out.status.success() {
                        Ok(text)
                    } else {
                        Err(text)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.running = None;
                forget_claude_auth();
                let (ok, text) = match result {
                    Ok(text) => (true, text),
                    Err(text) => (false, text),
                };
                this.log = text
                    .lines()
                    .rev()
                    .take(8)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if ok {
                    cx.emit(SettingsEvent::Changed);
                }
                this.reload(cx);
                cx.notify();
            });
        });
        cx.notify();
    }

    fn set_plan(&mut self, plan: &'static str, cx: &mut Context<Self>) {
        let agent = pi_rpc::resolve_homes().agent;
        forget_claude_auth();
        match save_bridge_plan(&agent, plan) {
            Ok(()) => {
                self.log = vec![format!(
                    "Claude bridge plan set to {plan}. New sessions use it."
                )];
                cx.emit(SettingsEvent::Changed);
            }
            Err(e) => self.log = vec![format!("Could not write claude-bridge.json: {e}")],
        }
        self.reload(cx);
    }

    /// Gentle Shell's own `upgrade` toward `target` (it knows how it was
    /// installed; since 4.7 it moves to its self-contained install), then ask
    /// the launcher the app resolves now which version it runs: that's the
    /// result shown.
    fn run_upgrade(&mut self, target: String, cx: &mut Context<Self>) {
        if self.upgrading {
            return;
        }
        self.upgrading = true;
        self.upgrade = None;
        let cwd = self.cwd.clone();
        self._job = cx.spawn(async move |this, cx| {
            let (succeeded, text, running, launcher) = cx
                .background_executor()
                .spawn(async move {
                    let spec = match pi_rpc::resolve_backend(&cwd) {
                        Ok(spec) => spec,
                        Err(error) => return (false, format!("{error:#}"), None, None),
                    };
                    let mut command = pi_rpc::command(&spec.program);
                    command
                        .args(spec.args.iter().take_while(|a| *a != "--mode"))
                        .arg("upgrade")
                        .current_dir(&spec.cwd)
                        .stdin(std::process::Stdio::null());
                    for (k, v) in &spec.env {
                        command.env(k, v);
                    }
                    let (succeeded, text) = match command.output() {
                        Ok(out) => (
                            out.status.success(),
                            format!(
                                "{}{}",
                                String::from_utf8_lossy(&out.stdout),
                                String::from_utf8_lossy(&out.stderr)
                            ),
                        ),
                        Err(error) => (false, error.to_string()),
                    };
                    // The upgrade may have installed Gentle Shell somewhere
                    // new (its self-contained install): resolve it afresh.
                    pi_rpc::forget_cached();
                    let now = pi_rpc::resolve_backend(&cwd).ok();
                    let running = now.as_ref().and_then(pi_rpc::backend_version);
                    (succeeded, text, running, now.map(|spec| spec.program))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.upgrading = false;
                forget_claude_auth();
                let lines: Vec<String> = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(str::to_string)
                    .collect();
                let outcome =
                    upgrade_outcome(succeeded, &target, running.as_deref(), launcher.as_deref());
                if matches!(outcome, UpgradeOutcome::Installed(_)) {
                    cx.emit(SettingsEvent::BackendUpdated);
                }
                this.upgrade = Some(UpgradeResult {
                    target,
                    outcome,
                    lines: lines[lines.len().saturating_sub(8)..].to_vec(),
                });
                this.reload(cx);
                cx.notify();
            });
        });
        cx.notify();
    }

    fn skip_update(&mut self, version: String, cx: &mut Context<Self>) {
        if let Err(error) = crate::updates::skip(&version) {
            self.skip_error = Some(format!("Could not remember the skip: {error}"));
        }
        self.reload(cx);
    }

    /// "Gentle Shell X is available": update, release notes or skip, as the
    /// terminal's startup prompt offers.
    fn render_update(
        &self,
        offer: crate::updates::UpdateOffer,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let notes = offer.notes_url.clone();
        let skipped = offer.latest.clone();
        let target = offer.latest.clone();
        v_flex()
            .mt_2()
            .p_3()
            .gap_2()
            .rounded_lg()
            .bg(ui::accent().opacity(0.10))
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::ArrowUp).small().text_color(ui::accent()))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui::fg())
                            .child(format!("Gentle Shell {} is available", offer.latest)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui::muted())
                            .child(format!("you have {}", offer.current)),
                    ),
            )
            .child(div().text_xs().text_color(ui::muted()).child(
                "Runs gentle-shell upgrade. Open agent tabs then restart on the new version and keep their conversations; a tab mid-turn switches when the turn ends.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("update-now")
                            .primary()
                            .small()
                            .label(if self.upgrading { "Updating…" } else { "Update now" })
                            .loading(self.upgrading)
                            .disabled(self.upgrading)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_upgrade(target.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new("update-notes")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::ExternalLink))
                            .label("Release notes")
                            .on_click(move |_, _, cx| cx.open_url(&notes)),
                    )
                    .child(
                        Button::new("update-skip")
                            .ghost()
                            .small()
                            .label("Skip this version")
                            .disabled(self.upgrading)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.skip_update(skipped.clone(), cx)
                            })),
                    ),
            )
            .children(
                self.skip_error
                    .clone()
                    .map(|error| upgrade_output(vec![error], true)),
            )
    }

    /// Ask the shell itself to change the policy, as typing
    /// `/gentle:background-subagents enable|disable` would: the shell owns the
    /// file and its validation, and posts its own status report.
    fn set_background(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.background_pending {
            return;
        }
        let verb = if on { "enable" } else { "disable" };
        self.background_pending = true;
        self.background_note = None;
        cx.emit(SettingsEvent::ShellCommand(format!(
            "/gentle:background-subagents {verb}"
        )));
        cx.notify();
    }

    /// The shell's answer to a [`SettingsEvent::ShellCommand`]; re-reads the
    /// files so the row shows what the shell actually wrote.
    pub fn shell_done(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.background_pending = false;
        self.background_note = Some(match result {
            Ok(()) => (
                "Gentle Shell applied it; its report is in 🔔. Helpers launched from now on use it."
                    .into(),
                false,
            ),
            Err(error) => (error, true),
        });
        self.reload(cx);
    }
}

/// Copy for each alert's switch.
fn alert_hint(alert: notifications::Alert) -> &'static str {
    match alert {
        notifications::Alert::TurnEnded => "When a turn finishes (agent.completed).",
        notifications::Alert::NeedsYou => "When el Gentleman asks you something (agent.attention).",
        notifications::Alert::ContextHigh => {
            "When the context reaches the threshold (context.high)."
        }
    }
}

/// One Off/On row of the Notifications section.
fn alert_switch(
    id: &'static str,
    label: &'static str,
    on: bool,
    hint: &'static str,
    dim: bool,
    change: impl Fn(bool) -> notifications::Change + 'static,
    cx: &mut Context<SettingsView>,
) -> Div {
    v_flex()
        .pt_2()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(if dim { ui::faint() } else { ui::muted() })
                        .child(label),
                )
                .child(ui::segmented(
                    id,
                    &["Off", "On"],
                    Some(usize::from(on)),
                    cx.listener(move |this, index: &usize, _, cx| {
                        this.set_notification(change(*index == 1), cx)
                    }),
                )),
        )
        .child(div().text_xs().text_color(ui::faint()).child(hint))
}

impl SettingsView {
    /// Write one change to Gentle Shell's notifications.json; the section
    /// re-reads the file on the next render.
    fn set_notification(&mut self, change: notifications::Change, cx: &mut Context<Self>) {
        self.notifications_note = alerts::apply(change)
            .err()
            .map(|e| format!("Could not write notifications.json: {e}"));
        cx.notify();
    }

    /// Gentle Shell's notification policy, shared with the terminal.
    fn notifications_section(&self, cx: &mut Context<Self>) -> Div {
        let policy = alerts::policy();
        let enabled = policy.enabled();
        let mut card = section(IconName::Bell, "Notifications")
            .child(row("File", policy.file.display().to_string()))
            .child(row(
                "Context threshold",
                format!(
                    "{:.0} % (fixed by Gentle Shell)",
                    alerts::context_threshold(cx)
                ),
            ))
            .child(alert_switch(
                "alerts-enabled",
                "Sounds",
                enabled,
                "Master switch, off by default. Off also stops the Dock bounce / taskbar flash while the app is in the background.",
                false,
                notifications::Change::Enabled,
                cx,
            ));
        for alert in notifications::Alert::ALL {
            let id = match alert {
                notifications::Alert::TurnEnded => "alert-turn-ended",
                notifications::Alert::NeedsYou => "alert-needs-you",
                notifications::Alert::ContextHigh => "alert-context-high",
            };
            card = card.child(alert_switch(
                id,
                alert.label(),
                policy.sound(alert).is_some(),
                alert_hint(alert),
                !enabled,
                move |on| notifications::Change::Event(alert, on),
                cx,
            ));
        }
        let warning = match policy.state {
            notifications::FileState::Invalid => Some(
                "notifications.json isn't valid for Gentle Shell, so sounds are off. A change here replaces it with the default preset.",
            ),
            notifications::FileState::Unreadable => Some(
                "notifications.json can't be read, so sounds are off. A change here rewrites it with the default preset.",
            ),
            notifications::FileState::Missing | notifications::FileState::Valid => None,
        };
        card.child(
            div().pt_2().text_xs().text_color(ui::faint()).child(
                "Same file as /gentle:customize in the terminal. Gentle Shell plays nothing for app tabs (RPC), so the app plays the same tones itself; other events (failures, subagents) keep their terminal settings.",
            ),
        )
        .when_some(warning, |d, text| {
            d.child(div().text_xs().text_color(ui::warning()).child(text))
        })
        .when_some(self.notifications_note.clone(), |d, text| {
            d.child(div().text_xs().text_color(ui::danger()).child(text))
        })
    }
}

fn row(label: &'static str, value: impl Into<SharedString>) -> impl IntoElement {
    h_flex()
        .gap_3()
        .py_1()
        .items_start()
        .border_b_1()
        .border_color(ui::border().opacity(0.5))
        .child(
            div()
                .w(relative(0.32))
                .max_w(px(180.))
                .flex_shrink_0()
                .truncate()
                .text_xs()
                .text_color(ui::muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .font_family(ui::MONO)
                .text_color(ui::fg())
                .child(value.into()),
        )
}

fn section(icon: IconName, title: &'static str) -> Div {
    ui::card().gap_0p5().p_4().child(
        h_flex()
            .pb_2()
            .gap_2()
            .child(Icon::new(icon).xsmall().text_color(ui::champagne()))
            .child(ui::section_label(title)),
    )
}

/// Tern-style theme grid: a card per theme drawn in its own colors; clicking
/// applies it to every window and remembers it. A slider above it sets how
/// much of the blurred desktop shows through.
fn theme_section(transparency: &Entity<SliderState>, cx: &App) -> Div {
    let state = cx.global::<themes::Themes>();
    let groups = [
        (themes::Source::Gentle, "Gentle Shell"),
        (themes::Source::Pi, "Your Pi themes"),
        (themes::Source::Builtin, "Popular"),
    ];
    let percent = (state.transparency * 100.).round();
    let mut card = section(IconName::Sparkles, "Theme").child(
        h_flex()
            .pt_1()
            .pb_2()
            .gap_3()
            .child(
                div()
                    .w(px(120.))
                    .text_sm()
                    .text_color(ui::muted())
                    .child("Transparency"),
            )
            .child(
                div().flex_1().child(
                    Slider::new(transparency)
                        .horizontal()
                        .accessibility_label("Window transparency"),
                ),
            )
            .child(
                div()
                    .w(px(44.))
                    .text_right()
                    .text_sm()
                    .font_family(ui::MONO)
                    .text_color(ui::fg())
                    .child(format!("{percent:.0}%")),
            ),
    );
    for (source, label) in groups {
        let cards: Vec<_> = state
            .catalog
            .iter()
            .enumerate()
            .filter(|(_, t)| t.source == source)
            .map(|(i, t)| theme_card(i, t, i == state.active))
            .collect();
        if cards.is_empty() {
            continue;
        }
        card = card
            .child(div().pt_2().pb_1p5().child(ui::section_label(label)))
            .child(div().grid().grid_cols(4).gap_2().children(cards));
    }
    card
}

fn theme_card(index: usize, theme: &themes::Theme, selected: bool) -> Stateful<Div> {
    let solid = |hex: u32| -> Hsla { rgb(hex).into() };
    // Red, yellow, green, cyan, blue, magenta, then the accent.
    let swatches = [1usize, 3, 2, 6, 4, 5]
        .into_iter()
        .map(|i| theme.ansi[i])
        .chain([theme.accent]);
    div()
        .id(("theme-card", index))
        .flex()
        .flex_col()
        .gap_1p5()
        .p_1p5()
        .rounded_xl()
        .border_2()
        .border_color(if selected { ui::accent() } else { ui::border() })
        .when(selected, |d| d.bg(ui::accent().opacity(0.08)))
        .cursor_pointer()
        .hover(|d| d.bg(ui::surface_hover()))
        .on_click(move |_, _, cx| themes::select(index, cx))
        .child(
            v_flex()
                .gap_2()
                .px_2p5()
                .py_2()
                .rounded_lg()
                .bg(solid(theme.bg))
                .border_1()
                .border_color(solid(theme.border).opacity(0.7))
                .font_family(ui::MONO)
                .text_xs()
                .child(
                    h_flex()
                        .gap_1()
                        .overflow_hidden()
                        .child(div().text_color(solid(theme.accent)).child("❯"))
                        .child(div().text_color(solid(theme.fg)).child("Aa"))
                        .child(div().text_color(solid(theme.ansi[2])).child("ok"))
                        .child(div().text_color(solid(theme.muted)).child("~/src")),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .children(swatches.map(|c| div().size_2().rounded_full().bg(solid(c)))),
                ),
        )
        .child(
            h_flex()
                .gap_1()
                .px(px(2.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(if selected { ui::fg() } else { ui::muted() })
                        .child(theme.name.clone()),
                )
                .when(selected, |d| {
                    d.child(Icon::new(IconName::Check).xsmall().text_color(ui::accent()))
                }),
        )
}

fn or_dash(value: &Option<String>, fallback: &str) -> String {
    value.clone().unwrap_or_else(|| fallback.to_string())
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
}

/// This app's own version and its updates from GitHub releases.
fn app_section(cx: &App) -> Div {
    let status = app_update::status(cx).clone();
    let version = if app_update::is_dev(app_update::CURRENT) {
        format!("{} (development build)", app_update::CURRENT)
    } else {
        app_update::CURRENT.to_string()
    };
    let line = |icon: Option<IconName>, color: Hsla, text: String| {
        h_flex()
            .pt_2()
            .gap_2()
            .text_xs()
            .text_color(color)
            .children(icon.map(|icon| Icon::new(icon).xsmall().flex_shrink_0()))
            .child(div().flex_1().min_w_0().child(text))
    };
    let releases_button = |id: &'static str, url: String| {
        Button::new(id)
            .ghost()
            .small()
            .icon(Icon::new(IconName::ExternalLink))
            .label("GitHub releases")
            .on_click(move |_, _, cx| cx.open_url(&url))
    };
    let state: AnyElement = match &status {
        app_update::Status::Idle if app_update::is_dev(app_update::CURRENT) => line(
            None,
            ui::faint(),
            "Development build: automatic update checks are off.".into(),
        )
        .into_any_element(),
        app_update::Status::Idle => div().into_any_element(),
        app_update::Status::Checking => h_flex()
            .pt_2()
            .gap_2()
            .text_xs()
            .text_color(ui::muted())
            .child(Spinner::new().xsmall().color(ui::accent()))
            .child("Checking GitHub for a newer release…")
            .into_any_element(),
        app_update::Status::UpToDate => line(
            Some(IconName::CircleCheck),
            ui::success(),
            "You're on the latest release.".into(),
        )
        .into_any_element(),
        app_update::Status::Skipped(version) => line(
            None,
            ui::muted(),
            format!("Skipping {version}. Check for updates to see it again."),
        )
        .into_any_element(),
        app_update::Status::Available(release)
        | app_update::Status::Failed {
            release: Some(release),
            ..
        } => {
            let problem = match &status {
                app_update::Status::Failed { problem, .. } => Some(problem.clone()),
                _ => None,
            };
            let notes = release.notes_url.clone();
            v_flex()
                .mt_2()
                .p_3()
                .gap_2()
                .rounded_lg()
                .bg(ui::accent().opacity(0.10))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::ArrowUp).small().text_color(ui::accent()))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui::fg())
                                .child(format!("Gentle Shell {} is available", release.version)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(format!("you have {}", app_update::CURRENT)),
                        ),
                )
                .child(div().text_xs().text_color(ui::muted()).child(
                    "Downloads the release for this computer, checks it against the published SHA-256 and replaces the app. The running app keeps working until you restart.",
                ))
                .when_some(problem.clone(), |d, problem| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(ui::danger())
                            .child(problem.message),
                    )
                })
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new("app-update-install")
                                .primary()
                                .small()
                                .icon(Icon::new(IconName::ArrowDown))
                                .label(if problem.is_some() {
                                    "Try again"
                                } else {
                                    "Download & install"
                                })
                                .on_click(|_, _, cx| app_update::install(cx)),
                        )
                        .child(
                            Button::new("app-update-notes")
                                .ghost()
                                .small()
                                .icon(Icon::new(IconName::ExternalLink))
                                .label("Release notes")
                                .on_click(move |_, _, cx| cx.open_url(&notes)),
                        )
                        .when_some(problem.and_then(|p| p.link), |d, url| {
                            d.child(releases_button("app-update-releases", url))
                        })
                        .child(
                            Button::new("app-update-skip")
                                .ghost()
                                .small()
                                .label("Skip this version")
                                .on_click(|_, _, cx| app_update::skip(cx)),
                        ),
                )
                .into_any_element()
        }
        app_update::Status::Downloading {
            release,
            done,
            total,
        } => {
            let fraction = total
                .filter(|t| *t > 0)
                .map(|t| (*done as f32 / t as f32).clamp(0., 1.));
            v_flex()
                .mt_2()
                .p_3()
                .gap_2()
                .rounded_lg()
                .bg(ui::accent().opacity(0.10))
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .child(Spinner::new().xsmall().color(ui::accent()))
                        .child(
                            div()
                                .flex_1()
                                .text_color(ui::fg())
                                .child(format!("Downloading Gentle Shell {}…", release.version)),
                        )
                        .child(div().font_family(ui::MONO).text_color(ui::muted()).child(
                            match total {
                                Some(total) => format!("{} of {}", mb(*done), mb(*total)),
                                None => mb(*done),
                            },
                        )),
                )
                .when_some(fraction, |d, fraction| {
                    d.child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded_full()
                            .bg(ui::well())
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(fraction))
                                    .rounded_full()
                                    .bg(ui::accent()),
                            ),
                    )
                })
                .into_any_element()
        }
        app_update::Status::Installing(release) => h_flex()
            .pt_2()
            .gap_2()
            .text_xs()
            .text_color(ui::muted())
            .child(Spinner::new().xsmall().color(ui::accent()))
            .child(format!(
                "Verified. Installing Gentle Shell {}…",
                release.version
            ))
            .into_any_element(),
        app_update::Status::Ready { release, .. } => v_flex()
            .mt_2()
            .p_3()
            .gap_2()
            .rounded_lg()
            .bg(ui::success().opacity(0.10))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(IconName::CircleCheck)
                            .small()
                            .text_color(ui::success()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui::fg())
                            .child(format!("Gentle Shell {} is installed", release.version)),
                    ),
            )
            .child(div().text_xs().text_color(ui::muted()).child(
                "Restart to use it. Workspaces are saved first and reopen as you left them.",
            ))
            .child(
                h_flex().child(
                    Button::new("app-update-restart")
                        .primary()
                        .small()
                        .icon(Icon::new(IconName::RotateCw))
                        .label("Restart now")
                        .on_click(|_, _, cx| app_update::restart(cx)),
                ),
            )
            .into_any_element(),
        app_update::Status::Failed {
            problem,
            release: None,
        } => v_flex()
            .child(line(
                Some(IconName::CircleAlert),
                ui::danger(),
                problem.message.clone(),
            ))
            .when_some(problem.link.clone(), |d, url| {
                d.child(
                    h_flex()
                        .pt_1()
                        .child(releases_button("app-update-releases", url)),
                )
            })
            .into_any_element(),
    };
    section(IconName::Info, "App")
        .child(row("Version", version))
        .child(state)
        .child(
            h_flex().pt_2().child(
                Button::new("app-update-check")
                    .ghost()
                    .small()
                    .icon(Icon::new(IconName::RefreshCw))
                    .label("Check for updates")
                    .loading(matches!(status, app_update::Status::Checking))
                    .disabled(status.busy())
                    .on_click(|_, _, cx| app_update::check(true, cx)),
            ),
        )
}

/// How `gentle-shell upgrade` ended, as the app sees it afterwards.
#[derive(Clone, Debug, PartialEq)]
enum UpgradeOutcome {
    /// The launcher the app resolves now runs this version.
    Installed(String),
    /// The upgrade ran, but the launcher the app resolves still runs
    /// `running` (another install comes first).
    Stale {
        running: String,
        launcher: String,
    },
    Failed,
}

#[derive(Clone)]
struct UpgradeResult {
    target: String,
    outcome: UpgradeOutcome,
    /// Gentle Shell's last output lines.
    lines: Vec<String>,
}

/// Decide the outcome from the upgrade's exit and what the launcher the app
/// now resolves (`launcher`) reports as its version (`running`).
fn upgrade_outcome(
    succeeded: bool,
    target: &str,
    running: Option<&str>,
    launcher: Option<&Path>,
) -> UpgradeOutcome {
    match (succeeded, running) {
        (false, _) | (true, None) => UpgradeOutcome::Failed,
        (true, Some(running)) if running == target => {
            UpgradeOutcome::Installed(running.to_string())
        }
        (true, Some(running)) => UpgradeOutcome::Stale {
            running: running.to_string(),
            launcher: launcher.map_or_else(
                || "gentle-shell".to_string(),
                |path| path.display().to_string(),
            ),
        },
    }
}

/// The upgrade's result: what happened and what's left to do, then
/// Gentle Shell's own last lines.
fn upgrade_result(result: &UpgradeResult) -> Div {
    let (icon, color, title, line) = match &result.outcome {
        UpgradeOutcome::Installed(version) => (
            IconName::CircleCheck,
            ui::success(),
            format!("Gentle Shell {version} is installed"),
            "Open agent tabs restarted on it and kept their conversations. Nothing else to do."
                .to_string(),
        ),
        UpgradeOutcome::Stale { running, launcher } => (
            IconName::TriangleAlert,
            ui::warning(),
            format!(
                "Gentle Shell {} installed, but the app still runs {running}",
                result.target
            ),
            format!(
                "{launcher} comes first. Remove the old package with `npm uninstall -g gentle-pi`, then open Configuration again."
            ),
        ),
        UpgradeOutcome::Failed => (
            IconName::CircleX,
            ui::danger(),
            "The update didn't finish".to_string(),
            "Gentle Shell's output is below; the version you had keeps running.".to_string(),
        ),
    };
    let failed = result.outcome == UpgradeOutcome::Failed;
    v_flex()
        .mt_2()
        .p_3()
        .gap_1()
        .rounded_lg()
        .bg(color.opacity(0.10))
        .child(
            h_flex()
                .gap_2()
                .child(Icon::new(icon).small().text_color(color))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui::fg())
                        .child(title),
                ),
        )
        .child(div().text_xs().text_color(ui::muted()).child(line))
        .when(!result.lines.is_empty(), |d| {
            d.child(upgrade_output(result.lines.clone(), failed))
        })
}

/// The last lines `gentle-shell upgrade` printed, tinted by outcome.
fn upgrade_output(lines: Vec<String>, failed: bool) -> Div {
    v_flex()
        .mt_1()
        .p_2()
        .rounded_md()
        .bg(ui::well())
        .font_family(ui::MONO)
        .text_xs()
        .text_color(if failed { ui::danger() } else { ui::muted() })
        .children(lines.into_iter().map(|line| div().child(line)))
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = match &self.snapshot {
            None => h_flex()
                .p_6()
                .gap_2()
                .justify_center()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::accent()))
                .child("Reading your Gentle Shell configuration…")
                .into_any_element(),
            Some(s) => {
                let installed = |source: &str| {
                    s.packages
                        .iter()
                        .any(|p| npm_name(&p.source) == npm_name(source))
                };
                let bridge_installed = installed("npm:pi-claude-bridge");
                let plan = match s.bridge_plan.as_deref() {
                    Some("max") => Some(1),
                    Some("pro") | None => Some(0),
                    Some(_) => None,
                };
                ui::appear(
                    "settings-content",
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            section(IconName::SquareTerminal, "Gentle Shell")
                                .child(row(
                                    "Home",
                                    if s.home_mode.is_empty() {
                                        s.agent.display().to_string()
                                    } else {
                                        s.home_mode.clone()
                                    },
                                ))
                                .child(row("Agent dir", s.agent.display().to_string()))
                                .child(row("Config dir", s.config.display().to_string()))
                                .child(row(
                                    "Launcher",
                                    s.launcher
                                        .as_ref()
                                        .map(|p| p.display().to_string())
                                        .unwrap_or_else(|| "not found".into()),
                                ))
                                .child(row("Versions", s.versions.join(" · ")))
                                .when_some(s.update.clone(), |d, offer| {
                                    d.child(self.render_update(offer, cx))
                                })
                                .children(self.upgrade.as_ref().map(upgrade_result))
                                .child(
                                    h_flex()
                                        .pt_2()
                                        .gap_2()
                                        .flex_wrap()
                                        .child({
                                            let dir = s.agent.clone();
                                            Button::new("open-agent-dir")
                                                .ghost()
                                                .small()
                                                .icon(Icon::new(IconName::FolderOpen))
                                                .label("Open agent dir")
                                                .on_click(move |_, _, cx| {
                                                    cx.open_url(&format!(
                                                        "file://{}",
                                                        dir.display()
                                                    ))
                                                })
                                        })
                                        .child({
                                            let dir = s.config.clone();
                                            Button::new("open-config-dir")
                                                .ghost()
                                                .small()
                                                .icon(Icon::new(IconName::FolderOpen))
                                                .label("Open config dir")
                                                .on_click(move |_, _, cx| {
                                                    cx.open_url(&format!(
                                                        "file://{}",
                                                        dir.display()
                                                    ))
                                                })
                                        }),
                                ),
                        )
                        .child(
                            section(IconName::CircleCheck, "Setup & requirements")
                                .child(self.setup.clone()),
                        )
                        .child(theme_section(&self.transparency, cx))
                        .child(
                            section(IconName::Cpu, "Models")
                                .child(row(
                                    "Default model",
                                    or_dash(&s.default_model, "first signed-in model"),
                                ))
                                .child(row(
                                    "Default effort",
                                    or_dash(&s.default_thinking, "medium"),
                                ))
                                .child(row(
                                    "Scoped models",
                                    if s.enabled_models.is_empty() {
                                        "all".into()
                                    } else {
                                        s.enabled_models.join(", ")
                                    },
                                ))
                                .child(row("Active profile", or_dash(&s.active_profile, "none")))
                                .child(row(
                                    "Profiles",
                                    if s.profiles.is_empty() {
                                        "—".into()
                                    } else {
                                        s.profiles.join(", ")
                                    },
                                ))
                                .child(row(
                                    "Sign-ins",
                                    if s.sign_ins.is_empty() {
                                        "none".into()
                                    } else {
                                        s.sign_ins
                                            .iter()
                                            .map(|(p, t)| format!("{p} ({t})"))
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    },
                                ))
                                .child(row(
                                    "Custom providers",
                                    if s.custom_providers.is_empty() {
                                        "—".into()
                                    } else {
                                        s.custom_providers.join(", ")
                                    },
                                )),
                        )
                        .child(
                            section(IconName::ShieldCheck, "Gentle AI")
                                .child(row(
                                    "Permissions (new sessions)",
                                    or_dash(&s.permission_mode, "normal"),
                                ))
                                .child(row("Persona", or_dash(&s.persona, "gentleman")))
                                .child(row(
                                    "Background subagents",
                                    s.background_subagents.describe(),
                                ))
                                .child(
                                    v_flex()
                                        .pt_2()
                                        .pb_1()
                                        .gap_1p5()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(ui::muted())
                                                        .child("Send helpers to the background"),
                                                )
                                                .child(ui::segmented(
                                                    "background-subagents",
                                                    &["Off", "On"],
                                                    Some(usize::from(s.background_subagents.on)),
                                                    cx.listener(|this, index: &usize, _, cx| {
                                                        this.set_background(*index == 1, cx)
                                                    }),
                                                ))
                                                .when(self.background_pending, |d| {
                                                    d.child(Spinner::new().xsmall())
                                                }),
                                        )
                                        .child(
                                            div().text_xs().text_color(ui::faint()).child(
                                                "On: el Gentleman launches helpers in the background and keeps working; each answer arrives when it finishes. Applied by Gentle Shell (/gentle:background-subagents), so the terminal sees the same setting.",
                                            ),
                                        )
                                        // The switch writes the global file; a project file wins over it.
                                        .when(
                                            s.background_subagents.source
                                                == PolicySource::ProjectFile,
                                            |d| {
                                                d.child(
                                                    div().text_xs().text_color(ui::warning()).child(
                                                        format!(
                                                            "{} decides for this project, so the switch (global) won't change it here.",
                                                            s.background_subagents
                                                                .project_file
                                                                .display()
                                                        ),
                                                    ),
                                                )
                                            },
                                        )
                                        .when_some(self.background_note.clone(), |d, (text, failed)| {
                                            d.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(if failed {
                                                        ui::danger()
                                                    } else {
                                                        ui::success()
                                                    })
                                                    .child(text),
                                            )
                                        }),
                                )
                                .child(row(
                                    "Banner",
                                    or_dash(&s.banner, "pink · rose on · text on"),
                                ))
                                .child(row("Theme (terminal)", or_dash(&s.theme, "default")))
                                .child(row(
                                    "MCP servers",
                                    if s.mcp_servers.is_empty() {
                                        "—".into()
                                    } else {
                                        s.mcp_servers.join(", ")
                                    },
                                )),
                        )
                        .child(self.notifications_section(cx))
                        .child(
                            section(IconName::Bot, "Claude bridge")
                                .child(row(
                                    "Package",
                                    if bridge_installed {
                                        "pi-claude-bridge installed".to_string()
                                    } else {
                                        "not installed".to_string()
                                    },
                                ))
                                .child(row("Claude Code", or_dash(&s.claude_code, "checking…")))
                                .child(row(
                                    "Plan",
                                    or_dash(&s.bridge_plan, "pro (default; set max for Opus 1M)"),
                                ))
                                .when(bridge_installed, |d| {
                                    d.child(
                                        h_flex()
                                            .pt_2()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(ui::muted())
                                                    .child("Plan"),
                                            )
                                            .child(ui::segmented(
                                                "bridge-plan",
                                                &["Pro", "Max"],
                                                plan,
                                                cx.listener(|this, index: &usize, _, cx| {
                                                    this.set_plan(
                                                        if *index == 1 { "max" } else { "pro" },
                                                        cx,
                                                    )
                                                }),
                                            )),
                                    )
                                }),
                        )
                        .child(
                            section(IconName::Layers, "Packages")
                                .children(s.packages.iter().enumerate().map(|(i, p)| {
                                    let source = p.source.clone();
                                    h_flex()
                                        .gap_2()
                                        .px_2()
                                        .py_1()
                                        .rounded_lg()
                                        .hover(|d| d.bg(ui::surface_hover()))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_xs()
                                                .font_family(ui::MONO)
                                                .text_color(ui::fg())
                                                .child(p.source.clone()),
                                        )
                                        .when_some(p.version.clone(), |d, v| {
                                            d.child(ui::chip(v, ui::info()).flex_shrink_0())
                                        })
                                        .child(
                                            Button::new(("remove-package", i))
                                                .xsmall()
                                                .ghost()
                                                .icon(Icon::new(IconName::Trash))
                                                .tooltip("gentle-shell remove")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.run_package("remove", source.clone(), cx)
                                                })),
                                        )
                                }))
                                .when(!s.extensions.is_empty(), |d| {
                                    d.child(row("Extension filters", s.extensions.join(", ")))
                                })
                                .when(RECOMMENDED.iter().any(|(src, _)| !installed(src)), |d| {
                                    d.child(
                                        div().pt_3().pb_1().child(ui::section_label("Recommended")),
                                    )
                                })
                                .children(
                                    RECOMMENDED.iter().filter(|(src, _)| !installed(src)).map(
                                        |(src, why)| {
                                            h_flex()
                                                .gap_3()
                                                .px_2()
                                                .py_1p5()
                                                .rounded_lg()
                                                .bg(ui::accent().opacity(0.05))
                                                .border_1()
                                                .border_color(ui::accent().opacity(0.15))
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .child(
                                                            div()
                                                                .truncate()
                                                                .text_xs()
                                                                .font_family(ui::MONO)
                                                                .text_color(ui::accent())
                                                                .child(*src),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(ui::muted())
                                                                .child(*why),
                                                        ),
                                                )
                                                .child(
                                                    Button::new(SharedString::from(format!(
                                                        "install-{src}"
                                                    )))
                                                    .small()
                                                    .ghost()
                                                    .icon(Icon::new(IconName::Plus))
                                                    .label("Install")
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.run_package(
                                                            "install",
                                                            src.to_string(),
                                                            cx,
                                                        )
                                                    })),
                                                )
                                        },
                                    ),
                                )
                                .child(
                                    h_flex()
                                        .pt_3()
                                        .gap_2()
                                        .child(
                                            ui::field()
                                                .flex_1()
                                                .min_w_0()
                                                .child(
                                                    Icon::new(IconName::Plus)
                                                        .small()
                                                        .text_color(ui::faint()),
                                                )
                                                .child(
                                                    Input::new(&self.package_input)
                                                        .appearance(false)
                                                        .small(),
                                                ),
                                        )
                                        .child(
                                            Button::new("install-package")
                                                .primary()
                                                .icon(Icon::new(IconName::ArrowDown))
                                                .label("Install")
                                                .disabled(self.running.is_some())
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    let source = this
                                                        .package_input
                                                        .read(cx)
                                                        .value()
                                                        .trim()
                                                        .to_string();
                                                    if !source.is_empty() {
                                                        this.package_input.update(cx, |s, cx| {
                                                            s.set_value("", window, cx)
                                                        });
                                                        this.run_package("install", source, cx);
                                                    }
                                                })),
                                        ),
                                )
                                .when_some(self.running.clone(), |d, job| {
                                    d.child(
                                        h_flex()
                                            .pt_2()
                                            .gap_2()
                                            .text_xs()
                                            .text_color(ui::muted())
                                            .child(Spinner::new().xsmall().color(ui::accent()))
                                            .child(format!("gentle-shell {job}…")),
                                    )
                                })
                                .when(!self.log.is_empty(), |d| {
                                    d.child(ui::well_box().mt_2().px_3().py_2().gap_0p5().children(
                                        self.log.iter().map(|l| {
                                            div().text_color(ui::muted()).child(l.clone())
                                        }),
                                    ))
                                }),
                        ),
                )
                .into_any_element()
            }
        };
        let (backdrop, panel) = ui::screen(Some(820.));
        backdrop.occlude().child(ui::pop_in(
            "settings-screen",
            panel
                .w_full()
                .max_w(px(820.))
                .child(
                    ui::screen_header()
                        .child(
                            Icon::new(IconName::Settings)
                                .small()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Configuration"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child("Your Gentle Shell setup, as the terminal sees it"),
                        )
                        .child(
                            Button::new("settings-refresh")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::RefreshCw))
                                .tooltip("Reload")
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        )
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("settings-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close))),
                        ),
                )
                .child(
                    div()
                        .id("settings-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(div().px_4().pt_4().child(app_section(cx)))
                        .child(body),
                ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BACKGROUND_SUBAGENTS_FILE, PolicySource, UpgradeOutcome, npm_name, read_snapshot,
        resolve_background_policy, save_bridge_plan, upgrade_outcome,
    };

    #[test]
    fn upgrade_result_follows_what_the_app_runs_afterwards() {
        use std::path::Path;
        let old = Path::new("/fnm/bin/gentle-shell");
        // The app now runs the new version.
        assert_eq!(
            upgrade_outcome(true, "4.7.0", Some("4.7.0"), Some(old)),
            UpgradeOutcome::Installed("4.7.0".into())
        );
        // It installed, but an older launcher still comes first.
        assert_eq!(
            upgrade_outcome(true, "4.7.0", Some("4.6.0"), Some(old)),
            UpgradeOutcome::Stale {
                running: "4.6.0".into(),
                launcher: "/fnm/bin/gentle-shell".into()
            }
        );
        // A failed run, or no launcher answering afterwards, is a failure.
        assert_eq!(
            upgrade_outcome(false, "4.7.0", Some("4.7.0"), Some(old)),
            UpgradeOutcome::Failed
        );
        assert_eq!(
            upgrade_outcome(true, "4.7.0", None, None),
            UpgradeOutcome::Failed
        );
    }

    #[test]
    fn probes_are_reused_until_expiry_or_stamp_change() {
        use super::{Probe, Stamp, reuse_or_run};
        use std::sync::Mutex;
        use std::time::{Duration, Instant, SystemTime};

        let path =
            std::env::temp_dir().join(format!("gentle-settings-probe-{}.json", std::process::id()));
        std::fs::write(&path, "{}").unwrap();
        let set_mtime = |secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        set_mtime(4_000_000);
        let slot: Mutex<Option<Probe<u32>>> = Mutex::new(None);
        let ttl = Duration::from_secs(60);
        let start = Instant::now();
        let mut runs = 0;
        let mut probe = |now: Instant| {
            reuse_or_run(&slot, now, ttl, Stamp::of(&path), || {
                runs += 1;
                runs
            })
        };
        assert_eq!(probe(start), 1);
        assert_eq!(probe(start + Duration::from_secs(30)), 1);
        // Expired.
        assert_eq!(probe(start + Duration::from_secs(61)), 2);
        // The watched file changed.
        set_mtime(4_000_001);
        assert_eq!(probe(start + Duration::from_secs(62)), 3);
        assert_eq!(probe(start + Duration::from_secs(63)), 3);
        // Forgotten.
        *slot.lock().unwrap() = None;
        assert_eq!(probe(start + Duration::from_secs(64)), 4);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn npm_names_drop_versions() {
        assert_eq!(npm_name("npm:pi-claude-bridge"), Some("pi-claude-bridge"));
        assert_eq!(npm_name("npm:pi-web-access@0.47.0"), Some("pi-web-access"));
        assert_eq!(npm_name("npm:@scope/tool@1"), Some("@scope/tool"));
        assert_eq!(npm_name("git:github.com/a/b"), None);
    }

    #[test]
    fn snapshot_reads_existing_configuration() {
        let root = std::env::temp_dir().join(format!("gs-settings-test-{}", std::process::id()));
        let (agent, config) = (root.join("agent"), root.join("gentle-ai"));
        std::fs::create_dir_all(agent.join("npm/node_modules/pi-claude-bridge")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            agent.join("settings.json"),
            r#"{"defaultProvider":"claude-bridge","defaultModel":"claude-opus-5-5","packages":["npm:pi-claude-bridge",{"source":"npm:pi-btw"}]}"#,
        )
        .unwrap();
        std::fs::write(
            agent.join("npm/node_modules/pi-claude-bridge/package.json"),
            r#"{"version":"0.10.0"}"#,
        )
        .unwrap();
        std::fs::write(
            agent.join("auth.json"),
            r#"{"anthropic":{"type":"oauth","access":"secret"}}"#,
        )
        .unwrap();
        std::fs::write(
            config.join("profiles.json"),
            r#"{"active":"claude","profiles":{"claude":{},"current":{}}}"#,
        )
        .unwrap();
        std::fs::write(
            config.join("permission-mode.json"),
            r#"{"schema":"gentle-pi.permission-mode/v1","mode":"yolo"}"#,
        )
        .unwrap();
        save_bridge_plan(&agent, "max").unwrap();

        let s = read_snapshot(&agent, &config, &root);
        assert_eq!(
            s.default_model.as_deref(),
            Some("claude-bridge/claude-opus-5-5")
        );
        assert_eq!(s.packages.len(), 2);
        assert_eq!(s.packages[0].version.as_deref(), Some("0.10.0"));
        assert_eq!(
            s.sign_ins,
            vec![("anthropic".to_string(), "oauth".to_string())],
            "types only, never secrets"
        );
        assert_eq!(s.active_profile.as_deref(), Some("claude"));
        assert_eq!(s.permission_mode.as_deref(), Some("yolo"));
        assert_eq!(s.bridge_plan.as_deref(), Some("max"));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Same order and fail-closed rules as the shell's
    /// `resolveBackgroundSubagentsPolicy`, so the row never disagrees with it.
    #[test]
    fn background_policy_resolves_like_the_shell() {
        let root = std::env::temp_dir().join(format!("gs-bg-test-{}", std::process::id()));
        let (cwd, config) = (root.join("repo"), root.join("gentle-ai"));
        let project = cwd.join(".pi").join("gentle-ai");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        let on = r#"{"schema":"gentle-pi.background-subagents/v1","policy":"on"}"#;
        let off = r#"{"schema":"gentle-pi.background-subagents/v1","policy":"off"}"#;
        let resolve = |env| resolve_background_policy(&cwd, &config, env);

        // Nothing set: off by default; the environment applies next.
        assert_eq!(
            (resolve(None).on, resolve(None).source),
            (false, PolicySource::Default)
        );
        assert_eq!(
            (resolve(Some("on")).on, resolve(Some("on")).source),
            (true, PolicySource::Environment)
        );
        assert_eq!(
            resolve(Some("yes")).source,
            PolicySource::Default,
            "unknown env is inert"
        );

        // The global file beats the environment.
        std::fs::write(config.join(BACKGROUND_SUBAGENTS_FILE), on).unwrap();
        let global = resolve(Some("off"));
        assert_eq!((global.on, global.source), (true, PolicySource::GlobalFile));

        // A project file beats the global one.
        std::fs::write(project.join(BACKGROUND_SUBAGENTS_FILE), off).unwrap();
        let local = resolve(None);
        assert_eq!((local.on, local.source), (false, PolicySource::ProjectFile));

        // Malformed (an extra key) fails closed and keeps the decision there.
        std::fs::write(
            project.join(BACKGROUND_SUBAGENTS_FILE),
            r#"{"schema":"gentle-pi.background-subagents/v1","policy":"on","x":1}"#,
        )
        .unwrap();
        let broken = resolve(Some("on"));
        assert_eq!(
            (broken.on, broken.source, broken.malformed),
            (false, PolicySource::ProjectFile, true)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
