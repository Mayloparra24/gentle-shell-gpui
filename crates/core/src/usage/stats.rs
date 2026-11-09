//! Local token and cost totals from session records (`lib/stats-collector.ts`).

use std::collections::HashMap;

use serde_json::Value;

use super::finite;
use super::time::{local_day, parse_iso_ms};

const CHART_DAYS: i64 = 30;
pub const UNKNOWN: &str = "unknown";

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

fn count(value: Option<&Value>) -> u64 {
    finite(value).filter(|v| *v >= 0.0).map_or(0, |v| v as u64)
}

pub fn text_or(value: Option<&Value>, fallback: &str) -> String {
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
