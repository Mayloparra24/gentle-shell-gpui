//! Provider subscription windows (`lib/shell-usage.ts`): the Codex
//! `wham/usage` and NaN Cloud quota payloads, row grouping and the Codex
//! token's account claim.

use serde_json::{Map, Value};

use super::finite;
use super::time::{DAY, HOUR, MINUTE, WEEK, parse_iso_ms};

pub const CODEX_PROVIDER: &str = "openai-codex";
pub const OPENAI_PROVIDER: &str = "openai";
pub const ANTHROPIC_PROVIDER: &str = "anthropic";
pub const NAN_PROVIDER: &str = "nan";
const CODEX_MAIN_LIMIT: &str = "codex";
const CODEX_ACCOUNT_CLAIM: &str = "https://api.openai.com/auth";
const NAN_DEFAULT_WINDOW_TOKENS: f64 = 400_000_000.0;
const NAN_DEFAULT_WINDOW_HOURS: f64 = 4.0;

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

#[cfg(test)]
mod tests {
    use super::{group_usage_limits, parse_codex_usage, parse_nan_quota};
    use crate::usage::time::parse_iso_ms;
    use serde_json::Value;
    use serde_json::json;

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
}
