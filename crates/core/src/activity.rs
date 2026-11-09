//! `gentle-agents.activity/v1`: live subagent state that Gentle Shell pushes
//! to interactive RPC hosts through `setWidget("gentle-agents", [json])`.
//! Schema: gentle-pi `docs/gentle-agents-activity.md`.

use serde::Deserialize;

pub const SCHEMA: &str = "gentle-agents.activity/v1";

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Activity {
    #[serde(default)]
    pub summary: Summary,
    #[serde(default)]
    pub tasks: Vec<AgentTask>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
pub struct Summary {
    #[serde(default)]
    pub running: u64,
    #[serde(default)]
    pub queued: u64,
    #[serde(default)]
    pub waiting: u64,
    #[serde(default)]
    pub finished: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AgentTask {
    pub summary: TaskSummary,
    #[serde(default)]
    pub thread: Thread,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub status: String,
    pub created_at: Option<u64>,
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
    pub last_step: Option<String>,
    pub last_activity_at: Option<u64>,
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub tool_calls: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Thread {
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub items: Vec<ThreadItem>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThreadItem {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        text: String,
    },
    Note {
        #[serde(default)]
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Tool {
        #[serde(default)]
        name: String,
        /// The tool arguments, `JSON.stringify`'d.
        #[serde(default)]
        args: String,
        #[serde(default)]
        running: bool,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        output: Option<String>,
    },
    #[serde(other)]
    Other,
}

/// Parse the widget lines; `None` for a cleared widget or another schema.
pub fn parse(lines: &[String]) -> Option<Activity> {
    let value: serde_json::Value = serde_json::from_str(&lines.concat()).ok()?;
    if value["schema"] != SCHEMA {
        return None;
    }
    serde_json::from_value(value).ok()
}

impl TaskSummary {
    pub fn is_active(&self) -> bool {
        matches!(self.status.as_str(), "running" | "queued" | "waiting")
    }

    /// Run time so far, or the final duration once ended.
    pub fn elapsed_ms(&self, now_ms: u64) -> Option<u64> {
        let start = self.started_at?;
        Some(self.ended_at.unwrap_or(now_ms).saturating_sub(start))
    }
}

impl ThreadItem {
    /// The tool's most telling argument (`path`, `command`, …), for one-line rows.
    pub fn tool_subject(args: &str) -> String {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(args) else {
            return args.to_string();
        };
        [
            "path", "command", "pattern", "query", "url", "task", "prompt",
        ]
        .iter()
        .find_map(|k| value[*k].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| args.to_string())
    }
}

/// Gentle Agents forgets finished helpers once they are persisted, so the
/// host keeps the last state it saw of any task the new document drops
/// (the official desktop app does the same). Newest documents win per id.
pub fn retain_finished(previous: Option<Activity>, next: Option<Activity>) -> Activity {
    const KEEP: usize = 50;
    let mut next = next.unwrap_or_default();
    if let Some(previous) = previous {
        for task in previous.tasks {
            if next.tasks.iter().any(|t| t.summary.id == task.summary.id) {
                continue;
            }
            let mut task = task;
            // A task that vanished while active finished without a final frame.
            if task.summary.is_active() {
                task.summary.status = "completed".into();
                task.summary.ended_at = task.summary.ended_at.or(task.summary.last_activity_at);
            }
            next.tasks.push(task);
        }
    }
    next.tasks.truncate(KEEP);
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = r#"{"schema":"gentle-agents.activity/v1","summary":{"running":1,"queued":0,"waiting":0,"finished":2},"tasks":[{"summary":{"id":"t_abc123","agent":"explore","label":"Map the auth module","prompt":"Explore how authentication works","status":"running","createdAt":1732000000000,"startedAt":1732000000100,"endedAt":null,"lastStep":"reading lib/auth.ts","lastActivityAt":1732000005000,"turns":2,"toolCalls":3,"error":null},"thread":{"version":7,"dropped":0,"items":[{"kind":"text","text":"Looking at the auth flow first."},{"kind":"tool","name":"read","args":"{\"path\":\"lib/auth.ts\"}","running":false,"isError":false,"output":"contents"},{"kind":"future"}]}}]}"#;

    #[test]
    fn documented_payload_parses() {
        let activity = parse(&[PAYLOAD.to_string()]).unwrap();
        assert_eq!(activity.summary.running, 1);
        let task = &activity.tasks[0];
        assert_eq!(task.summary.agent, "explore");
        assert!(task.summary.is_active());
        assert_eq!(task.summary.elapsed_ms(1732000001100), Some(1000));
        assert_eq!(task.thread.items.len(), 3);
        match &task.thread.items[1] {
            ThreadItem::Tool {
                name,
                args,
                is_error,
                ..
            } => {
                assert_eq!(name, "read");
                assert!(!is_error);
                assert_eq!(ThreadItem::tool_subject(args), "lib/auth.ts");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(task.thread.items[2], ThreadItem::Other);
    }

    #[test]
    fn dropped_tasks_are_kept_as_finished() {
        let first = parse(&[PAYLOAD.to_string()]);
        let empty = parse(&[
            r#"{"schema":"gentle-agents.activity/v1","summary":{},"tasks":[]}"#.to_string(),
        ]);
        let merged = retain_finished(first, empty);
        assert_eq!(merged.tasks.len(), 1);
        assert_eq!(merged.tasks[0].summary.status, "completed");
        assert_eq!(merged.tasks[0].summary.ended_at, Some(1732000005000));
        // A cleared widget keeps history too.
        assert_eq!(retain_finished(Some(merged), None).tasks.len(), 1);
    }

    #[test]
    fn split_lines_and_other_schemas() {
        let (a, b) = PAYLOAD.split_at(40);
        assert!(parse(&[a.to_string(), b.to_string()]).is_some());
        assert!(parse(&[r#"{"schema":"other/v1","tasks":[]}"#.to_string()]).is_none());
    }
}
