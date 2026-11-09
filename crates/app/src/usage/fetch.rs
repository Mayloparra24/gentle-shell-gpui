//! Subscription usage requests: one HTTP GET per provider card.

use std::time::Duration;

use gentle_core::usage::subscription::{group_usage_limits, parse_codex_usage, parse_nan_quota};
use serde_json::Value;

use super::credentials::{CardState, Fetch, resolve_config_value};

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const NAN_QUOTA_URL: &str = "https://cloud-api.nan.builders/api/usage/quota";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // Never follow a redirect: the bearer token must not reach another origin.
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(HTTP_TIMEOUT))
        .user_agent("gentle-pi")
        .build()
        .into()
}

fn get_json(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(&str, &str)],
) -> Result<(u16, Option<Value>), String> {
    let mut request = agent.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request
        .call()
        .map_err(|error| format!("Network error: {error}"))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Ok((status, None));
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("Could not read the response: {error}"))?;
    let json = serde_json::from_str(&body).map_err(|_| "The response was not JSON.".to_string())?;
    Ok((status, Some(json)))
}

pub(super) fn run_fetch(fetch: Fetch, now: i64) -> CardState {
    let agent = http_agent();
    match fetch {
        Fetch::Codex { token, account_id } => {
            let bearer = format!("Bearer {token}");
            let headers = [
                ("Authorization", bearer.as_str()),
                ("chatgpt-account-id", account_id.as_str()),
                ("originator", "pi"),
            ];
            match get_json(&agent, CODEX_USAGE_URL, &headers) {
                Ok((_, Some(json))) => {
                    let usage = parse_codex_usage(&json, now);
                    if usage.limits.is_empty() {
                        CardState::Note(
                            "ChatGPT reported no rate-limit windows for this plan.".into(),
                        )
                    } else {
                        CardState::Ready(usage)
                    }
                }
                Ok((401 | 403, None)) => {
                    CardState::SignIn("ChatGPT rejected the stored token — sign in again.".into())
                }
                Ok((status, None)) => {
                    CardState::Failed(format!("Usage request failed (HTTP {status})."))
                }
                Err(error) => CardState::Failed(error),
            }
        }
        Fetch::Nan { key, env } => {
            let Some(key) = resolve_config_value(&key, &env) else {
                return CardState::Failed(
                    "The NaN API key could not be resolved (an environment variable it references is unset, or its command failed).".into(),
                );
            };
            let bearer = format!("Bearer {key}");
            let headers = [
                ("Authorization", bearer.as_str()),
                ("Accept", "application/json"),
                ("Cache-Control", "no-store"),
            ];
            match get_json(&agent, NAN_QUOTA_URL, &headers) {
                Ok((_, Some(json))) => {
                    let mut usage = parse_nan_quota(&json, now);
                    if usage.limits.is_empty() {
                        CardState::Failed("The quota response carried no metered models.".into())
                    } else {
                        usage.limits = group_usage_limits(&usage.limits);
                        CardState::Ready(usage)
                    }
                }
                Ok((401 | 403, None)) => CardState::SignIn(
                    "NaN rejected the API key — set it again in Providers.".into(),
                ),
                Ok((status @ 300..=399, None)) => CardState::Failed(format!(
                    "Refused a redirect from the quota endpoint (HTTP {status})."
                )),
                Ok((status, None)) => {
                    CardState::Failed(format!("Quota request failed (HTTP {status})."))
                }
                Err(error) => CardState::Failed(error),
            }
        }
    }
}
