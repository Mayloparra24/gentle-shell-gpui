//! The Claude bridge: Claude Code's sign-in (probed, briefly reused) and the
//! bridge's plan in `claude-bridge.json`.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::file_cache::Stamp;
use serde_json::Value;

use super::layout::{or_dash, row, section};
use super::snapshot::{Snapshot, read_json};
use super::{SettingsEvent, SettingsView};
use crate::shared::ui;

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
pub(super) fn claude_auth_status() -> Option<String> {
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

impl SettingsView {
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

    /// The bridge's package, Claude Code's sign-in and the Pro/Max plan.
    pub(super) fn bridge_section(&self, s: &Snapshot, cx: &mut Context<Self>) -> Div {
        let bridge_installed = s.has_package("npm:pi-claude-bridge");
        let plan = match s.bridge_plan.as_deref() {
            Some("max") => Some(1),
            Some("pro") | None => Some(0),
            Some(_) => None,
        };
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
                        .child(div().text_xs().text_color(ui::muted()).child("Plan"))
                        .child(ui::segmented(
                            "bridge-plan",
                            &["Pro", "Max"],
                            plan,
                            cx.listener(|this, index: &usize, _, cx| {
                                this.set_plan(if *index == 1 { "max" } else { "pro" }, cx)
                            }),
                        )),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Probe, Stamp, reuse_or_run};

    #[test]
    fn probes_are_reused_until_expiry_or_stamp_change() {
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
}
