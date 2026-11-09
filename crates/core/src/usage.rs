//! Usage: provider subscription windows (`/gentle:usage`) and local token
//! and cost totals (`/gentle:stats`).
//!
//! Parsing mirrors gentle-pi's `lib/shell-usage.ts` and `lib/stats-collector.ts`.
//! The app reads `auth.json`, calls the providers, walks the session files
//! and reads the clock; this module only parses and aggregates what they return.

use serde_json::Value;

pub mod stats;
pub mod subscription;
pub mod time;

fn finite(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|v| v.is_finite())
}
