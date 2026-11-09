//! The npm registry's `gentle-pi/latest`: one blocking request with the
//! launcher's timeout; only a stable `version` counts as an answer.

use std::time::Duration;

use gentle_core::version::stable_parts;
use serde_json::Value;

const REGISTRY: &str = "https://registry.npmjs.org/gentle-pi/latest";
const TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn fetch_latest() -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(REGISTRY)
        .header("Accept", "application/json")
        .call()
        .ok()?;
    if !(200..300).contains(&response.status().as_u16()) {
        return None;
    }
    let body: Value = serde_json::from_str(&response.body_mut().read_to_string().ok()?).ok()?;
    let version = body.get("version")?.as_str()?;
    stable_parts(version).map(|_| version.to_string())
}
