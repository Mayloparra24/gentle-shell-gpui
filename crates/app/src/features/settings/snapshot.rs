//! Reading the user's Gentle Shell configuration: its files, from the same
//! homes the launcher uses, plus the launcher's own read-only probes.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::background::{BackgroundPolicy, resolve_background_policy};

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
    pub update: Option<crate::features::updates::UpdateOffer>,
}

impl Snapshot {
    /// `source` is among the packages, whatever version either names.
    pub(super) fn has_package(&self, source: &str) -> bool {
        self.packages
            .iter()
            .any(|p| npm_name(&p.source) == npm_name(source))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    pub source: String,
    pub version: Option<String>,
}

pub(super) fn read_json(path: &Path) -> Option<Value> {
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
pub(super) fn probe(spec: &pi_rpc::LaunchSpec, snapshot: &mut Snapshot) {
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
    snapshot.claude_code = super::claude::claude_auth_status();
}

#[cfg(test)]
mod tests {
    use super::super::claude::save_bridge_plan;
    use super::{npm_name, read_snapshot};

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
}
