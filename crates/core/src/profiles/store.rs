//! The profiles store (gentle-pi `lib/agent-profiles.ts`): the
//! `profiles.json` format, profile names and the edits the screen makes.

use serde_json::{Map, Value, json};

use super::pretty;
use super::routing::{Routing, normalize_routing};

const PROFILES_KIND: &str = "gentle-pi.agent_model_profiles";

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::RouteEntry;

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
}
