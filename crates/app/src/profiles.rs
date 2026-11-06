//! Profiles: the native `/gentle:profiles` models screen. Profiles are named
//! routing snapshots in `<config home>/profiles.json`; the table edits one
//! profile's orchestrator and agent routes, and the footer decides where it
//! applies. Semantics follow gentle-pi `lib/agent-profiles.ts`,
//! `lib/model-routing-authority.ts`, `lib/agent-profile-pin.ts`,
//! `lib/profiles-orchestrator.ts` and the `RoutingTablePanel` handler in
//! `extensions/gentle-ai.ts`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Map, Value, json};

use crate::ui;

// ---------------------------------------------------------------------------
// Routing model (lib/model-routing-authority.ts)
// ---------------------------------------------------------------------------

pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const PROFILES_KIND: &str = "gentle-pi.agent_model_profiles";
const PIN_KIND: &str = "gentle-pi.agent_model_profile_pin";
const FALLBACKS_KIND: &str = "gentle-pi.profile_model_fallbacks";
/// Reserved routing key: the orchestrator lives in Pi's `settings.json`, never
/// in `subagents.json` or an agent file.
pub const ORCHESTRATOR: &str = "orchestrator";
/// Routed by key only; they inherit Pi's persisted default (agents-config.ts).
const PROVIDER_REVIEW_ROLES: [&str; 2] = ["review-refuter", "review-validator"];
/// `CORE_MODEL_AGENT_NAMES`: listed first, the rest alphabetically.
const CORE_AGENTS: [&str; 3] = ["jd-judge-a", "jd-judge-b", "jd-fix-agent"];
/// lib/openai-fast-tier.ts `FAST_MODELS`.
const FAST_MODELS: [&str; 8] = [
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-6-astra",
    "gpt-6-sol",
    "gpt-6-luna",
    "gpt-6.1-sol",
    "gpt-6.1-terra",
];

/// One model from RPC `get_available_models`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub reasoning: bool,
}

impl ModelChoice {
    fn route_id(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }

    /// `supportsOpenAIFast`, minus the `api` check the catalog entry here lacks.
    fn supports_fast(&self) -> bool {
        FAST_MODELS.contains(&self.id.as_str())
            && matches!(self.provider.as_str(), "openai" | "openai-codex")
    }
}

fn is_thinking_level(value: &str) -> bool {
    THINKING_LEVELS.contains(&value)
}

fn is_model_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "._~:@/+%-".contains(c)
}

/// `normalizeModelId`: trimmed, non-empty, `SAFE_MODEL_ID_PATTERN`.
pub fn normalize_model_id(value: &str) -> Option<String> {
    let model = value.trim();
    (!model.is_empty() && model.chars().all(is_model_char)).then(|| model.to_string())
}

/// `isSafeAgentName`.
fn is_safe_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:@/+%-".contains(c))
}

fn is_review_role(name: &str) -> bool {
    PROVIDER_REVIEW_ROLES.contains(&name)
}

/// One routing entry: `{ model?, thinking?, tier? }`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteEntry {
    pub model: Option<String>,
    pub thinking: Option<String>,
    /// `standard` | `fast`.
    pub tier: Option<String>,
}

impl RouteEntry {
    fn model(model: &str) -> Self {
        Self {
            model: Some(model.to_string()),
            ..Self::default()
        }
    }

    /// `isClearRoutingEntry`: no model and no thinking (tier does not count).
    fn is_clear(&self) -> bool {
        self.model.is_none() && self.thinking.is_none()
    }

    fn is_empty(&self) -> bool {
        self.is_clear() && self.tier.is_none()
    }

    fn to_json(&self) -> Value {
        let mut map = Map::new();
        if let Some(model) = &self.model {
            map.insert("model".into(), json!(model));
        }
        if let Some(thinking) = &self.thinking {
            map.insert("thinking".into(), json!(thinking));
        }
        if let Some(tier) = &self.tier {
            map.insert("tier".into(), json!(tier));
        }
        Value::Object(map)
    }
}

/// `normalizeRoutingEntry`: a legacy string is a model; an object keeps its
/// valid fields (`effort` aliases `thinking`); an empty object is an explicit
/// inherit; anything else is dropped.
pub fn normalize_entry(value: &Value) -> Option<RouteEntry> {
    match value {
        Value::String(model) => normalize_model_id(model).map(|m| RouteEntry::model(&m)),
        Value::Object(map) => {
            let text = |key: &str| map.get(key).and_then(Value::as_str);
            let model = text("model").and_then(normalize_model_id);
            let thinking = text("thinking")
                .filter(|t| is_thinking_level(t))
                .or_else(|| text("effort").filter(|t| is_thinking_level(t)))
                .map(str::to_string);
            let tier = text("tier")
                .filter(|t| matches!(*t, "standard" | "fast"))
                .map(str::to_string);
            if model.is_none() && thinking.is_none() && tier.is_none() {
                return map.is_empty().then(RouteEntry::default);
            }
            Some(RouteEntry {
                model,
                thinking,
                tier,
            })
        }
        _ => None,
    }
}

/// An ordered routing map (`AgentModelConfig`), key order as written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Routing(Vec<(String, RouteEntry)>);

impl Routing {
    pub fn get(&self, key: &str) -> Option<&RouteEntry> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, e)| e)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn set(&mut self, key: &str, entry: RouteEntry) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, slot)) => *slot = entry,
            None => self.0.push((key.to_string(), entry)),
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    pub fn iter(&self) -> impl Iterator<Item = &(String, RouteEntry)> {
        self.0.iter()
    }

    /// `profileRoleEntries`: everything but the orchestrator.
    pub fn roles(&self) -> impl Iterator<Item = &(String, RouteEntry)> {
        self.0.iter().filter(|(k, _)| k != ORCHESTRATOR)
    }

    /// `readProfileOrchestrator`: the entry when it says anything.
    pub fn orchestrator(&self) -> Option<&RouteEntry> {
        self.get(ORCHESTRATOR).filter(|e| !e.is_empty())
    }

    fn to_json(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(k, e)| (k.clone(), e.to_json()))
                .collect(),
        )
    }
}

/// `normalizeModelConfig`: unsafe keys and invalid entries are dropped.
fn normalize_routing(value: &Value) -> Option<Routing> {
    let map = value.as_object()?;
    Some(Routing(
        map.iter()
            .filter(|(name, _)| is_safe_agent_name(name))
            .filter_map(|(name, v)| normalize_entry(v).map(|e| (name.clone(), e)))
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Profiles store (lib/agent-profiles.ts)
// ---------------------------------------------------------------------------

/// `isValidProfileName`: 1-64 of `[A-Za-z0-9._-]`, starting alphanumeric.
pub fn is_valid_profile_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !matches!(name, "constructor" | "prototype")
}

fn require_valid_name(name: &str) -> Result<(), String> {
    if is_valid_profile_name(name) {
        Ok(())
    } else {
        Err(format!(
            "Invalid profile name: {name:?}. Use 1-64 characters (letters, numbers, '.', '_', '-') starting with a letter or number."
        ))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfilesFile {
    pub active: Option<String>,
    pub profiles: Vec<(String, Routing)>,
    /// Top-level keys this version does not know, written back unchanged.
    extra: Map<String, Value>,
}

impl ProfilesFile {
    pub fn get(&self, name: &str) -> Option<&Routing> {
        self.profiles
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| r)
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.profiles.iter().map(|(n, _)| n.as_str())
    }
}

/// What normalization dropped while reading, so the screen can say so.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drops {
    pub profiles: Vec<String>,
    pub agents: Vec<(String, String)>,
    pub active: Option<String>,
}

#[derive(Debug)]
pub enum StoreRead {
    Missing,
    Invalid,
    Valid(ProfilesFile, Drops),
}

/// `normalizeProfilesFile`, keeping unknown top-level keys aside.
pub fn parse_profiles(text: &str) -> Option<(ProfilesFile, Drops)> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.get("kind").and_then(Value::as_str) != Some(PROFILES_KIND)
        || object.get("version").and_then(Value::as_f64) != Some(1.0)
    {
        return None;
    }
    let raw = object.get("profiles")?.as_object()?;
    let mut drops = Drops::default();
    let mut profiles = Vec::new();
    for (name, raw_config) in raw {
        let config = is_valid_profile_name(name)
            .then(|| normalize_routing(raw_config))
            .flatten();
        let Some(config) = config else {
            drops.profiles.push(name.clone());
            continue;
        };
        if let Some(keys) = raw_config.as_object() {
            for agent in keys.keys().filter(|k| !config.contains(k)) {
                drops.agents.push((name.clone(), agent.clone()));
            }
        }
        profiles.push((name.clone(), config));
    }
    let active = match object.get("active") {
        None => None,
        Some(Value::String(name))
            if is_valid_profile_name(name) && profiles.iter().any(|(n, _)| n == name) =>
        {
            Some(name.clone())
        }
        Some(other) => {
            drops.active = Some(
                other
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| other.to_string()),
            );
            None
        }
    };
    let extra = object
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "kind" | "version" | "profiles" | "active"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Some((
        ProfilesFile {
            active,
            profiles,
            extra,
        },
        drops,
    ))
}

/// `serializeProfilesFile` (kind, version, profiles, active), then any
/// unknown top-level keys the file carried.
pub fn serialize_profiles(file: &ProfilesFile) -> String {
    let mut map = Map::new();
    map.insert("kind".into(), json!(PROFILES_KIND));
    map.insert("version".into(), json!(1));
    map.insert(
        "profiles".into(),
        Value::Object(
            file.profiles
                .iter()
                .map(|(n, r)| (n.clone(), r.to_json()))
                .collect(),
        ),
    );
    if let Some(active) = &file.active {
        map.insert("active".into(), json!(active));
    }
    for (k, v) in &file.extra {
        map.insert(k.clone(), v.clone());
    }
    pretty(&Value::Object(map))
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    text
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

pub fn create_profile(
    file: &ProfilesFile,
    name: &str,
    config: Routing,
) -> Result<ProfilesFile, String> {
    require_valid_name(name)?;
    if file.has(name) {
        return Err(format!("Profile already exists: {name}."));
    }
    let mut next = file.clone();
    next.profiles.push((name.to_string(), config));
    Ok(next)
}

pub fn update_profile(
    file: &ProfilesFile,
    name: &str,
    config: Routing,
) -> Result<ProfilesFile, String> {
    require_valid_name(name)?;
    let mut next = file.clone();
    let slot = next
        .profiles
        .iter_mut()
        .find(|(n, _)| n == name)
        .ok_or_else(|| format!("Profile does not exist: {name}."))?;
    slot.1 = config;
    Ok(next)
}

pub fn duplicate_profile(
    file: &ProfilesFile,
    from: &str,
    to: &str,
) -> Result<ProfilesFile, String> {
    require_valid_name(from)?;
    let config = file
        .get(from)
        .cloned()
        .ok_or_else(|| format!("Profile does not exist: {from}."))?;
    create_profile(file, to, config)
}

pub fn rename_profile(file: &ProfilesFile, from: &str, to: &str) -> Result<ProfilesFile, String> {
    require_valid_name(from)?;
    if !file.has(from) {
        return Err(format!("Profile does not exist: {from}."));
    }
    require_valid_name(to)?;
    if file.has(to) {
        return Err(format!("Profile already exists: {to}."));
    }
    let mut next = file.clone();
    for (name, _) in &mut next.profiles {
        if name == from {
            *name = to.to_string();
        }
    }
    if next.active.as_deref() == Some(from) {
        next.active = Some(to.to_string());
    }
    Ok(next)
}

pub fn delete_profile(file: &ProfilesFile, name: &str) -> Result<ProfilesFile, String> {
    require_valid_name(name)?;
    if !file.has(name) {
        return Err(format!("Profile does not exist: {name}."));
    }
    if file.active.as_deref() == Some(name) {
        return Err(format!(
            "Profile {name} is active. Apply another profile before deleting it."
        ));
    }
    let mut next = file.clone();
    next.profiles.retain(|(n, _)| n != name);
    Ok(next)
}

pub fn set_active(file: &ProfilesFile, name: &str) -> Result<ProfilesFile, String> {
    require_valid_name(name)?;
    if !file.has(name) {
        return Err(format!("Profile does not exist: {name}."));
    }
    let mut next = file.clone();
    next.active = Some(name.to_string());
    Ok(next)
}

/// `bootstrapProfilesFile`: a `current` profile from the routing in effect,
/// marked active only when it routes anything.
pub fn bootstrap_profiles(current: &Routing) -> ProfilesFile {
    let config = normalize_routing(&current.to_json()).unwrap_or_default();
    let active = (!config.0.is_empty()).then(|| "current".to_string());
    ProfilesFile {
        active,
        profiles: vec![("current".into(), config)],
        extra: Map::new(),
    }
}

/// `formatOrchestratorSelection`.
fn format_selection(entry: Option<&RouteEntry>) -> String {
    let Some(entry) = entry else {
        return "inherit".into();
    };
    match &entry.model {
        None => match &entry.thinking {
            None => "inherit".into(),
            Some(t) => format!("inherit · {t}"),
        },
        Some(model) => {
            let mut parts = vec![model.clone()];
            parts.extend(entry.thinking.clone());
            match entry.tier.as_deref() {
                Some("fast") => parts.push("Fast".into()),
                Some("standard") => parts.push("Standard".into()),
                _ => {}
            }
            parts.join(" · ")
        }
    }
}

// ---------------------------------------------------------------------------
// Atomic file writes
// ---------------------------------------------------------------------------

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_token() -> String {
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
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path)?,
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

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// A missing file restores by removal.
fn restore_bytes(path: &Path, previous: Option<&str>) -> io::Result<()> {
    match previous {
        Some(text) => write_atomic(path, text),
        None => match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// Rebuild an object without `key`, keeping the order of the rest.
fn without_key(map: Map<String, Value>, key: &str) -> Map<String, Value> {
    map.into_iter().filter(|(k, _)| k != key).collect()
}

// ---------------------------------------------------------------------------
// Environment: homes, package root, agent discovery
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentSource {
    Builtin,
    User,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEntry {
    pub name: String,
    pub source: AgentSource,
    pub path: PathBuf,
}

/// Every location the screen reads or writes, resolved once off the UI thread.
#[derive(Clone, Debug)]
pub struct Env {
    pub cwd: PathBuf,
    pub agent_home: PathBuf,
    pub config_home: PathBuf,
    pub home: PathBuf,
    /// The installed `gentle-pi` package (for built-in agent directories).
    pub package_root: Option<PathBuf>,
}

impl Env {
    fn resolve(cwd: PathBuf) -> Self {
        let homes = pi_rpc::resolve_homes();
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let package_root = pi_rpc::resolve_backend(&cwd)
            .ok()
            .and_then(|spec| gentle_pi_root(&spec.program));
        Self {
            cwd,
            agent_home: homes.agent,
            config_home: homes.config,
            home,
            package_root,
        }
    }

    fn profiles_path(&self) -> PathBuf {
        self.config_home.join("profiles.json")
    }

    fn models_path(&self) -> PathBuf {
        self.config_home.join("models.json")
    }

    fn settings_path(&self) -> PathBuf {
        self.agent_home.join("settings.json")
    }

    fn subagents_path(&self, source: AgentSource) -> PathBuf {
        match source {
            AgentSource::Project => self.cwd.join(".pi").join("subagents.json"),
            _ => self.agent_home.join("subagents.json"),
        }
    }
}

/// `<gentle-pi>` from the launcher: `<gentle-pi>/bin/gentle-shell.mjs`, or the
/// Windows shim beside `node_modules/gentle-pi`.
pub(crate) fn gentle_pi_root(program: &Path) -> Option<PathBuf> {
    let launcher = fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    let mut roots = Vec::new();
    if let Some(root) = launcher.parent().and_then(Path::parent) {
        roots.push(root.to_path_buf());
    }
    if let Some(dir) = program.parent() {
        roots.push(dir.join("node_modules").join("gentle-pi"));
        roots.push(
            dir.join("..")
                .join("lib")
                .join("node_modules")
                .join("gentle-pi"),
        );
    }
    roots
        .into_iter()
        .find(|root| root.join("extensions").join("gentle-ai.ts").is_file())
}

/// `^key:\s*["']?([^"'\n]+)["']?\s*$` (multiline, first match), trimmed.
fn frontmatter_value(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in content.split('\n') {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(['"', '\'']).unwrap_or(rest);
        let end = rest.find(['"', '\'']).unwrap_or(rest.len());
        let (value, tail) = rest.split_at(end);
        let tail = tail.strip_prefix(['"', '\'']).unwrap_or(tail);
        if value.is_empty() || !tail.trim().is_empty() {
            continue;
        }
        let value = value.trim();
        return (!value.is_empty()).then(|| value.to_string());
    }
    None
}

/// `parseAgentName`: `name:`, prefixed by `package:` when present.
pub fn parse_agent_name(content: &str) -> Option<String> {
    let name = frontmatter_value(content, "name")?;
    Some(match frontmatter_value(content, "package") {
        Some(package) => format!("{package}.{name}"),
        None => name,
    })
}

/// `listAgentFilesRecursive`: `.md` but not `.chain.md`, skipping `skills/`.
fn list_agent_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            if name != "skills" {
                list_agent_files(&entry.path(), out);
            }
        } else if kind.is_file() && name.ends_with(".md") && !name.ends_with(".chain.md") {
            out.push(entry.path());
        }
    }
}

fn agents_in(dir: &Path, source: AgentSource) -> Vec<AgentEntry> {
    let mut files = Vec::new();
    list_agent_files(dir, &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let name = parse_agent_name(&fs::read_to_string(&path).ok()?)?;
            Some(AgentEntry { name, source, path })
        })
        .collect()
}

/// `discoverableNonBuiltinAgentRoots`, deduplicated by canonical path with
/// later-root precedence (and the package-managed root's lexical path kept).
fn non_builtin_roots(env: &Env) -> Vec<(PathBuf, AgentSource)> {
    let roots = [
        (env.agent_home.join("agents"), AgentSource::User, true),
        (env.agent_home.join("subagents"), AgentSource::User, false),
        (env.home.join(".agents"), AgentSource::User, false),
        (env.cwd.join(".agents"), AgentSource::Project, false),
        (
            env.cwd.join(".pi").join("agents"),
            AgentSource::Project,
            false,
        ),
        (
            env.cwd.join(".pi").join("subagents"),
            AgentSource::Project,
            false,
        ),
    ];
    let mut unique: Vec<(PathBuf, (PathBuf, AgentSource, bool))> = Vec::new();
    for (dir, source, managed) in roots {
        let canonical = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if let Some(index) = unique.iter().position(|(c, _)| *c == canonical) {
            let (_, existing) = unique.remove(index);
            let managed_dir = if existing.2 {
                Some(existing.0)
            } else if managed {
                Some(dir.clone())
            } else {
                None
            };
            let is_managed = managed_dir.is_some();
            unique.push((canonical, (managed_dir.unwrap_or(dir), source, is_managed)));
        } else {
            unique.push((canonical, (dir, source, managed)));
        }
    }
    unique
        .into_iter()
        .map(|(_, (dir, source, _))| (dir, source))
        .collect()
}

fn builtin_dirs(env: &Env) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for package in ["pi-subagents-j0k3r", "pi-subagents"] {
        if let Some(root) = &env.package_root {
            dirs.push(root.join("..").join(package).join("agents"));
        }
        dirs.push(
            env.cwd
                .join(".pi")
                .join("npm")
                .join("node_modules")
                .join(package)
                .join("agents"),
        );
        dirs.push(
            env.home
                .join(".local")
                .join("lib")
                .join("node_modules")
                .join(package)
                .join("agents"),
        );
    }
    dirs
}

/// `listDiscoverableAgents`: built-ins, then user and project roots; a later
/// definition of the same name wins. Core agents first, the rest sorted.
pub fn discover_agents(env: &Env) -> Vec<AgentEntry> {
    let mut all = Vec::new();
    for dir in builtin_dirs(env) {
        all.extend(agents_in(&dir, AgentSource::Builtin));
    }
    for (dir, source) in non_builtin_roots(env) {
        all.extend(agents_in(&dir, source));
    }
    let mut by_name: Vec<AgentEntry> = Vec::new();
    for agent in all {
        match by_name.iter_mut().find(|a| a.name == agent.name) {
            Some(slot) => *slot = agent,
            None => by_name.push(agent),
        }
    }
    let mut ordered: Vec<AgentEntry> = CORE_AGENTS
        .iter()
        .filter_map(|core| by_name.iter().find(|a| a.name == *core).cloned())
        .collect();
    let mut rest: Vec<AgentEntry> = by_name
        .into_iter()
        .filter(|a| !CORE_AGENTS.contains(&a.name.as_str()))
        .collect();
    rest.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    ordered.extend(rest);
    ordered
}

/// `modelAssignmentNames`: provider review roles, then discovered agents.
pub fn assignment_names(agents: &[AgentEntry]) -> Vec<String> {
    let mut names: Vec<String> = PROVIDER_REVIEW_ROLES
        .iter()
        .map(|s| s.to_string())
        .collect();
    for agent in agents {
        if !names.contains(&agent.name) {
            names.push(agent.name.clone());
        }
    }
    names
}

// ---------------------------------------------------------------------------
// Saved and materialized routing (gentle-ai.ts readModelConfig & friends)
// ---------------------------------------------------------------------------

enum RoutingRead {
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
fn read_saved_routing(env: &Env) -> RoutingRead {
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

/// `readFrontmatterRouting`.
fn read_frontmatter_routing(content: &str) -> Option<RouteEntry> {
    let body = content.strip_prefix("---\n")?;
    let end = body.find("\n---")?;
    let mut raw = Map::new();
    for line in body[..end].split('\n') {
        if let Some(v) = line.strip_prefix("model:") {
            raw.insert("model".into(), json!(v.trim()));
        } else if let Some(v) = line.strip_prefix("thinking:") {
            raw.insert("thinking".into(), json!(v.trim()));
        }
    }
    if raw.is_empty() {
        return None;
    }
    normalize_entry(&Value::Object(raw)).filter(|e| !e.is_clear())
}

/// `routingEntryFromModelProfile`: a `subagents.json` `{ model, effort }`.
fn entry_from_model_profile(value: &Value) -> Option<RouteEntry> {
    let map = value.as_object()?;
    let mut raw = Map::new();
    if let Some(model) = map.get("model") {
        raw.insert("model".into(), model.clone());
    }
    if let Some(effort) = map.get("effort") {
        raw.insert("thinking".into(), effort.clone());
    }
    normalize_entry(&Value::Object(raw)).filter(|e| !e.is_clear())
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
fn global_effective(env: &Env, agents: &[AgentEntry], base: Routing) -> Routing {
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

// ---------------------------------------------------------------------------
// Repository pins (lib/agent-profile-pin.ts)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinRead {
    Missing,
    Invalid,
    Valid(String),
}

#[derive(Clone, Debug)]
pub struct PinStatus {
    pub local_path: PathBuf,
    pub repo_path: PathBuf,
    pub local: PinRead,
    pub repo: PinRead,
}

#[derive(Clone, Debug, Default)]
pub struct PinEval {
    /// (layer, profile, path)
    pub winner: Option<(&'static str, String, PathBuf)>,
    pub invalid: Vec<(&'static str, PathBuf)>,
    pub stale: Vec<(&'static str, String, PathBuf)>,
}

fn parse_pin(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("kind").and_then(Value::as_str) != Some(PIN_KIND)
        || value.get("version").and_then(Value::as_f64) != Some(1.0)
    {
        return None;
    }
    let profile = value.get("profile")?.as_str()?;
    is_valid_profile_name(profile).then(|| profile.to_string())
}

fn read_pin(path: &Path) -> PinRead {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => PinRead::Missing,
        Err(_) => PinRead::Invalid,
        Ok(text) => parse_pin(&text).map_or(PinRead::Invalid, PinRead::Valid),
    }
}

fn serialize_pin(profile: &str) -> String {
    pretty(&json!({ "kind": PIN_KIND, "version": 1, "profile": profile }))
}

fn write_pin(path: &Path, profile: &str) -> Result<(), String> {
    if !is_valid_profile_name(profile) {
        return Err(format!("Invalid profile name: {profile:?}."));
    }
    write_atomic(path, &serialize_pin(profile)).map_err(|e| e.to_string())
}

fn clear_pin(path: &Path) -> io::Result<()> {
    restore_bytes(path, None)
}

/// `resolveSessionWorktreeWithGit`: worktree root and Git common directory.
fn worktree_identity(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    let git = |arg: &str| -> Option<PathBuf> {
        let output = Command::new("git")
            .args(["--no-optional-locks", "-C"])
            .arg(cwd)
            .args(["rev-parse", "--path-format=absolute", arg])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        fs::canonicalize(text.trim_end_matches(['\n', '\r'])).ok()
    };
    Some((git("--show-toplevel")?, git("--git-common-dir")?))
}

pub fn read_pin_status(cwd: &Path) -> Option<PinStatus> {
    let (root, common) = worktree_identity(cwd)?;
    Some(pin_status_at(root, &common))
}

fn pin_status_at(root: PathBuf, common: &Path) -> PinStatus {
    let local_path = common.join("gentle-ai").join("profile-pin.json");
    let repo_path = root.join(".pi").join("gentle-ai").join("profile.json");
    PinStatus {
        local: read_pin(&local_path),
        repo: read_pin(&repo_path),
        local_path,
        repo_path,
    }
}

/// `evaluateProfilePin`: local, then repo; the first legal name the store
/// defines wins; everything else is reported.
pub fn evaluate_pin(status: Option<&PinStatus>, file: &ProfilesFile) -> PinEval {
    let mut eval = PinEval::default();
    let Some(status) = status else { return eval };
    for (layer, path, read) in [
        ("local", &status.local_path, &status.local),
        ("repo", &status.repo_path, &status.repo),
    ] {
        match read {
            PinRead::Missing => {}
            PinRead::Invalid => eval.invalid.push((layer, path.clone())),
            PinRead::Valid(profile) => {
                if !file.has(profile) {
                    eval.stale.push((layer, profile.clone(), path.clone()));
                } else if eval.winner.is_none() {
                    eval.winner = Some((layer, profile.clone(), path.clone()));
                }
            }
        }
    }
    eval
}

/// Rename follows the clone pin; a committed declaration is only reported.
fn follow_renamed_pin(cwd: &Path, from: &str, to: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let Some(status) = read_pin_status(cwd) else {
        return (Vec::new(), Vec::new());
    };
    let mut followed = Vec::new();
    let mut declared = Vec::new();
    if status.local == PinRead::Valid(from.to_string()) && write_pin(&status.local_path, to).is_ok()
    {
        followed.push(status.local_path.clone());
    }
    if status.repo == PinRead::Valid(from.to_string()) {
        declared.push(status.repo_path.clone());
    }
    (followed, declared)
}

/// `followProfileRename` for `profile-fallbacks.json`; a broken file is left alone.
fn follow_fallbacks(config_home: &Path, from: &str, to: Option<&str>) {
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
// Materialization (applyModelConfig) and Pi settings (profiles-orchestrator.ts)
// ---------------------------------------------------------------------------

/// `updateFrontmatterRouting`: drop `model:`/`thinking:` lines and insert the
/// entry's after `description:` (else after the first line).
pub fn update_frontmatter_routing(content: &str, entry: &RouteEntry) -> String {
    let Some(rest) = content.strip_prefix("---\n") else {
        return content.to_string();
    };
    let Some(end) = rest.find("\n---") else {
        return content.to_string();
    };
    let (frontmatter, body) = rest.split_at(end);
    let mut lines: Vec<String> = frontmatter
        .split('\n')
        .filter(|l| !l.starts_with("model:") && !l.starts_with("thinking:"))
        .map(str::to_string)
        .collect();
    let mut insert = Vec::new();
    if let Some(model) = &entry.model {
        insert.push(format!("model: {model}"));
    }
    if let Some(thinking) = &entry.thinking {
        insert.push(format!("thinking: {thinking}"));
    }
    if !insert.is_empty() {
        let at = lines
            .iter()
            .position(|l| l.starts_with("description:"))
            .map(|i| i + 1)
            .unwrap_or_else(|| lines.len().min(1));
        lines.splice(at..at, insert);
    }
    format!("---\n{}{body}", lines.join("\n"))
}

/// `modelProfileForRoutingEntry`: `{ model, effort }`, nothing for a clear entry.
fn model_profile_for(entry: &RouteEntry) -> Option<Value> {
    if entry.is_clear() {
        return None;
    }
    let mut map = Map::new();
    if let Some(model) = &entry.model {
        map.insert("model".into(), json!(model));
    }
    if let Some(thinking) = &entry.thinking {
        map.insert("effort".into(), json!(thinking));
    }
    Some(Value::Object(map))
}

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
fn sha256_hex(data: &[u8]) -> String {
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
fn update_managed_ownership(
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

/// `withOmittedAgentsClearedAsync`: a profile is a complete snapshot.
fn with_omitted_cleared(agents: &[AgentEntry], config: &Routing) -> Routing {
    let mut completed = config.clone();
    for agent in agents {
        if !is_review_role(&agent.name) && !completed.contains(&agent.name) {
            completed.set(&agent.name, RouteEntry::default());
        }
    }
    completed
}

/// `parseOrchestratorModelRef`: `provider/model`, both halves non-empty.
pub fn parse_model_ref(model: &str) -> Option<(String, String)> {
    let model = normalize_model_id(model)?;
    let (provider, id) = model.split_once('/')?;
    (!provider.is_empty() && !id.is_empty()).then(|| (provider.to_string(), id.to_string()))
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

/// `readOrchestratorSettings`, entry only.
fn read_orchestrator_settings(path: &Path) -> Option<RouteEntry> {
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

/// `writeModelConfig`: what `models.json` holds, normalized.
fn normalized_json(config: &Routing) -> Value {
    normalize_routing(&config.to_json())
        .unwrap_or_default()
        .to_json()
}

// ---------------------------------------------------------------------------
// Loading the screen
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Loaded {
    file: ProfilesFile,
    names: Vec<String>,
    pin: Option<PinStatus>,
    notices: Vec<String>,
    pi_default: Option<RouteEntry>,
    status: Option<String>,
}

fn load_screen(env: &Env) -> Result<Loaded, String> {
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

/// What the screen must not hide about the repository pins
/// (`routingScreenNotices`, `profilePinScopeNote`).
fn pin_notes(pin: Option<&PinStatus>, file: &ProfilesFile) -> Vec<String> {
    let eval = evaluate_pin(pin, file);
    let mut notes = Vec::new();
    for (layer, path) in &eval.invalid {
        notes.push(format!(
            "The {layer} pin file {} is not a valid pin and is ignored. Fix or remove it.",
            path.display()
        ));
    }
    for (layer, profile, path) in &eval.stale {
        notes.push(format!(
            "The {layer} pin names \"{profile}\", which no profile defines, so it is ignored ({}).",
            path.display()
        ));
    }
    if let Some((layer, profile, path)) = &eval.winner {
        notes.push(format!(
            "This repository pins \"{profile}\" ({layer} pin at {}), so its subagent launches resolve that profile instead of the global routing.",
            path.display()
        ));
    }
    notes
}

/// Unmatched routing keys, as the screen notes them (`unmatchedRoutingNotes`).
fn unmatched_notes(config: &Routing, names: &[String]) -> Vec<String> {
    config
        .roles()
        .filter(|(key, _)| !names.contains(key))
        .map(|(key, _)| match names.iter().find(|n| n.ends_with(&format!(".{key}"))) {
            Some(suggestion) => format!("Profile key \"{key}\" matches no agent; did you mean {suggestion}?"),
            None => format!("Profile key \"{key}\" matches no agent configured in gentle-shell, so it is ignored at launch."),
        })
        .collect()
}

/// `clampToSupportedLevel`.
fn clamp_level(level: &str, supported: &[&str]) -> String {
    if supported.contains(&level) {
        return level.to_string();
    }
    let start = THINKING_LEVELS
        .iter()
        .position(|l| *l == level)
        .unwrap_or(0);
    THINKING_LEVELS[start + 1..]
        .iter()
        .chain(THINKING_LEVELS[..start].iter().rev())
        .find(|l| supported.contains(l))
        .or(supported.first())
        .map_or_else(|| "off".to_string(), |l| l.to_string())
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

pub enum ProfilesEvent {
    Close,
    /// Apply the profile to the running session: send RPC `set_model` with
    /// `model` (provider, id) and then `set_thinking_level` with `thinking`.
    /// `None` means the profile leaves the orchestrator untouched. The draft
    /// was saved to `profiles.json` first when it had edits.
    UseInSession {
        profile: String,
        model: Option<(String, String)>,
        thinking: Option<String>,
    },
    /// Make default finished outside a pinned repository: the TUI also moves
    /// the running session to the new default orchestrator. Send RPC
    /// `set_model` (provider, id) and, when `thinking` is set,
    /// `set_thinking_level`. Files were already written (`Changed` follows).
    SwitchOrchestrator {
        model: (String, String),
        thinking: Option<String>,
    },
    /// Send this slash command as a prompt (`/gentle:profiles clear`,
    /// `/gentle:profiles publish <name>`).
    RunCommand(String),
    /// `profiles.json`, routing, pins or Pi settings were written.
    Changed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Good,
    Bad,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NameMode {
    New,
    Duplicate,
    Rename,
}

#[derive(Clone, PartialEq, Eq)]
enum Leave {
    Switch(String),
    New,
    Duplicate,
    Close,
}

enum Confirm {
    Unsaved(Leave),
    Delete(String),
    MakeDefault(DefaultPlan),
}

/// Rows: the orchestrator, "every agent", then each assignment name.
const ALL_ROW: &str = "__all";

/// Route table columns. The name column keeps its width; the model, effort
/// and tier cells wrap under it together when the window is too narrow.
const NAME_W: f32 = 168.;
const EFFORT_W: f32 = 118.;
const TIER_W: f32 = 92.;
const CELLS_MIN_W: f32 = 340.;

/// One route row's (or the column header's) two wrapping halves.
fn route_columns() -> Div {
    h_flex().flex_wrap().gap_x_2().gap_y_1()
}

fn route_name_column() -> Div {
    v_flex().w(px(NAME_W)).flex_shrink_0().min_w_0().gap_0p5()
}

fn route_cells() -> Div {
    h_flex().flex_1().min_w(px(CELLS_MIN_W)).gap_2()
}

/// A two-line menu entry: the label over a muted detail.
fn menu_entry(label: &SharedString, detail: &SharedString) -> Div {
    v_flex()
        .min_w_0()
        .py_0p5()
        .child(div().text_sm().text_color(ui::fg()).child(label.clone()))
        .child(
            div()
                .text_xs()
                .font_family(ui::MONO)
                .text_color(ui::muted())
                .child(detail.clone()),
        )
}

pub struct ProfilesView {
    env: Option<Env>,
    cwd: PathBuf,
    models: Rc<Vec<ModelChoice>>,
    loading: bool,
    error: Option<String>,
    file: ProfilesFile,
    names: Vec<String>,
    pin: Option<PinStatus>,
    notices: Vec<String>,
    /// Pi's current `settings.json` default orchestrator.
    pi_default: Option<RouteEntry>,
    selected: Option<String>,
    stored: Routing,
    draft: Routing,
    status: Option<(String, Tone)>,
    busy: bool,
    name_mode: Option<NameMode>,
    name_input: Entity<InputState>,
    custom_target: Option<String>,
    custom_input: Entity<InputState>,
    confirm: Option<Confirm>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProfilesEvent> for ProfilesView {}

impl ProfilesView {
    pub fn new(
        cwd: PathBuf,
        models: Vec<ModelChoice>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("profile-name"));
        let custom_input = cx.new(|cx| InputState::new(window, cx).placeholder("provider/model"));
        let subscriptions = vec![
            cx.subscribe_in(
                &name_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit_name(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &custom_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit_custom(window, cx);
                    }
                },
            ),
        ];
        let mut this = Self {
            env: None,
            cwd,
            models: Rc::new(models),
            loading: true,
            error: None,
            file: ProfilesFile::default(),
            names: Vec::new(),
            pin: None,
            notices: Vec::new(),
            pi_default: None,
            selected: None,
            stored: Routing::default(),
            draft: Routing::default(),
            status: None,
            busy: false,
            name_mode: None,
            name_input,
            custom_target: None,
            custom_input,
            confirm: None,
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    /// Run blocking file work off the UI thread, then apply its result. The
    /// task is detached so a write already under way finishes even if the
    /// screen closes (the result is then dropped).
    fn run_io<R: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> R + Send + 'static,
        done: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        self.busy = true;
        let job = cx.background_executor().spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                done(this, result, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Re-read the store, agents and pins; unsaved edits to the shown profile
    /// survive when that profile still exists.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let env = self.env.clone();
        let cwd = self.cwd.clone();
        self.run_io(
            cx,
            move || {
                let env = env.unwrap_or_else(|| Env::resolve(cwd));
                let loaded = load_screen(&env);
                (env, loaded)
            },
            |this, (env, loaded), cx| {
                this.loading = false;
                this.env = Some(env);
                match loaded {
                    Err(error) => this.error = Some(error),
                    Ok(loaded) => {
                        this.error = None;
                        let dirty = this.change_count() > 0;
                        let pin_winner = evaluate_pin(loaded.pin.as_ref(), &loaded.file)
                            .winner
                            .map(|w| w.1);
                        this.file = loaded.file;
                        this.names = loaded.names;
                        this.pin = loaded.pin;
                        this.notices = loaded.notices;
                        this.pi_default = loaded.pi_default;
                        if let Some(status) = loaded.status {
                            this.say(status, Tone::Info);
                            cx.emit(ProfilesEvent::Changed);
                        }
                        let keep = this.selected.clone().filter(|s| this.file.has(s));
                        let target = keep
                            .clone()
                            .or(pin_winner)
                            .or_else(|| this.file.active.clone().filter(|a| this.file.has(a)))
                            .or_else(|| this.file.names().next().map(str::to_string));
                        let keep_draft = dirty && keep.is_some() && keep == target;
                        this.select(target, keep_draft);
                    }
                }
            },
        );
    }

    fn select(&mut self, name: Option<String>, keep_draft: bool) {
        self.stored = name
            .as_deref()
            .and_then(|n| self.file.get(n))
            .cloned()
            .unwrap_or_default();
        if !keep_draft {
            self.draft = self.stored.clone();
        }
        self.selected = name;
        self.custom_target = None;
    }

    fn say(&mut self, message: impl Into<String>, tone: Tone) {
        self.status = Some((message.into(), tone));
    }

    fn pin_winner(&self) -> Option<String> {
        evaluate_pin(self.pin.as_ref(), &self.file)
            .winner
            .map(|w| w.1)
    }

    fn catalog_model(&self, model: &str) -> Option<&ModelChoice> {
        self.models.iter().find(|m| m.route_id() == model)
    }

    fn fast_capable(&self, model: Option<&str>) -> bool {
        model
            .and_then(|m| self.catalog_model(m))
            .is_some_and(ModelChoice::supports_fast)
    }

    /// The levels a model supports: a catalog model without reasoning offers
    /// only `off`; reasoning or unknown models offer every level.
    fn supported_levels(&self, model: Option<&str>) -> Option<Vec<&'static str>> {
        let choice = self.catalog_model(model?)?;
        (!choice.reasoning).then(|| vec!["off"])
    }

    fn row_keys(&self) -> Vec<String> {
        let mut keys = vec![ORCHESTRATOR.to_string()];
        keys.extend(self.names.iter().cloned());
        keys
    }

    fn change_count(&self) -> usize {
        self.row_keys()
            .iter()
            .map(|key| {
                let (d, s) = (self.draft.get(key), self.stored.get(key));
                let field = |f: fn(&RouteEntry) -> &Option<String>| {
                    d.and_then(|e| f(e).as_ref()) != s.and_then(|e| f(e).as_ref())
                };
                usize::from(field(|e| &e.model))
                    + usize::from(field(|e| &e.thinking))
                    + usize::from(field(|e| &e.tier))
            })
            .sum()
    }

    fn target_keys(&self, key: &str) -> Vec<String> {
        if key == ALL_ROW {
            self.names.clone()
        } else {
            vec![key.to_string()]
        }
    }

    fn row_label(key: &str) -> &str {
        match key {
            ORCHESTRATOR => "the orchestrator",
            ALL_ROW => "every agent",
            other => other,
        }
    }

    // -- draft edits (RoutingTablePanel.setModel / setThinking / toggleTier) --

    fn set_model(&mut self, key: &str, model: Option<String>, cx: &mut Context<Self>) {
        for target in self.target_keys(key) {
            if target == ORCHESTRATOR && model.is_none() {
                self.draft.remove(&target);
                continue;
            }
            let mut entry = self.draft.get(&target).cloned().unwrap_or_default();
            if entry.model != model {
                entry.tier = None;
            }
            entry.model = model.clone();
            self.draft.set(
                &target,
                if entry.is_clear() {
                    RouteEntry::default()
                } else {
                    entry
                },
            );
        }
        let label = model.unwrap_or_else(|| self.inherit_label(key).to_string());
        let selected = self.selected.clone().unwrap_or_default();
        self.say(
            format!(
                "{}: model {label}. Save keeps it in {selected}.",
                Self::row_label(key)
            ),
            Tone::Info,
        );
        cx.notify();
    }

    fn set_thinking(&mut self, key: &str, thinking: Option<String>, cx: &mut Context<Self>) {
        for target in self.target_keys(key) {
            if target == ORCHESTRATOR
                && self
                    .draft
                    .get(&target)
                    .and_then(|e| e.model.as_ref())
                    .is_none()
            {
                continue;
            }
            let mut entry = self.draft.get(&target).cloned().unwrap_or_default();
            entry.thinking = thinking.clone();
            self.draft.set(
                &target,
                if entry.is_clear() {
                    RouteEntry::default()
                } else {
                    entry
                },
            );
        }
        let selected = self.selected.clone().unwrap_or_default();
        let level = thinking.unwrap_or_else(|| "inherit".into());
        self.say(
            format!(
                "{}: effort {level}. Save keeps it in {selected}.",
                Self::row_label(key)
            ),
            Tone::Info,
        );
        cx.notify();
    }

    fn toggle_tier(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(mut entry) = self.draft.get(key).cloned() else {
            return;
        };
        if !self.fast_capable(entry.model.as_deref()) {
            self.say(
                "Standard/Fast is available only for compatible OpenAI models.",
                Tone::Bad,
            );
            cx.notify();
            return;
        }
        let fast = entry.tier.as_deref() != Some("fast");
        entry.tier = Some(if fast { "fast" } else { "standard" }.into());
        self.draft.set(key, entry);
        let selected = self.selected.clone().unwrap_or_default();
        self.say(
            format!(
                "{}: {}. Save keeps it in {selected}.{}",
                Self::row_label(key),
                if fast { "Fast" } else { "Standard" },
                if fast { " Fast depends on plan/model; subscription usage 2.5x or purchased credits 2x. API pricing differs." } else { "" }
            ),
            Tone::Info,
        );
        cx.notify();
    }

    fn all_fast(&mut self, cx: &mut Context<Self>) {
        let mut count = 0;
        for key in self.row_keys() {
            let Some(mut entry) = self.draft.get(&key).cloned() else {
                continue;
            };
            if !self.fast_capable(entry.model.as_deref()) {
                continue;
            }
            entry.tier = Some("fast".into());
            self.draft.set(&key, entry);
            count += 1;
        }
        if count == 0 {
            self.say("No compatible OpenAI routes in this profile.", Tone::Info);
        } else {
            self.say(
                format!(
                    "All Fast: {count} compatible route{}. Save to keep. Subscription 2.5x; purchased credits 2x. API pricing differs.",
                    if count == 1 { "" } else { "s" }
                ),
                Tone::Info,
            );
        }
        cx.notify();
    }

    fn open_custom(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.custom_target = Some(key.to_string());
        let current = self
            .draft
            .get(key)
            .and_then(|e| e.model.clone())
            .unwrap_or_default();
        self.custom_input.update(cx, |s, cx| {
            s.set_value(current, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn submit_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.custom_target.take() else {
            return;
        };
        let answer = self.custom_input.read(cx).value().trim().to_string();
        self.custom_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        if answer.is_empty() {
            self.say("No custom model set.", Tone::Info);
        } else if let Some(model) = normalize_model_id(&answer) {
            self.set_model(&key, Some(model), cx);
        } else {
            self.say(
                "Custom model id must be one provider/model line using letters, numbers, '.', '-', '_', '~', ':', '@', '/', '+', '%' only.",
                Tone::Bad,
            );
        }
        cx.notify();
    }

    // -- store actions ----------------------------------------------------

    /// Write `next` to profiles.json, then adopt it.
    fn write_store(
        &mut self,
        next: ProfilesFile,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let Some(env) = self.env.clone() else { return };
        let text = serialize_profiles(&next);
        self.run_io(
            cx,
            move || write_atomic(&env.profiles_path(), &text).map_err(|e| e.to_string()),
            move |this, result, cx| match result {
                Ok(()) => {
                    this.file = next;
                    cx.emit(ProfilesEvent::Changed);
                    done(this, cx);
                }
                Err(error) => {
                    this.say(format!("profiles.json was not written: {error}"), Tone::Bad)
                }
            },
        );
    }

    /// The draft as the store would hold it (create or update), or `None`
    /// when it already matches.
    fn saved_draft(&self) -> Result<Option<ProfilesFile>, String> {
        let Some(name) = &self.selected else {
            return Ok(None);
        };
        let normalized = normalize_routing(&self.draft.to_json()).unwrap_or_default();
        let stored = self
            .file
            .get(name)
            .map(|r| normalize_routing(&r.to_json()).unwrap_or_default());
        if stored.as_ref() == Some(&normalized) {
            return Ok(None);
        }
        if self.file.has(name) {
            update_profile(&self.file, name, self.draft.clone()).map(Some)
        } else {
            create_profile(&self.file, name, self.draft.clone()).map(Some)
        }
    }

    /// Save pending edits, then continue with `then` (told whether it saved).
    fn save_then(
        &mut self,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, bool, &mut Context<Self>) + 'static,
    ) {
        match self.saved_draft() {
            Err(error) => self.say(format!("Not saved: {error}"), Tone::Bad),
            Ok(None) => then(self, false, cx),
            Ok(Some(next)) => self.write_store(next, cx, move |this, cx| {
                this.stored = this.draft.clone();
                then(this, true, cx);
            }),
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.change_count() == 0 {
            self.say("Nothing to save.", Tone::Info);
            cx.notify();
            return;
        }
        self.save_then(cx, |this, saved, _| {
            let name = this.selected.clone().unwrap_or_default();
            this.say(
                if saved {
                    format!("Saved \"{name}\".")
                } else {
                    "Nothing to save.".into()
                },
                Tone::Good,
            );
        });
    }

    fn use_in_session(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        self.save_then(cx, move |this, saved, cx| {
            let orchestrator = this.draft.orchestrator();
            let model = orchestrator
                .and_then(|e| e.model.as_deref())
                .and_then(parse_model_ref);
            let thinking = model
                .as_ref()
                .and_then(|_| orchestrator.and_then(|e| e.thinking.clone()));
            let prefix = if saved {
                format!("Saved \"{name}\". ")
            } else {
                String::new()
            };
            let live = match &model {
                Some((provider, id)) => format!(" Switching this session to {provider}/{id}."),
                None => " The orchestrator is left untouched.".into(),
            };
            this.say(
                format!("{prefix}Using \"{name}\" in this session.{live}"),
                Tone::Good,
            );
            cx.emit(ProfilesEvent::UseInSession {
                profile: name,
                model,
                thinking,
            });
        });
    }

    fn request_make_default(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        self.save_then(cx, move |this, saved, cx| {
            let Some(env) = this.env.clone() else { return };
            let file = this.file.clone();
            let config = file.get(&name).cloned().unwrap_or_default();
            if saved {
                this.say(format!("Saved \"{name}\"."), Tone::Good);
            }
            this.run_io(
                cx,
                move || plan_make_default(&env, &file, &name, &config),
                |this, plan, _| this.confirm = Some(Confirm::MakeDefault(plan)),
            );
        });
    }

    fn make_default(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(env) = self.env.clone() else { return };
        let file = self.file.clone();
        self.run_io(
            cx,
            move || {
                let outcome = make_default(&env, &file, &name);
                (
                    outcome,
                    read_pin_status(&env.cwd),
                    read_orchestrator_settings(&env.settings_path()),
                )
            },
            |this, (outcome, pin, pi_default), cx| {
                this.pin = pin;
                this.pi_default = pi_default;
                match outcome {
                    Ok(outcome) => {
                        this.file = outcome.file;
                        this.say(outcome.message, Tone::Good);
                        cx.emit(ProfilesEvent::Changed);
                        if let Some(entry) = outcome.orchestrator
                            && let Some(model) = entry.model.as_deref().and_then(parse_model_ref)
                        {
                            cx.emit(ProfilesEvent::SwitchOrchestrator {
                                model,
                                thinking: entry.thinking,
                            });
                        }
                    }
                    Err(error) => {
                        this.say(error, Tone::Bad);
                        cx.emit(ProfilesEvent::Changed);
                    }
                }
            },
        );
    }

    fn toggle_repo_pin(&mut self, cx: &mut Context<Self>) {
        let (Some(name), Some(env)) = (self.selected.clone(), self.env.clone()) else {
            return;
        };
        self.save_then(cx, move |this, _, cx| {
            this.run_io(
                cx,
                move || {
                    let Some(status) = read_pin_status(&env.cwd) else {
                        return (Err("This folder is not inside a Git worktree, so nothing was pinned.".to_string()), None);
                    };
                    let pinned = status.local == PinRead::Valid(name.clone());
                    let result = if pinned {
                        clear_pin(&status.local_path).map_err(|e| e.to_string()).map(|_| {
                            format!(
                                "Removed the local pin from {}. This clone falls back to the repository declaration, then to the global routing.",
                                status.local_path.display()
                            )
                        })
                    } else {
                        write_pin(&status.local_path, &name).map(|_| {
                            format!(
                                "Pinned \"{name}\" for this clone in {}. Subagent launches here resolve it; the orchestrator and other repositories keep their global routing.",
                                status.local_path.display()
                            )
                        })
                    };
                    (result, read_pin_status(&env.cwd))
                },
                |this, (result, pin), cx| {
                    match result {
                        Ok(message) => {
                            this.pin = pin;
                            this.say(message, Tone::Good);
                            cx.emit(ProfilesEvent::Changed);
                        }
                        Err(error) => this.say(error, Tone::Bad),
                    }
                },
            );
        });
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        self.save_then(cx, move |this, _, cx| {
            this.say(
                format!("Sent /gentle:profiles publish {name}. Refresh to see the declaration."),
                Tone::Info,
            );
            cx.emit(ProfilesEvent::RunCommand(format!(
                "/gentle:profiles publish {name}"
            )));
        });
    }

    fn clear_session(&mut self, cx: &mut Context<Self>) {
        self.say("Sent /gentle:profiles clear.", Tone::Info);
        cx.emit(ProfilesEvent::RunCommand("/gentle:profiles clear".into()));
        cx.notify();
    }

    /// Leaving the shown profile with unsaved edits asks first.
    fn leave(&mut self, leave: Leave, window: &mut Window, cx: &mut Context<Self>) {
        if self.change_count() > 0 {
            self.confirm = Some(Confirm::Unsaved(leave));
        } else {
            self.proceed(leave, window, cx);
        }
        cx.notify();
    }

    fn proceed(&mut self, leave: Leave, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm = None;
        match leave {
            Leave::Switch(name) => {
                self.select(Some(name.clone()), false);
                self.say(format!("Showing \"{name}\"."), Tone::Info);
            }
            Leave::Close => cx.emit(ProfilesEvent::Close),
            Leave::New => self.open_name(NameMode::New, window, cx),
            Leave::Duplicate => self.open_name(NameMode::Duplicate, window, cx),
        }
        cx.notify();
    }

    fn open_name(&mut self, mode: NameMode, window: &mut Window, cx: &mut Context<Self>) {
        self.name_mode = Some(mode);
        self.name_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn submit_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mode) = self.name_mode else { return };
        let answer = self.name_input.read(cx).value().trim().to_string();
        let current = self.selected.clone().unwrap_or_default();
        let result = match mode {
            NameMode::New if answer.is_empty() => {
                Err("No profile created: no name entered.".to_string())
            }
            NameMode::New => {
                create_profile(&self.file, &answer, Routing::default()).map(|f| (f, answer.clone()))
            }
            NameMode::Duplicate => {
                let to = if answer.is_empty() {
                    format!("{current}-copy")
                } else {
                    answer.clone()
                };
                duplicate_profile(&self.file, &current, &to).map(|f| (f, to))
            }
            NameMode::Rename if answer.is_empty() => Err(format!(
                "Profile \"{current}\" unchanged: no new name entered."
            )),
            NameMode::Rename => {
                rename_profile(&self.file, &current, &answer).map(|f| (f, answer.clone()))
            }
        };
        self.name_input
            .update(cx, |s, cx| s.set_value("", window, cx));
        match result {
            Err(error) => self.say(error, Tone::Bad),
            Ok((next, name)) => {
                self.name_mode = None;
                let from = current.clone();
                self.write_store(next, cx, move |this, cx| {
                    match mode {
                        NameMode::Rename => {
                            let keep = this.change_count() > 0;
                            this.select(Some(name.clone()), keep);
                            this.say(format!("Profile \"{from}\" renamed to \"{name}\"."), Tone::Good);
                            if let Some(env) = this.env.clone() {
                                let (old, new) = (from.clone(), name.clone());
                                this.run_io(
                                    cx,
                                    move || {
                                        follow_fallbacks(&env.config_home, &old, Some(&new));
                                        let follow = follow_renamed_pin(&env.cwd, &old, &new);
                                        (follow, read_pin_status(&env.cwd))
                                    },
                                    move |this, ((followed, declared), pin), _| {
                                        this.pin = pin;
                                        let mut message = format!("Profile \"{from}\" renamed to \"{name}\".");
                                        for path in followed {
                                            message.push_str(&format!(" Updated the clone pin {}.", path.display()));
                                        }
                                        for path in declared {
                                            message.push_str(&format!(
                                                " The committed declaration {} still names \"{from}\"; Publish to repo republishes it.",
                                                path.display()
                                            ));
                                        }
                                        this.say(message, Tone::Good);
                                    },
                                );
                            }
                        }
                        NameMode::New | NameMode::Duplicate => {
                            this.select(Some(name.clone()), false);
                            let verb = if mode == NameMode::New { "created".to_string() } else { format!("duplicated from \"{from}\"") };
                            this.say(format!("Profile \"{name}\" {verb}."), Tone::Good);
                        }
                    }
                });
            }
        }
        cx.notify();
    }

    fn request_delete(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        if self.file.active.as_deref() == Some(name.as_str()) {
            self.say(
                format!("Profile {name} is active. Apply another profile before deleting it."),
                Tone::Bad,
            );
        } else if let Some(status) = &self.pin
            && let layers = [
                ("local", &status.local, &status.local_path),
                ("repo", &status.repo, &status.repo_path),
            ]
            .into_iter()
            .filter(|(_, read, _)| **read == PinRead::Valid(name.clone()))
            .map(|(layer, _, path)| format!("{layer} pin at {}", path.display()))
            .collect::<Vec<_>>()
            && !layers.is_empty()
        {
            self.say(
                format!(
                    "Profile \"{name}\" is pinned for this repository ({}). Remove the pin first (Use in this repo for the clone pin, Publish to repo for the declaration), then delete it.",
                    layers.join(" and ")
                ),
                Tone::Bad,
            );
        } else {
            self.confirm = Some(Confirm::Delete(name));
        }
        cx.notify();
    }

    fn delete(&mut self, name: String, cx: &mut Context<Self>) {
        self.confirm = None;
        match delete_profile(&self.file, &name) {
            Err(error) => self.say(format!("Profile not deleted: {error}"), Tone::Bad),
            Ok(next) => self.write_store(next, cx, move |this, cx| {
                let first = this.file.names().next().map(str::to_string);
                this.select(first, false);
                this.say(format!("Deleted \"{name}\"."), Tone::Good);
                if let Some(env) = this.env.clone() {
                    this.run_io(
                        cx,
                        move || follow_fallbacks(&env.config_home, &name, None),
                        |_, _, _| {},
                    );
                }
            }),
        }
        cx.notify();
    }

    // -- rendering ----------------------------------------------------------

    fn inherit_label(&self, key: &str) -> &'static str {
        match key {
            ORCHESTRATOR => "leave untouched",
            ALL_ROW => "inherit",
            k if is_review_role(k) => "Pi persisted default model",
            _ => "inherit",
        }
    }

    /// The value one row shows; the every-agent row shows the shared value or
    /// "mixed".
    fn current_value(
        &self,
        key: &str,
        field: fn(&RouteEntry) -> &Option<String>,
    ) -> Result<Option<String>, ()> {
        if key == ALL_ROW {
            let values: Vec<Option<String>> = self
                .names
                .iter()
                .map(|n| self.draft.get(n).and_then(|e| field(e).clone()))
                .collect();
            return match values.first() {
                Some(first) if values.iter().all(|v| v == first) => Ok(first.clone()),
                Some(_) => Err(()),
                None => Ok(None),
            };
        }
        Ok(self.draft.get(key).and_then(|e| field(e).clone()))
    }

    fn is_edited(&self, key: &str, field: fn(&RouteEntry) -> &Option<String>) -> bool {
        if key == ALL_ROW {
            return false;
        }
        self.draft.get(key).and_then(|e| field(e).as_ref())
            != self.stored.get(key).and_then(|e| field(e).as_ref())
    }

    fn render_model_cell(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let models = self.models.clone();
        let current = self.current_value(key, |e| &e.model);
        let inherit = self.inherit_label(key);
        let label = match &current {
            Ok(Some(model)) => model.clone(),
            Ok(None) if key != ORCHESTRATOR && is_review_role(key) => "Pi default".into(),
            Ok(None) => inherit.to_string(),
            Err(()) => "mixed".into(),
        };
        let edited = self.is_edited(key, |e| &e.model);
        let color = if edited {
            ui::warning()
        } else if matches!(current, Ok(Some(_))) {
            ui::fg()
        } else {
            ui::muted()
        };
        let key = key.to_string();
        let current = current.ok().flatten();
        // A stretched column gives the trigger a definite width, so a long
        // model id ellipsizes instead of spilling into the effort column.
        v_flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(
                Button::new(SharedString::from(format!("model-{key}")))
                    .ghost()
                    .xsmall()
                    .w_full()
                    .font_family(ui::MONO)
                    .text_color(color)
                    .label(label)
                    .dropdown_caret(true)
                    .disabled(self.busy)
                    .dropdown_menu(move |mut menu, _, _| {
                        let pick = |model: Option<String>| {
                            let weak = weak.clone();
                            let key = key.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                let _ = weak
                                    .update(cx, |this, cx| this.set_model(&key, model.clone(), cx));
                            }
                        };
                        menu = menu.item(
                            PopupMenuItem::new(inherit)
                                .checked(current.is_none())
                                .on_click(pick(None)),
                        );
                        let mut provider = String::new();
                        for model in models.iter() {
                            if model.provider != provider {
                                provider = model.provider.clone();
                                menu = menu.label(provider.clone());
                            }
                            let id = model.route_id();
                            let Some(id) = normalize_model_id(&id) else {
                                continue;
                            };
                            let name = SharedString::from(model.name.clone());
                            let model_id = SharedString::from(model.id.clone());
                            menu = menu.item(
                                PopupMenuItem::element(move |_, _| menu_entry(&name, &model_id))
                                    .checked(current.as_deref() == Some(id.as_str()))
                                    .on_click(pick(Some(id))),
                            );
                        }
                        let weak = weak.clone();
                        let key = key.clone();
                        menu.separator()
                            .item(
                                PopupMenuItem::new("Custom model id…")
                                    .icon(Icon::new(IconName::Plus))
                                    .on_click(move |_, window, cx| {
                                        let _ = weak.update(cx, |this, cx| {
                                            this.open_custom(&key, window, cx)
                                        });
                                    }),
                            )
                            .max_h(px(420.))
                            .scrollable(true)
                    }),
            )
            .into_any_element()
    }

    fn render_effort_cell(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let model = if key == ALL_ROW {
            None
        } else {
            self.draft.get(key).and_then(|e| e.model.clone())
        };
        if key == ORCHESTRATOR && model.is_none() {
            return div()
                .w(px(EFFORT_W))
                .flex_shrink_0()
                .px_2()
                .text_xs()
                .text_color(ui::faint())
                .child("follows model")
                .into_any_element();
        }
        let supported = if key == ALL_ROW {
            let known: Vec<Vec<&str>> = self
                .names
                .iter()
                .filter_map(|n| {
                    self.supported_levels(self.draft.get(n).and_then(|e| e.model.as_deref()))
                })
                .collect();
            (!known.is_empty()).then(|| {
                THINKING_LEVELS
                    .iter()
                    .copied()
                    .filter(|l| known.iter().all(|k| k.contains(l)))
                    .collect::<Vec<_>>()
            })
        } else {
            self.supported_levels(model.as_deref())
        };
        let current = self.current_value(key, |e| &e.thinking);
        let label = match &current {
            Ok(Some(level)) => match &supported {
                Some(levels) if !levels.contains(&level.as_str()) => {
                    format!("{level} → {}", clamp_level(level, levels))
                }
                _ => level.clone(),
            },
            Ok(None) if is_review_role(key) => "Pi default".into(),
            Ok(None) => "inherit".into(),
            Err(()) => "mixed".into(),
        };
        let edited = self.is_edited(key, |e| &e.thinking);
        let color = if edited {
            ui::warning()
        } else if matches!(current, Ok(Some(_))) {
            ui::fg()
        } else {
            ui::muted()
        };
        let options: Vec<&'static str> = THINKING_LEVELS
            .iter()
            .copied()
            .filter(|l| supported.as_ref().is_none_or(|s| s.contains(l)))
            .collect();
        let inherit = if is_review_role(key) {
            "Pi persisted default effort"
        } else {
            "Inherit effort"
        };
        let weak = cx.entity().downgrade();
        let key = key.to_string();
        let current = current.ok().flatten();
        v_flex()
            .w(px(EFFORT_W))
            .flex_shrink_0()
            .overflow_hidden()
            .child(
                Button::new(SharedString::from(format!("effort-{key}")))
                    .ghost()
                    .xsmall()
                    .w_full()
                    .font_family(ui::MONO)
                    .text_color(color)
                    .label(label)
                    .dropdown_caret(true)
                    .disabled(self.busy)
                    .dropdown_menu(move |mut menu, _, _| {
                        let pick = |level: Option<String>| {
                            let weak = weak.clone();
                            let key = key.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.set_thinking(&key, level.clone(), cx)
                                });
                            }
                        };
                        menu = menu.item(
                            PopupMenuItem::new(inherit)
                                .checked(current.is_none())
                                .on_click(pick(None)),
                        );
                        for level in &options {
                            menu = menu.item(
                                PopupMenuItem::new(*level)
                                    .checked(current.as_deref() == Some(*level))
                                    .on_click(pick(Some(level.to_string()))),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }

    fn render_tier_cell(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let entry = self.draft.get(key);
        let model = entry.and_then(|e| e.model.as_deref());
        let tier = entry.and_then(|e| e.tier.as_deref());
        let edited = self.is_edited(key, |e| &e.tier);
        let cell = div().w(px(TIER_W)).flex_shrink_0();
        if key == ALL_ROW {
            return cell.into_any_element();
        }
        if !self.fast_capable(model) {
            let text = match tier {
                Some(tier) => format!("{tier} (n/a)"),
                None => "—".into(),
            };
            return cell
                .px_2()
                .text_xs()
                .text_color(ui::faint())
                .child(text)
                .into_any_element();
        }
        let fast = tier == Some("fast");
        let key = key.to_string();
        cell.child(
            Button::new(SharedString::from(format!("tier-{key}")))
                .xsmall()
                .when(fast, |b| b.outline().selected(true))
                .when(!fast, |b| b.ghost())
                .when(edited, |b| b.text_color(ui::warning()))
                .icon(Icon::new(IconName::Zap).when(fast, |i| i.text_color(ui::champagne())))
                .label(match tier {
                    Some("fast") => "Fast",
                    Some(_) => "Standard",
                    None => "Standard",
                })
                .tooltip("OpenAI Fast (service_tier priority): 2.5x subscription usage or 2x purchased credits")
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_tier(&key, cx))),
        )
        .into_any_element()
    }

    fn render_row(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let (label, hint) = match key {
            ORCHESTRATOR => ("orchestrator".to_string(), Some("settings.json")),
            ALL_ROW => ("every agent".to_string(), Some("sets all rows")),
            k if is_review_role(k) => (k.to_string(), Some("review role")),
            k => (k.to_string(), None),
        };
        let custom = self.custom_target.as_deref() == Some(key);
        let row = v_flex()
            .rounded_lg()
            .hover(|s| s.bg(ui::surface_hover()))
            .when(custom, |d| d.bg(ui::raised()))
            .child(
                route_columns()
                    .px_2()
                    .py_1p5()
                    .child(
                        // The hint sits under the name so the column stays
                        // narrow enough for the name itself.
                        route_name_column()
                            .child(
                                div()
                                    .truncate()
                                    .font_family(ui::MONO)
                                    .text_xs()
                                    .text_color(if key == ALL_ROW {
                                        ui::muted()
                                    } else {
                                        ui::fg()
                                    })
                                    .child(label),
                            )
                            .when_some(hint, |d, hint| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(ui::muted())
                                        .child(hint),
                                )
                            }),
                    )
                    .child(
                        route_cells()
                            .child(self.render_model_cell(key, cx))
                            .child(self.render_effort_cell(key, cx))
                            .child(self.render_tier_cell(key, cx)),
                    ),
            )
            .when(custom, |d| {
                d.child(
                    h_flex()
                        .px_2()
                        .pb_2()
                        .gap_2()
                        .child(
                            ui::field()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&self.custom_input).appearance(false).small()),
                        )
                        .child(
                            Button::new("custom-apply")
                                .small()
                                .outline()
                                .icon(Icon::new(IconName::Check))
                                .label("Set")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_custom(window, cx)
                                })),
                        )
                        .child(
                            Button::new("custom-cancel")
                                .small()
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.custom_target = None;
                                    cx.notify();
                                })),
                        ),
                )
            });
        ui::appear(SharedString::from(format!("route-{key}")), row).into_any_element()
    }

    fn render_profile_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pin_winner = self.pin_winner();
        let declared = match self.pin.as_ref().map(|p| &p.repo) {
            Some(PinRead::Valid(name)) => Some(name.clone()),
            _ => None,
        };
        let has_selection = self.selected.is_some();
        let count = self.file.profiles.len();
        v_flex()
            .w(px(210.))
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(ui::border())
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .gap_2()
                    .child(ui::section_label("Profiles"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui::faint())
                            .child(count.to_string()),
                    ),
            )
            .child(
                div()
                    .id("profiles-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .child(
                        v_flex()
                            .gap_0p5()
                            .children(self.file.profiles.iter().enumerate().map(
                                |(ix, (name, config))| {
                                    let selected = self.selected.as_deref() == Some(name.as_str());
                                    let active = self.file.active.as_deref() == Some(name.as_str());
                                    let pinned = pin_winner.as_deref() == Some(name.as_str());
                                    let is_declared = declared.as_deref() == Some(name.as_str());
                                    let roles = config.roles().count();
                                    let target = name.clone();
                                    let row = v_flex()
                                        .id(("profile", ix))
                                        .px_2()
                                        .py_1p5()
                                        .gap_1()
                                        .rounded_lg()
                                        .cursor_pointer()
                                        .when(selected, |d| d.bg(ui::raised()))
                                        .when(!selected, |d| d.hover(|s| s.bg(ui::surface_hover())))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if this.selected.as_deref() != Some(target.as_str()) {
                                                this.leave(
                                                    Leave::Switch(target.clone()),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }))
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_sm()
                                                        .text_color(if selected {
                                                            ui::fg()
                                                        } else {
                                                            ui::muted()
                                                        })
                                                        .child(name.clone()),
                                                )
                                                .when(active, |d| {
                                                    d.child(
                                                        Icon::new(IconName::Star)
                                                            .xsmall()
                                                            .text_color(ui::accent()),
                                                    )
                                                })
                                                .when(pinned, |d| {
                                                    d.child(
                                                        Icon::new(IconName::GitBranch)
                                                            .xsmall()
                                                            .text_color(ui::warning()),
                                                    )
                                                }),
                                        )
                                        .child(
                                            h_flex()
                                                .flex_wrap()
                                                .gap_1()
                                                .child(
                                                    div().text_xs().text_color(ui::faint()).child(
                                                        format!(
                                                            "{roles} {}",
                                                            if roles == 1 {
                                                                "role"
                                                            } else {
                                                                "roles"
                                                            }
                                                        ),
                                                    ),
                                                )
                                                .when(active, |d| {
                                                    d.child(ui::chip("default", ui::accent()))
                                                })
                                                .when(pinned, |d| {
                                                    d.child(ui::chip("this repo", ui::warning()))
                                                })
                                                .when(is_declared && !pinned, |d| {
                                                    d.child(ui::chip("declared", ui::info()))
                                                }),
                                        );
                                    ui::appear(("profile-in", ix), div().child(row))
                                },
                            )),
                    ),
            )
            .child(
                v_flex()
                    .p_2()
                    .gap_2()
                    .border_t_1()
                    .border_color(ui::border())
                    .when_some(self.name_mode, |d, mode| {
                        let title = match mode {
                            NameMode::New => "New empty profile".to_string(),
                            NameMode::Duplicate => format!(
                                "Duplicate as (empty = {}-copy)",
                                self.selected.as_deref().unwrap_or_default()
                            ),
                            NameMode::Rename => format!(
                                "Rename \"{}\" to",
                                self.selected.as_deref().unwrap_or_default()
                            ),
                        };
                        d.child(
                            v_flex()
                                .gap_1p5()
                                .child(div().text_xs().text_color(ui::muted()).child(title))
                                .child(
                                    ui::field().child(
                                        Input::new(&self.name_input).appearance(false).small(),
                                    ),
                                )
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Button::new("name-ok")
                                                .small()
                                                .outline()
                                                .icon(Icon::new(IconName::Check))
                                                .label("OK")
                                                .disabled(self.busy)
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.submit_name(window, cx)
                                                })),
                                        )
                                        .child(
                                            Button::new("name-cancel")
                                                .small()
                                                .ghost()
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.name_mode = None;
                                                    cx.notify();
                                                })),
                                        ),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("profile-new")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Plus))
                                    .label("New")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.leave(Leave::New, window, cx)
                                    })),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("profile-duplicate")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Copy))
                                    .tooltip("Duplicate")
                                    .disabled(self.busy || !has_selection)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.leave(Leave::Duplicate, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("profile-rename")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Replace))
                                    .tooltip("Rename")
                                    .disabled(self.busy || !has_selection)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_name(NameMode::Rename, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("profile-delete")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Trash))
                                    .tooltip("Delete")
                                    .disabled(self.busy || !has_selection)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.request_delete(cx)),
                                    ),
                            ),
                    ),
            )
    }

    fn render_scope(&self) -> impl IntoElement {
        let name = self.selected.clone().unwrap_or_default();
        let mut parts: Vec<(&str, Hsla)> = Vec::new();
        if self.file.active.as_deref() == Some(name.as_str()) {
            parts.push(("Default for new sessions", ui::accent()));
        }
        if self.pin_winner().as_deref() == Some(name.as_str()) {
            parts.push(("Used in this repo", ui::warning()));
        }
        let default_line = match &self.file.active {
            Some(active) => format!("Global default: {active}"),
            None => "No global default profile".into(),
        };
        let pin_line = match &self.pin {
            None => "Repo pin: none (not inside a Git worktree)".to_string(),
            Some(status) => {
                let layer = |read: &PinRead| match read {
                    PinRead::Missing => "none".to_string(),
                    PinRead::Invalid => "invalid file".to_string(),
                    PinRead::Valid(p) => p.clone(),
                };
                format!(
                    "Repo pin: local {} · declaration {}",
                    layer(&status.local),
                    layer(&status.repo)
                )
            }
        };
        ui::card()
            .p_3()
            .gap_1()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::fg())
                            .child(name),
                    )
                    .when(parts.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(ui::muted())
                                .child("Not in use. Pick a button below to use it."),
                        )
                    })
                    .children(parts.into_iter().map(|(part, color)| ui::chip(part, color))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_x_3()
                    .gap_y_0p5()
                    .text_xs()
                    .text_color(ui::muted())
                    .child(default_line)
                    .child(pin_line),
            )
    }

    fn render_table(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let notes: Vec<String> = self
            .notices
            .iter()
            .cloned()
            .chain(pin_notes(self.pin.as_ref(), &self.file))
            .chain(unmatched_notes(&self.draft, &self.names))
            .collect();
        let orchestrator_now = self.pi_default.clone();
        v_flex()
            .gap_3()
            .child(self.render_scope())
            .children(notes.into_iter().map(|note| {
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(ui::warning().opacity(0.22))
                    .bg(ui::warning().opacity(0.08))
                    .text_xs()
                    .text_color(ui::warning())
                    .child(Icon::new(IconName::TriangleAlert).xsmall().flex_shrink_0())
                    .child(div().flex_1().min_w_0().child(note))
            }))
            .child(
                h_flex()
                    .px_1()
                    .gap_2()
                    .child(ui::section_label("Routes"))
                    .child(div().flex_1())
                    .child(
                        Button::new("all-fast")
                            .small()
                            .ghost()
                            .icon(Icon::new(IconName::Zap))
                            .label("All Fast")
                            .tooltip("Set every compatible OpenAI route to Fast (draft only)")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.all_fast(cx))),
                    ),
            )
            .child(
                ui::card()
                    .p_1()
                    .gap_0p5()
                    .child(
                        route_columns()
                            .px_2()
                            .pt_1p5()
                            .pb_1()
                            .child(route_name_column().child(ui::section_label("Route")))
                            .child(
                                route_cells()
                                    .child(div().flex_1().min_w_0().px_1().child(ui::section_label("Model")))
                                    .child(
                                        div()
                                            .w(px(EFFORT_W))
                                            .flex_shrink_0()
                                            .px_1()
                                            .child(ui::section_label("Effort")),
                                    )
                                    .child(
                                        div()
                                            .w(px(TIER_W))
                                            .flex_shrink_0()
                                            .px_1()
                                            .child(ui::section_label("Tier")),
                                    ),
                            ),
                    )
                    .child(self.render_row(ORCHESTRATOR, cx))
                    .child(self.render_row(ALL_ROW, cx))
                    .when(!self.names.is_empty(), |d| {
                        d.child(div().mx_2().my_1().h(px(1.)).bg(ui::border()))
                            .child(
                                h_flex()
                                    .px_2()
                                    .pt_0p5()
                                    .gap_2()
                                    .child(ui::section_label("Agents"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(ui::faint())
                                            .child(self.names.len().to_string()),
                                    ),
                            )
                    })
                    .children(self.names.iter().map(|key| self.render_row(key, cx))),
            )
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(ui::faint())
                    .child(format!(
                        "Pi default orchestrator now: {}. Fast applies only to openai / openai-codex GPT-5.6 / 6 / 6.1 models; the selector is a request, not a billing guarantee.",
                        orchestrator_now.map_or_else(|| "not set".to_string(), |e| format_selection(Some(&e)))
                    )),
            )
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.confirm.as_ref()?;
        let (title, lines, buttons): (String, Vec<String>, Vec<AnyElement>) = match confirm {
            Confirm::Unsaved(leave) => {
                let name = self.selected.clone().unwrap_or_default();
                let save_leave = leave.clone();
                let discard_leave = leave.clone();
                (
                    format!("\"{name}\" has {} unsaved edit(s)", self.change_count()),
                    vec!["Save them before leaving this profile, or discard them.".into()],
                    vec![
                        Button::new("unsaved-discard")
                            .small()
                            .danger()
                            .icon(Icon::new(IconName::Trash))
                            .label("Discard")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.draft = this.stored.clone();
                                this.proceed(discard_leave.clone(), window, cx);
                            }))
                            .into_any_element(),
                        Button::new("unsaved-save")
                            .small()
                            .primary()
                            .icon(Icon::new(IconName::Check))
                            .label("Save and continue")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let leave = save_leave.clone();
                                this.confirm = None;
                                this.save_then(cx, move |this, _, cx| {
                                    this.proceed_without_window(leave, cx)
                                });
                            }))
                            .into_any_element(),
                    ],
                )
            }
            Confirm::Delete(name) => {
                let target = name.clone();
                let models = self
                    .env
                    .as_ref()
                    .map(|e| e.models_path().display().to_string())
                    .unwrap_or_default();
                (
                    "Delete profile?".into(),
                    vec![format!(
                        "Delete profile \"{name}\" from profiles.json? The routing in {models} is not changed."
                    )],
                    vec![
                        Button::new("delete-ok")
                            .small()
                            .danger()
                            .icon(Icon::new(IconName::Trash))
                            .label("Delete")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.delete(target.clone(), cx)),
                            )
                            .into_any_element(),
                    ],
                )
            }
            Confirm::MakeDefault(plan) => {
                let target = plan.name.clone();
                (
                    plan.title.clone(),
                    plan.lines.clone(),
                    vec![
                        Button::new("default-ok")
                            .small()
                            .primary()
                            .icon(Icon::new(if plan.pinned {
                                IconName::GitBranch
                            } else {
                                IconName::Star
                            }))
                            .label(if plan.pinned {
                                "Pin this clone"
                            } else {
                                "Apply"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.confirm = None;
                                this.make_default(target.clone(), cx);
                            }))
                            .into_any_element(),
                    ],
                )
            }
        };
        let icon = match confirm {
            Confirm::MakeDefault(_) => Icon::new(IconName::Star).text_color(ui::accent()),
            Confirm::Unsaved(_) => Icon::new(IconName::TriangleAlert).text_color(ui::warning()),
            Confirm::Delete(_) => Icon::new(IconName::Trash).text_color(ui::danger()),
        };
        Some(
            ui::pop_in(
                "profiles-confirm",
                v_flex()
                    .mx_4()
                    .mb_3()
                    .p_4()
                    .gap_2()
                    .rounded_xl()
                    .border_1()
                    .border_color(ui::border_strong())
                    .bg(ui::raised())
                    .shadow_lg()
                    .child(
                        h_flex().gap_2().child(icon.small().flex_shrink_0()).child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child(title),
                        ),
                    )
                    .child(
                        div()
                            .id("confirm-lines")
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .child(
                                v_flex().gap_0p5().children(
                                    lines
                                        .into_iter()
                                        .map(|l| div().text_xs().text_color(ui::muted()).child(l)),
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .pt_1()
                            .gap_2()
                            .flex_wrap()
                            .justify_end()
                            .child(
                                Button::new("confirm-cancel")
                                    .small()
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm = None;
                                        cx.notify();
                                    })),
                            )
                            .children(buttons),
                    ),
            )
            .into_any_element(),
        )
    }

    /// `proceed` for paths that run after async I/O and have no window.
    fn proceed_without_window(&mut self, leave: Leave, cx: &mut Context<Self>) {
        match leave {
            Leave::Switch(name) => {
                self.select(Some(name.clone()), false);
                self.say(format!("Saved. Showing \"{name}\"."), Tone::Info);
            }
            Leave::Close => cx.emit(ProfilesEvent::Close),
            Leave::New => self.name_mode = Some(NameMode::New),
            Leave::Duplicate => self.name_mode = Some(NameMode::Duplicate),
        }
        cx.notify();
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has = self.selected.is_some();
        let disabled = self.busy || !has;
        let unsaved = self.change_count();
        let name = self.selected.clone().unwrap_or_default();
        let pinned_here =
            matches!(self.pin.as_ref().map(|p| &p.local), Some(PinRead::Valid(p)) if *p == name);
        let declared_here =
            matches!(self.pin.as_ref().map(|p| &p.repo), Some(PinRead::Valid(p)) if *p == name);
        v_flex()
            .border_t_1()
            .border_color(ui::border())
            .when_some(self.status.as_ref(), |d, (message, tone)| {
                let (icon, color) = match tone {
                    Tone::Info => (IconName::Info, ui::muted()),
                    Tone::Good => (IconName::CircleCheck, ui::success()),
                    Tone::Bad => (IconName::CircleAlert, ui::danger()),
                };
                d.child(
                    h_flex()
                        .px_4()
                        .pt_2()
                        .gap_2()
                        .items_start()
                        .text_xs()
                        .text_color(color)
                        .child(Icon::new(icon).xsmall().flex_shrink_0().mt(px(1.)))
                        .child(div().flex_1().min_w_0().child(message.clone())),
                )
            })
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .flex_wrap()
                    // Where the profile applies beyond this screen.
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("repo-pin")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::GitBranch))
                                    .label(if pinned_here { "Stop using in this repo" } else { "Use in this repo" })
                                    .tooltip("Clone-local pin in the Git common dir; subagent launches here resolve it")
                                    .disabled(disabled || self.pin.is_none())
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_repo_pin(cx))),
                            )
                            .child(
                                Button::new("publish")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Upload))
                                    .label(if declared_here { "Unpublish from repo" } else { "Publish to repo" })
                                    .tooltip("Toggle the committed .pi/gentle-ai/profile.json declaration")
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.publish(cx))),
                            )
                            .child(
                                Button::new("clear-session")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Unlink))
                                    .label("Clear session binding")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_session(cx))),
                            ),
                    )
                    .child(
                        h_flex()
                            .ml_auto()
                            .gap_2()
                            .flex_wrap()
                            .when(unsaved > 0, |d| {
                                d.child(ui::chip(format!("{unsaved} unsaved"), ui::warning()))
                            })
                            .child(
                                Button::new("save")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Check))
                                    .label("Save")
                                    .disabled(disabled || unsaved == 0)
                                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                            )
                            .child(
                                Button::new("make-default")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Star))
                                    .label("Make default")
                                    .tooltip("Default for new sessions: writes models.json, agent routing and settings.json")
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.request_make_default(cx))),
                            )
                            .child(
                                Button::new("footer-close")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::X))
                                    .label("Close")
                                    .on_click(cx.listener(|this, _, window, cx| this.leave(Leave::Close, window, cx))),
                            )
                            .child(
                                Button::new("use-session")
                                    .small()
                                    .primary()
                                    .icon(Icon::new(IconName::CircleCheck))
                                    .label("Use in this session")
                                    .tooltip("Switch this session's orchestrator to the profile (saves edits first)")
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.use_in_session(cx))),
                            ),
                    ),
            )
    }
}

impl Render for ProfilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.loading {
            h_flex()
                .p_4()
                .gap_2()
                .text_sm()
                .text_color(ui::muted())
                .child(Spinner::new().small().color(ui::muted()))
                .child("Loading profiles…")
                .into_any_element()
        } else if let Some(error) = &self.error {
            v_flex()
                .p_4()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(ui::danger())
                        .child(error.clone()),
                )
                .child(
                    Button::new("profiles-retry")
                        .small()
                        .ghost()
                        .icon(Icon::new(IconName::RefreshCw))
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                )
                .into_any_element()
        } else {
            h_flex()
                .flex_1()
                .min_h_0()
                .items_start()
                .child(self.render_profile_list(cx))
                .child(
                    div()
                        .id("profiles-table")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .p_4()
                        .child(if self.selected.is_some() {
                            self.render_table(cx).into_any_element()
                        } else {
                            v_flex()
                                .items_center()
                                .py_8()
                                .gap_2()
                                .child(Icon::new(IconName::Layers).text_color(ui::faint()))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(ui::muted())
                                        .child("No profiles left. Create one with New."),
                                )
                                .into_any_element()
                        }),
                )
                .into_any_element()
        };
        let ready = !self.loading && self.error.is_none();
        let (backdrop, panel) = ui::screen(None);
        backdrop.occlude().child(ui::pop_in(
            "profiles-screen",
            panel
                .max_w(px(1100.))
                .max_h(px(760.))
                .child(
                    ui::screen_header()
                        .child(
                            Icon::new(IconName::Layers)
                                .small()
                                .flex_shrink_0()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Profiles"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child("Agent-model routing snapshots, like /gentle:profiles."),
                        )
                        .when(self.busy, |d| {
                            d.child(Spinner::new().small().color(ui::muted()))
                        })
                        .child(
                            Button::new("profiles-refresh")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::RefreshCw))
                                .tooltip("Re-read profiles, agents and pins")
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        )
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("profiles-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.leave(Leave::Close, window, cx)
                                })),
                        ),
                )
                .child(div().flex().flex_col().flex_1().min_h_0().child(body))
                .children(self.render_confirm(cx))
                .when(ready, |d| d.child(self.render_footer(cx))),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` exports GPUI's `test` attribute; these are plain tests.
    use core::prelude::v1::test;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("gsg-profiles-{tag}-{}", unique_token()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn env(&self) -> Env {
            Env {
                cwd: self.0.join("project"),
                agent_home: self.0.join("agent"),
                config_home: self.0.join("config"),
                home: self.0.join("home"),
                package_root: None,
            }
        }

        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn entry(model: Option<&str>, thinking: Option<&str>, tier: Option<&str>) -> RouteEntry {
        RouteEntry {
            model: model.map(str::to_string),
            thinking: thinking.map(str::to_string),
            tier: tier.map(str::to_string),
        }
    }

    #[test]
    fn parses_profiles_with_legacy_entries_and_drops() {
        let text = r#"{
          "kind": "gentle-pi.agent_model_profiles", "version": 1, "active": "work",
          "profiles": {
            "work": {
              "orchestrator": {"model": "openai/gpt-6-sol", "thinking": "high", "tier": "fast"},
              "gentle-ai-worker": "anthropic/claude-opus",
              "jd-judge-a": {"effort": "low"},
              "bad name!": {"model": "x/y"},
              "inherit-me": {},
              "broken": {"thinking": "ultra"}
            },
            "-bad": {}
          }
        }"#;
        let (file, drops) = parse_profiles(text).unwrap();
        assert_eq!(file.active.as_deref(), Some("work"));
        let work = file.get("work").unwrap();
        assert_eq!(
            work.orchestrator(),
            Some(&entry(Some("openai/gpt-6-sol"), Some("high"), Some("fast")))
        );
        assert_eq!(
            work.get("gentle-ai-worker"),
            Some(&entry(Some("anthropic/claude-opus"), None, None))
        );
        assert_eq!(
            work.get("jd-judge-a"),
            Some(&entry(None, Some("low"), None))
        );
        assert_eq!(work.get("inherit-me"), Some(&RouteEntry::default()));
        assert!(!work.contains("broken"));
        assert_eq!(drops.profiles, vec!["-bad".to_string()]);
        assert!(drops.agents.contains(&("work".into(), "bad name!".into())));
        assert!(drops.agents.contains(&("work".into(), "broken".into())));
        assert!(parse_profiles(r#"{"kind":"other","version":1,"profiles":{}}"#).is_none());
        assert!(
            parse_profiles(
                r#"{"kind":"gentle-pi.agent_model_profiles","version":2,"profiles":{}}"#
            )
            .is_none()
        );
        let (stale, drops) =
            parse_profiles(r#"{"kind":"gentle-pi.agent_model_profiles","version":1,"active":"gone","profiles":{}}"#).unwrap();
        assert_eq!(stale.active, None);
        assert_eq!(drops.active.as_deref(), Some("gone"));
    }

    #[test]
    fn validates_profile_names() {
        for good in ["a", "current", "Deep-work_2.1", &"x".repeat(64)] {
            assert!(is_valid_profile_name(good), "{good}");
        }
        for bad in [
            "",
            "-a",
            ".a",
            "_a",
            "a b",
            "a/b",
            "é",
            &"x".repeat(65),
            "constructor",
            "prototype",
            "__proto__",
        ] {
            assert!(!is_valid_profile_name(bad), "{bad}");
        }
        let file = bootstrap_profiles(&Routing::default());
        assert!(create_profile(&file, "bad name", Routing::default()).is_err());
        assert!(
            create_profile(&file, "current", Routing::default())
                .unwrap_err()
                .contains("already exists")
        );
        let renamed =
            rename_profile(&set_active(&file, "current").unwrap(), "current", "main").unwrap();
        assert_eq!(renamed.active.as_deref(), Some("main"));
        assert!(
            delete_profile(&renamed, "main")
                .unwrap_err()
                .contains("is active")
        );
    }

    #[test]
    fn save_is_atomic_and_keeps_unknown_top_level_keys() {
        let scratch = Scratch::new("save");
        let path = scratch.write(
            "config/profiles.json",
            r#"{"kind":"gentle-pi.agent_model_profiles","version":1,"futureKey":{"a":1},"profiles":{"p":{"w":"a/b"}}}"#,
        );
        let StoreRead::Valid(file, _) = read_store(&path) else {
            panic!("store should parse")
        };
        let next = create_profile(
            &file,
            "q",
            Routing(vec![(
                "orchestrator".into(),
                entry(Some("x/y"), Some("max"), None),
            )]),
        )
        .unwrap();
        write_atomic(&path, &serialize_profiles(&next)).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.ends_with("}\n"));
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["futureKey"], json!({"a": 1}));
        assert_eq!(value["profiles"]["p"]["w"], json!({"model": "a/b"}));
        assert_eq!(
            value["profiles"]["q"]["orchestrator"],
            json!({"model": "x/y", "thinking": "max"})
        );
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["kind", "version", "profiles", "futureKey"]);
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        // Identical bytes are not rewritten.
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        write_atomic(&path, &text).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn seeds_current_from_saved_and_materialized_routing() {
        let scratch = Scratch::new("seed");
        let env = scratch.env();
        scratch.write(
            "config/models.json",
            r#"{"gentle-ai-worker":{"model":"openai/gpt-6-sol","thinking":"high"}}"#,
        );
        scratch.write(
            "agent/agents/gentle-ai-explore.md",
            "---\nname: gentle-ai-explore\ndescription: d\nmodel: anthropic/haiku\nthinking: low\n---\nbody\n",
        );
        scratch.write(
            "agent/agents/gentle-ai-worker.md",
            "---\nname: gentle-ai-worker\nmodel: ignored/model\n---\n",
        );
        scratch.write("agent/agents/plain.md", "---\nname: plain\n---\n");
        scratch.write(
            "agent/subagents.json",
            r#"{"model_profiles":{"gentle-ai-explore":{"effort":"medium"}}}"#,
        );
        scratch.write(
            "project/.pi/agents/proj.md",
            "---\nname: \"proj\"\npackage: team\n---\n",
        );
        let agents = discover_agents(&env);
        let names = assignment_names(&agents);
        assert_eq!(&names[..2], ["review-refuter", "review-validator"]);
        assert!(names.contains(&"team.proj".to_string()));
        let file = seed_profiles(&env, &agents);
        assert_eq!(file.active.as_deref(), Some("current"));
        let current = file.get("current").unwrap();
        assert_eq!(
            current.get("gentle-ai-worker"),
            Some(&entry(Some("openai/gpt-6-sol"), Some("high"), None))
        );
        // subagents.json effort wins over frontmatter; model falls back to it.
        assert_eq!(
            current.get("gentle-ai-explore"),
            Some(&entry(Some("anthropic/haiku"), Some("medium"), None))
        );
        assert!(!current.contains("plain"));
        // Nothing routed: seeded but not active.
        let empty = Scratch::new("seed-empty");
        let env = empty.env();
        let file = seed_profiles(&env, &discover_agents(&env));
        assert_eq!(file.active, None);
        assert_eq!(file.get("current"), Some(&Routing::default()));
    }

    #[test]
    fn make_default_settings_merge_keeps_other_keys() {
        let scratch = Scratch::new("settings");
        let path = scratch.write(
            "agent/settings.json",
            "{\n  \"theme\": \"dark\",\n  \"defaultModel\": \"old\",\n  \"defaultThinkingLevel\": \"low\",\n  \"packages\": [\"npm:x\"]\n}\n",
        );
        let written = apply_orchestrator_settings(
            &path,
            &entry(Some("openai/gpt-6-sol"), Some("high"), Some("fast")),
        )
        .unwrap();
        assert_eq!(written, SettingsWrite::Written);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "theme",
                "defaultModel",
                "defaultThinkingLevel",
                "packages",
                "defaultProvider"
            ]
        );
        assert_eq!(value["defaultProvider"], "openai");
        assert_eq!(value["defaultModel"], "gpt-6-sol");
        assert_eq!(value["defaultThinkingLevel"], "high");
        assert_eq!(value["packages"], json!(["npm:x"]));
        assert_eq!(
            apply_orchestrator_settings(
                &path,
                &entry(Some("openai/gpt-6-sol"), Some("high"), None)
            )
            .unwrap(),
            SettingsWrite::Unchanged
        );
        apply_orchestrator_settings(&path, &entry(Some("openai/gpt-6-sol"), None, None)).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value.get("defaultThinkingLevel").is_none());
        assert_eq!(
            apply_orchestrator_settings(&path, &entry(None, Some("high"), None)).unwrap(),
            SettingsWrite::Unchanged
        );
        assert!(
            apply_orchestrator_settings(&path, &entry(Some("no-provider"), None, None)).is_err()
        );
        let broken = scratch.write("agent/broken.json", "{nope");
        assert!(apply_orchestrator_settings(&broken, &entry(Some("a/b"), None, None)).is_err());
        assert_eq!(fs::read_to_string(&broken).unwrap(), "{nope");
    }

    #[test]
    fn make_default_writes_store_routing_agents_and_settings() {
        let scratch = Scratch::new("default");
        let env = scratch.env();
        fs::create_dir_all(&env.cwd).unwrap();
        let worker = scratch.write(
            "agent/agents/worker.md",
            "---\nname: worker\ndescription: w\nmodel: old/model\n---\nbody\n",
        );
        scratch.write(
            "agent/agents/other.md",
            "---\nname: other\nthinking: low\n---\n",
        );
        scratch.write(
            "agent/subagents.json",
            r#"{"keep":true,"model_profiles":{"other":{"effort":"low"}}}"#,
        );
        scratch.write("agent/settings.json", r#"{"theme":"x"}"#);
        let config = Routing(vec![
            (
                "orchestrator".into(),
                entry(Some("openai/gpt-6-sol"), Some("high"), None),
            ),
            (
                "worker".into(),
                entry(Some("anthropic/opus"), Some("max"), None),
            ),
        ]);
        let file =
            create_profile(&bootstrap_profiles(&Routing::default()), "deep", config).unwrap();
        let outcome = make_default(&env, &file, "deep").unwrap();
        assert_eq!(outcome.file.active.as_deref(), Some("deep"));
        assert_eq!(
            outcome.orchestrator,
            Some(entry(Some("openai/gpt-6-sol"), Some("high"), None))
        );
        let StoreRead::Valid(stored, _) = read_store(&env.profiles_path()) else {
            panic!()
        };
        assert_eq!(stored.active.as_deref(), Some("deep"));
        let models: Value =
            serde_json::from_str(&fs::read_to_string(env.models_path()).unwrap()).unwrap();
        assert_eq!(
            models["worker"],
            json!({"model": "anthropic/opus", "thinking": "max"})
        );
        assert_eq!(
            fs::read_to_string(&worker).unwrap(),
            "---\nname: worker\ndescription: w\nmodel: anthropic/opus\nthinking: max\n---\nbody\n"
        );
        assert_eq!(
            fs::read_to_string(scratch.0.join("agent/agents/other.md")).unwrap(),
            "---\nname: other\n---\n"
        );
        let subagents: Value = serde_json::from_str(
            &fs::read_to_string(env.subagents_path(AgentSource::User)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            subagents,
            json!({"keep": true, "model_profiles": {"worker": {"model": "anthropic/opus", "effort": "max"}}})
        );
        let settings: Value =
            serde_json::from_str(&fs::read_to_string(env.settings_path()).unwrap()).unwrap();
        assert_eq!(
            settings,
            json!({"theme": "x", "defaultProvider": "openai", "defaultModel": "gpt-6-sol", "defaultThinkingLevel": "high"})
        );
    }

    #[test]
    fn managed_agent_files_keep_their_manifest_hash() {
        let scratch = Scratch::new("managed");
        let env = scratch.env();
        let original = "---\nname: worker\n---\n";
        let path = scratch.write("agent/agents/worker.md", original);
        scratch.write(
            "agent/gentle-ai/managed-assets.json",
            &json!({"schemaVersion": 1, "assets": {"agents/worker.md": sha256_hex(original.as_bytes())}}).to_string(),
        );
        let next = update_frontmatter_routing(original, &entry(Some("a/b"), None, None));
        assert!(update_managed_ownership(&env.agent_home, &path, original, &next).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), next);
        let manifest: Value = serde_json::from_str(
            &fs::read_to_string(scratch.0.join("agent/gentle-ai/managed-assets.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["assets"]["agents/worker.md"],
            json!(sha256_hex(next.as_bytes()))
        );
        assert!(
            !scratch
                .0
                .join("agent/gentle-ai/managed-assets.lock")
                .exists()
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pins_resolve_local_then_repo() {
        let scratch = Scratch::new("pin");
        let file = create_profile(
            &bootstrap_profiles(&Routing::default()),
            "team",
            Routing::default(),
        )
        .unwrap();
        let status = pin_status_at(scratch.0.join("root"), &scratch.0.join("git"));
        write_pin(&status.repo_path, "team").unwrap();
        scratch.write("git/gentle-ai/profile-pin.json", "{}");
        let status = pin_status_at(scratch.0.join("root"), &scratch.0.join("git"));
        let eval = evaluate_pin(Some(&status), &file);
        assert_eq!(
            eval.winner.as_ref().map(|w| (w.0, w.1.as_str())),
            Some(("repo", "team"))
        );
        assert_eq!(eval.invalid.len(), 1);
        write_pin(&status.local_path, "gone").unwrap();
        let status = pin_status_at(scratch.0.join("root"), &scratch.0.join("git"));
        let eval = evaluate_pin(Some(&status), &file);
        assert_eq!(eval.stale.len(), 1);
        assert_eq!(eval.winner.map(|w| w.0), Some("repo"));
        assert_eq!(
            fs::read_to_string(&status.repo_path).unwrap(),
            "{\n  \"kind\": \"gentle-pi.agent_model_profile_pin\",\n  \"version\": 1,\n  \"profile\": \"team\"\n}\n"
        );
    }

    #[test]
    fn agent_names_and_clamping() {
        assert_eq!(
            parse_agent_name("---\nname: 'x-y'\n---"),
            Some("x-y".into())
        );
        assert_eq!(parse_agent_name("---\nname: a\"b\n---"), None);
        assert_eq!(
            parse_agent_name("---\nname: a\npackage: p\n---"),
            Some("p.a".into())
        );
        assert_eq!(clamp_level("high", &["off"]), "off");
        assert_eq!(clamp_level("minimal", &["off", "low", "high"]), "low");
        assert_eq!(
            parse_model_ref("openai/gpt/x"),
            Some(("openai".into(), "gpt/x".into()))
        );
        assert_eq!(parse_model_ref("/x"), None);
    }
}
