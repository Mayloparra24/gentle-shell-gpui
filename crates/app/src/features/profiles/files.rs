//! File effects: atomic writes, reading `profiles.json`, the saved and
//! materialized routing, Pi's `settings.json`, following a renamed profile in
//! `profile-fallbacks.json`, and the screen's initial load.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gentle_core::profiles::{
    AgentEntry, Drops, PinStatus, ProfilesFile, RouteEntry, Routing, assignment_names,
    bootstrap_profiles, entry_from_model_profile, is_review_role, is_thinking_level,
    normalize_entry, parse_profiles, pretty, read_frontmatter_routing, serialize_profiles,
    without_key,
};
use serde_json::{Map, Value};

use super::env::{Env, discover_agents};
use super::pins::read_pin_status;

const FALLBACKS_KIND: &str = "gentle-pi.profile_model_fallbacks";

// ---------------------------------------------------------------------------
// Atomic file writes
// ---------------------------------------------------------------------------

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn unique_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{nanos:x}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// `writeJsonFileAtomicallySync`: identical bytes are skipped; otherwise a
/// sibling temp file is renamed over the destination (through a symlink to its
/// target, keeping existing permissions) and removed on every failure path.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|current| current == text) {
        return Ok(());
    }
    let target = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => dunce::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = dir.join(format!(".{file_name}.{}.tmp", unique_token()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = fs::metadata(&target) {
            fs::set_permissions(&temp, meta.permissions())?;
        }
        fs::rename(&temp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub(super) fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// A missing file restores by removal.
pub(super) fn restore_bytes(path: &Path, previous: Option<&str>) -> io::Result<()> {
    match previous {
        Some(text) => write_atomic(path, text),
        None => match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

#[derive(Debug)]
pub enum StoreRead {
    Missing,
    Invalid,
    Valid(ProfilesFile, Drops),
}

pub fn read_store(path: &Path) -> StoreRead {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => StoreRead::Missing,
        Err(_) => StoreRead::Invalid,
        Ok(text) => match parse_profiles(&text) {
            Some((file, drops)) => StoreRead::Valid(file, drops),
            None => StoreRead::Invalid,
        },
    }
}

pub(super) enum RoutingRead {
    Missing,
    Invalid,
    Valid(Routing),
}

/// `readModelConfigFile`: entries normalized, keys taken as written.
fn read_routing_file(path: &Path) -> RoutingRead {
    let text = match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return RoutingRead::Missing,
        Err(_) => return RoutingRead::Invalid,
        Ok(text) => text,
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => RoutingRead::Valid(Routing(
            map.iter()
                .filter_map(|(k, v)| normalize_entry(v).map(|e| (k.clone(), e)))
                .collect(),
        )),
        _ => RoutingRead::Invalid,
    }
}

/// `readSavedModelConfig`: global `models.json`, else the legacy project file
/// (an unreadable legacy file counts as empty).
pub(super) fn read_saved_routing(env: &Env) -> RoutingRead {
    match read_routing_file(&env.models_path()) {
        RoutingRead::Missing => {
            let legacy = env.cwd.join(".pi").join("gentle-ai").join("models.json");
            match read_routing_file(&legacy) {
                RoutingRead::Invalid => RoutingRead::Valid(Routing::default()),
                other => other,
            }
        }
        other => other,
    }
}

fn read_model_profiles(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("model_profiles").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

/// `readMaterializedRoutingEntry`: `subagents.json` first, frontmatter for
/// whatever field it leaves unset.
fn read_materialized(
    env: &Env,
    agent: &AgentEntry,
    cache: &mut HashMap<PathBuf, Map<String, Value>>,
) -> Option<RouteEntry> {
    let path = env.subagents_path(agent.source);
    let profiles = cache
        .entry(path.clone())
        .or_insert_with(|| read_model_profiles(&path));
    let from_profile = profiles.get(&agent.name).and_then(entry_from_model_profile);
    let from_file = fs::read_to_string(&agent.path)
        .ok()
        .and_then(|c| read_frontmatter_routing(&c));
    match (from_profile, from_file) {
        (Some(p), Some(f)) => Some(RouteEntry {
            model: p.model.or(f.model),
            thinking: p.thinking.or(f.thinking),
            tier: None,
        }),
        (p, f) => p.or(f),
    }
}

/// `readGlobalEffectiveModelConfigFrom`: the saved routing, plus the
/// materialized route of every discoverable agent it is silent about.
pub(super) fn global_effective(env: &Env, agents: &[AgentEntry], base: Routing) -> Routing {
    let mut effective = base;
    let mut cache = HashMap::new();
    for agent in agents {
        if is_review_role(&agent.name) || effective.contains(&agent.name) {
            continue;
        }
        if let Some(entry) = read_materialized(env, agent, &mut cache) {
            effective.set(&agent.name, entry);
        }
    }
    effective
}

/// The seed for a missing store (`loadProfilesStore`). No pin can win while
/// the store is missing, so the global effective routing is what's in effect.
pub fn seed_profiles(env: &Env, agents: &[AgentEntry]) -> ProfilesFile {
    let saved = match read_saved_routing(env) {
        RoutingRead::Valid(routing) => routing,
        _ => Routing::default(),
    };
    bootstrap_profiles(&global_effective(env, agents, saved))
}

/// `readOrchestratorSettings`, entry only.
pub(super) fn read_orchestrator_settings(path: &Path) -> Option<RouteEntry> {
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    let provider = value
        .get("defaultProvider")?
        .as_str()
        .filter(|s| !s.is_empty())?;
    let model = value
        .get("defaultModel")?
        .as_str()
        .filter(|s| !s.is_empty())?;
    let thinking = value
        .get("defaultThinkingLevel")
        .and_then(Value::as_str)
        .filter(|t| is_thinking_level(t));
    Some(RouteEntry {
        model: Some(format!("{provider}/{model}")),
        thinking: thinking.map(str::to_string),
        tier: None,
    })
}

/// `followProfileRename` for `profile-fallbacks.json`; a broken file is left alone.
pub(super) fn follow_fallbacks(config_home: &Path, from: &str, to: Option<&str>) {
    let path = config_home.join("profile-fallbacks.json");
    let Some(Value::Object(mut root)) = fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
    else {
        return;
    };
    if root.get("kind").and_then(Value::as_str) != Some(FALLBACKS_KIND)
        || root.get("version").and_then(Value::as_f64) != Some(1.0)
    {
        return;
    }
    let Some(Value::Object(profiles)) = root.remove("profiles") else {
        return;
    };
    let Some(chains) = profiles.get(from).cloned() else {
        return;
    };
    let mut profiles = without_key(profiles, from);
    if let Some(to) = to {
        profiles.insert(to.to_string(), chains);
    }
    root.insert("profiles".into(), Value::Object(profiles));
    let _ = write_atomic(&path, &pretty(&Value::Object(root)));
}

// ---------------------------------------------------------------------------
// Loading the screen
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(super) struct Loaded {
    pub(super) file: ProfilesFile,
    pub(super) names: Vec<String>,
    pub(super) pin: Option<PinStatus>,
    pub(super) notices: Vec<String>,
    pub(super) pi_default: Option<RouteEntry>,
    pub(super) status: Option<String>,
}

pub(super) fn load_screen(env: &Env) -> Result<Loaded, String> {
    let path = env.profiles_path();
    let agents = discover_agents(env);
    let mut notices = Vec::new();
    let mut status = None;
    let file = match read_store(&path) {
        StoreRead::Invalid => {
            return Err(format!(
                "Cannot open agent profiles because {} is invalid JSON or not a profiles file. Fix or remove the file, then open Profiles again.",
                path.display()
            ));
        }
        StoreRead::Valid(file, drops) => {
            let mut parts = Vec::new();
            if !drops.profiles.is_empty() {
                parts.push(format!("profiles: {}", drops.profiles.join(", ")));
            }
            if !drops.agents.is_empty() {
                let list: Vec<String> = drops
                    .agents
                    .iter()
                    .map(|(p, a)| format!("{p}/{a}"))
                    .collect();
                parts.push(format!("routing entries: {}", list.join(", ")));
            }
            if let Some(active) = &drops.active {
                parts.push(format!("active marker: {active}"));
            }
            if !parts.is_empty() {
                notices.push(format!(
                    "Dropped invalid entries while loading {} — {}.",
                    path.display(),
                    parts.join("; ")
                ));
            }
            file
        }
        StoreRead::Missing => {
            let file = seed_profiles(env, &agents);
            write_atomic(&path, &serialize_profiles(&file))
                .map_err(|e| format!("Could not create {}: {e}", path.display()))?;
            status = Some(format!(
                "Seeded the \"current\" profile in {} from the routing currently in effect.",
                path.display()
            ));
            file
        }
    };
    let pin = read_pin_status(&env.cwd);
    let pi_default = read_orchestrator_settings(&env.settings_path());
    Ok(Loaded {
        file,
        names: assignment_names(&agents),
        pin,
        notices,
        status,
        pi_default,
    })
}
