//! Applying a profile: materializing agent routes (frontmatter,
//! `subagents.json`, the installer's managed-assets manifest), Pi's
//! orchestrator settings, and the guarded Make default.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gentle_core::profiles::{
    AgentEntry, AgentSource, ORCHESTRATOR, ProfilesFile, RouteEntry, Routing, evaluate_pin,
    format_selection, is_review_role, model_profile_for, normalized_json, parse_model_ref, pretty,
    serialize_profiles, set_active, update_frontmatter_routing, with_omitted_cleared, without_key,
};
use serde_json::{Map, Value, json};

use super::env::{Env, discover_agents};
use super::files::{
    RoutingRead, global_effective, read_optional, read_saved_routing, unique_token, write_atomic,
};
use super::pins::{read_pin_status, write_pin};

// ---------------------------------------------------------------------------
// Materialization (applyModelConfig) and Pi settings (profiles-orchestrator.ts)
// ---------------------------------------------------------------------------

/// `updateSubagentModelProfileAtPath`: every other key survives; a no-op
/// touches no file.
fn update_subagent_profile(path: &Path, name: &str, entry: &RouteEntry) -> io::Result<bool> {
    let mut config = match fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let profiles = match config.get("model_profiles") {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };
    let profile = model_profile_for(entry);
    if profiles.get(name) == profile.as_ref() {
        return Ok(false);
    }
    let profiles = match profile {
        Some(profile) => {
            let mut profiles = profiles;
            profiles.insert(name.to_string(), profile);
            profiles
        }
        None => without_key(profiles, name),
    };
    if profiles.is_empty() {
        config = without_key(config, "model_profiles");
    } else {
        config.insert("model_profiles".into(), Value::Object(profiles));
    }
    write_atomic(path, &pretty(&Value::Object(config)))?;
    Ok(true)
}

/// SHA-256, hex: the managed-assets manifest hash (`managedAssetHash`).
pub(super) fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The installer's `<agent home>/gentle-ai/managed-assets.lock`.
struct ManagedLock {
    path: PathBuf,
    token: String,
}

impl ManagedLock {
    fn acquire(agent_home: &Path) -> io::Result<Self> {
        let parent = agent_home.join("gentle-ai");
        let path = parent.join("managed-assets.lock");
        fs::create_dir_all(&parent)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let token = unique_token();
            let created = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let owner = json!({ "schemaVersion": 1, "token": token, "pid": std::process::id(), "createdAtMs": created as u64 });
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(owner.to_string().as_bytes())?;
                    return Ok(Self { path, token });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    match fs::symlink_metadata(&path) {
                        Ok(meta) if !meta.is_file() => {
                            return Err(io::Error::other(format!(
                                "Managed-assets lock path is unsafe and must be a regular file: {}",
                                path.display()
                            )));
                        }
                        Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                        _ => {}
                    }
                    if Instant::now() >= deadline {
                        return Err(io::Error::other(format!(
                            "Timed out acquiring managed-assets lock file {}. Verify no installer is active before removing this exact lock file.",
                            path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for ManagedLock {
    fn drop(&mut self) {
        let owner = fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        if owner
            .as_ref()
            .and_then(|o| o.get("token"))
            .and_then(Value::as_str)
            == Some(self.token.as_str())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// `updatePackageManagedSddAgentOwnership`: a package-managed agent file whose
/// manifest hash matches its previous content is rewritten together with its
/// manifest entry under the installer lock, so a later install still owns it.
/// `false` means the caller writes the file itself.
pub(super) fn update_managed_ownership(
    agent_home: &Path,
    path: &Path,
    previous: &str,
    next: &str,
) -> io::Result<bool> {
    let Ok(relative) = path.strip_prefix(agent_home.join("agents")) else {
        return Ok(false);
    };
    if relative.as_os_str().is_empty() {
        return Ok(false);
    }
    let key = format!(
        "agents/{}",
        relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    );
    let _lock = ManagedLock::acquire(agent_home)?;
    let manifest_path = agent_home.join("gentle-ai").join("managed-assets.json");
    let mut assets = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(|v| v.get("schemaVersion").and_then(Value::as_f64) == Some(1.0))
        .and_then(|v| v.get("assets").and_then(Value::as_object).cloned())
        .map(|assets| {
            assets
                .into_iter()
                .filter(|(_, v)| v.is_string())
                .collect::<Map<_, _>>()
        })
        .unwrap_or_default();
    if assets.get(&key).and_then(Value::as_str) != Some(sha256_hex(previous.as_bytes()).as_str()) {
        return Ok(false);
    }
    let installed = fs::read_to_string(path)?;
    if installed != next && installed != previous {
        return Ok(false);
    }
    let result = (|| {
        if installed == previous {
            write_atomic(path, next)?;
        }
        assets.insert(key, json!(sha256_hex(next.as_bytes())));
        let manifest = json!({ "schemaVersion": 1, "assets": Value::Object(assets) });
        write_atomic(
            &manifest_path,
            &serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )
    })();
    if let Err(error) = result {
        write_atomic(path, previous).map_err(|rollback| {
            io::Error::other(format!(
                "Managed routing update failed and could not restore {}: {rollback}",
                path.display()
            ))
        })?;
        return Err(error);
    }
    Ok(true)
}

/// `applyModelConfig`: agent frontmatter and `subagents.json` for every
/// discoverable agent the routing names; clear entries for unknown agents
/// remove their user model profile. Returns (updated, skipped).
fn apply_routing(env: &Env, agents: &[AgentEntry], config: &Routing) -> io::Result<(usize, usize)> {
    let (mut updated, mut skipped) = (0, 0);
    let mut count = |changed: bool| if changed { updated += 1 } else { skipped += 1 };
    let mut seen = HashSet::new();
    for agent in agents {
        if is_review_role(&agent.name) {
            continue;
        }
        seen.insert(agent.name.as_str());
        let Some(entry) = config.get(&agent.name) else {
            count(false);
            continue;
        };
        let subagents = env.subagents_path(agent.source);
        if agent.source == AgentSource::Builtin {
            count(update_subagent_profile(&subagents, &agent.name, entry)?);
            continue;
        }
        if !agent.path.exists() {
            count(false);
        } else {
            let original = fs::read_to_string(&agent.path)?;
            let next = update_frontmatter_routing(&original, entry);
            if next == original {
                count(false);
            } else {
                if !update_managed_ownership(&env.agent_home, &agent.path, &original, &next)? {
                    write_atomic(&agent.path, &next)?;
                }
                count(true);
            }
        }
        count(update_subagent_profile(&subagents, &agent.name, entry)?);
    }
    for (name, entry) in config.iter() {
        if is_review_role(name)
            || name == ORCHESTRATOR
            || seen.contains(name.as_str())
            || !entry.is_clear()
        {
            continue;
        }
        count(update_subagent_profile(
            &env.subagents_path(AgentSource::User),
            name,
            entry,
        )?);
    }
    Ok((updated, skipped))
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettingsWrite {
    Unchanged,
    Written,
}

/// `applyOrchestratorSettings`: set Pi's `defaultProvider`/`defaultModel`/
/// `defaultThinkingLevel`, keep every other key, refuse an unreadable file.
pub fn apply_orchestrator_settings(
    path: &Path,
    entry: &RouteEntry,
) -> Result<SettingsWrite, String> {
    let Some(model) = &entry.model else {
        return Ok(SettingsWrite::Unchanged);
    };
    let (provider, id) = parse_model_ref(model)
        .ok_or_else(|| format!("model id {model:?} is not a provider/model pair"))?;
    let previous = read_optional(path).map_err(|e| e.to_string())?;
    let mut next = match &previous {
        None => Map::new(),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => map,
            Ok(_) => return Err("the file is not a JSON object".into()),
            Err(e) => return Err(e.to_string()),
        },
    };
    next.insert("defaultProvider".into(), json!(provider));
    next.insert("defaultModel".into(), json!(id));
    match &entry.thinking {
        Some(thinking) => {
            next.insert("defaultThinkingLevel".into(), json!(thinking));
        }
        None => next = without_key(next, "defaultThinkingLevel"),
    }
    let text = pretty(&Value::Object(next));
    if previous.as_deref() == Some(text.as_str()) {
        return Ok(SettingsWrite::Unchanged);
    }
    write_atomic(path, &text).map_err(|e| e.to_string())?;
    Ok(SettingsWrite::Written)
}

// ---------------------------------------------------------------------------
// Make default (runProfilesPanelAction "apply-global")
// ---------------------------------------------------------------------------

/// What the confirmation shows before Make default writes anything.
#[derive(Clone, Debug)]
pub struct DefaultPlan {
    pub name: String,
    pub title: String,
    pub lines: Vec<String>,
    /// A winning pin makes the apply repo-scoped: only the clone pin moves.
    pub pinned: bool,
}

pub fn plan_make_default(
    env: &Env,
    file: &ProfilesFile,
    name: &str,
    config: &Routing,
) -> DefaultPlan {
    if let Some((layer, profile, path)) =
        evaluate_pin(read_pin_status(&env.cwd).as_ref(), file).winner
    {
        return DefaultPlan {
            name: name.to_string(),
            title: format!("Use \"{name}\" for this repository?"),
            lines: vec![
                format!("This repository pins \"{profile}\" ({layer} pin at {}).", path.display()),
                "Make default re-pins this clone instead: no global routing, settings.json or active marker is written.".into(),
            ],
            pinned: true,
        };
    }
    let orchestrator = config.orchestrator();
    let has_roles = config.roles().next().is_some();
    let models = env.models_path();
    if !has_roles {
        let (title, line) = match orchestrator {
            Some(_) => (
                "Apply orchestrator-only profile?".to_string(),
                format!(
                    "Profile \"{name}\" has an orchestrator entry but no agent routing entries. Applying it will replace global routing in {} with the orchestrator entry alone, clear every materialized agent route so every agent returns to inherit its default model, set the configured orchestrator entry in settings.json, and switch this session to that orchestrator model.",
                    models.display()
                ),
            ),
            None => (
                "Apply empty profile?".to_string(),
                format!(
                    "Profile \"{name}\" has no routing entries. Applying it will replace global routing in {} with an empty configuration and return every agent to inherit its default model.",
                    models.display()
                ),
            ),
        };
        return DefaultPlan {
            name: name.to_string(),
            title,
            lines: vec![line],
            pinned: false,
        };
    }
    let effects = if orchestrator.is_some() {
        ", set the configured orchestrator entry in settings.json, and switch this session to that orchestrator model"
    } else {
        ""
    };
    let mut lines = Vec::new();
    match read_saved_routing(env) {
        RoutingRead::Valid(saved) => {
            let agents = discover_agents(env);
            let current = global_effective(env, &agents, saved);
            let mut names: Vec<&str> = current.roles().chain(config.roles()).map(|(k, _)| k.as_str()).collect();
            names.sort_unstable();
            names.dedup();
            let (mut replaced, mut cleared, mut added) = (Vec::new(), Vec::new(), Vec::new());
            for key in names {
                match (current.get(key), config.get(key)) {
                    (from, None) => cleared.push(format!("{key}: {} → inherit (cleared)", format_selection(from))),
                    (None, Some(to)) => added.push(format!("{key}: {} (added)", format_selection(Some(to)))),
                    (Some(from), Some(to)) if from != to => {
                        replaced.push(format!("{key}: {} → {}", format_selection(Some(from)), format_selection(Some(to))))
                    }
                    _ => {}
                }
            }
            let changes: Vec<String> = replaced.into_iter().chain(cleared).chain(added).collect();
            lines.push(format!(
                "Applying \"{name}\" replaces global routing in {} with this profile's routes{effects}.",
                models.display()
            ));
            if changes.is_empty() {
                lines.push("Its agent routes already match the current global routing.".into());
            }
            lines.extend(changes);
        }
        _ => lines.push(format!(
            "Profile \"{name}\" has agent routing entries, but the current global routing in {path} could not be read, so existing routes are not listed. Applying it will replace global routing in {path} with this profile's routes, so every existing agent route may be replaced or cleared back to inherit{effects}.",
            path = models.display()
        )),
    }
    DefaultPlan {
        name: name.to_string(),
        title: format!("Apply profile \"{name}\"?"),
        lines,
        pinned: false,
    }
}

#[derive(Debug)]
pub struct DefaultOutcome {
    pub file: ProfilesFile,
    pub message: String,
    /// The orchestrator the live session should switch to (no pin won).
    pub orchestrator: Option<RouteEntry>,
}

/// The guarded global apply: claim the profile in the store, write
/// `models.json`, materialize agent routes, then Pi's settings; anything that
/// fails after the claim restores the previous claim and routing.
pub fn make_default(env: &Env, file: &ProfilesFile, name: &str) -> Result<DefaultOutcome, String> {
    let config = file
        .get(name)
        .cloned()
        .ok_or_else(|| format!("Profile does not exist: {name}."))?;
    let pin = read_pin_status(&env.cwd);
    if let Some((layer, profile, path)) = evaluate_pin(pin.as_ref(), file).winner {
        let local = pin
            .as_ref()
            .map(|p| p.local_path.clone())
            .unwrap_or_default();
        write_pin(&local, name).map_err(|e| {
            format!(
                "Could not pin profile \"{name}\" in {}: {e}",
                local.display()
            )
        })?;
        let mut message = format!(
            "Applied \"{name}\" repo-scoped: this clone now pins it in {}. No global routing or orchestrator was written.",
            local.display()
        );
        if layer == "repo" && path != local {
            message.push_str(&format!(
                " The committed declaration {} still declares \"{profile}\"; the clone pin now takes precedence.",
                path.display()
            ));
        }
        return Ok(DefaultOutcome {
            file: file.clone(),
            message,
            orchestrator: None,
        });
    }
    let profiles_path = env.profiles_path();
    let claimed = set_active(file, name)?;
    write_atomic(&profiles_path, &serialize_profiles(&claimed))
        .map_err(|e| format!("Could not update {}: {e}", profiles_path.display()))?;
    let agents = discover_agents(env);
    let previous_active = file.active.as_deref().and_then(|a| file.get(a)).cloned();
    let models_path = env.models_path();
    let settings_path = env.settings_path();
    // Pi's settings are the last write, so a failure before them never has
    // settings to restore.
    let revert = |routing_written: bool, error: String| -> String {
        let mut restored = Vec::new();
        if routing_written && let Some(previous) = &previous_active {
            let ok = write_atomic(&models_path, &pretty(&normalized_json(previous))).is_ok()
                && apply_routing(env, &agents, &with_omitted_cleared(&agents, previous)).is_ok();
            if ok {
                restored.push("routing");
            }
        }
        if write_atomic(&profiles_path, &serialize_profiles(file)).is_ok() {
            restored.push("active marker");
        }
        let restored = if restored.is_empty() {
            "nothing".to_string()
        } else {
            restored.join(" and ")
        };
        let unresolved = if routing_written && previous_active.is_none() {
            format!(
                " {} still holds this profile's routing because no previously active profile was recorded to restore.",
                models_path.display()
            )
        } else {
            String::new()
        };
        format!("Could not apply profile \"{name}\": {error}. Restored: {restored}.{unresolved}")
    };
    if let Err(e) = write_atomic(&models_path, &pretty(&normalized_json(&config))) {
        return Err(revert(
            false,
            format!("could not write {}: {e}", models_path.display()),
        ));
    }
    let (updated, _) = match apply_routing(env, &agents, &with_omitted_cleared(&agents, &config)) {
        Ok(counts) => counts,
        Err(e) => {
            return Err(revert(
                true,
                format!("could not materialize the routing: {e}"),
            ));
        }
    };
    let orchestrator = config.orchestrator().cloned();
    let mut message = format!(
        "Applied profile \"{name}\" — {updated} agent{} updated. New routing takes effect on the next subagent launch.",
        if updated == 1 { "" } else { "s" }
    );
    if let Some(entry) = &orchestrator {
        match apply_orchestrator_settings(&settings_path, entry) {
            Err(reason) => {
                return Err(revert(
                    true,
                    format!(
                        "could not set the orchestrator: {reason}; {} was left unchanged",
                        settings_path.display()
                    ),
                ));
            }
            Ok(SettingsWrite::Written) => {
                message.push_str(&format!(
                    " Orchestrator set to {} in {}.",
                    format_selection(Some(entry)),
                    settings_path.display()
                ));
            }
            Ok(SettingsWrite::Unchanged) => {}
        }
    }
    let eval = evaluate_pin(read_pin_status(&env.cwd).as_ref(), file);
    if !eval.stale.is_empty() || !eval.invalid.is_empty() {
        message.push_str(" This repository also has a pin layer that does not resolve; the global routing governs its subagents until the pin is fixed.");
    }
    Ok(DefaultOutcome {
        file: claimed,
        message,
        orchestrator,
    })
}
