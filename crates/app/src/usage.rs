//! Usage: native `/gentle:usage` (provider subscription windows) and
//! `/gentle:stats` (local token and cost totals from session files).
//!
//! Parsing mirrors gentle-pi's `lib/shell-usage.ts` and `lib/stats-collector.ts`.
//! Tokens and API keys only ever travel from `auth.json` into the HTTP request
//! header; they are never stored in view state, logged, or rendered.

use std::collections::HashMap;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::file_cache::{FileCache, Stamp};
use serde_json::{Map, Value};

use crate::transcript::format_tokens;
use crate::ui;

const CODEX_PROVIDER: &str = "openai-codex";
const OPENAI_PROVIDER: &str = "openai";
const ANTHROPIC_PROVIDER: &str = "anthropic";
const NAN_PROVIDER: &str = "nan";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const NAN_QUOTA_URL: &str = "https://cloud-api.nan.builders/api/usage/quota";
const CODEX_MAIN_LIMIT: &str = "codex";
const CODEX_ACCOUNT_CLAIM: &str = "https://api.openai.com/auth";
const NAN_DEFAULT_WINDOW_TOKENS: f64 = 400_000_000.0;
const NAN_DEFAULT_WINDOW_HOURS: f64 = 4.0;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// Session files above this size are skipped rather than streamed.
const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;
const CHART_DAYS: i64 = 30;
const MINUTE: i64 = 60;
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;
const WEEK: i64 = 604_800;
const UNKNOWN: &str = "unknown";

pub enum UsageEvent {
    Close,
}

// ---------------------------------------------------------------------------
// Subscription usage model and parsers (lib/shell-usage.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    /// "5h", "week", … Empty for NaN's billing-period allowance.
    pub label: String,
    pub used_percent: f64,
    pub window_seconds: f64,
    /// Unix milliseconds.
    pub reset_at: Option<i64>,
    /// Raw allowance numbers; only NaN reports them.
    pub used: Option<f64>,
    pub budget: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimit {
    pub name: String,
    pub windows: Vec<UsageWindow>,
    pub limit_reached: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsage {
    pub provider: String,
    pub plan: Option<String>,
    pub limits: Vec<UsageLimit>,
    pub fetched_at: i64,
}

fn finite(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|v| v.is_finite())
}

pub fn window_label(seconds: i64) -> String {
    if seconds == WEEK {
        "week".into()
    } else if seconds >= DAY && seconds % DAY == 0 {
        format!("{}d", seconds / DAY)
    } else if seconds >= HOUR && seconds % HOUR == 0 {
        format!("{}h", seconds / HOUR)
    } else {
        format!("{}m", (seconds as f64 / MINUTE as f64).round() as i64)
    }
}

pub fn format_reset(reset_at: Option<i64>, now: i64) -> String {
    let Some(reset_at) = reset_at else {
        return String::new();
    };
    let seconds = (reset_at - now).div_euclid(1000);
    if seconds <= 0 {
        "resets now".into()
    } else if seconds < HOUR {
        format!(
            "resets in {}m",
            ((seconds as f64 / MINUTE as f64).round() as i64).max(1)
        )
    } else if seconds < DAY {
        format!(
            "resets in {}h {}m",
            seconds / HOUR,
            (seconds % HOUR) / MINUTE
        )
    } else {
        format!("resets in {}d {}h", seconds / DAY, (seconds % DAY) / HOUR)
    }
}

fn parse_codex_window(raw: Option<&Value>, now: i64) -> Option<UsageWindow> {
    let raw = raw?.as_object()?;
    let used = finite(raw.get("used_percent"))?;
    let seconds = finite(raw.get("limit_window_seconds")).filter(|s| *s > 0.0)?;
    let reset_at = match (
        raw.get("reset_at").and_then(Value::as_f64),
        raw.get("reset_after_seconds").and_then(Value::as_f64),
    ) {
        (Some(at), _) => Some((at * 1000.0) as i64),
        (None, Some(after)) => Some(now + (after * 1000.0) as i64),
        _ => None,
    };
    Some(UsageWindow {
        label: window_label(seconds.round() as i64),
        used_percent: used,
        window_seconds: seconds,
        reset_at,
        used: None,
        budget: None,
    })
}

fn parse_codex_rate_limit(name: &str, raw: Option<&Value>, now: i64) -> Option<UsageLimit> {
    let raw = raw?.as_object()?;
    let windows: Vec<UsageWindow> = [raw.get("primary_window"), raw.get("secondary_window")]
        .into_iter()
        .filter_map(|window| parse_codex_window(window, now))
        .collect();
    if windows.is_empty() {
        return None;
    }
    Some(UsageLimit {
        name: name.to_string(),
        windows,
        limit_reached: raw.get("limit_reached") == Some(&Value::Bool(true)),
    })
}

/// The `wham/usage` payload: the main Codex limit plus any additional ones.
pub fn parse_codex_usage(payload: &Value, now: i64) -> ProviderUsage {
    let mut limits = Vec::new();
    if let Some(main) = parse_codex_rate_limit(CODEX_MAIN_LIMIT, payload.get("rate_limit"), now) {
        limits.push(main);
    }
    for extra in payload
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = extra
            .get("limit_name")
            .and_then(Value::as_str)
            .unwrap_or("limit");
        if let Some(limit) = parse_codex_rate_limit(name, extra.get("rate_limit"), now) {
            limits.push(limit);
        }
    }
    ProviderUsage {
        provider: CODEX_PROVIDER.into(),
        plan: payload
            .get("plan_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        limits,
        fetched_at: now,
    }
}

fn quota_timestamp(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| (v * 1000.0) as i64),
        Value::String(s) => parse_iso_ms(s),
        _ => None,
    }
}

fn quota_number(value: Option<&Value>, positive: bool) -> Option<f64> {
    let v = finite(value)?;
    if positive {
        (v > 0.0).then_some(v)
    } else {
        (v >= 0.0).then_some(v)
    }
}

fn nan_rolling_window(raw: &Map<String, Value>) -> Option<UsageWindow> {
    let used = quota_number(raw.get("windowTokensUsed"), false)?;
    let budget = quota_number(raw.get("fullWindowTokens"), true)
        .or_else(|| quota_number(raw.get("windowTokens"), true))
        .unwrap_or(NAN_DEFAULT_WINDOW_TOKENS);
    let hours = quota_number(raw.get("windowHours"), true).unwrap_or(NAN_DEFAULT_WINDOW_HOURS);
    let seconds = hours * HOUR as f64;
    Some(UsageWindow {
        label: window_label(seconds.round() as i64),
        used_percent: used / budget * 100.0,
        window_seconds: seconds,
        reset_at: quota_timestamp(raw.get("windowResetsAt")),
        used: None,
        budget: None,
    })
}

/// NaN Cloud's quota payload: one billing-period allowance per metered model
/// plus its rolling window. A metered model whose usage is unreadable makes
/// the whole read empty (schema drift), exactly like the source.
pub fn parse_nan_quota(payload: &Value, now: i64) -> ProviderUsage {
    let empty = |limits| ProviderUsage {
        provider: NAN_PROVIDER.into(),
        plan: None,
        limits,
        fetched_at: now,
    };
    let fallback_reset = quota_timestamp(payload.get("periodEnd"));
    let mut limits = Vec::new();
    for entry in payload
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(model) = entry.as_object() else {
            continue;
        };
        let Some(name) = model
            .get("model")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(allowance) = quota_number(model.get("fullCap"), true)
            .or_else(|| quota_number(model.get("cap"), true))
        else {
            continue;
        };
        let Some(tokens_used) = quota_number(model.get("tokensUsed"), false) else {
            return empty(Vec::new());
        };
        let reset_at = quota_timestamp(model.get("periodEnd")).or(fallback_reset);
        let mut windows = vec![UsageWindow {
            label: String::new(),
            used_percent: tokens_used / allowance * 100.0,
            window_seconds: reset_at
                .map_or(0.0, |at| ((at - now) as f64 / 1000.0).round().max(0.0)),
            reset_at,
            used: Some(tokens_used),
            budget: Some(allowance),
        }];
        windows.extend(nan_rolling_window(model));
        limits.push(UsageLimit {
            name: name.to_string(),
            windows,
            limit_reached: tokens_used >= allowance,
        });
    }
    empty(limits)
}

fn raw_allowance(limit: &UsageLimit) -> Option<(f64, f64)> {
    let first = limit.windows.first()?;
    match (first.used, first.budget) {
        (Some(used), Some(budget)) if budget > 0.0 => Some((used, budget)),
        _ => None,
    }
}

/// The leading alphabetic run of a model id: `glm5.3-flash` → `glm`.
pub fn model_family(model_id: &str) -> String {
    let run: String = model_id
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if run.is_empty() {
        model_id.to_lowercase()
    } else {
        run.to_lowercase()
    }
}

/// NaN rows: families together, families by Σused/Σbudget, members most used
/// first. Providers without raw allowances keep their payload order.
pub fn group_usage_limits(limits: &[UsageLimit]) -> Vec<UsageLimit> {
    if limits.is_empty() || !limits.iter().all(|l| raw_allowance(l).is_some()) {
        return limits.to_vec();
    }
    let percent = |l: &UsageLimit| l.windows.first().map_or(0.0, |w| w.used_percent);
    let mut families: Vec<(String, Vec<UsageLimit>)> = Vec::new();
    for limit in limits {
        let family = model_family(&limit.name);
        match families.iter_mut().find(|(f, _)| *f == family) {
            Some((_, members)) => members.push(limit.clone()),
            None => families.push((family, vec![limit.clone()])),
        }
    }
    let mut ranked: Vec<(f64, Vec<UsageLimit>)> = families
        .into_iter()
        .map(|(_, mut members)| {
            members.sort_by(|a, b| percent(b).total_cmp(&percent(a)));
            let (used, budget) = members
                .iter()
                .filter_map(raw_allowance)
                .fold((0.0, 0.0), |(u, b), (mu, mb)| (u + mu, b + mb));
            let share = if budget > 0.0 {
                used / budget * 100.0
            } else {
                percent(&members[0])
            };
            (share, members)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked
        .into_iter()
        .flat_map(|(_, members)| members)
        .collect()
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The ChatGPT account id claim inside a Codex access token.
pub fn account_id_from_token(token: &str) -> Option<String> {
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let claims: Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    claims
        .get(CODEX_ACCOUNT_CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Credentials (auth.json) → what each provider card can show
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum CardState {
    Loading,
    Ready(ProviderUsage),
    /// Informational, nothing to fetch.
    Note(String),
    /// The stored credential cannot be used until the user signs in again.
    SignIn(String),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
struct ProviderCard {
    id: String,
    title: &'static str,
    state: CardState,
}

/// A pending request. Holds a secret, so it deliberately has no `Debug`.
enum Fetch {
    Codex {
        token: String,
        account_id: String,
    },
    Nan {
        key: String,
        env: HashMap<String, String>,
    },
}

struct Discovery {
    cards: Vec<(ProviderCard, Option<Fetch>)>,
    /// Signed-in providers that expose no usage endpoint.
    others: Vec<String>,
}

fn provider_title(id: &str) -> &'static str {
    match id {
        CODEX_PROVIDER => "ChatGPT · Codex",
        OPENAI_PROVIDER => "OpenAI · ChatGPT",
        ANTHROPIC_PROVIDER => "Anthropic · Claude",
        NAN_PROVIDER => "NaN Cloud",
        _ => "Provider",
    }
}

fn discover(auth: &Map<String, Value>, now: i64) -> Discovery {
    let mut cards = Vec::new();
    let mut card = |id: &str, state: CardState, fetch: Option<Fetch>| {
        cards.push((
            ProviderCard {
                id: id.to_string(),
                title: provider_title(id),
                state,
            },
            fetch,
        ));
    };
    for id in [CODEX_PROVIDER, OPENAI_PROVIDER] {
        let Some(cred) = auth.get(id).and_then(Value::as_object) else {
            continue;
        };
        if cred.get("type").and_then(Value::as_str) != Some("oauth") {
            if id == CODEX_PROVIDER {
                card(
                    id,
                    CardState::Note(
                        "API key credential — subscription usage needs a ChatGPT sign-in.".into(),
                    ),
                    None,
                );
            }
            continue;
        }
        let expires = cred.get("expires").and_then(Value::as_f64);
        let Some(token) = cred
            .get("access")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        else {
            card(
                id,
                CardState::SignIn("No access token stored — sign in again.".into()),
                None,
            );
            continue;
        };
        if expires.is_some_and(|at| (at as i64) < now) {
            card(
                id,
                CardState::SignIn("The ChatGPT token expired — sign in again (a chat turn with this provider also refreshes it).".into()),
                None,
            );
            continue;
        }
        let account_id = cred
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| account_id_from_token(token));
        match account_id {
            Some(account_id) => card(
                id,
                CardState::Loading,
                Some(Fetch::Codex {
                    token: token.to_string(),
                    account_id,
                }),
            ),
            None => card(
                id,
                CardState::SignIn(
                    "The token carries no ChatGPT account id — sign in again.".into(),
                ),
                None,
            ),
        }
    }
    if auth.contains_key(ANTHROPIC_PROVIDER) {
        card(
            ANTHROPIC_PROVIDER,
            CardState::Note(
                "Claude reports its 5h and weekly windows only in response headers, so they are available while chatting in the terminal UI."
                    .into(),
            ),
            None,
        );
    }
    if let Some(cred) = auth.get(NAN_PROVIDER).and_then(Value::as_object) {
        let key = cred
            .get("key")
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty());
        match (cred.get("type").and_then(Value::as_str), key) {
            (Some("api_key"), Some(key)) => {
                let env = cred
                    .get("env")
                    .and_then(Value::as_object)
                    .map(|env| {
                        env.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                card(
                    NAN_PROVIDER,
                    CardState::Loading,
                    Some(Fetch::Nan {
                        key: key.to_string(),
                        env,
                    }),
                );
            }
            _ => card(
                NAN_PROVIDER,
                CardState::Failed("No NaN API key stored.".into()),
                None,
            ),
        }
    }
    let mut others: Vec<String> = auth
        .keys()
        .filter(|id| {
            ![
                CODEX_PROVIDER,
                OPENAI_PROVIDER,
                ANTHROPIC_PROVIDER,
                NAN_PROVIDER,
            ]
            .contains(&id.as_str())
        })
        .cloned()
        .collect();
    if let Some(cred) = auth.get(OPENAI_PROVIDER)
        && cred.get("type").and_then(Value::as_str) != Some("oauth")
    {
        others.push(OPENAI_PROVIDER.into());
    }
    others.sort();
    Discovery { cards, others }
}

fn is_env_name_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_env_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && is_env_name_start(bytes[0])
        && bytes
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

/// Pi's config-value template: `$VAR`/`${VAR}` interpolate (stored env map
/// first, then the process env), `$$` and `$!` escape. `None` when a
/// referenced variable is missing.
fn resolve_template(config: &str, env: &HashMap<String, String>) -> Option<String> {
    let lookup = |name: &str| {
        env.get(name)
            .filter(|v| !v.is_empty())
            .cloned()
            .or_else(|| std::env::var(name).ok().filter(|v| !v.is_empty()))
    };
    let mut out = String::new();
    let mut index = 0;
    while let Some(offset) = config[index..].find('$') {
        let dollar = index + offset;
        out.push_str(&config[index..dollar]);
        let rest = &config[dollar + 1..];
        match rest.as_bytes().first() {
            Some(c @ (b'$' | b'!')) => {
                out.push(*c as char);
                index = dollar + 2;
            }
            Some(b'{') => match rest.find('}') {
                None => {
                    out.push('$');
                    index = dollar + 1;
                }
                Some(end) => {
                    let name = &rest[1..end];
                    if is_env_name(name) {
                        out.push_str(&lookup(name)?);
                    } else {
                        out.push_str(&config[dollar..=dollar + 1 + end]);
                    }
                    index = dollar + 2 + end;
                }
            },
            Some(c) if is_env_name_start(*c) => {
                let len = rest
                    .bytes()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                    .count();
                out.push_str(&lookup(&rest[..len])?);
                index = dollar + 1 + len;
            }
            _ => {
                out.push('$');
                index = dollar + 1;
            }
        }
    }
    out.push_str(&config[index..]);
    Some(out)
}

/// `!command` runs through the shell (stdout, trimmed); anything else is a template.
fn resolve_config_value(config: &str, env: &HashMap<String, String>) -> Option<String> {
    let Some(command) = config.strip_prefix('!') else {
        return resolve_template(config, env).filter(|v| !v.is_empty());
    };
    use std::process::{Command, Stdio};
    #[cfg(windows)]
    let mut cmd = Command::new("cmd");
    #[cfg(windows)]
    cmd.args(["/C", command]);
    #[cfg(not(windows))]
    let mut cmd = Command::new("sh");
    #[cfg(not(windows))]
    cmd.args(["-c", command]);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < COMMAND_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|v| !v.is_empty())
}

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

fn run_fetch(fetch: Fetch, now: i64) -> CardState {
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

fn read_auth(path: &Path) -> Result<Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Map::new()),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Ok(map),
            _ => Err(format!("{} is not a JSON object.", path.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(format!("Could not read {}: {error}", path.display())),
    }
}

// ---------------------------------------------------------------------------
// Time helpers (no date crate: ISO-8601 UTC strings and civil-day math)
// ---------------------------------------------------------------------------

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if month <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        month,
        day,
    )
}

/// `YYYY-MM-DD[THH:MM[:SS[.fff]]][Z|±HH:MM|±HHMM]` → Unix milliseconds.
/// A missing offset is read as UTC (Pi always writes `toISOString()`).
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    let s = text.trim().as_bytes();
    let digits = |from: usize, len: usize| -> Option<i64> {
        let slice = s.get(from..from + len)?;
        slice
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| slice.iter().fold(0i64, |n, d| n * 10 + (d - b'0') as i64))
    };
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    if s.get(4) != Some(&b'-')
        || s.get(7) != Some(&b'-')
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    let mut ms = days_from_civil(year, month, day) * DAY * 1000;
    if s.len() == 10 {
        return Some(ms);
    }
    if !matches!(s.get(10), Some(b'T' | b't' | b' ')) || s.get(13) != Some(&b':') {
        return None;
    }
    let (hour, minute) = (digits(11, 2)?, digits(14, 2)?);
    let mut at = 16;
    let mut second = 0;
    if s.get(at) == Some(&b':') {
        second = digits(17, 2)?;
        at = 19;
    }
    let mut frac_ms = 0i64;
    if s.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while s.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
        let padded: String = std::str::from_utf8(&s[start..at])
            .ok()?
            .chars()
            .chain("000".chars())
            .take(3)
            .collect();
        frac_ms = padded.parse().ok()?;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    ms += ((hour * 60 + minute) * 60 + second) * 1000 + frac_ms;
    let offset_s = match s.get(at) {
        None => 0,
        Some(b'Z' | b'z') if at + 1 == s.len() => 0,
        Some(sign @ (b'+' | b'-')) => {
            let h = digits(at + 1, 2)?;
            let (m, end) = if s.get(at + 3) == Some(&b':') {
                (digits(at + 4, 2)?, at + 6)
            } else {
                (digits(at + 3, 2)?, at + 5)
            };
            if end != s.len() {
                return None;
            }
            let total = (h * 60 + m) * 60;
            if *sign == b'+' { total } else { -total }
        }
        _ => return None,
    };
    Some(ms - offset_s * 1000)
}

/// The local UTC offset in seconds right now (DST drift across a 30-day
/// chart is ignored). Falls back to UTC when it cannot be determined.
fn local_offset_secs() -> i64 {
    #[cfg(not(windows))]
    {
        let output = std::process::Command::new("date").arg("+%z").output();
        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout);
            let t = text.trim().as_bytes();
            if t.len() == 5 && matches!(t[0], b'+' | b'-') && t[1..].iter().all(u8::is_ascii_digit)
            {
                let n = |i: usize| (t[i] - b'0') as i64;
                let secs = ((n(1) * 10 + n(2)) * 60 + n(3) * 10 + n(4)) * 60;
                return if t[0] == b'-' { -secs } else { secs };
            }
        }
    }
    0
}

fn local_day(ms: i64, offset_secs: i64) -> i64 {
    (ms.div_euclid(1000) + offset_secs).div_euclid(DAY)
}

fn day_label(day: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, month, day) = civil_from_days(day);
    format!("{} {day}", MONTHS[(month - 1) as usize])
}

// ---------------------------------------------------------------------------
// Local stats (lib/stats-collector.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub timestamp: i64,
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    /// The session's working directory from its header; empty when absent.
    pub cwd: String,
    /// The session directory name under `sessions/`.
    pub dir: String,
    pub started_at: Option<i64>,
    pub messages: Vec<UsageRecord>,
}

#[derive(Debug, Default)]
pub struct StatsData {
    pub sessions: Vec<SessionRecord>,
    pub files: usize,
    pub skipped_large: Vec<(PathBuf, u64)>,
    pub unreadable: usize,
}

fn count(value: Option<&Value>) -> u64 {
    finite(value).filter(|v| *v >= 0.0).map_or(0, |v| v as u64)
}

fn text_or(value: Option<&Value>, fallback: &str) -> String {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// One assistant usage record from a parsed session entry.
pub fn usage_record(entry: &Value) -> Option<UsageRecord> {
    if entry.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let message = entry.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage = message.get("usage")?.as_object()?;
    let timestamp = parse_iso_ms(entry.get("timestamp")?.as_str()?)?;
    let (input, output) = (count(usage.get("input")), count(usage.get("output")));
    let (cache_read, cache_write) = (
        count(usage.get("cacheRead")),
        count(usage.get("cacheWrite")),
    );
    let total = finite(usage.get("totalTokens"))
        .filter(|v| *v >= 0.0)
        .map_or(input + output + cache_read + cache_write, |v| v as u64);
    let cost = finite(usage.get("cost").and_then(|c| c.get("total")))
        .filter(|v| *v >= 0.0)
        .unwrap_or(0.0);
    Some(UsageRecord {
        timestamp,
        provider: text_or(message.get("provider"), UNKNOWN),
        model: text_or(message.get("model"), UNKNOWN),
        input,
        output,
        cache_read,
        cache_write,
        total,
        cost,
    })
}

/// Streams one session file: the header line, then only lines that mention
/// `"assistant"`. `None` for a file without a session header.
fn read_session_file(path: &Path, dir: &str) -> std::io::Result<Option<SessionRecord>> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut line = Vec::new();
    let mut record: Option<SessionRecord> = None;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let Some(session) = record.as_mut() else {
            if text.trim().is_empty() {
                continue;
            }
            let header: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if header.get("type").and_then(Value::as_str) != Some("session") {
                return Ok(None);
            }
            record = Some(SessionRecord {
                id: text_or(header.get("id"), UNKNOWN),
                cwd: text_or(header.get("cwd"), ""),
                dir: dir.to_string(),
                started_at: header
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_iso_ms),
                messages: Vec::new(),
            });
            continue;
        };
        if !text.contains("\"assistant\"") {
            continue;
        }
        if let Some(usage) = serde_json::from_str::<Value>(&text)
            .ok()
            .as_ref()
            .and_then(usage_record)
        {
            session.messages.push(usage);
        }
    }
    if let Some(record) = record.as_mut()
        && record.started_at.is_none()
    {
        record.started_at = record.messages.first().map(|m| m.timestamp);
    }
    Ok(record)
}

fn last_usage_at(record: &SessionRecord) -> i64 {
    record
        .messages
        .iter()
        .map(|m| m.timestamp)
        .max()
        .unwrap_or(i64::MIN)
}

/// Parsed session files, so reopening Stats only reads new or changed ones.
static SESSION_RECORDS: FileCache<Option<SessionRecord>> = FileCache::new(16_384);

/// Top-level session files only (`sessions/<dir>/*.jsonl`; nested subagent
/// runs live deeper and are excluded, like Pi's `listSessionFiles`). Copies
/// of one session id keep the richer one.
pub fn load_sessions(root: &Path, max_bytes: u64) -> StatsData {
    load_sessions_in(&SESSION_RECORDS, root, max_bytes)
}

fn load_sessions_in(
    cache: &FileCache<Option<SessionRecord>>,
    root: &Path,
    max_bytes: u64,
) -> StatsData {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.file_name().to_string_lossy().into_owned();
        for child in std::fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = child.path();
            if child.file_type().is_ok_and(|t| t.is_file())
                && path.extension().is_some_and(|e| e == "jsonl")
            {
                files.push((path, dir.clone()));
            }
        }
    }
    files.sort();
    let mut data = StatsData {
        files: files.len(),
        ..StatsData::default()
    };
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (path, dir) in files {
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                data.unreadable += 1;
                continue;
            }
        };
        let size = meta.len();
        if size > max_bytes {
            data.skipped_large.push((path, size));
            continue;
        }
        let stamp = Stamp::from_metadata(&meta);
        let read = match cache.get(&path, stamp) {
            Some(cached) => Ok(cached),
            // Read errors are not cached; the next load tries again.
            None => read_session_file(&path, &dir)
                .inspect(|record| cache.insert(&path, stamp, record.clone())),
        };
        let record = match read {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(_) => {
                data.unreadable += 1;
                continue;
            }
        };
        if record.id == UNKNOWN {
            data.sessions.push(record);
            continue;
        }
        match by_id.get(&record.id) {
            Some(&index) => {
                let kept = &data.sessions[index];
                let replace = if record.messages.len() != kept.messages.len() {
                    record.messages.len() > kept.messages.len()
                } else {
                    last_usage_at(&record) > last_usage_at(kept)
                };
                if replace {
                    data.sessions[index] = record;
                }
            }
            None => {
                by_id.insert(record.id.clone(), data.sessions.len());
                data.sessions.push(record);
            }
        }
    }
    data
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsRange {
    All,
    Month,
    Week,
}

impl StatsRange {
    fn days(self) -> Option<i64> {
        match self {
            Self::All => None,
            Self::Month => Some(30),
            Self::Week => Some(7),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub cost: f64,
}

impl TokenTotals {
    fn add(&mut self, record: &UsageRecord) {
        self.input += record.input;
        self.output += record.output;
        self.cache_read += record.cache_read;
        self.cache_write += record.cache_write;
        self.total += record.total;
        self.cost += record.cost;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelStats {
    pub model: String,
    pub provider: String,
    pub tokens: u64,
    pub cost: f64,
    pub messages: usize,
    pub share: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectStats {
    /// The session cwd, or the session directory name when the header has none.
    pub project: String,
    pub sessions: usize,
    pub messages: usize,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayStats {
    /// Local days since the Unix epoch.
    pub day: i64,
    pub tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsSummary {
    pub totals: TokenTotals,
    pub messages: usize,
    pub sessions: usize,
    pub active_days: usize,
    pub models: Vec<ModelStats>,
    pub projects: Vec<ProjectStats>,
    /// The last 30 local days, oldest first, regardless of the range.
    pub daily: Vec<DayStats>,
}

pub fn aggregate_stats(
    sessions: &[SessionRecord],
    range: StatsRange,
    now: i64,
    offset_secs: i64,
) -> StatsSummary {
    let today = local_day(now, offset_secs);
    let start = range.days().map(|days| today - (days - 1));
    let chart_start = today - (CHART_DAYS - 1);
    let mut totals = TokenTotals::default();
    let mut messages = 0;
    let mut session_count = 0;
    let mut active = std::collections::BTreeSet::new();
    let mut models: HashMap<&str, ModelStats> = HashMap::new();
    let mut projects: HashMap<&str, ProjectStats> = HashMap::new();
    let mut daily: Vec<DayStats> = (chart_start..=today)
        .map(|day| DayStats {
            day,
            tokens: 0,
            cost: 0.0,
        })
        .collect();
    for session in sessions {
        let project_key = if session.cwd.is_empty() {
            session.dir.as_str()
        } else {
            session.cwd.as_str()
        };
        let mut counted = false;
        for message in &session.messages {
            let day = local_day(message.timestamp, offset_secs);
            if (chart_start..=today).contains(&day) {
                let slot = &mut daily[(day - chart_start) as usize];
                slot.tokens += message.total;
                slot.cost += message.cost;
            }
            if start.is_some_and(|start| day < start) {
                continue;
            }
            counted = true;
            messages += 1;
            active.insert(day);
            totals.add(message);
            let model = models
                .entry(message.model.as_str())
                .or_insert_with(|| ModelStats {
                    model: message.model.clone(),
                    provider: message.provider.clone(),
                    tokens: 0,
                    cost: 0.0,
                    messages: 0,
                    share: 0.0,
                });
            model.tokens += message.total;
            model.cost += message.cost;
            model.messages += 1;
            let project = projects.entry(project_key).or_insert_with(|| ProjectStats {
                project: project_key.to_string(),
                sessions: 0,
                messages: 0,
                tokens: 0,
                cost: 0.0,
            });
            project.messages += 1;
            project.tokens += message.total;
            project.cost += message.cost;
        }
        if counted {
            session_count += 1;
            if let Some(project) = projects.get_mut(project_key) {
                project.sessions += 1;
            }
        }
    }
    let mut models: Vec<ModelStats> = models
        .into_values()
        .map(|mut model| {
            model.share = if totals.total > 0 {
                model.tokens as f64 / totals.total as f64
            } else {
                0.0
            };
            model
        })
        .collect();
    models.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then(b.messages.cmp(&a.messages))
            .then_with(|| a.model.cmp(&b.model))
    });
    let mut projects: Vec<ProjectStats> = projects.into_values().collect();
    projects.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then(b.messages.cmp(&a.messages))
            .then_with(|| a.project.cmp(&b.project))
    });
    StatsSummary {
        totals,
        messages,
        sessions: session_count,
        active_days: active.len(),
        models,
        projects,
        daily,
    }
}

pub fn format_cost(cost: f64) -> String {
    if cost <= 0.0 {
        "$0".into()
    } else if cost < 0.01 {
        format!("${cost:.4}")
    } else {
        format!("${cost:.2}")
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Usage,
    Stats,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Tokens,
    Cost,
}

enum UsageState {
    Loading,
    Failed(String),
    Loaded {
        cards: Vec<ProviderCard>,
        others: Vec<String>,
    },
}

enum StatsState {
    Idle,
    Loading,
    Loaded {
        data: Arc<StatsData>,
        offset: i64,
        summary: StatsSummary,
    },
}

pub struct UsageView {
    tab: Tab,
    usage: UsageState,
    stats: StatsState,
    range: StatsRange,
    metric: Metric,
    _usage_task: Task<()>,
    _stats_task: Task<()>,
}

impl EventEmitter<UsageEvent> for UsageView {}

/// Meter fill by how much of the window is spent.
fn bar_color(percent: f64) -> Hsla {
    if percent >= 95.0 {
        ui::danger()
    } else if percent >= 80.0 {
        ui::warning()
    } else if percent >= 50.0 {
        ui::accent()
    } else {
        ui::success()
    }
}

/// Segmented toggle frame; children are small ghost buttons, the selected one
/// `.selected(true)`.
fn segmented() -> Div {
    h_flex()
        .flex_shrink_0()
        .p(px(2.))
        .gap(px(2.))
        .rounded_lg()
        .bg(ui::well())
        .border_1()
        .border_color(ui::border())
}

/// A colored note line with an optional leading icon.
fn status_line(color: Hsla, icon: Option<IconName>, text: impl Into<SharedString>) -> Div {
    h_flex()
        .gap_2()
        .min_w_0()
        .text_xs()
        .text_color(color)
        .when_some(icon, |d, icon| {
            d.child(Icon::new(icon).xsmall().flex_shrink_0())
        })
        .child(div().min_w_0().child(text.into()))
}

fn updated_ago(fetched_at: i64, now: i64) -> String {
    let minutes = (now - fetched_at) / 60_000;
    if minutes < 1 {
        "updated just now".into()
    } else {
        format!("updated {minutes}m ago")
    }
}

impl UsageView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            tab: Tab::Usage,
            usage: UsageState::Loading,
            stats: StatsState::Idle,
            range: StatsRange::All,
            metric: Metric::Tokens,
            _usage_task: Task::ready(()),
            _stats_task: Task::ready(()),
        };
        this.refresh_usage(cx);
        this
    }

    /// Replacing the task cancels an in-flight refresh, so stale results never land.
    fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        self.usage = UsageState::Loading;
        cx.notify();
        let auth_path = pi_rpc::resolve_homes().agent.join("auth.json");
        self._usage_task = cx.spawn(async move |this, cx| {
            let now = now_ms();
            let discovery = cx
                .background_executor()
                .spawn(async move { read_auth(&auth_path).map(|auth| discover(&auth, now)) })
                .await;
            let discovery = match discovery {
                Ok(discovery) => discovery,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.usage = UsageState::Failed(error);
                        cx.notify();
                    });
                    return;
                }
            };
            let mut pending = Vec::new();
            let mut cards = Vec::new();
            for (card, fetch) in discovery.cards {
                if let Some(fetch) = fetch {
                    let id = card.id.clone();
                    pending.push((
                        id,
                        cx.background_executor()
                            .spawn(async move { run_fetch(fetch, now) }),
                    ));
                }
                cards.push(card);
            }
            if this
                .update(cx, |this, cx| {
                    this.usage = UsageState::Loaded {
                        cards,
                        others: discovery.others,
                    };
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            for (id, task) in pending {
                let state = task.await;
                let updated = this.update(cx, |this, cx| {
                    if let UsageState::Loaded { cards, .. } = &mut this.usage
                        && let Some(card) = cards.iter_mut().find(|c| c.id == id)
                    {
                        card.state = state;
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    return;
                }
            }
        });
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        self.stats = StatsState::Loading;
        cx.notify();
        let root = pi_rpc::resolve_homes().agent.join("sessions");
        let range = self.range;
        self._stats_task = cx.spawn(async move |this, cx| {
            let (data, offset, summary) = cx
                .background_executor()
                .spawn(async move {
                    let offset = local_offset_secs();
                    let data = load_sessions(&root, MAX_SESSION_BYTES);
                    let summary = aggregate_stats(&data.sessions, range, now_ms(), offset);
                    (Arc::new(data), offset, summary)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // The range may have changed while loading; re-aggregate if so.
                let summary = if this.range == range {
                    summary
                } else {
                    aggregate_stats(&data.sessions, this.range, now_ms(), offset)
                };
                this.stats = StatsState::Loaded {
                    data,
                    offset,
                    summary,
                };
                cx.notify();
            });
        });
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Stats && matches!(self.stats, StatsState::Idle) {
            self.refresh_stats(cx);
        }
        cx.notify();
    }

    fn set_range(&mut self, range: StatsRange, cx: &mut Context<Self>) {
        self.range = range;
        if let StatsState::Loaded {
            data,
            offset,
            summary,
        } = &mut self.stats
        {
            *summary = aggregate_stats(&data.sessions, range, now_ms(), *offset);
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Usage => self.refresh_usage(cx),
            Tab::Stats => self.refresh_stats(cx),
        }
    }

    fn render_usage(&self) -> AnyElement {
        let (cards, others) = match &self.usage {
            UsageState::Loading => {
                return h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(Spinner::new().small().color(ui::muted()))
                    .child("Reading credentials…")
                    .into_any_element();
            }
            UsageState::Failed(error) => {
                return ui::card()
                    .p_4()
                    .gap_2()
                    .border_color(ui::danger().opacity(0.35))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .small()
                                    .text_color(ui::danger()),
                            )
                            .child(ui::section_label("Could not read credentials")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui::danger())
                            .child(error.clone()),
                    )
                    .into_any_element();
            }
            UsageState::Loaded { cards, others } => (cards, others),
        };
        let now = now_ms();
        v_flex()
            .gap_3()
            .child(ui::section_label("Subscriptions"))
            .when(cards.is_empty(), |d| {
                d.child(
                    ui::card().p_4().child(
                        div()
                            .text_sm()
                            .text_color(ui::muted())
                            .child("No subscription provider is signed in. Sign in to ChatGPT (Codex), Claude or NaN from Providers."),
                    ),
                )
            })
            .children(
                cards
                    .iter()
                    .enumerate()
                    .map(|(i, card)| ui::appear(("usage-card", i), Self::render_card(card, now))),
            )
            .when(!others.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(ui::faint())
                        .child(format!("No subscription usage for: {}", others.join(", "))),
                )
            })
            .into_any_element()
    }

    fn render_card(card: &ProviderCard, now: i64) -> Div {
        let (plan, fetched_at) = match &card.state {
            CardState::Ready(usage) => (usage.plan.clone(), Some(usage.fetched_at)),
            _ => (None, None),
        };
        let status = match &card.state {
            CardState::Loading => Some(ui::chip("fetching", ui::info())),
            CardState::SignIn(_) => Some(ui::chip("signed out", ui::warning())),
            CardState::Failed(_) => Some(ui::chip("error", ui::danger())),
            CardState::Note(_) | CardState::Ready(_) => None,
        };
        let body: AnyElement = match &card.state {
            CardState::Loading => h_flex()
                .gap_2()
                .text_xs()
                .text_color(ui::muted())
                .child(Spinner::new().xsmall().color(ui::muted()))
                .child("Fetching usage…")
                .into_any_element(),
            CardState::Note(note) => {
                status_line(ui::muted(), None, note.clone()).into_any_element()
            }
            CardState::SignIn(note) => {
                status_line(ui::warning(), Some(IconName::LogIn), note.clone()).into_any_element()
            }
            CardState::Failed(error) => {
                status_line(ui::danger(), Some(IconName::CircleAlert), error.clone())
                    .into_any_element()
            }
            CardState::Ready(usage) => v_flex()
                .gap_3()
                .children(
                    usage
                        .limits
                        .iter()
                        .map(|limit| Self::render_limit(limit, now)),
                )
                .into_any_element(),
        };
        ui::card()
            .gap_3()
            .p_4()
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::champagne())
                            .child(card.title),
                    )
                    .when_some(plan, |d, plan| {
                        d.child(ui::chip(plan, ui::accent()).flex_shrink_0())
                    })
                    .children(status)
                    .child(div().flex_1())
                    .when_some(fetched_at, |d, at| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::faint())
                                .child(updated_ago(at, now)),
                        )
                    }),
            )
            .child(body)
    }

    fn render_limit(limit: &UsageLimit, now: i64) -> AnyElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui::muted())
                            .child(limit.name.clone()),
                    )
                    .when(limit.limit_reached, |d| {
                        d.child(ui::chip("limit reached", ui::danger()).flex_shrink_0())
                    }),
            )
            .children(limit.windows.iter().map(|window| {
                let percent = window.used_percent.max(0.0);
                let color = bar_color(percent);
                let label = if window.label.is_empty() {
                    "period".to_string()
                } else {
                    window.label.clone()
                };
                h_flex()
                    .gap_3()
                    .min_w_0()
                    .child(
                        div()
                            .w(px(52.))
                            .flex_shrink_0()
                            .truncate()
                            .text_xs()
                            .text_color(ui::muted())
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(60.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(ui::well())
                            .border_1()
                            .border_color(ui::border())
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((percent / 100.0).min(1.0) as f32))
                                    .rounded_full()
                                    .bg(color),
                            ),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .flex_shrink_0()
                            .text_xs()
                            .font_family(ui::MONO)
                            .text_color(color)
                            .text_right()
                            .child(format!("{}%", percent.round() as i64)),
                    )
                    .child(
                        div()
                            .w(px(128.))
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(ui::faint())
                            .child(format_reset(window.reset_at, now)),
                    )
            }))
            .into_any_element()
    }

    fn render_stats(&self, cx: &mut Context<Self>) -> AnyElement {
        let (data, summary) = match &self.stats {
            StatsState::Idle | StatsState::Loading => {
                return h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(Spinner::new().small().color(ui::muted()))
                    .child("Reading session files…")
                    .into_any_element();
            }
            StatsState::Loaded { data, summary, .. } => (data, summary),
        };
        let range_button = |id: &'static str, label: &'static str, range: StatsRange| {
            Button::new(id)
                .xsmall()
                .ghost()
                .selected(self.range == range)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_range(range, cx)))
        };
        let mut notes = Vec::new();
        if !data.skipped_large.is_empty() {
            notes.push(format!(
                "Skipped {} session file{} larger than 64 MiB.",
                data.skipped_large.len(),
                if data.skipped_large.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        if data.unreadable > 0 {
            notes.push(format!(
                "{} session file(s) could not be read.",
                data.unreadable
            ));
        }
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::section_label("Range"))
                    .child(div().flex_1())
                    .child(
                        segmented()
                            .child(range_button("usage-range-all", "All time", StatsRange::All))
                            .child(range_button(
                                "usage-range-30d",
                                "30 days",
                                StatsRange::Month,
                            ))
                            .child(range_button("usage-range-7d", "7 days", StatsRange::Week)),
                    ),
            )
            .child(Self::render_totals(summary))
            .child(self.render_chart(summary, cx))
            .child(Self::render_models(summary))
            .child(Self::render_projects(summary))
            .children(
                notes
                    .into_iter()
                    .map(|note| status_line(ui::warning(), Some(IconName::CircleAlert), note)),
            )
            .into_any_element()
    }

    fn render_totals(summary: &StatsSummary) -> AnyElement {
        let tile = |i: usize, label: &'static str, value: String, color: Hsla| {
            ui::appear(
                ("usage-tile", i),
                ui::card()
                    .flex_1()
                    .min_w(px(104.))
                    .gap_1()
                    .px_3()
                    .py_2p5()
                    .child(ui::section_label(label))
                    .child(
                        div()
                            .truncate()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .font_family(ui::MONO)
                            .text_color(color)
                            .child(value),
                    ),
            )
        };
        let totals = &summary.totals;
        let detail = |label: &'static str, value: u64| {
            h_flex()
                .gap_1p5()
                .text_xs()
                .child(div().text_color(ui::faint()).child(label))
                .child(
                    div()
                        .text_color(ui::muted())
                        .font_family(ui::MONO)
                        .child(format_tokens(value)),
                )
        };
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(tile(0, "Tokens", format_tokens(totals.total), ui::fg()))
                    .child(tile(1, "Cost", format_cost(totals.cost), ui::champagne()))
                    .child(tile(2, "Sessions", summary.sessions.to_string(), ui::fg()))
                    .child(tile(3, "Messages", summary.messages.to_string(), ui::fg()))
                    .child(tile(
                        4,
                        "Active days",
                        summary.active_days.to_string(),
                        ui::fg(),
                    )),
            )
            .child(
                h_flex()
                    .gap_4()
                    .flex_wrap()
                    .px_1()
                    .child(detail("input", totals.input))
                    .child(detail("output", totals.output))
                    .child(detail("cache read", totals.cache_read))
                    .child(detail("cache write", totals.cache_write)),
            )
            .into_any_element()
    }

    fn render_chart(&self, summary: &StatsSummary, cx: &mut Context<Self>) -> AnyElement {
        let metric = self.metric;
        let value = |day: &DayStats| match metric {
            Metric::Tokens => day.tokens as f64,
            Metric::Cost => day.cost,
        };
        let max = summary.daily.iter().map(value).fold(0.0, f64::max);
        let metric_button = |id: &'static str, label: &'static str, m: Metric| {
            Button::new(id)
                .xsmall()
                .ghost()
                .selected(metric == m)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.metric = m;
                    cx.notify();
                }))
        };
        let first = summary
            .daily
            .first()
            .map(|d| day_label(d.day))
            .unwrap_or_default();
        let last = summary
            .daily
            .last()
            .map(|d| day_label(d.day))
            .unwrap_or_default();
        let bar_color = match metric {
            Metric::Tokens => ui::accent(),
            Metric::Cost => ui::champagne(),
        };
        ui::card()
            .gap_3()
            .p_4()
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::section_label("Last 30 days"))
                    .child(div().flex_1())
                    .child(
                        segmented()
                            .child(metric_button(
                                "usage-metric-tokens",
                                "Tokens",
                                Metric::Tokens,
                            ))
                            .child(metric_button("usage-metric-cost", "Cost", Metric::Cost)),
                    ),
            )
            .child(
                h_flex()
                    .h(px(110.))
                    .items_end()
                    .gap(px(3.))
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(ui::border())
                    .bg(ui::well())
                    .children(summary.daily.iter().map(|day| {
                        let v = value(day);
                        let fraction = if max > 0.0 { (v / max) as f32 } else { 0.0 };
                        let tip = format!(
                            "{} · {} tokens · {}",
                            day_label(day.day),
                            format_tokens(day.tokens),
                            format_cost(day.cost)
                        );
                        div()
                            .id(SharedString::from(format!("usage-day-{}", day.day)))
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .rounded_sm()
                            .hover(|s| s.bg(ui::surface_hover()))
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                            .child(
                                div()
                                    .w_full()
                                    .h(relative(fraction))
                                    .when(v > 0.0, |d| d.min_h(px(2.)))
                                    .rounded_sm()
                                    .bg(bar_color),
                            )
                    })),
            )
            .child(
                h_flex()
                    .text_xs()
                    .text_color(ui::faint())
                    .child(first)
                    .child(div().flex_1())
                    .child(div().font_family(ui::MONO).child(format!(
                        "peak {}",
                        match metric {
                            Metric::Tokens => format_tokens(max as u64),
                            Metric::Cost => format_cost(max),
                        }
                    )))
                    .child(div().flex_1())
                    .child(last),
            )
            .into_any_element()
    }

    fn table_header(columns: &[(&'static str, f32)]) -> Div {
        h_flex()
            .px_2()
            .pb_1p5()
            .gap_2()
            .border_b_1()
            .border_color(ui::border())
            .text_xs()
            .text_color(ui::faint())
            .child(div().flex_1().min_w_0().truncate().child(columns[0].0))
            .children(columns[1..].iter().map(|(label, width)| {
                div()
                    .w(px(*width))
                    .flex_shrink_0()
                    .text_right()
                    .child(*label)
            }))
    }

    fn numeric(width: f32, text: String) -> Div {
        div()
            .w(px(width))
            .flex_shrink_0()
            .text_right()
            .text_xs()
            .font_family(ui::MONO)
            .text_color(ui::muted())
            .child(text)
    }

    /// A table card: section heading, column header, rows.
    fn table(title: &'static str, columns: &[(&'static str, f32)], empty: bool) -> Div {
        ui::card()
            .gap_2()
            .p_3()
            .child(div().px_2().pt_1().child(ui::section_label(title)))
            .child(Self::table_header(columns))
            .when(empty, |d| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(ui::muted())
                        .child("No usage in this range."),
                )
            })
    }

    fn table_row() -> Div {
        h_flex()
            .px_2()
            .py_1p5()
            .gap_2()
            .min_w_0()
            .rounded_lg()
            .text_sm()
            .hover(|s| s.bg(ui::surface_hover()))
    }

    fn render_models(summary: &StatsSummary) -> AnyElement {
        Self::table(
            "Models",
            &[
                ("Model", 0.),
                ("Messages", 64.),
                ("Tokens", 64.),
                ("Cost", 72.),
                ("Share", 44.),
            ],
            summary.models.is_empty(),
        )
        .child(
            v_flex()
                .gap_0p5()
                .children(summary.models.iter().enumerate().map(|(i, model)| {
                    ui::appear(
                        ("usage-model", i),
                        Self::table_row()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_color(ui::fg())
                                            .truncate()
                                            .child(model.model.clone()),
                                    )
                                    .child(
                                        ui::chip(model.provider.clone(), ui::info())
                                            .flex_shrink_0(),
                                    ),
                            )
                            .child(Self::numeric(64., model.messages.to_string()))
                            .child(Self::numeric(64., format_tokens(model.tokens)))
                            .child(Self::numeric(72., format_cost(model.cost)))
                            .child(Self::numeric(
                                44.,
                                format!("{}%", (model.share * 100.0).round() as i64),
                            )),
                    )
                })),
        )
        .into_any_element()
    }

    fn render_projects(summary: &StatsSummary) -> AnyElement {
        Self::table(
            "Projects",
            &[
                ("Project", 0.),
                ("Sessions", 60.),
                ("Messages", 64.),
                ("Tokens", 64.),
                ("Cost", 72.),
            ],
            summary.projects.is_empty(),
        )
        .child(
            v_flex()
                .gap_0p5()
                .children(summary.projects.iter().enumerate().map(|(i, project)| {
                    let path = Path::new(&project.project);
                    let (prefix, leaf) = if path.is_absolute() {
                        ui::split_path(path)
                    } else {
                        (String::new(), project.project.clone())
                    };
                    ui::appear(
                        ("usage-project", i),
                        Self::table_row()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(ui::faint())
                                            .child(prefix),
                                    )
                                    .child(
                                        div().min_w_0().truncate().text_color(ui::fg()).child(leaf),
                                    ),
                            )
                            .child(Self::numeric(60., project.sessions.to_string()))
                            .child(Self::numeric(64., project.messages.to_string()))
                            .child(Self::numeric(64., format_tokens(project.tokens)))
                            .child(Self::numeric(72., format_cost(project.cost))),
                    )
                })),
        )
        .into_any_element()
    }
}

impl Render for UsageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tab {
            Tab::Usage => self.render_usage(),
            Tab::Stats => self.render_stats(cx),
        };
        let busy = match self.tab {
            Tab::Usage => {
                matches!(self.usage, UsageState::Loading)
                    || matches!(&self.usage, UsageState::Loaded { cards, .. } if cards.iter().any(|c| c.state == CardState::Loading))
            }
            Tab::Stats => matches!(self.stats, StatsState::Loading),
        };
        let (subtitle, status) = match self.tab {
            Tab::Usage => (
                "Subscription windows of signed-in providers",
                match &self.usage {
                    UsageState::Loaded { cards, .. } => format!(
                        "{} provider{}",
                        cards.len(),
                        if cards.len() == 1 { "" } else { "s" }
                    ),
                    _ => String::new(),
                },
            ),
            Tab::Stats => (
                "Token and cost totals from local sessions",
                match &self.stats {
                    StatsState::Loaded { data, .. } => format!("{} session files", data.files),
                    _ => String::new(),
                },
            ),
        };
        let tab_button = |id: &'static str, label: &'static str, icon: IconName, tab: Tab| {
            Button::new(id)
                .xsmall()
                .ghost()
                .selected(self.tab == tab)
                .icon(Icon::new(icon))
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
        };
        let (backdrop, panel) = ui::screen(Some(760.));
        backdrop.occlude().child(ui::pop_in(
            "usage-screen",
            panel
                .w_full()
                .max_w(px(760.))
                .child(
                    ui::screen_header()
                        .child(Icon::new(IconName::Gauge).small().text_color(ui::muted()))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Usage"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(subtitle),
                        )
                        .child(
                            segmented()
                                .child(tab_button(
                                    "usage-tab-usage",
                                    "Subscription",
                                    IconName::Zap,
                                    Tab::Usage,
                                ))
                                .child(tab_button(
                                    "usage-tab-stats",
                                    "Local stats",
                                    IconName::ChartPie,
                                    Tab::Stats,
                                )),
                        )
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("usage-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(UsageEvent::Close))),
                        ),
                )
                .child(
                    div()
                        .id("usage-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_4()
                        .child(body),
                )
                .child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap_2()
                        .border_t_1()
                        .border_color(ui::border())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::faint())
                                .child(status),
                        )
                        .child(
                            Button::new("usage-refresh")
                                .primary()
                                .small()
                                .icon(Icon::new(IconName::RefreshCw))
                                .label("Refresh")
                                .loading(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        ),
                ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CardState, DAY, Fetch, MAX_SESSION_BYTES, StatsRange, account_id_from_token,
        aggregate_stats, civil_from_days, days_from_civil, discover, format_reset,
        group_usage_limits, load_sessions, now_ms, parse_codex_usage, parse_iso_ms,
        parse_nan_quota, resolve_config_value, window_label,
    };
    use serde_json::Value;
    use serde_json::json;
    use std::collections::HashMap;

    const NOW: i64 = 1_762_400_000_000;

    #[test]
    fn codex_usage_parses_main_and_additional_limits() {
        let payload = json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": { "used_percent": 42, "limit_window_seconds": 18000, "reset_after_seconds": 3600, "reset_at": 1_762_403_600 },
                "secondary_window": { "used_percent": 81.5, "limit_window_seconds": 604800, "reset_after_seconds": 7200 }
            },
            "additional_rate_limits": [
                { "limit_name": "codex_other", "rate_limit": { "limit_reached": true, "primary_window": { "used_percent": 100, "limit_window_seconds": 86400 } } },
                { "limit_name": "empty", "rate_limit": { "primary_window": null, "secondary_window": null } },
                { "rate_limit": { "primary_window": { "used_percent": 5, "limit_window_seconds": 0 } } }
            ]
        });
        let usage = parse_codex_usage(&payload, NOW);
        assert_eq!(usage.provider, "openai-codex");
        assert_eq!(usage.plan.as_deref(), Some("plus"));
        assert_eq!(usage.limits.len(), 2);
        let main = &usage.limits[0];
        assert_eq!(main.name, "codex");
        assert!(!main.limit_reached);
        assert_eq!(main.windows[0].label, "5h");
        assert_eq!(main.windows[0].used_percent, 42.0);
        assert_eq!(
            main.windows[0].reset_at,
            Some(1_762_403_600_000),
            "reset_at wins over reset_after_seconds"
        );
        assert_eq!(main.windows[1].label, "week");
        assert_eq!(main.windows[1].reset_at, Some(NOW + 7_200_000));
        let extra = &usage.limits[1];
        assert_eq!(extra.name, "codex_other");
        assert!(extra.limit_reached);
        assert_eq!(extra.windows[0].label, "1d");
        assert_eq!(extra.windows[0].reset_at, None);
        assert!(parse_codex_usage(&Value::Null, NOW).limits.is_empty());
    }

    #[test]
    fn nan_quota_parses_period_and_rolling_windows() {
        let payload = json!({
            "periodEnd": "2025-11-30T00:00:00.000Z",
            "models": [
                { "model": "glm5.2", "cap": 500, "fullCap": 1000, "tokensUsed": 250,
                  "windowHours": 4, "fullWindowTokens": 200, "windowTokensUsed": 50, "windowResetsAt": 1_762_401_000 },
                { "model": "kimi-k2", "cap": 100, "tokensUsed": 100, "periodEnd": "2025-11-20T00:00:00Z" },
                { "model": "free-model", "tokensUsed": 9 },
                { "model": "", "cap": 10, "tokensUsed": 1 },
                "junk"
            ]
        });
        let usage = parse_nan_quota(&payload, NOW);
        assert_eq!(usage.provider, "nan");
        assert_eq!(
            usage.limits.len(),
            2,
            "unmetered and nameless models are skipped"
        );
        let glm = &usage.limits[0];
        assert_eq!(glm.name, "glm5.2");
        assert_eq!(glm.windows[0].label, "");
        assert_eq!(
            glm.windows[0].used_percent, 25.0,
            "fullCap is the allowance"
        );
        assert_eq!(glm.windows[0].budget, Some(1000.0));
        assert_eq!(
            glm.windows[0].reset_at,
            parse_iso_ms("2025-11-30T00:00:00Z"),
            "falls back to the payload periodEnd"
        );
        assert_eq!(glm.windows[1].label, "4h");
        assert_eq!(glm.windows[1].used_percent, 25.0);
        assert_eq!(glm.windows[1].reset_at, Some(1_762_401_000_000));
        assert!(!glm.limit_reached);
        let kimi = &usage.limits[1];
        assert!(kimi.limit_reached);
        assert_eq!(kimi.windows.len(), 1);
        assert_eq!(
            kimi.windows[0].reset_at,
            parse_iso_ms("2025-11-20T00:00:00Z")
        );

        // Rolling defaults: 400M tokens over 4h.
        let defaults = parse_nan_quota(
            &json!({ "models": [{ "model": "m", "cap": 10, "tokensUsed": 0, "windowTokensUsed": 40_000_000 }] }),
            NOW,
        );
        assert_eq!(defaults.limits[0].windows[1].used_percent, 10.0);
        assert_eq!(defaults.limits[0].windows[1].label, "4h");

        // A metered model with unreadable usage is drift: the whole read is empty.
        let drift = json!({ "models": [{ "model": "a", "cap": 10, "tokensUsed": 1 }, { "model": "b", "cap": 10, "tokensUsed": "x" }] });
        assert!(parse_nan_quota(&drift, NOW).limits.is_empty());

        // Grouping: kimi (100%) family before glm (25%).
        let grouped = group_usage_limits(&usage.limits);
        assert_eq!(grouped[0].name, "kimi-k2");
    }

    #[test]
    fn account_id_and_discovery_never_fetch_expired_tokens() {
        let payload = r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_123"}}"#;
        let encoded: String = {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let bytes = payload.as_bytes();
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let n = chunk
                    .iter()
                    .enumerate()
                    .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
                for i in 0..=chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            out
        };
        let token = format!("e30.{encoded}.sig");
        assert_eq!(account_id_from_token(&token).as_deref(), Some("acct_123"));
        assert_eq!(account_id_from_token("not-a-jwt"), None);

        let auth = json!({
            "openai-codex": { "type": "oauth", "access": token, "refresh": "r", "expires": NOW + 1000 },
            "openai": { "type": "oauth", "access": "x.y.z", "refresh": "r", "expires": NOW - 1, "accountId": "a" },
            "anthropic": { "type": "oauth", "access": "a", "refresh": "r", "expires": NOW + 1 },
            "nan": { "type": "api_key", "key": "$NAN_KEY", "env": { "NAN_KEY": "k" } },
            "openrouter": { "type": "api_key", "key": "k" }
        });
        let discovery = discover(auth.as_object().unwrap(), NOW);
        let states: Vec<(&str, bool, &CardState)> = discovery
            .cards
            .iter()
            .map(|(card, fetch)| (card.id.as_str(), fetch.is_some(), &card.state))
            .collect();
        assert_eq!(states[0].0, "openai-codex");
        assert!(states[0].1);
        assert!(
            matches!(&discovery.cards[0].1, Some(Fetch::Codex { account_id, .. }) if account_id == "acct_123")
        );
        assert_eq!(states[1].0, "openai");
        assert!(
            !states[1].1 && matches!(states[1].2, CardState::SignIn(_)),
            "expired tokens are not fetched"
        );
        assert!(matches!(
            states[2],
            ("anthropic", false, CardState::Note(_))
        ));
        assert!(matches!(states[3], ("nan", true, CardState::Loading)));
        assert_eq!(discovery.others, vec!["openrouter".to_string()]);

        let env = HashMap::from([("NAN_KEY".to_string(), "secret".to_string())]);
        assert_eq!(
            resolve_config_value("$NAN_KEY", &env).as_deref(),
            Some("secret")
        );
        assert_eq!(
            resolve_config_value("pre-${NAN_KEY}-$$x", &env).as_deref(),
            Some("pre-secret-$x")
        );
        assert_eq!(
            resolve_config_value("$GENTLE_USAGE_TEST_UNSET_VAR", &env),
            None
        );
        assert_eq!(
            resolve_config_value("plain", &env).as_deref(),
            Some("plain")
        );
    }

    #[test]
    fn iso_timestamps_and_resets() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_ms("2025-11-06T12:34:56.789Z"),
            Some(1_762_432_496_789)
        );
        assert_eq!(
            parse_iso_ms("2025-11-06T14:34:56.789+02:00"),
            Some(1_762_432_496_789)
        );
        assert_eq!(parse_iso_ms("2025-11-06"), Some(1_762_387_200_000));
        assert_eq!(parse_iso_ms("nope"), None);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(format_reset(Some(NOW + 30 * 60_000), NOW), "resets in 30m");
        assert_eq!(
            format_reset(Some(NOW + (2 * 3600 + 5 * 60) * 1000), NOW),
            "resets in 2h 5m"
        );
        assert_eq!(
            format_reset(Some(NOW + (3 * 86400 + 4 * 3600) * 1000), NOW),
            "resets in 3d 4h"
        );
        assert_eq!(format_reset(Some(NOW - 1), NOW), "resets now");
        assert_eq!(window_label(18_000), "5h");
        assert_eq!(window_label(604_800), "week");
    }

    fn iso(ms: i64) -> String {
        let days = ms.div_euclid(DAY * 1000);
        let rem = ms.rem_euclid(DAY * 1000) / 1000;
        let (y, m, d) = civil_from_days(days);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }

    fn assistant(
        ms: i64,
        provider: &str,
        model: &str,
        input: u64,
        output: u64,
        cost: f64,
    ) -> String {
        json!({
            "type": "message",
            "timestamp": iso(ms),
            "message": { "role": "assistant", "provider": provider, "model": model,
                         "usage": { "input": input, "output": output, "cacheRead": 10, "cacheWrite": 0, "cost": { "total": cost } } }
        })
        .to_string()
    }

    #[test]
    fn stats_aggregate_over_synthetic_session_dir() {
        let root = std::env::temp_dir().join(format!(
            "gentle-usage-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let day = DAY * 1000;
        let write = |dir: &str, name: &str, lines: &[String]| {
            let path = root.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(name), lines.join("\n")).unwrap();
        };
        let header = |id: &str, cwd: &str, ms: i64| {
            json!({ "type": "session", "id": id, "cwd": cwd, "timestamp": iso(ms) }).to_string()
        };
        write(
            "--Users-me-alpha--",
            "a.jsonl",
            &[
                header("s1", "/Users/me/alpha", NOW - 2 * day),
                json!({ "type": "message", "timestamp": iso(NOW - 2 * day), "message": { "role": "user", "content": "hi" } }).to_string(),
                assistant(NOW - 2 * day, "anthropic", "claude", 100, 50, 0.5),
                assistant(NOW - day, "openai-codex", "gpt", 200, 100, 0.25),
                "{ not json \"assistant\"".into(),
            ],
        );
        // A second copy of s1 with fewer records loses.
        write(
            "--Users-me-alpha--",
            "b.jsonl",
            &[
                header("s1", "/Users/me/alpha", NOW - 2 * day),
                assistant(NOW, "x", "x", 1, 1, 9.0),
            ],
        );
        write(
            "--Users-me-beta--",
            "c.jsonl",
            &[
                header("s2", "", NOW - 40 * day),
                assistant(NOW - 40 * day, "anthropic", "claude", 1000, 0, 1.0),
                assistant(NOW, "anthropic", "claude", 10, 0, 0.0),
            ],
        );
        write("--Users-me-beta--", "notes.txt", &["ignored".into()]);
        write(
            "--Users-me-beta--",
            "no-header.jsonl",
            &[assistant(NOW, "anthropic", "claude", 1, 1, 1.0)],
        );
        write(
            "--Users-me-beta--/nested",
            "sub.jsonl",
            &[
                header("s3", "", NOW),
                assistant(NOW, "a", "nested", 5, 5, 5.0),
            ],
        );

        let data = load_sessions(&root, MAX_SESSION_BYTES);
        assert_eq!(data.files, 4, "only sessions/<dir>/*.jsonl");
        assert_eq!(data.sessions.len(), 2);
        assert!(data.skipped_large.is_empty());

        let all = aggregate_stats(&data.sessions, StatsRange::All, NOW, 0);
        assert_eq!(all.sessions, 2);
        assert_eq!(all.messages, 4);
        assert_eq!(all.totals.input, 1310);
        assert_eq!(all.totals.output, 150);
        assert_eq!(all.totals.cache_read, 40);
        assert_eq!(all.totals.total, 1310 + 150 + 40);
        assert!((all.totals.cost - 1.75).abs() < 1e-9);
        assert_eq!(all.models[0].model, "claude");
        assert_eq!(all.models[0].messages, 3);
        assert_eq!(all.models[0].tokens, 160 + 1010 + 20);
        assert_eq!(all.models[1].model, "gpt");
        assert_eq!(all.projects.len(), 2);
        assert_eq!(
            all.projects[0].project, "--Users-me-beta--",
            "a header without cwd falls back to the session dir"
        );
        assert_eq!(all.projects[0].sessions, 1);
        assert_eq!(all.projects[1].project, "/Users/me/alpha");
        assert_eq!(all.projects[1].messages, 2);
        assert_eq!(all.daily.len(), 30);
        assert_eq!(all.daily.last().unwrap().tokens, 20);
        assert_eq!(all.daily[29 - 2].tokens, 160);
        assert!((all.daily[29 - 1].cost - 0.25).abs() < 1e-9);
        assert_eq!(all.active_days, 4);

        let week = aggregate_stats(&data.sessions, StatsRange::Week, NOW, 0);
        assert_eq!(week.messages, 3, "the 40-day-old message is outside 7d");
        assert_eq!(week.sessions, 2);
        assert_eq!(week.daily, all.daily, "the chart ignores the range");

        let capped = load_sessions(&root, 10);
        assert_eq!(capped.skipped_large.len(), 4);
        assert!(capped.sessions.is_empty());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn session_records_are_cached_by_mtime_and_len() {
        use pi_rpc::file_cache::FileCache;
        use std::time::{Duration, UNIX_EPOCH};
        let root = std::env::temp_dir().join(format!(
            "gentle-usage-cache-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let dir = root.join("--w--");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jsonl");
        let write = |cost: f64| {
            let lines = [
                json!({ "type": "session", "id": "s1", "cwd": "/w", "timestamp": iso(NOW) })
                    .to_string(),
                assistant(NOW, "anthropic", "claude", 100, 50, cost),
            ];
            std::fs::write(&path, lines.join("\n")).unwrap();
        };
        let set_mtime = |secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        let cost = |data: &super::StatsData| data.sessions[0].messages[0].cost;
        let cache = FileCache::new(8);

        write(0.5);
        set_mtime(7_000_000);
        let first = super::load_sessions_in(&cache, &root, MAX_SESSION_BYTES);
        assert_eq!(cost(&first), 0.5);

        // Same mtime and length: the cached record, not the new cost.
        write(0.7);
        set_mtime(7_000_000);
        let cached = super::load_sessions_in(&cache, &root, MAX_SESSION_BYTES);
        assert_eq!(cached.sessions, first.sessions);

        set_mtime(7_000_001);
        let fresh = super::load_sessions_in(&cache, &root, MAX_SESSION_BYTES);
        assert_eq!(cost(&fresh), 0.7);
        assert_eq!(
            fresh.sessions,
            load_sessions(&root, MAX_SESSION_BYTES).sessions
        );

        std::fs::remove_dir_all(&root).unwrap();
    }
}
