//! Engram's HTTP protocol, pure side: the observation model, decoding API
//! answers and percent-encoding query values. The app does the requests.

use serde::Deserialize;

/// Engram's `store.Observation` (internal/store/store.go), the fields shown here.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Observation {
    pub id: i64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub project: Option<String>,
    #[serde(default)]
    pub scope: String,
    pub topic_key: Option<String>,
    #[serde(default)]
    pub revision_count: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

pub fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn parse_observations(value: serde_json::Value) -> Vec<Observation> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, parse_observations};

    #[test]
    fn observations_and_search_results_parse() {
        // `/search` returns SearchResult = Observation + rank, flattened.
        let value = serde_json::json!([
            {"id": 7, "sync_id": "s", "session_id": "x", "type": "decision", "title": "Use GPUI", "content": "Because…",
             "project": "gentle-shell-gpui", "scope": "project", "topic_key": "arch/ui", "revision_count": 2,
             "duplicate_count": 0, "created_at": "2026-11-06T10:00:00Z", "updated_at": "2026-11-06T10:05:00Z", "rank": -1.5},
            {"unexpected": true}
        ]);
        let items = parse_observations(value);
        assert_eq!(items.len(), 1);
        assert_eq!(
            (
                items[0].id,
                items[0].kind.as_str(),
                items[0].topic_key.as_deref()
            ),
            (7, "decision", Some("arch/ui"))
        );
        assert!(parse_observations(serde_json::json!({"error": "x"})).is_empty());
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("/Users/a b/c&d"), "%2FUsers%2Fa%20b%2Fc%26d");
    }
}
