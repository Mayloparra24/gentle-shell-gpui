//! Agents as routing sees them: discovered agent entries, their
//! frontmatter (`name:`, `model:`, `thinking:`) and `subagents.json` model
//! profiles, the assignment names the screen lists and the notes about keys
//! that match no agent.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::routing::{PROVIDER_REVIEW_ROLES, RouteEntry, Routing, is_review_role, normalize_entry};

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

/// `readFrontmatterRouting`.
pub fn read_frontmatter_routing(content: &str) -> Option<RouteEntry> {
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
pub fn entry_from_model_profile(value: &Value) -> Option<RouteEntry> {
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
pub fn model_profile_for(entry: &RouteEntry) -> Option<Value> {
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

/// `withOmittedAgentsClearedAsync`: a profile is a complete snapshot.
pub fn with_omitted_cleared(agents: &[AgentEntry], config: &Routing) -> Routing {
    let mut completed = config.clone();
    for agent in agents {
        if !is_review_role(&agent.name) && !completed.contains(&agent.name) {
            completed.set(&agent.name, RouteEntry::default());
        }
    }
    completed
}

/// Unmatched routing keys, as the screen notes them (`unmatchedRoutingNotes`).
pub fn unmatched_notes(config: &Routing, names: &[String]) -> Vec<String> {
    config
        .roles()
        .filter(|(key, _)| !names.contains(key))
        .map(|(key, _)| match names.iter().find(|n| n.ends_with(&format!(".{key}"))) {
            Some(suggestion) => format!("Profile key \"{key}\" matches no agent; did you mean {suggestion}?"),
            None => format!("Profile key \"{key}\" matches no agent configured in gentle-shell, so it is ignored at launch."),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{clamp_level, parse_model_ref};

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
