//! Gentle Shell state an RPC host rebuilds itself: permission mode, background
//! jobs, and the ODD phase label. Sources are cited per item (gentle-pi 4.6).

use serde_json::Value;

// ── Permission mode ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Normal,
    Auto,
    Yolo,
    SuperYolo,
}

impl PermissionMode {
    pub const ALL: [Self; 4] = [Self::Normal, Self::Auto, Self::Yolo, Self::SuperYolo];

    pub fn id(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Auto => "auto",
            Self::Yolo => "yolo",
            Self::SuperYolo => "super-yolo",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Auto => "Auto",
            Self::Yolo => "YOLO",
            Self::SuperYolo => "Super YOLO",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Normal => "Asks before risky actions",
            Self::Auto => "A classifier decides with the conversation's context",
            Self::Yolo => "Ordinary work runs freely; destructive confirmations remain",
            Self::SuperYolo => "No permission prompts; private paths stay blocked",
        }
    }

    /// From the `gentle:mode` status text, `Permissions: SUPER-YOLO`
    /// (lib/yolo-session-policy.ts).
    pub fn from_status(text: &str) -> Option<Self> {
        let mode = text
            .trim()
            .strip_prefix("Permissions:")?
            .trim()
            .to_ascii_lowercase();
        Self::ALL.into_iter().find(|m| m.id() == mode)
    }
}

// ── Background jobs ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub output_path: Option<String>,
}

/// How a job is doing, for its glyph and color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Running,
    Succeeded,
    Failed,
}

impl Job {
    pub fn is_running(&self) -> bool {
        matches!(self.status.as_str(), "running" | "started" | "")
    }

    pub fn outcome(&self) -> JobOutcome {
        if self.is_running() {
            JobOutcome::Running
        } else if self.exit_code.unwrap_or(0) == 0 && self.status != "failed" {
            JobOutcome::Succeeded
        } else {
            JobOutcome::Failed
        }
    }

    /// The command that was started, or `job <id>` when it came without one.
    pub fn display_label(&self) -> String {
        if self.label.is_empty() {
            format!("job {}", self.id)
        } else {
            self.label.clone()
        }
    }

    /// `exit <code>` once it has ended, otherwise the reported status.
    pub fn state_text(&self) -> String {
        match self.exit_code {
            Some(code) => format!("exit {code}"),
            None => self.status.clone(),
        }
    }
}

/// `details.gentleJobs` on `bash_background`/`monitor`/`job_*` results and on
/// `gentle-jobs.notice` messages (lib/background-jobs-tools.ts).
pub fn job_from_details(details: &Value) -> Option<Job> {
    let j = details.get("gentleJobs")?;
    let id = match &j["jobId"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    Some(Job {
        id,
        kind: j["kind"].as_str().unwrap_or("job").to_string(),
        label: j["label"].as_str().unwrap_or_default().to_string(),
        status: j["status"].as_str().unwrap_or_default().to_string(),
        exit_code: j["exitCode"].as_i64(),
        output_path: j["outputPath"].as_str().map(str::to_string),
    })
}

/// Insert or update a job, newest first.
pub fn upsert_job(jobs: &mut Vec<Job>, job: Job, command_label: Option<&str>) {
    if let Some(existing) = jobs.iter_mut().find(|j| j.id == job.id) {
        let label = if job.label.is_empty() {
            existing.label.clone()
        } else {
            job.label.clone()
        };
        *existing = Job { label, ..job };
    } else {
        let label = if job.label.is_empty() {
            command_label.unwrap_or_default().to_string()
        } else {
            job.label.clone()
        };
        jobs.insert(0, Job { label, ..job });
    }
}

// ── ODD phase ──────────────────────────────────────────────────────────────

/// The working-state label from tool activity, a port of gentle-pi
/// lib/odd-phase-inference.ts. Conservative like the original: `None` leaves
/// the current label unchanged. `gentle_odd_phase` sets it explicitly.
pub fn infer_phase(tool: &str, args: &Value) -> Option<&'static str> {
    let name = tool.rsplit("__").next().unwrap_or(tool);
    let arg = |k: &str| args[k].as_str();
    match name {
        "gentle_odd_phase" => explicit_phase(arg("phase")?),
        "read" | "grep" | "find" | "ls" | "codegraph" => Some("exploring"),
        "ask_user_choice" | "ask_user_question" => Some("deciding"),
        "todo" => Some("planning"),
        "gentle_review"
        | "gentle_review_scope"
        | "gentle_review_capture"
        | "gentle_review_capture_group" => Some("checking"),
        "subagent_run" => match arg("agent")? {
            "gentle-ai-worker" => Some("implementing"),
            "gentle-ai-verify" => Some("checking"),
            "gentle-ai-explore" => Some("exploring"),
            _ => None,
        },
        "edit" | "write" => {
            let path = arg("path")
                .or(arg("file_path"))
                .unwrap_or_default()
                .replace('\\', "/");
            Some(
                if path.starts_with("odd/tasks/") || path.contains("/odd/tasks/") {
                    "planning"
                } else {
                    "implementing"
                },
            )
        }
        "bash" | "powershell" => shell_phase(arg("command")?),
        _ => None,
    }
}

fn explicit_phase(phase: &str) -> Option<&'static str> {
    [
        "authorizing",
        "exploring",
        "researching",
        "deciding",
        "planning",
        "implementing",
        "checking",
        "closing",
    ]
    .into_iter()
    .find(|p| *p == phase)
}

static CHECKING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(concat!(
        r"^(?:(?:pnpm|npm|yarn|bun)\s+(?:run\s+)?(?:test|typecheck|lint|build|check)\b",
        r"|node\b.*\s--test\b",
        r"|(?:(?:npx|pnpx|bunx)\s+|(?:pnpm|yarn)\s+exec\s+)?(?:vitest|jest|mocha|tsc|eslint|pytest)\b",
        r"|python3?\s+-m\s+pytest\b|go\s+(?:test|build|vet)\b|cargo\s+(?:test|build|check|clippy)\b|make\b",
        r"|gh\s+pr\s+checks\b|gh\s+run\s+(?:view|watch|list)\b)"
    ))
    .expect("checking pattern")
});

static EXPLORING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(concat!(
        r"^(?:git\s+(?:status|log|diff|show|blame|rev-parse|rev-list|merge-base|ls-files|ls-remote|ls-tree|cat-file|describe|shortlog|reflog|grep|check-ignore|for-each-ref|name-rev|count-objects)\b",
        r"|gh\s+(?:pr|issue|repo|release)\s+(?:view|list|diff|status)\b",
        r"|(?:ls|cat|head|tail|grep|egrep|rg|wc|pwd|tree|stat|file|which|type|readlink|realpath|dirname|basename|cut|uniq|tr|nl|column|strings|diff|cmp|od|xxd|hexdump|shasum|sha256sum|md5|md5sum|du|df|date|uname|whoami|ps|pgrep|lsof|jq|awk)\b",
        r"|command\s+-v\b)"
    ))
    .expect("exploring pattern")
});

static NEUTRAL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"^(?:(?:cd|pushd|popd|echo|printf|sleep|true|false|export|set)\b|(?:done|fi|esac|\}|\))$)",
    )
    .expect("neutral pattern")
});

/// A command checks when any segment runs a checker; it explores only when
/// every segment is read-only inspection or neutral.
fn shell_phase(command: &str) -> Option<&'static str> {
    let segments: Vec<String> = split_shell(command)
        .into_iter()
        .map(|s| strip_segment(&s))
        .filter(|s| !s.is_empty() && !NEUTRAL.is_match(s))
        .collect();
    if segments.is_empty() {
        return None;
    }
    if segments.iter().any(|s| CHECKING.is_match(s)) {
        return Some("checking");
    }
    segments
        .iter()
        .all(|s| EXPLORING.is_match(s))
        .then_some("exploring")
}

/// Split on top-level `&&`, `||`, `;`, `|`, `&` and newlines, honouring quotes.
fn split_shell(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let (mut out, mut current, mut quote) = (Vec::new(), String::new(), None::<char>);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                current.push(c);
                if c == q {
                    quote = None;
                }
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                current.push(c);
            }
            None if c == '\\' => {
                current.push(c);
                if let Some(n) = chars.get(i + 1) {
                    current.push(*n);
                    i += 1;
                }
            }
            None if matches!(c, ';' | '|' | '\n')
                || (c == '&'
                    && i.checked_sub(1).map(|p| chars[p]) != Some('>')
                    && chars.get(i + 1) != Some(&'>')) =>
            {
                out.push(std::mem::take(&mut current));
                if matches!(c, '|' | '&') && chars.get(i + 1) == Some(&c) {
                    i += 1;
                }
            }
            None => current.push(c),
        }
        i += 1;
    }
    out.push(current);
    out
}

/// Drop leading shell keywords and `VAR=value` assignments.
fn strip_segment(segment: &str) -> String {
    let mut rest = segment.trim();
    loop {
        let next = [
            "do ", "then ", "else ", "elif ", "if ", "while ", "until ", "time ", "! ", "{ ", "( ",
        ]
        .iter()
        .find_map(|k| rest.strip_prefix(k))
        .or_else(|| {
            let (word, tail) = rest.split_once(char::is_whitespace)?;
            let (name, _) = word.split_once('=')?;
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .then_some(tail)
        });
        match next {
            Some(tail) => rest = tail.trim_start(),
            None => return rest.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mode_status_parses() {
        assert_eq!(
            PermissionMode::from_status("Permissions: SUPER-YOLO"),
            Some(PermissionMode::SuperYolo)
        );
        assert_eq!(
            PermissionMode::from_status("Permissions: NORMAL"),
            Some(PermissionMode::Normal)
        );
        assert_eq!(PermissionMode::from_status("something else"), None);
    }

    #[test]
    fn jobs_merge_by_id_keeping_labels() {
        let mut jobs = Vec::new();
        let start = job_from_details(
            &json!({"gentleJobs": {"jobId": 1, "kind": "bash", "label": "", "status": "running"}}),
        )
        .unwrap();
        upsert_job(&mut jobs, start, Some("npm run dev"));
        let end = job_from_details(
            &json!({"gentleJobs": {"jobId": 1, "kind": "bash", "status": "exited", "exitCode": 0}}),
        )
        .unwrap();
        upsert_job(&mut jobs, end, None);
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            (
                jobs[0].label.as_str(),
                jobs[0].status.as_str(),
                jobs[0].exit_code
            ),
            ("npm run dev", "exited", Some(0))
        );
        assert!(!jobs[0].is_running());
        assert!(job_from_details(&json!({})).is_none());
    }

    fn job(label: &str, status: &str, exit_code: Option<i64>) -> Job {
        Job {
            id: "7".into(),
            kind: "bash".into(),
            label: label.into(),
            status: status.into(),
            exit_code,
            output_path: None,
        }
    }

    #[test]
    fn job_outcome_tells_running_done_and_failed_apart() {
        assert_eq!(job("", "running", None).outcome(), JobOutcome::Running);
        // A job reported before it has a status is still running.
        assert_eq!(job("", "started", None).outcome(), JobOutcome::Running);
        assert_eq!(job("", "", None).outcome(), JobOutcome::Running);
        assert_eq!(job("", "exited", Some(0)).outcome(), JobOutcome::Succeeded);
        // No exit code yet reported but not running: nothing says it failed.
        assert_eq!(job("", "exited", None).outcome(), JobOutcome::Succeeded);
        assert_eq!(job("", "exited", Some(2)).outcome(), JobOutcome::Failed);
        assert_eq!(job("", "failed", Some(0)).outcome(), JobOutcome::Failed);
        assert_eq!(job("", "failed", None).outcome(), JobOutcome::Failed);
    }

    #[test]
    fn running_status_takes_precedence_over_exit_code() {
        for status in ["", "started", "running"] {
            for code in [0, 2] {
                let job = job("npm run dev", status, Some(code));
                assert_eq!(job.outcome(), JobOutcome::Running);
                assert_eq!(job.state_text(), format!("exit {code}"));
            }
        }
    }

    #[test]
    fn job_row_text_falls_back_to_the_id_and_the_status() {
        assert_eq!(
            job("npm run dev", "running", None).display_label(),
            "npm run dev"
        );
        assert_eq!(job("", "running", None).display_label(), "job 7");
        assert_eq!(job("", "exited", Some(1)).state_text(), "exit 1");
        assert_eq!(job("", "running", None).state_text(), "running");
    }

    #[test]
    fn phase_inference_follows_tool_activity() {
        assert_eq!(infer_phase("read", &json!({})), Some("exploring"));
        assert_eq!(
            infer_phase("edit", &json!({"path": "odd/tasks/x.md"})),
            Some("planning")
        );
        assert_eq!(
            infer_phase("edit", &json!({"path": "src/a.rs"})),
            Some("implementing")
        );
        assert_eq!(
            infer_phase("bash", &json!({"command": "cargo test -p app"})),
            Some("checking")
        );
        assert_eq!(
            infer_phase("bash", &json!({"command": "git status && rg foo | head"})),
            Some("exploring")
        );
        assert_eq!(
            infer_phase(
                "bash",
                &json!({"command": "cd crates && CI=1 npm test 2>&1"})
            ),
            Some("checking")
        );
        assert_eq!(
            infer_phase("bash", &json!({"command": "rm -rf target"})),
            None
        );
        assert_eq!(
            infer_phase("bash", &json!({"command": "echo 'a && cargo test'"})),
            None
        );
        assert_eq!(
            infer_phase("subagent_run", &json!({"agent": "gentle-ai-verify"})),
            Some("checking")
        );
        assert_eq!(
            infer_phase("subagent_run", &json!({"agent": "review-risk"})),
            None
        );
        assert_eq!(
            infer_phase("gentle_odd_phase", &json!({"phase": "closing"})),
            Some("closing")
        );
        assert_eq!(infer_phase("mystery", &json!({})), None);
    }
}
