//! One tab: a `gentle-shell --mode rpc` backend and its conversation UI.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{InputEvent, InputState, Position, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;
use pi_rpc::{Incoming, PiProcess, UiAnswer, UiMethod, UiRequest};
use serde_json::{Value, json};

use crate::git::{self, GitSnapshot};
use crate::tools::{self, TodoTask};
use crate::transcript::{self, Block, Item, NoticeLevel, Transcript};
use crate::ui;

mod agents;
mod attachments;
mod view;

pub use agents::AgentsPanel;

actions!(session, [Abort, ToggleAgents, NewSession]);

pub const CONTEXT: &str = "GentleSession";

/// Requests a tab makes of its window.
pub enum SessionEvent {
    OpenProviders,
    /// An extension's `notify`, for the window's notification center.
    /// Warnings and errors are also toasted by the tab itself.
    Notice {
        level: NoticeLevel,
        message: String,
    },
    /// Something the window chrome shows may have changed: [`SessionView::title`],
    /// [`SessionView::is_running`], [`SessionView::session_file`],
    /// [`SessionView::cwd`] or [`SessionView::model_choices`]. Streaming
    /// output does not emit it, so the window need not redraw per token.
    ChromeChanged,
}

/// What the window reads from a tab, to emit [`SessionEvent::ChromeChanged`]
/// only when it differs.
#[derive(PartialEq)]
struct Chrome {
    title: String,
    running: bool,
    session_file: Option<PathBuf>,
    models: u64,
}

/// Least time between working-tree refreshes a turn's tool calls trigger.
const GIT_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);

/// Whether the right rail is shown; one setting for every tab.
#[derive(Clone, Copy)]
pub struct RailVisible(pub bool);
impl Global for RailVisible {}

enum Backend {
    Starting,
    Ready(Arc<PiProcess>),
    Failed(String),
    Exited(Option<i32>),
}

#[derive(Clone, Debug, PartialEq)]
struct ModelInfo {
    provider: String,
    id: String,
    name: String,
    context_window: u64,
    reasoning: bool,
    /// `input` lists `"image"` (assumed when Pi does not say).
    images: bool,
}

impl ModelInfo {
    fn parse(value: &Value) -> Option<Self> {
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

struct Dialog {
    id: String,
    method: UiMethod,
}

pub struct SessionView {
    cwd: PathBuf,
    backend: Backend,
    transcript: Transcript,
    /// Rendered Markdown per (item, block), with the byte length already pushed.
    markdown: HashMap<(usize, usize), (Entity<TextViewState>, usize)>,
    expanded: HashSet<usize>,
    composer: Entity<TextareaState>,
    /// Images waiting to go with the next prompt; `[Image #N]` is the N-th.
    attachments: Vec<attachments::Attachment>,
    /// Decoded conversation images, by data hash.
    image_cache: attachments::ImageCache,
    dialogs: VecDeque<Dialog>,
    dialog_input: Entity<TextareaState>,
    statuses: BTreeMap<String, String>,
    widgets: BTreeMap<String, Vec<String>>,
    agents: Option<crate::activity::Activity>,
    agents_panel: AgentsPanel,
    jobs: Vec<crate::gentle_state::Job>,
    /// `bash_background`/`monitor` commands by tool call, to label their jobs.
    job_commands: HashMap<String, String>,
    phase: Option<&'static str>,
    mode: Option<crate::gentle_state::PermissionMode>,
    _agents_ticker: Task<()>,
    todo: Vec<TodoTask>,
    git: Option<GitSnapshot>,
    model: Option<ModelInfo>,
    models: Vec<ModelInfo>,
    models_loaded: bool,
    thinking: String,
    context_percent: Option<f64>,
    cost: f64,
    commands: Vec<(String, String)>,
    session_name: Option<String>,
    session_file: Option<PathBuf>,
    title: Option<String>,
    queued: usize,
    active_shell: Option<usize>,
    stderr: VecDeque<String>,
    version: Option<String>,
    banner: crate::banner::BannerConfig,
    banner_reveal: f32,
    _banner_anim: Task<()>,
    /// Sent once the backend is up (`--prompt`).
    pending_prompt: Option<String>,
    /// Resumed once the backend is up (a session opened from another project).
    pending_session: Option<PathBuf>,
    /// A new Gentle Shell was installed mid-turn: restart on it once idle.
    restart_pending: bool,
    /// The conversation's virtualized list and its per-item display caches.
    list: view::TranscriptList,
    /// The backend was started at least once (tabs start when first shown).
    started: bool,
    /// What the window was last told about: see [`SessionEvent::ChromeChanged`].
    chrome: Option<Chrome>,
    /// Bumped whenever `models` is replaced.
    models_rev: u64,
    /// When the working tree was last refreshed.
    git_at: Option<Instant>,
    /// Bumped per refresh; a deferred refresh runs only if none ran since.
    git_seq: u64,
    /// A deferred refresh is waiting.
    git_scheduled: bool,
    /// The pane's own width, measured each frame; narrow layouts adapt to it.
    pane_width: std::rc::Rc<std::cell::Cell<f32>>,
    /// The whole pane's width (rail included), measured each frame.
    root_width: std::rc::Rc<std::cell::Cell<f32>>,
    /// The pane was wide enough for the rail last frame (for hysteresis).
    rail_fits: bool,
    focus_handle: FocusHandle,
    _io: Task<()>,
    _subscriptions: Vec<Subscription>,
    /// View-only state: live timers, the prompt in flight, picker highlights.
    live: view::Live,
    model_search: Entity<InputState>,
    /// Rate-limit usage extensions reported, by window (`seven_day`).
    rate_limits: BTreeMap<String, view::RateLimit>,
    /// When each warning toast was last shown, to drop repeats.
    recent_toasts: HashMap<String, Instant>,
}

impl EventEmitter<SessionEvent> for SessionView {}

impl SessionView {
    /// `start_now: false` defers the backend until [`Self::ensure_started`]
    /// (tabs that are not shown yet); `prompt` and a resumed session still
    /// run once it starts.
    pub fn new(
        cwd: PathBuf,
        prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
        start_now: bool,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .submit_on_enter(true)
                .placeholder("What are we cooking?")
        });
        let dialog_input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8));
        let model_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models…"));
        let weak = cx.entity().downgrade();
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => this.submit(window, cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &dialog_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter {
                        secondary,
                        shift: false,
                    } = event
                    {
                        let submits = match this.dialogs.front().map(|d| &d.method) {
                            Some(UiMethod::Input { .. }) => true,
                            Some(UiMethod::Editor { .. }) => *secondary,
                            _ => false,
                        };
                        if submits {
                            let value = this.dialog_input.read(cx).value().to_string();
                            this.answer_dialog(UiAnswer::Value(value), window, cx);
                        }
                    }
                },
            ),
            cx.subscribe(&model_search, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.model_query_changed(cx);
                }
            }),
            // The model picker's ↑/↓/Enter must beat its focused search field.
            cx.intercept_keystrokes(move |event, window, cx| {
                let consumed = weak
                    .update(cx, |this, cx| {
                        this.model_picker_key(&event.keystroke, window, cx)
                    })
                    .unwrap_or(false);
                if consumed {
                    cx.stop_propagation();
                }
            }),
        ];
        let mut this = Self {
            cwd,
            backend: Backend::Starting,
            transcript: Transcript::default(),
            markdown: HashMap::new(),
            expanded: HashSet::new(),
            composer,
            attachments: Vec::new(),
            image_cache: attachments::ImageCache::default(),
            dialogs: VecDeque::new(),
            dialog_input,
            statuses: BTreeMap::new(),
            widgets: BTreeMap::new(),
            agents: None,
            agents_panel: AgentsPanel::default(),
            jobs: Vec::new(),
            job_commands: HashMap::new(),
            phase: None,
            mode: None,
            _agents_ticker: Task::ready(()),
            todo: Vec::new(),
            git: None,
            model: None,
            models: Vec::new(),
            models_loaded: false,
            thinking: "off".into(),
            context_percent: None,
            cost: 0.0,
            pane_width: std::rc::Rc::new(std::cell::Cell::new(10_000.)),
            root_width: std::rc::Rc::new(std::cell::Cell::new(0.)),
            rail_fits: false,
            commands: Vec::new(),
            session_name: None,
            session_file: None,
            title: None,
            queued: 0,
            active_shell: None,
            stderr: VecDeque::new(),
            version: None,
            banner: crate::banner::read_config(&pi_rpc::resolve_homes().config),
            banner_reveal: 0.0,
            _banner_anim: Task::ready(()),
            pending_prompt: prompt,
            pending_session: None,
            restart_pending: false,
            list: view::TranscriptList::new(),
            started: false,
            chrome: None,
            models_rev: 0,
            git_at: None,
            git_seq: 0,
            git_scheduled: false,
            focus_handle: cx.focus_handle(),
            _io: Task::ready(()),
            _subscriptions: subscriptions,
            live: view::Live::default(),
            model_search,
            rate_limits: BTreeMap::new(),
            recent_toasts: HashMap::new(),
        };
        if start_now {
            this.ensure_started(window, cx);
        }
        this
    }

    /// Start the backend unless it was started before. Idempotent.
    pub fn ensure_started(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.start(window, cx);
        self.animate_banner(cx);
    }

    /// Write the wordmark in over ~1.2 s, like the terminal intro.
    fn animate_banner(&mut self, cx: &mut Context<Self>) {
        self.banner_reveal = 0.0;
        self._banner_anim = cx.spawn(async move |this, cx| {
            const FRAMES: u32 = 72;
            for frame in 1..=FRAMES {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let t = frame as f32 / FRAMES as f32;
                let eased = 1.0 - (1.0 - t).powi(3);
                if this
                    .update(cx, |this, cx| {
                        this.banner_reveal = eased;
                        // Only the hero shows the banner: repaint just this pane, and only then.
                        if this.hero_shown() {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
    }

    pub fn title(&self) -> String {
        self.session_name
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| ui::split_path(&self.cwd).1)
    }

    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    pub fn is_running(&self) -> bool {
        self.transcript.running
    }

    pub fn composer(&self) -> Entity<TextareaState> {
        self.composer.clone()
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        self.composer
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// New credentials only reach a backend through a fresh runtime: restart
    /// tabs without a conversation, tell the others. A tab not started yet
    /// picks them up when it starts.
    pub fn reload_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.started {
            return;
        }
        if self.transcript.items.is_empty() {
            self.start(window, cx);
        } else {
            self.transcript.notice(
                NoticeLevel::Info,
                "New sign-in saved. Open a new tab to use its models here.".into(),
            );
            cx.notify();
        }
    }

    /// Restart the backend in another folder; the conversation starts fresh.
    pub fn change_cwd(&mut self, cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.cwd = cwd;
        if self.started {
            self.start(window, cx);
        } else {
            self.sync_chrome(cx);
        }
    }

    /// Gentle Shell was updated on disk: run this tab on the new version,
    /// keeping its conversation (the session file is resumed on the new
    /// backend). A tab in the middle of a turn switches when the turn ends.
    pub fn restart_for_update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A tab not started yet starts on the new version anyway.
        if !self.started {
            return;
        }
        if self.transcript.running || !self.dialogs.is_empty() {
            self.restart_pending = true;
            self.transcript.notice(
                NoticeLevel::Info,
                "Gentle Shell was updated; this tab switches to it when the current turn ends."
                    .into(),
            );
            cx.notify();
        } else {
            self.restart_backend(window, cx);
        }
    }

    fn restart_backend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restart_pending = false;
        let resume = self.session_file();
        self.start(window, cx);
        if let Some(path) = resume {
            self.resume_when_ready(path, cx);
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Backend::Ready(process) = &self.backend {
            process.shutdown(Duration::from_secs(3));
        }
        self.started = true;
        self.backend = Backend::Starting;
        self.models_loaded = false;
        self.transcript = Transcript::default();
        self.markdown.clear();
        self.expanded.clear();
        self.live = view::Live::default();
        self.dialogs.clear();
        self.statuses.clear();
        self.widgets.clear();
        self.agents = None;
        self.jobs.clear();
        self.job_commands.clear();
        self.phase = None;
        self.todo.clear();
        self.session_name = None;
        self.title = None;
        self.refresh_git(cx);
        let cwd = self.cwd.clone();
        self._io = cx.spawn_in(window, async move |this, cx| {
            let spawned = cx
                .background_executor()
                .spawn(async move {
                    let spec = pi_rpc::resolve_backend(&cwd)?;
                    let version = pi_rpc::backend_version(&spec);
                    let (process, rx) = PiProcess::spawn(&spec)?;
                    anyhow::Ok((process, rx, version))
                })
                .await;
            let rx = match spawned {
                Ok((process, rx, version)) => {
                    let ok = this.update(cx, |this, cx| {
                        this.backend = Backend::Ready(process);
                        this.version = version;
                        this.refresh_state(cx);
                        // Kept until the switch lands, so the window saves this
                        // path rather than the blank session it replaces.
                        if let Some(path) = this.pending_session.clone() {
                            this.open_session(path, cx);
                        }
                        if let Some(prompt) = this.pending_prompt.take() {
                            this.send_prompt(prompt, cx);
                        }
                        cx.notify();
                    });
                    if ok.is_err() {
                        return;
                    }
                    rx
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.backend = Backend::Failed(format!("{error:#}"));
                        cx.notify();
                    });
                    return;
                }
            };
            while let Ok(first) = rx.recv().await {
                // One update (and one frame) for everything already queued.
                let batch = drain_queued(first, &rx);
                if this
                    .update_in(cx, |this, window, cx| this.handle_batch(batch, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.sync_chrome(cx);
        cx.notify();
    }

    fn process(&self) -> Option<Arc<PiProcess>> {
        match &self.backend {
            Backend::Ready(process) => Some(process.clone()),
            _ => None,
        }
    }

    /// Send `command`; `then` runs on the UI thread with the response data.
    fn request(
        &self,
        command: Value,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<Value>, &mut Context<Self>) + 'static,
    ) {
        let Some(process) = self.process() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = process.request(command).await.map(|r| r.data);
            let _ = this.update(cx, |this, cx| {
                then(this, result, cx);
                this.sync_view(cx);
                this.sync_chrome(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn report(&mut self, result: Result<Value>) -> Option<Value> {
        match result {
            Ok(data) => Some(data),
            Err(error) => {
                self.transcript
                    .notice(NoticeLevel::Error, format!("{error:#}"));
                None
            }
        }
    }

    fn refresh_state(&mut self, cx: &mut Context<Self>) {
        self.request(json!({"type": "get_state"}), cx, |this, result, _| {
            if let Some(state) = this.report(result) {
                this.model = ModelInfo::parse(&state["model"]);
                this.thinking = state["thinkingLevel"].as_str().unwrap_or("off").to_string();
                this.session_name = state["sessionName"].as_str().map(str::to_string);
                this.session_file = state["sessionFile"].as_str().map(PathBuf::from);
            }
        });
        self.request(
            json!({"type": "get_available_models"}),
            cx,
            |this, result, _| {
                this.models_loaded = true;
                this.models_rev += 1;
                if let Some(data) = this.report(result) {
                    this.models = data["models"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(ModelInfo::parse)
                        .collect();
                }
            },
        );
        self.request(json!({"type": "get_commands"}), cx, |this, result, _| {
            if let Some(data) = this.report(result) {
                this.commands = data["commands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| {
                        Some((
                            c["name"].as_str()?.to_string(),
                            c["description"].as_str().unwrap_or_default().to_string(),
                        ))
                    })
                    .collect();
            }
        });
        self.request(json!({"type": "get_messages"}), cx, |this, result, _| {
            if let Some(data) = this.report(result) {
                let messages = data["messages"].as_array().cloned().unwrap_or_default();
                if !messages.is_empty() {
                    this.transcript.load_messages(&messages);
                    this.markdown.clear();
                    this.sync_todo_from_history();
                }
            }
        });
        self.refresh_stats(cx);
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        self.request(
            json!({"type": "get_session_stats"}),
            cx,
            |this, result, _| {
                if let Ok(stats) = result {
                    this.cost = stats["cost"].as_f64().unwrap_or(0.0);
                    this.context_percent = stats["contextUsage"]["percent"].as_f64();
                }
            },
        );
    }

    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        self.git_seq += 1;
        self.git_at = Some(Instant::now());
        let cwd = self.cwd.clone();
        cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move { git::snapshot(&cwd) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.git = snapshot;
                cx.notify();
            });
        })
        .detach();
    }

    /// [`Self::refresh_git`] at most once per [`GIT_REFRESH_INTERVAL`]: a
    /// call inside it defers one refresh to its end.
    fn refresh_git_soon(&mut self, cx: &mut Context<Self>) {
        if self.git_scheduled {
            return;
        }
        let wait = git_refresh_wait(self.git_at.map(|at| at.elapsed()));
        if wait.is_zero() {
            self.refresh_git(cx);
            return;
        }
        self.git_scheduled = true;
        let seq = self.git_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                this.git_scheduled = false;
                // A refresh since (the turn ended) already covered it.
                if this.git_seq == seq {
                    this.refresh_git(cx);
                }
            });
        })
        .detach();
    }

    fn sync_todo_from_history(&mut self) {
        for item in self.transcript.items.iter().rev() {
            if let Item::Tool { name, details, .. } = item
                && name == "todo"
                && let Some(tasks) = tools::todo_tasks(details)
            {
                self.todo = tasks;
                return;
            }
        }
    }

    /// Apply a batch of backend messages in order, then sync the view and
    /// repaint once.
    fn handle_batch(&mut self, batch: Vec<Incoming>, window: &mut Window, cx: &mut Context<Self>) {
        let follow = self.list.at_bottom();
        for incoming in batch {
            self.handle(incoming, window, cx);
            if self.restart_pending && !self.transcript.running && self.dialogs.is_empty() {
                // The rest of the batch belongs to the backend being replaced.
                self.restart_backend(window, cx);
                break;
            }
        }
        self.sync_view(cx);
        if follow {
            self.list.scroll_to_end();
        }
        self.sync_chrome(cx);
        cx.notify();
    }

    fn handle(&mut self, incoming: Incoming, window: &mut Window, cx: &mut Context<Self>) {
        match incoming {
            Incoming::Event(event) => self.handle_event(&event, cx),
            Incoming::Ui(request) => self.handle_ui(request, window, cx),
            Incoming::Stderr(line) => {
                if self.stderr.len() == 40 {
                    self.stderr.pop_front();
                }
                self.stderr.push_back(line);
            }
            Incoming::Exited(code) => {
                self.backend = Backend::Exited(code);
                self.transcript.running = false;
                self.dialogs.clear();
            }
        }
    }

    /// Tell the window when something it shows changed.
    fn sync_chrome(&mut self, cx: &mut Context<Self>) {
        let chrome = Chrome {
            title: self.title(),
            running: self.is_running(),
            session_file: self.session_file(),
            models: self.models_rev,
        };
        if self.chrome.as_ref() != Some(&chrome) {
            self.chrome = Some(chrome);
            cx.emit(SessionEvent::ChromeChanged);
        }
    }

    fn handle_event(&mut self, event: &Value, cx: &mut Context<Self>) {
        let effects = self.transcript.apply(event);
        match event["type"].as_str() {
            Some("queue_update") => {
                let count = |key: &str| event[key].as_array().map_or(0, Vec::len);
                self.queued = count("steering") + count("followUp");
            }
            Some("session_info_changed") => {
                self.session_name = event["name"].as_str().map(str::to_string);
            }
            Some("thinking_level_changed") => {
                self.thinking = event["level"].as_str().unwrap_or("off").to_string();
            }
            Some("bash_execution_update") => {
                if let (Some(index), Some(delta)) = (self.active_shell, event["delta"].as_str()) {
                    self.transcript.shell_output(index, delta);
                }
            }
            Some("tool_execution_start") => {
                let tool = event["toolName"].as_str().unwrap_or_default();
                if let Some(phase) = crate::gentle_state::infer_phase(tool, &event["args"]) {
                    self.phase = Some(phase);
                }
                if matches!(tool, "bash_background" | "monitor") {
                    let label = event["args"]["label"]
                        .as_str()
                        .or(event["args"]["command"].as_str())
                        .unwrap_or_default();
                    self.job_commands.insert(
                        transcript::str_field(event, "toolCallId"),
                        label.to_string(),
                    );
                }
            }
            Some("tool_execution_end") => {
                let details = &event["result"]["details"];
                if event["toolName"] == "todo"
                    && let Some(tasks) = tools::todo_tasks(details)
                {
                    self.todo = tasks;
                }
                if let Some(job) = crate::gentle_state::job_from_details(details) {
                    let label = self
                        .job_commands
                        .remove(&transcript::str_field(event, "toolCallId"));
                    crate::gentle_state::upsert_job(&mut self.jobs, job, label.as_deref());
                }
            }
            Some("message_end") if event["message"]["role"] == "custom" => {
                if let Some(job) =
                    crate::gentle_state::job_from_details(&event["message"]["details"])
                {
                    crate::gentle_state::upsert_job(&mut self.jobs, job, None);
                }
            }
            _ => {}
        }
        if effects.settled {
            self.phase = None;
            self.queued = 0;
            self.refresh_stats(cx);
        }
        if effects.settled {
            // Always once at turn end, whatever the tool calls scheduled.
            self.refresh_git(cx);
        } else if effects.files_touched {
            self.refresh_git_soon(cx);
        }
    }

    fn handle_ui(&mut self, request: UiRequest, window: &mut Window, cx: &mut Context<Self>) {
        if request.method.is_dialog() {
            if let Some(timeout) = request.timeout {
                let id = request.id.clone();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(timeout))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.dialogs.retain(|d| d.id != id);
                        cx.notify();
                    });
                })
                .detach();
            }
            let first = self.dialogs.is_empty();
            self.dialogs.push_back(Dialog {
                id: request.id,
                method: request.method,
            });
            if first {
                self.prepare_dialog(window, cx);
            }
            return;
        }
        match request.method {
            UiMethod::Notify {
                message,
                notify_type,
            } => {
                let level = match notify_type.as_deref() {
                    Some("error") => NoticeLevel::Error,
                    Some("warning") => NoticeLevel::Warning,
                    _ => NoticeLevel::Info,
                };
                let error = level == NoticeLevel::Error;
                // Usage warnings repeat every turn: keep them as a composer chip.
                if !error && let Some((percent, window_key)) = view::parse_rate_limit(&message) {
                    self.rate_limits.insert(
                        window_key,
                        view::RateLimit {
                            percent,
                            text: message,
                        },
                    );
                } else {
                    // Info goes to the notification center only; warnings and
                    // errors also toast.
                    if level != NoticeLevel::Info && (error || self.first_toast(&message)) {
                        let note = if error {
                            Notification::error(message.clone())
                        } else {
                            Notification::warning(message.clone())
                        };
                        window.push_notification(note, cx);
                    }
                    cx.emit(SessionEvent::Notice { level, message });
                }
            }
            UiMethod::SetStatus { key, text } => match text.filter(|t| !t.trim().is_empty()) {
                Some(text) => {
                    if key == "gentle:mode" {
                        self.mode = crate::gentle_state::PermissionMode::from_status(&text);
                    }
                    self.statuses.insert(key, text);
                }
                None => {
                    self.statuses.remove(&key);
                }
            },
            UiMethod::SetWidget { key, lines, .. } => {
                let lines = lines.filter(|l| !l.is_empty());
                if key == "gentle-agents" {
                    let next = lines.and_then(|l| crate::activity::parse(&l));
                    self.agents = Some(crate::activity::retain_finished(self.agents.take(), next));
                } else if let Some(lines) = lines {
                    self.widgets.insert(key, lines);
                } else {
                    self.widgets.remove(&key);
                }
            }
            UiMethod::SetTitle { title } => {
                self.title = Some(title).filter(|t| !t.is_empty());
            }
            UiMethod::SetEditorText { text } => {
                self.composer
                    .update(cx, |state, cx| state.set_value(text, window, cx));
            }
            _ => {}
        }
    }

    /// False when the same message toasted within the last ten minutes.
    fn first_toast(&mut self, message: &str) -> bool {
        const WINDOW: Duration = Duration::from_secs(600);
        self.recent_toasts.retain(|_, at| at.elapsed() < WINDOW);
        if self.recent_toasts.contains_key(message) {
            return false;
        }
        self.recent_toasts
            .insert(message.to_string(), Instant::now());
        true
    }

    /// Point keyboard focus at the front dialog: its text field, or the
    /// session itself so typing does not leak into the composer.
    fn prepare_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, placeholder) = match self.dialogs.front().map(|d| &d.method) {
            Some(UiMethod::Input { placeholder, .. }) => (None, placeholder.clone()),
            Some(UiMethod::Editor { prefill, .. }) => (prefill.clone(), None),
            Some(_) => {
                window.focus(&self.focus_handle, cx);
                return;
            }
            None => {
                self.focus_composer(window, cx);
                return;
            }
        };
        self.dialog_input.update(cx, |state, cx| {
            state.set_value(text.unwrap_or_default(), window, cx);
            state.set_placeholder(placeholder.unwrap_or_default(), window, cx);
            state.focus(window, cx);
        });
    }

    fn answer_dialog(&mut self, answer: UiAnswer, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialogs.pop_front() else {
            return;
        };
        if let Some(process) = self.process()
            && let Err(error) = process.respond_ui(&dialog.id, answer)
        {
            self.transcript
                .notice(NoticeLevel::Error, format!("{error:#}"));
        }
        self.prepare_dialog(window, cx);
        cx.notify();
    }

    /// Bring the rendered Markdown up to date for items `from..` (earlier
    /// items have not changed since the last sync).
    fn sync_markdown(&mut self, from: usize, cx: &mut Context<Self>) {
        let items = self.transcript.items.get(from..).unwrap_or_default();
        for (item_ix, item) in (from..).zip(items) {
            let Item::Assistant { blocks, .. } = item else {
                continue;
            };
            for (block_ix, block) in blocks.iter().enumerate() {
                let Block::Text(text) = block else { continue };
                match self.markdown.get_mut(&(item_ix, block_ix)) {
                    Some((state, pushed)) if *pushed != text.len() => {
                        let appended = text.is_char_boundary(*pushed) && *pushed < text.len();
                        state.update(cx, |state, cx| {
                            if appended {
                                state.push_str(&text[*pushed..], cx);
                            } else {
                                state.set_text(text, cx);
                            }
                        });
                        *pushed = text.len();
                    }
                    Some(_) => {}
                    None => {
                        let state = cx.new(|cx| TextViewState::markdown(text, cx));
                        self.markdown
                            .insert((item_ix, block_ix), (state, text.len()));
                    }
                }
            }
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        let shell = text.starts_with('!');
        let images = !shell && !self.attachments.is_empty();
        if (text.is_empty() && !images) || self.process().is_none() {
            return;
        }
        if shell && !self.attachments.is_empty() {
            // Clearing the composer would strand the images with no tokens left.
            let message =
                "Shell commands cannot carry images. Remove the attachments or drop the leading !.";
            self.warn(message.into(), window, cx);
            return;
        }
        if images && let Some(model) = self.model.as_ref().filter(|m| !m.images) {
            let message = format!(
                "{} does not accept images. Switch to a model with image input, or remove the attachments.",
                model.name
            );
            self.warn(message, window, cx);
            return;
        }
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.list.scroll_to_end();

        // Pi's TUI convention: `!cmd` runs and shares output, `!!cmd` keeps it out of context.
        if let Some(command) = text.strip_prefix('!') {
            let (command, exclude) = match command.strip_prefix('!') {
                Some(rest) => (rest.trim().to_string(), true),
                None => (command.trim().to_string(), false),
            };
            if command.is_empty() {
                return;
            }
            let index = self.transcript.start_shell(command.clone());
            self.active_shell = Some(index);
            self.request(
                json!({"type": "bash", "command": command, "excludeFromContext": exclude}),
                cx,
                move |this, result, _| {
                    this.active_shell = None;
                    match result {
                        Ok(data) => this.transcript.finish_shell(
                            index,
                            data["exitCode"].as_i64(),
                            data["output"].as_str().map(str::to_string),
                        ),
                        Err(error) => {
                            this.transcript
                                .finish_shell(index, None, Some(format!("{error:#}")))
                        }
                    }
                },
            );
            cx.notify();
            return;
        }

        let images = std::mem::take(&mut self.attachments);
        self.send_prompt_with(text, images, cx);
        cx.notify();
    }

    fn send_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        self.send_prompt_with(text, Vec::new(), cx);
    }

    fn send_prompt_with(
        &mut self,
        text: String,
        images: Vec<attachments::Attachment>,
        cx: &mut Context<Self>,
    ) {
        let steer = self.transcript.running;
        let command = attachments::prompt_command(&text, &images, steer);
        for image in &images {
            self.image_cache.insert(image);
        }
        if !steer {
            let shown = images.iter().map(|a| a.image.clone()).collect();
            self.live
                .expect_reply(&text, shown, self.transcript.items.len());
        }
        self.request(command, cx, |this, result, _| {
            if this.report(result).is_some() {
                this.live.reply_acknowledged();
            } else {
                this.live.reply_failed();
            }
        });
    }

    /// A warning toast, as extension warnings show.
    fn warn(&self, message: String, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification(Notification::warning(message), cx);
    }

    /// Attach validated images (inserting their tokens at the cursor) and warn
    /// about the rest.
    fn attach(
        &mut self,
        loaded: Vec<Result<attachments::Attachment, attachments::Rejection>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let first = self.attachments.len() + 1;
        let mut rejected = Vec::new();
        for result in loaded {
            match result {
                Ok(attachment) => self.attachments.push(attachment),
                Err(rejection) => rejected.push(rejection.to_string()),
            }
        }
        let added = first..self.attachments.len() + 1;
        if !added.is_empty() {
            self.composer.update(cx, |state, cx| {
                let value = state.value();
                let mut before = value[..state.cursor().min(value.len())].chars().next_back();
                let mut tokens = String::new();
                for n in added {
                    tokens.push_str(&attachments::insertion(before, n));
                    before = Some(' ');
                }
                state.insert(tokens, window, cx);
                state.focus(window, cx);
            });
        }
        for message in rejected {
            self.warn(message, window, cx);
        }
        cx.notify();
    }

    /// Read dropped or picked files off the UI thread, then attach them.
    fn attach_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let loading = cx.background_executor().spawn(async move {
            paths
                .iter()
                .map(|path| attachments::load_path(path))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let loaded = loading.await;
            let _ = this.update_in(cx, |this, window, cx| this.attach(loaded, window, cx));
        })
        .detach();
    }

    /// The composer's paste hook: clipboard images and copied image files
    /// become attachments; anything else pastes as text.
    fn paste_clipboard(
        &mut self,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut images = Vec::new();
        let mut paths = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => images.push(attachments::Attachment::from_bytes(
                    "The pasted image",
                    image.bytes.clone(),
                )),
                ClipboardEntry::ExternalPaths(external) => {
                    paths.extend(external.paths().iter().cloned())
                }
                _ => {}
            }
        }
        // Copied files that are not images paste as their names.
        if !paths.iter().any(|p| attachments::has_image_extension(p)) {
            paths.clear();
        }
        if images.is_empty() && paths.is_empty() {
            return false;
        }
        if !images.is_empty() {
            self.attach(images, window, cx);
        }
        self.attach_paths(paths, window, cx);
        true
    }

    /// The 📎 button: pick image files.
    fn pick_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.attach_paths(paths, window, cx));
        })
        .detach();
    }

    /// Drop attachment `index` (0-based) and its token; later tokens shift down.
    fn remove_attachment(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.attachments.len() {
            return;
        }
        self.attachments.remove(index);
        self.composer.update(cx, |state, cx| {
            let text = attachments::remove_token(&state.value(), index + 1);
            let line = text.split('\n').count().saturating_sub(1) as u32;
            let column = text
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .map(char::len_utf16)
                .sum::<usize>() as u32;
            state.replace_all(text, window, cx);
            state.set_cursor_position(Position::new(line, column), window, cx);
        });
        cx.notify();
    }

    fn on_new_session(&mut self, _: &NewSession, window: &mut Window, cx: &mut Context<Self>) {
        self.new_session(window, cx);
    }

    /// RPC `new_session`: a fresh conversation on the same backend. A fresh
    /// session also re-reads defaults such as the permission mode.
    pub fn new_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.request(json!({"type": "new_session"}), cx, |this, result, cx| {
            if let Some(data) = this.report(result)
                && data["cancelled"] != true
            {
                this.reset_conversation();
                this.refresh_state(cx);
                this.refresh_git(cx);
            }
        });
    }

    /// Switch to `path` as soon as the backend is ready.
    pub fn resume_when_ready(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.process().is_some() {
            self.open_session(path, cx);
        } else {
            self.pending_session = Some(path);
        }
    }

    /// RPC `switch_session` to a saved conversation.
    pub fn open_session(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let command = json!({"type": "switch_session", "sessionPath": path});
        self.request(command, cx, move |this, result, cx| {
            let switched = this
                .report(result)
                .is_some_and(|data| data["cancelled"] != true);
            if this.pending_session.as_ref() == Some(&path) {
                this.pending_session = None;
            }
            if switched {
                this.reset_conversation();
                this.session_file = Some(path);
                this.refresh_state(cx);
            }
        });
    }

    /// Send any RPC command for a window-level screen; failures surface in the transcript.
    pub fn send_command(&mut self, command: Value, cx: &mut Context<Self>) {
        self.request(command, cx, |this, result, cx| {
            this.report(result);
            this.refresh_state(cx);
        });
    }

    /// Run a slash command the way typing it would.
    pub fn run_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        self.send_prompt(text, cx);
    }

    /// Run a Gentle Shell extension command (`/gentle:…`) over RPC without
    /// adding a turn: Pi runs extension commands at once, even mid-stream.
    /// `done` gets `Ok` only when Pi reports the command as `handled`, so a
    /// missing or renamed command surfaces instead of being sent as a prompt.
    pub fn run_extension_command(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
        done: impl FnOnce(Result<(), String>, &mut App) + 'static,
    ) {
        if self.process().is_none() {
            done(
                Err("Gentle Shell is still starting in this tab; try again in a moment.".into()),
                cx,
            );
            return;
        }
        let command = json!({"type": "prompt", "message": text});
        self.request(command, cx, move |_, result, cx| {
            let outcome = match result {
                Ok(data) if data["disposition"] == "handled" => Ok(()),
                Ok(data) => Err(format!(
                    "Gentle Shell did not run {text} (disposition {}). Is gentle-pi up to date?",
                    data["disposition"]
                )),
                Err(error) => Err(format!("{error:#}")),
            };
            done(outcome, cx);
        });
    }

    /// Show an informational line in the conversation.
    pub fn note(&mut self, text: String, cx: &mut Context<Self>) {
        self.transcript.notice(NoticeLevel::Info, text);
        cx.notify();
    }

    pub fn commands(&self) -> Vec<(String, String)> {
        self.commands.clone()
    }

    /// The conversation this tab shows: a resume still in flight (or waiting
    /// for the backend to start) wins over the live session file.
    pub fn session_file(&self) -> Option<PathBuf> {
        self.pending_session
            .clone()
            .or_else(|| self.session_file.clone())
    }

    pub fn model_choices(&self) -> Vec<(String, String, String, bool)> {
        self.models
            .iter()
            .map(|m| {
                (
                    m.provider.clone(),
                    m.id.clone(),
                    m.name.clone(),
                    m.reasoning,
                )
            })
            .collect()
    }

    fn reset_conversation(&mut self) {
        self.transcript = Transcript::default();
        self.markdown.clear();
        self.expanded.clear();
        self.todo.clear();
        self.jobs.clear();
        self.phase = None;
        self.session_name = None;
        self.agents = None;
        self.live = view::Live::default();
    }

    /// Save `mode` as the default and start a fresh session so it applies
    /// (Gentle Shell only switches a live session's mode from its terminal UI).
    pub(super) fn choose_mode(
        &mut self,
        mode: crate::gentle_state::PermissionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let config = pi_rpc::resolve_homes().config;
        if let Err(error) = crate::gentle_state::save_default_mode(&config, mode) {
            self.transcript.notice(
                NoticeLevel::Error,
                format!("Could not save the permission mode: {error}"),
            );
            cx.notify();
            return;
        }
        if self.transcript.items.is_empty() {
            self.new_session(window, cx);
        } else {
            self.transcript.notice(
                NoticeLevel::Info,
                format!(
                    "{} saved as the default. It applies from the next session (⌘N).",
                    mode.label()
                ),
            );
            cx.notify();
        }
    }

    /// Gentle Shell has no host-side stop for helpers; ask the agent to cancel it.
    pub(super) fn ask_to_stop_agent(
        &mut self,
        task: &crate::activity::TaskSummary,
        cx: &mut Context<Self>,
    ) {
        let text = format!(
            "Stop the subagent task {} ({}: {}) now with subagent_cancel, then tell me what it had done.",
            task.id,
            task.agent,
            task.label.as_deref().unwrap_or("no label")
        );
        self.send_prompt(text, cx);
    }

    pub fn open_agents_panel(&mut self, cx: &mut Context<Self>) {
        self.open_agents(None, cx);
    }

    fn toggle_agents(&mut self, _: &ToggleAgents, _: &mut Window, cx: &mut Context<Self>) {
        if self.agents_panel.open {
            self.close_agents(cx);
        } else {
            self.open_agents(None, cx);
        }
    }

    fn abort(&mut self, _: &Abort, window: &mut Window, cx: &mut Context<Self>) {
        if self.agents_panel.open {
            self.close_agents(cx);
            return;
        }
        if !self.dialogs.is_empty() {
            self.answer_dialog(UiAnswer::Cancelled, window, cx);
            return;
        }
        if !self.busy() {
            cx.propagate();
            return;
        }
        self.stop(cx);
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        // Pi's Esc: drop queued messages first so abort does not resume them.
        self.request(json!({"type": "clear_queue"}), cx, |_, _, _| {});
        self.request(json!({"type": "abort"}), cx, |this, result, _| {
            this.report(result);
        });
    }

    fn set_model(&mut self, model: ModelInfo, cx: &mut Context<Self>) {
        let command = json!({"type": "set_model", "provider": model.provider, "modelId": model.id});
        self.request(command, cx, move |this, result, cx| {
            if this.report(result).is_some() {
                this.model = Some(model);
                this.refresh_stats(cx);
                this.request(json!({"type": "get_state"}), cx, |this, result, _| {
                    if let Ok(state) = result {
                        this.thinking =
                            state["thinkingLevel"].as_str().unwrap_or("off").to_string();
                    }
                });
            }
        });
    }

    fn cycle_thinking(&mut self, cx: &mut Context<Self>) {
        self.request(
            json!({"type": "cycle_thinking_level"}),
            cx,
            |this, result, _| {
                if let Some(data) = this.report(result)
                    && let Some(level) = data["level"].as_str()
                {
                    this.thinking = level.to_string();
                }
            },
        );
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.pop() else { return };
            let _ = this.update_in(cx, |this, window, cx| this.change_cwd(path, window, cx));
        })
        .detach();
    }

    fn insert_command(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = format!("/{name} ");
        let column = text.chars().map(char::len_utf16).sum::<usize>() as u32;
        self.composer.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.set_cursor_position(Position::new(0, column), window, cx);
            state.focus(window, cx);
        });
    }
}

/// `first` plus every message already queued behind it, in arrival order.
fn drain_queued<T>(first: T, rx: &async_channel::Receiver<T>) -> Vec<T> {
    let mut batch = Vec::with_capacity(rx.len() + 1);
    batch.push(first);
    while let Ok(next) = rx.try_recv() {
        batch.push(next);
    }
    batch
}

/// How long a working-tree refresh must wait, given the time since the last one.
fn git_refresh_wait(since_last: Option<Duration>) -> Duration {
    since_last.map_or(Duration::ZERO, |since| {
        GIT_REFRESH_INTERVAL.saturating_sub(since)
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{GIT_REFRESH_INTERVAL, drain_queued, git_refresh_wait};

    #[test]
    fn batches_drain_everything_queued_in_order() {
        let (tx, rx) = async_channel::unbounded();
        for n in 1..=4 {
            tx.try_send(n).unwrap();
        }
        let first = rx.try_recv().unwrap();
        assert_eq!(drain_queued(first, &rx), vec![1, 2, 3, 4]);
        assert!(rx.is_empty());

        // Later messages start the next batch.
        tx.try_send(5).unwrap();
        assert_eq!(drain_queued(rx.try_recv().unwrap(), &rx), vec![5]);

        // A closed channel still yields what was queued before closing.
        tx.try_send(6).unwrap();
        tx.try_send(7).unwrap();
        drop(tx);
        assert_eq!(drain_queued(rx.try_recv().unwrap(), &rx), vec![6, 7]);
    }

    #[test]
    fn git_refreshes_are_spaced_out() {
        assert_eq!(git_refresh_wait(None), Duration::ZERO);
        assert_eq!(
            git_refresh_wait(Some(Duration::from_millis(500))),
            Duration::from_millis(1000)
        );
        assert_eq!(git_refresh_wait(Some(GIT_REFRESH_INTERVAL)), Duration::ZERO);
        assert_eq!(
            git_refresh_wait(Some(Duration::from_secs(9))),
            Duration::ZERO
        );
    }
}
