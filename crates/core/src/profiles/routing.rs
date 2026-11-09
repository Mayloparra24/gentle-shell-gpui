//! Routing model (gentle-pi `lib/model-routing-authority.ts`): routing
//! entries, their normalization, the model catalog entry and the thinking
//! levels.

use serde_json::{Map, Value, json};

pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
/// Reserved routing key: the orchestrator lives in Pi's `settings.json`, never
/// in `subagents.json` or an agent file.
pub const ORCHESTRATOR: &str = "orchestrator";
/// Routed by key only; they inherit Pi's persisted default (agents-config.ts).
pub(super) const PROVIDER_REVIEW_ROLES: [&str; 2] = ["review-refuter", "review-validator"];
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
    pub fn route_id(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }

    /// `supportsOpenAIFast`, minus the `api` check the catalog entry here lacks.
    pub fn supports_fast(&self) -> bool {
        FAST_MODELS.contains(&self.id.as_str())
            && matches!(self.provider.as_str(), "openai" | "openai-codex")
    }
}

pub fn is_thinking_level(value: &str) -> bool {
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

pub fn is_review_role(name: &str) -> bool {
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
    pub fn is_clear(&self) -> bool {
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
pub struct Routing(pub Vec<(String, RouteEntry)>);

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

    pub fn to_json(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(k, e)| (k.clone(), e.to_json()))
                .collect(),
        )
    }
}

/// `normalizeModelConfig`: unsafe keys and invalid entries are dropped.
pub fn normalize_routing(value: &Value) -> Option<Routing> {
    let map = value.as_object()?;
    Some(Routing(
        map.iter()
            .filter(|(name, _)| is_safe_agent_name(name))
            .filter_map(|(name, v)| normalize_entry(v).map(|e| (name.clone(), e)))
            .collect(),
    ))
}

/// `formatOrchestratorSelection`.
pub fn format_selection(entry: Option<&RouteEntry>) -> String {
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

/// `parseOrchestratorModelRef`: `provider/model`, both halves non-empty.
pub fn parse_model_ref(model: &str) -> Option<(String, String)> {
    let model = normalize_model_id(model)?;
    let (provider, id) = model.split_once('/')?;
    (!provider.is_empty() && !id.is_empty()).then(|| (provider.to_string(), id.to_string()))
}

/// `writeModelConfig`: what `models.json` holds, normalized.
pub fn normalized_json(config: &Routing) -> Value {
    normalize_routing(&config.to_json())
        .unwrap_or_default()
        .to_json()
}

/// `clampToSupportedLevel`.
pub fn clamp_level(level: &str, supported: &[&str]) -> String {
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
