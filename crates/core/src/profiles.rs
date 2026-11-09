//! Agent-model profiles: the pure half of the native `/gentle:profiles`
//! screen. Profiles are named routing snapshots in `profiles.json`; this
//! module normalizes, edits and serializes them, reads agent frontmatter and
//! repository pins, and words what the screen notes. Reading and writing the
//! files, discovering agents and applying a profile stay in the app.
//! Semantics follow gentle-pi `lib/agent-profiles.ts`,
//! `lib/model-routing-authority.ts` and `lib/agent-profile-pin.ts`.

use serde_json::{Map, Value};

mod agents;
mod pins;
mod routing;
mod store;

pub use agents::{
    AgentEntry, AgentSource, assignment_names, entry_from_model_profile, model_profile_for,
    parse_agent_name, read_frontmatter_routing, unmatched_notes, update_frontmatter_routing,
    with_omitted_cleared,
};
pub use pins::{PinEval, PinRead, PinStatus, evaluate_pin, parse_pin, pin_notes, serialize_pin};
pub use routing::{
    ModelChoice, ORCHESTRATOR, RouteEntry, Routing, THINKING_LEVELS, clamp_level, format_selection,
    is_review_role, is_thinking_level, normalize_entry, normalize_model_id, normalize_routing,
    normalized_json, parse_model_ref,
};
pub use store::{
    Drops, ProfilesFile, bootstrap_profiles, create_profile, delete_profile, duplicate_profile,
    is_valid_profile_name, parse_profiles, rename_profile, serialize_profiles, set_active,
    update_profile,
};

pub fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    text
}

/// Rebuild an object without `key`, keeping the order of the rest.
pub fn without_key(map: Map<String, Value>, key: &str) -> Map<String, Value> {
    map.into_iter().filter(|(k, _)| k != key).collect()
}
