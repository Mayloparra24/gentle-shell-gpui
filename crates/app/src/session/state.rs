//! Groups of [`super::SessionView`] fields that always change together.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pi_rpc::{PiProcess, UiMethod};
use serde_json::Value;

use super::view;
use crate::git::GitSnapshot;
use gentle_core::tools::TodoTask;

/// Thinking levels a reasoning model has before the backend lists its own
/// (`xhigh` / `max` appear only for models that support them).
const DEFAULT_THINKING_LEVELS: [&str; 5] = ["off", "minimal", "low", "medium", "high"];

pub(super) enum BackendStatus {
    Starting,
    Ready(Arc<PiProcess>),
    Failed(String),
    Exited(Option<i32>),
}

/// The `gentle-shell --mode rpc` process and what waits on its start.
pub(super) struct Backend {
    pub(super) status: BackendStatus,
    /// The backend was started at least once (tabs start when first shown).
    pub(super) started: bool,
    pub(super) version: Option<String>,
    /// Its last stderr lines, shown when it exits.
    pub(super) stderr: VecDeque<String>,
    /// Sent once the backend is up (`--prompt`).
    pub(super) pending_prompt: Option<String>,
    /// Resumed once the backend is up (a session opened from another project).
    pub(super) pending_session: Option<PathBuf>,
    /// A new Gentle Shell was installed mid-turn: restart on it once idle.
    pub(super) restart_pending: bool,
}

impl Backend {
    pub(super) fn new(prompt: Option<String>) -> Self {
        Self {
            status: BackendStatus::Starting,
            started: false,
            version: None,
            stderr: VecDeque::new(),
            pending_prompt: prompt,
            pending_session: None,
            restart_pending: false,
        }
    }

    pub(super) fn is_ready(&self) -> bool {
        matches!(self.status, BackendStatus::Ready(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelInfo {
    pub(super) provider: String,
    pub(super) id: String,
    pub(super) name: String,
    pub(super) context_window: u64,
    pub(super) reasoning: bool,
    /// `input` lists `"image"` (assumed when Pi does not say).
    pub(super) images: bool,
}

impl ModelInfo {
    pub(super) fn parse(value: &Value) -> Option<Self> {
        let id = value["id"].as_str()?;
        if id == "unknown" {
            return None;
        }
        Some(Self {
            provider: value["provider"].as_str().unwrap_or_default().to_string(),
            id: id.to_string(),
            name: value["name"].as_str().unwrap_or(id).to_string(),
            context_window: value["contextWindow"].as_u64().unwrap_or(0),
            reasoning: value["reasoning"].as_bool().unwrap_or(false),
            images: value["input"]
                .as_array()
                .is_none_or(|input| input.iter().any(|kind| kind == "image")),
        })
    }
}

/// The session's model, the ones it can switch to, and its thinking level.
pub(super) struct Models {
    pub(super) current: Option<ModelInfo>,
    pub(super) available: Vec<ModelInfo>,
    /// `get_available_models` answered since the backend started.
    pub(super) loaded: bool,
    /// Bumped whenever `available` is replaced.
    pub(super) rev: u64,
    pub(super) thinking: String,
    /// What the current model accepts (`get_available_thinking_levels`), low
    /// to high; the effort slider has one stop per level.
    pub(super) thinking_levels: Vec<String>,
}

impl Default for Models {
    fn default() -> Self {
        Self {
            current: None,
            available: Vec::new(),
            loaded: false,
            rev: 0,
            thinking: "off".into(),
            thinking_levels: DEFAULT_THINKING_LEVELS
                .iter()
                .map(|l| l.to_string())
                .collect(),
        }
    }
}

/// Context and cost of the session, and its compaction.
#[derive(Default)]
pub(super) struct Usage {
    pub(super) context_percent: Option<f64>,
    pub(super) cost: f64,
    /// A compaction (manual or automatic) is running.
    pub(super) compacting: bool,
    /// `get_state.autoCompactionEnabled`.
    pub(super) auto_compaction: Option<bool>,
    /// Context usage crossed the alert threshold.
    pub(super) context_latch: crate::alerts::ContextLatch,
}

pub(super) struct Dialog {
    pub(super) id: String,
    pub(super) method: UiMethod,
}

/// What extensions put on screen through UI requests.
#[derive(Default)]
pub(super) struct ExtensionUi {
    /// Dialogs waiting for an answer; the front one shows.
    pub(super) dialogs: VecDeque<Dialog>,
    pub(super) statuses: BTreeMap<String, String>,
    pub(super) widgets: BTreeMap<String, Vec<String>>,
    /// Rate-limit usage extensions reported, by window (`seven_day`).
    pub(super) rate_limits: BTreeMap<String, view::RateLimit>,
    /// When each warning toast was last shown, to drop repeats.
    pub(super) recent_toasts: HashMap<String, Instant>,
}

impl ExtensionUi {
    /// False when the same message toasted within the last ten minutes.
    pub(super) fn first_toast(&mut self, message: &str) -> bool {
        const WINDOW: Duration = Duration::from_secs(600);
        self.recent_toasts.retain(|_, at| at.elapsed() < WINDOW);
        if self.recent_toasts.contains_key(message) {
            return false;
        }
        self.recent_toasts
            .insert(message.to_string(), Instant::now());
        true
    }
}

/// What the agent is doing besides the conversation: its phase, queued
/// messages, background jobs and helpers.
#[derive(Default)]
pub(super) struct TurnActivity {
    pub(super) phase: Option<&'static str>,
    pub(super) queued: usize,
    pub(super) jobs: Vec<gentle_core::gentle_state::Job>,
    /// `bash_background`/`monitor` commands by tool call, to label their jobs.
    pub(super) job_commands: HashMap<String, String>,
    /// Helpers (`gentle-agents` widget).
    pub(super) agents: Option<gentle_core::activity::Activity>,
}

/// The task list and its island's fold (see [`view`] for the morph).
pub(super) struct TodoIsland {
    pub(super) tasks: Vec<TodoTask>,
    /// The island is unfolded.
    pub(super) open: bool,
    /// Morph progress at `motion_at`, and where it heads (0 folded, 1 open).
    pub(super) motion_from: f32,
    pub(super) motion_to: f32,
    pub(super) motion_at: Instant,
}

impl Default for TodoIsland {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            open: false,
            motion_from: 0.0,
            motion_to: 0.0,
            motion_at: Instant::now(),
        }
    }
}

impl TodoIsland {
    /// Replace the task list. An emptied list folds the island away.
    pub(super) fn apply(&mut self, tasks: Vec<TodoTask>) {
        if tasks.is_empty() {
            self.open = false;
            self.motion_from = 0.0;
            self.motion_to = 0.0;
        }
        self.tasks = tasks;
    }
}

/// The working tree's state and the pacing of its refreshes.
#[derive(Default)]
pub(super) struct GitStatus {
    pub(super) snapshot: Option<GitSnapshot>,
    /// When the working tree was last refreshed.
    pub(super) at: Option<Instant>,
    /// Bumped per refresh; a deferred refresh runs only if none ran since.
    pub(super) seq: u64,
    /// A deferred refresh is waiting.
    pub(super) scheduled: bool,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ExtensionUi, ModelInfo, TodoIsland};
    use gentle_core::tools::TodoTask;

    #[test]
    fn models_parse_with_defaults_and_skip_unknown() {
        assert_eq!(ModelInfo::parse(&json!({"id": "unknown"})), None);
        assert_eq!(ModelInfo::parse(&json!({"name": "no id"})), None);
        let model = ModelInfo::parse(&json!({"id": "m", "provider": "p"})).unwrap();
        assert_eq!(model.name, "m");
        assert_eq!(model.context_window, 0);
        assert!(!model.reasoning);
        assert!(model.images, "image input is assumed when Pi does not say");
        let text_only = json!({"id": "m", "input": ["text"]});
        assert!(!ModelInfo::parse(&text_only).unwrap().images);
    }

    #[test]
    fn emptied_task_lists_fold_the_island() {
        let mut todo = TodoIsland {
            open: true,
            motion_from: 0.4,
            motion_to: 1.0,
            ..TodoIsland::default()
        };
        todo.apply(vec![TodoTask {
            title: "Ship".into(),
            status: "pending".into(),
            note: None,
        }]);
        assert!(todo.open, "a new list keeps the island as it was");
        assert_eq!(todo.tasks.len(), 1);

        todo.apply(Vec::new());
        assert!(!todo.open);
        assert_eq!((todo.motion_from, todo.motion_to), (0.0, 0.0));
        assert!(todo.tasks.is_empty());
    }

    #[test]
    fn repeated_toasts_show_once() {
        let mut ui = ExtensionUi::default();
        assert!(ui.first_toast("Usage is high"));
        assert!(!ui.first_toast("Usage is high"));
        assert!(ui.first_toast("Something else"));
    }
}
