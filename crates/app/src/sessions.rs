//! Sessions: browse and resume previous conversations, like Pi's `/resume`.
//!
//! Pi stores one JSONL file per session under `<agent>/sessions/--<cwd>--/`
//! (`<timestamp>_<id>.jsonl`), unless `settings.json` sets `sessionDir`, in
//! which case every session lives flat in that directory and is matched to a
//! project by the header's `cwd`. RPC mode has no listing command, so we read
//! the files ourselves, mirroring `SessionManager.list`/`listAll`.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::file_cache::{FileCache, Stamp};
use serde::Deserialize;
use serde_json::Value;

use crate::ui;

pub enum SessionsEvent {
    /// Resume `path`. When `cwd` is the tab's cwd send RPC
    /// `switch_session {sessionPath: path}`; otherwise the session belongs to
    /// another project and should open in a new tab rooted at `cwd`.
    Open {
        path: PathBuf,
        cwd: PathBuf,
    },
    /// Start a fresh conversation in this tab (RPC `new_session`).
    NewSession,
    Close,
}

/// Which sessions to list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Sessions recorded for the tab's cwd (`--continue` / `/resume`).
    Project,
    /// Every project (`/resume` "all").
    All,
}

/// What the browser shows for one session file.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub path: PathBuf,
    pub id: String,
    /// The header's `cwd`; empty when the header has none.
    pub cwd: PathBuf,
    /// Latest `session_info` name (`/name`, `set_session_name`); a later empty
    /// name clears it, as in Pi.
    pub name: Option<String>,
    /// Text of the first user message.
    pub first_message: Option<String>,
    /// Last user/assistant activity, else header time, else file mtime (ms).
    pub modified_ms: i64,
    /// Number of `message` entries (Pi counts every role).
    pub message_count: usize,
    /// The scan stopped at [`MAX_SCAN_BYTES`]; counts are a lower bound.
    pub truncated: bool,
    /// Model of the last assistant reply or `model_change`, whichever is later.
    pub model: Option<String>,
}

impl SessionSummary {
    pub fn title(&self) -> String {
        let text = self
            .name
            .as_deref()
            .or(self.first_message.as_deref())
            .unwrap_or("(no messages)");
        ui::first_line(text.trim(), 120)
    }

    pub fn matches(&self, query_lower: &str) -> bool {
        query_lower.is_empty()
            || [self.name.as_deref(), self.first_message.as_deref()]
                .into_iter()
                .flatten()
                .any(|t| t.to_lowercase().contains(query_lower))
            || self
                .cwd
                .to_string_lossy()
                .to_lowercase()
                .contains(query_lower)
            || self
                .path
                .to_string_lossy()
                .to_lowercase()
                .contains(query_lower)
    }
}

/// Stop reading a session after this many bytes; huge transcripts (inline
/// images, long tool output) would otherwise stall the list.
pub const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;

/// Most rows rendered at once; the search box narrows the rest.
const MAX_ROWS: usize = 200;

/// Pi's per-cwd directory name: drop the leading separator, map `/ \ :` to `-`.
pub fn session_dir_name(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let trimmed = s.strip_prefix(['/', '\\']).unwrap_or(&s);
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{safe}--")
}

/// `settings.json`'s `sessionDir`, with `~` expanded like Pi's `normalizePath`.
pub fn configured_session_dir(agent_home: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(agent_home.join("settings.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let dir = value.get("sessionDir")?.as_str()?.trim();
    if dir.is_empty() {
        return None;
    }
    let home = dirs::home_dir();
    Some(match (dir, home) {
        ("~", Some(home)) => home,
        (d, Some(home)) if d.starts_with("~/") => home.join(&d[2..]),
        (d, _) => PathBuf::from(d),
    })
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .collect()
}

/// Session files to inspect for `scope`, plus whether headers must match `cwd`.
fn candidate_files(agent_home: &Path, cwd: &Path, scope: Scope) -> (Vec<PathBuf>, bool) {
    if let Some(custom) = configured_session_dir(agent_home) {
        return (jsonl_files(&custom), scope == Scope::Project);
    }
    let root = agent_home.join("sessions");
    match scope {
        Scope::Project => (jsonl_files(&root.join(session_dir_name(cwd))), false),
        Scope::All => {
            let Ok(entries) = std::fs::read_dir(&root) else {
                return (Vec::new(), false);
            };
            let files = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .flat_map(|d| jsonl_files(&d))
                .collect();
            (files, false)
        }
    }
}

/// Sessions for `scope`, newest first. Blocking: call off the UI thread.
pub fn list_sessions(agent_home: &Path, cwd: &Path, scope: Scope) -> Vec<SessionSummary> {
    let (files, filter_cwd) = candidate_files(agent_home, cwd, scope);
    let mut sessions: Vec<SessionSummary> = read_summaries(&files)
        .into_iter()
        .filter(|s| !filter_cwd || s.cwd == cwd)
        .collect();
    sort_newest_first(&mut sessions);
    sessions
}

pub fn sort_newest_first(sessions: &mut [SessionSummary]) {
    sessions.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| b.path.cmp(&a.path))
    });
}

/// Parse `files` on a few threads; unreadable or non-session files are dropped.
fn read_summaries(files: &[PathBuf]) -> Vec<SessionSummary> {
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8)
        .min(files.len());
    if workers <= 1 {
        return files.iter().filter_map(|f| cached_summary(f)).collect();
    }
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(files.len()));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(i) else { break };
                    if let Some(summary) = cached_summary(file) {
                        out.lock().unwrap_or_else(|e| e.into_inner()).push(summary);
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// One JSONL line, keeping only the small fields we need. Unknown fields
/// (including message content) are skipped by serde without being built.
#[derive(Deserialize)]
struct LightEntry {
    #[serde(rename = "type")]
    kind: Option<Value>,
    id: Option<Value>,
    timestamp: Option<Value>,
    cwd: Option<Value>,
    name: Option<Value>,
    #[serde(rename = "modelId")]
    model_id: Option<Value>,
    message: Option<LightMessage>,
}

#[derive(Deserialize)]
struct LightMessage {
    role: Option<Value>,
    timestamp: Option<Value>,
    model: Option<Value>,
}

#[derive(Deserialize)]
struct ContentEntry {
    message: ContentMessage,
}

#[derive(Deserialize)]
struct ContentMessage {
    content: Value,
}

fn str_of(v: &Option<Value>) -> Option<&str> {
    v.as_ref().and_then(Value::as_str)
}

/// Text blocks of a message's `content` (string or block array), space-joined.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Summaries by file, so reopening the browser only parses new or changed
/// sessions. Holds non-session files too, as `None`.
static SUMMARIES: FileCache<Option<SessionSummary>> = FileCache::new(8_192);

fn cached_summary(path: &Path) -> Option<SessionSummary> {
    cached_summary_in(&SUMMARIES, path)
}

fn cached_summary_in(
    cache: &FileCache<Option<SessionSummary>>,
    path: &Path,
) -> Option<SessionSummary> {
    let file = File::open(path).ok()?;
    let Ok(meta) = file.metadata() else {
        return summarize(file, path).ok().flatten();
    };
    let stamp = Stamp::from_metadata(&meta);
    if let Some(summary) = cache.get(path, stamp) {
        return summary;
    }
    // A read error is not cached: the same bytes may read fine next time.
    let summary = summarize(file, path).ok()?;
    cache.insert(path, stamp, summary.clone());
    summary
}

/// Summarize one session file the way Pi's `buildSessionInfo` does.
/// `Ok(None)` for a file that is not a session.
fn summarize(file: File, path: &Path) -> std::io::Result<Option<SessionSummary>> {
    let mtime_ms = file
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut scanned = 0u64;
    let mut header: Option<(String, PathBuf, Option<i64>)> = None;
    let mut name = None;
    let mut first_message: Option<String> = None;
    let mut message_count = 0;
    let mut last_activity: Option<i64> = None;
    let mut model = None;
    let mut truncated = false;
    loop {
        if scanned >= MAX_SCAN_BYTES {
            truncated = !reader.fill_buf().map_or(true, |b| b.is_empty());
            break;
        }
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        scanned += read as u64;
        let Ok(entry) = serde_json::from_slice::<LightEntry>(&line) else {
            continue;
        };
        let kind = str_of(&entry.kind).unwrap_or_default();
        let Some(_) = &header else {
            // The first parsed entry must be the header.
            let Some(id) = str_of(&entry.id) else {
                return Ok(None);
            };
            if kind != "session" {
                return Ok(None);
            }
            let cwd = PathBuf::from(str_of(&entry.cwd).unwrap_or_default());
            header = Some((
                id.to_string(),
                cwd,
                str_of(&entry.timestamp).and_then(parse_iso_ms),
            ));
            continue;
        };
        match kind {
            "session_info" => {
                name = str_of(&entry.name)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string);
            }
            "model_change" => {
                if let Some(id) = str_of(&entry.model_id) {
                    model = Some(id.to_string());
                }
            }
            "message" => {
                message_count += 1;
                let Some(message) = &entry.message else {
                    continue;
                };
                let role = str_of(&message.role).unwrap_or_default();
                if role != "user" && role != "assistant" {
                    continue;
                }
                let at = message
                    .timestamp
                    .as_ref()
                    .and_then(Value::as_f64)
                    .map(|t| t as i64)
                    .or_else(|| str_of(&entry.timestamp).and_then(parse_iso_ms));
                if let Some(at) = at {
                    last_activity = Some(last_activity.map_or(at, |l| l.max(at)));
                }
                if role == "assistant" {
                    if let Some(m) = str_of(&message.model) {
                        model = Some(m.to_string());
                    }
                } else if first_message.is_none()
                    && let Ok(full) = serde_json::from_slice::<ContentEntry>(&line)
                {
                    let text = content_text(&full.message.content);
                    if !text.is_empty() {
                        first_message = Some(text);
                    }
                }
            }
            _ => {}
        }
    }
    let Some((id, cwd, created_ms)) = header else {
        return Ok(None);
    };
    let modified_ms = if truncated {
        mtime_ms.max(last_activity.unwrap_or(0))
    } else {
        last_activity
            .filter(|t| *t > 0)
            .or(created_ms)
            .unwrap_or(mtime_ms)
    };
    Ok(Some(SessionSummary {
        path: path.to_path_buf(),
        id,
        cwd,
        name,
        first_message,
        modified_ms,
        message_count,
        truncated,
        model,
    }))
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]` to Unix milliseconds.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    let mut i = 19;
    let mut ms = 0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let frac = s.get(start..i)?;
        let padded: String = frac.chars().chain("000".chars()).take(3).collect();
        ms = padded.parse::<i64>().ok()?;
    }
    let offset_min = match b.get(i) {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = num(i + 1..i + 3)?;
            let om = match b.get(i + 3) {
                Some(b':') => num(i + 4..i + 6)?,
                _ => num(i + 3..i + 5).unwrap_or(0),
            };
            let total = oh * 60 + om;
            if *sign == b'+' { total } else { -total }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some(((days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60) * 1000) + ms)
}

pub fn relative_time(now_ms: i64, then_ms: i64) -> String {
    let secs = (now_ms - then_ms).max(0) / 1000;
    match secs {
        0..60 => "just now".to_string(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        86_400..604_800 => format!("{}d ago", secs / 86_400),
        604_800..2_592_000 => format!("{}w ago", secs / 604_800),
        2_592_000..31_536_000 => format!("{}mo ago", secs / 2_592_000),
        _ => format!("{}y ago", secs / 31_536_000),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

pub struct SessionsView {
    cwd: PathBuf,
    current: Option<PathBuf>,
    scope: Scope,
    sessions: Vec<SessionSummary>,
    loading: bool,
    filter: Entity<InputState>,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SessionsEvent> for SessionsView {}

impl SessionsView {
    pub fn new(
        cwd: PathBuf,
        current_session_file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search by name, first message or path")
        });
        let subscriptions = vec![cx.subscribe_in(
            &filter,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event
                    && let Some(first) = this.visible(cx).first().map(|s| (*s).clone())
                {
                    this.open(&first, cx);
                }
                cx.notify();
            },
        )];
        let mut this = Self {
            cwd,
            current: current_session_file,
            scope: Scope::Project,
            sessions: Vec::new(),
            loading: false,
            filter,
            _load: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let agent = pi_rpc::resolve_homes().agent;
        let cwd = self.cwd.clone();
        let scope = self.scope;
        // Replacing the task drops (cancels) any load still in flight.
        self._load = cx.spawn(async move |this, cx| {
            let sessions = cx
                .background_executor()
                .spawn(async move { list_sessions(&agent, &cwd, scope) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.sessions = sessions;
                this.loading = false;
                cx.notify();
            });
        });
        cx.notify();
    }

    fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope != scope {
            self.scope = scope;
            self.sessions.clear();
            self.reload(cx);
        }
    }

    fn is_current(&self, session: &SessionSummary) -> bool {
        self.current
            .as_deref()
            .is_some_and(|c| same_file(c, &session.path))
    }

    fn visible(&self, cx: &App) -> Vec<&SessionSummary> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        self.sessions.iter().filter(|s| s.matches(&query)).collect()
    }

    fn open(&mut self, session: &SessionSummary, cx: &mut Context<Self>) {
        if self.is_current(session) {
            cx.emit(SessionsEvent::Close);
            return;
        }
        let cwd = if session.cwd.as_os_str().is_empty() {
            self.cwd.clone()
        } else {
            session.cwd.clone()
        };
        cx.emit(SessionsEvent::Open {
            path: session.path.clone(),
            cwd,
        });
    }

    fn render_row(
        &self,
        ix: usize,
        session: &SessionSummary,
        now: i64,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.is_current(session);
        let other_project = !session.cwd.as_os_str().is_empty() && session.cwd != self.cwd;
        let mut subtitle = Vec::new();
        if other_project || self.scope == Scope::All {
            let (prefix, leaf) = ui::split_path(&session.cwd);
            subtitle.push(format!("{prefix}{leaf}"));
        }
        if session.name.is_some()
            && let Some(first) = &session.first_message
        {
            subtitle.push(ui::first_line(first.trim(), 90));
        }
        let mut meta = Vec::new();
        if let Some(model) = &session.model {
            meta.push(model.clone());
        }
        meta.push(format!(
            "{}{} msg{}",
            session.message_count,
            if session.truncated { "+" } else { "" },
            if session.message_count == 1 { "" } else { "s" }
        ));
        meta.push(relative_time(now, session.modified_ms));
        let target = session.clone();
        let row = h_flex()
            .id(("session-row", ix))
            .relative()
            .px_3()
            .py_2()
            .gap_3()
            .rounded_lg()
            .cursor_pointer()
            .when(current, |d| d.bg(ui::raised()))
            .when(!current, |d| d.hover(|d| d.bg(ui::surface_hover())))
            .on_click(cx.listener(move |this, _, _, cx| this.open(&target, cx)))
            .when(current, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(10.))
                        .bottom(px(10.))
                        .w(px(2.))
                        .rounded_full()
                        .bg(ui::accent()),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(ui::well())
                    .border_1()
                    .border_color(ui::border_strong())
                    .child(
                        Icon::new(if current {
                            IconName::Check
                        } else {
                            IconName::FileText
                        })
                        .small()
                        .text_color(if current {
                            ui::accent()
                        } else {
                            ui::muted()
                        }),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(
                                        if session.name.is_some() || session.first_message.is_some()
                                        {
                                            ui::fg()
                                        } else {
                                            ui::faint()
                                        },
                                    )
                                    .child(session.title()),
                            )
                            .when(current, |d| {
                                d.child(ui::chip("current", ui::accent()).flex_shrink_0())
                            })
                            .when(other_project && self.scope == Scope::All, |d| {
                                d.child(ui::chip("new tab", ui::warning()).flex_shrink_0())
                            }),
                    )
                    .when(!subtitle.is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(subtitle.join(" · ")),
                        )
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(240.))
                    .truncate()
                    .text_xs()
                    .text_color(ui::faint())
                    .child(meta.join(" · ")),
            );
        ui::appear(("session-row-in", ix), div().child(row))
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let empty = |text: &'static str| {
            v_flex()
                .py_8()
                .items_center()
                .gap_2()
                .child(Icon::new(IconName::Inbox).text_color(ui::faint()))
                .child(div().text_sm().text_color(ui::muted()).child(text))
                .into_any_element()
        };
        if self.sessions.is_empty() {
            if self.loading {
                return h_flex()
                    .py_8()
                    .justify_center()
                    .gap_2()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(Spinner::new().small().color(ui::accent()))
                    .child("Reading sessions…")
                    .into_any_element();
            }
            return empty(match self.scope {
                Scope::Project => "No sessions for this project yet.",
                Scope::All => "No sessions found.",
            });
        }
        let visible = self.visible(cx);
        if visible.is_empty() {
            return empty("No sessions match.");
        }
        let hidden = visible.len().saturating_sub(MAX_ROWS);
        let now = now_ms();
        let rows: Vec<AnyElement> = visible
            .into_iter()
            .take(MAX_ROWS)
            .enumerate()
            .map(|(ix, s)| self.render_row(ix, s, now, cx).into_any_element())
            .collect();
        v_flex()
            .gap_0p5()
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .child(ui::section_label(match self.scope {
                        Scope::Project => "This project",
                        Scope::All => "All projects",
                    })),
            )
            .children(rows)
            .when(hidden > 0, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(ui::faint())
                        .child(format!("{hidden} more — refine the search")),
                )
            })
            .into_any_element()
    }
}

impl Render for SessionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all = self.scope == Scope::All;
        let count = self.sessions.len();
        let toolbar = h_flex()
            .gap_2()
            .child(
                ui::field()
                    .flex_1()
                    .min_w_0()
                    .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
                    .child(Input::new(&self.filter).appearance(false).small()),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded_lg()
                    .bg(ui::well())
                    .border_1()
                    .border_color(ui::border())
                    .children(
                        [
                            (Scope::Project, IconName::Folder, "This project"),
                            (Scope::All, IconName::Globe, "All"),
                        ]
                        .map(|(scope, icon, label)| {
                            let active = self.scope == scope;
                            h_flex()
                                .id(SharedString::from(format!("sessions-scope-{label}")))
                                .gap_1p5()
                                .px_2p5()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_xs()
                                .whitespace_nowrap()
                                .when(active, |d| d.bg(ui::raised()).text_color(ui::fg()))
                                .when(!active, |d| {
                                    d.text_color(ui::muted()).hover(|d| d.text_color(ui::fg()))
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.set_scope(scope, cx)),
                                )
                                .child(Icon::new(icon).xsmall())
                                .child(label)
                        }),
                    ),
            )
            .child(
                Button::new("sessions-refresh")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::RefreshCw))
                    .tooltip("Reload")
                    .loading(self.loading && count > 0)
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            );
        let (prefix, leaf) = ui::split_path(&self.cwd);
        let hint = |key: &'static str, label: &'static str| {
            h_flex()
                .gap_1p5()
                .text_xs()
                .text_color(ui::muted())
                .child(ui::kbd(key))
                .child(label)
        };
        let (backdrop, panel) = ui::screen(Some(760.));
        backdrop.occlude().child(ui::pop_in(
            "sessions-screen",
            panel
                .w_full()
                .max_w(px(760.))
                .child(
                    ui::screen_header()
                        .child(
                            Icon::new(IconName::FolderOpen)
                                .small()
                                .text_color(ui::muted()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Sessions"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(if all {
                                    "Every project".to_string()
                                } else {
                                    format!("{prefix}{leaf}")
                                }),
                        )
                        .when(count > 0, |d| {
                            d.child(ui::chip(format!("{count}"), ui::muted()).flex_shrink_0())
                        })
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("sessions-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionsEvent::Close))),
                        ),
                )
                .child(div().px_4().pt_4().pb_3().child(toolbar))
                .child(
                    div()
                        .id("sessions-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px_2()
                        .pb_3()
                        .child(self.render_list(cx)),
                )
                .child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap_4()
                        .border_t_1()
                        .border_color(ui::border())
                        .child(hint("↵", "open first match"))
                        .child(hint("esc", "close"))
                        .child(div().flex_1())
                        .child(
                            Button::new("sessions-new")
                                .small()
                                .primary()
                                .icon(Icon::new(IconName::Plus))
                                .label("New session")
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(SessionsEvent::NewSession)),
                                ),
                        ),
                ),
        ))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that re-exports `gpui_kit::*`, whose `test` attribute
    // shadows the built-in one and recurses.
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use pi_rpc::file_cache::FileCache;

    use super::{
        Scope, SessionSummary, cached_summary_in, list_sessions, parse_iso_ms, relative_time,
        session_dir_name, summarize,
    };

    fn read_summary(path: &Path) -> Option<SessionSummary> {
        summarize(std::fs::File::open(path).ok()?, path)
            .ok()
            .flatten()
    }

    #[test]
    fn summaries_are_cached_by_mtime_and_len() {
        use std::time::Duration;
        let tmp = TempDir::new("cache");
        let set_mtime = |path: &Path, secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        let cache = FileCache::new(8);
        let head = header("abc", "/w", "2024-12-03T14:00:00.000Z");
        let path = write_session(&tmp.0, "a.jsonl", &[head.clone(), user("u1", "aaaa", 1)]);
        set_mtime(&path, 6_000_000);
        let first = cached_summary_in(&cache, &path).unwrap();
        assert_eq!(first.first_message.as_deref(), Some("aaaa"));

        // Same mtime and length: the cached summary, not the new text.
        write_session(&tmp.0, "a.jsonl", &[head.clone(), user("u1", "bbbb", 1)]);
        set_mtime(&path, 6_000_000);
        assert_eq!(cached_summary_in(&cache, &path), Some(first));

        set_mtime(&path, 6_000_001);
        let second = cached_summary_in(&cache, &path).unwrap();
        assert_eq!(second.first_message.as_deref(), Some("bbbb"));
        assert_eq!(Some(second), read_summary(&path));

        // An appended turn changes the length.
        write_session(
            &tmp.0,
            "a.jsonl",
            &[head, user("u1", "bbbb", 1), user("u2", "c", 2)],
        );
        set_mtime(&path, 6_000_001);
        assert_eq!(cached_summary_in(&cache, &path).unwrap().message_count, 2);

        // Non-session files are cached as such.
        let other = write_session(&tmp.0, "b.jsonl", &[user("u1", "hi", 1)]);
        assert_eq!(cached_summary_in(&cache, &other), None);
        assert_eq!(
            cached_summary_in(&cache, &tmp.0.join("missing.jsonl")),
            None
        );
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("gs-sessions-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn header(id: &str, cwd: &str, ts: &str) -> String {
        format!(r#"{{"type":"session","version":3,"id":"{id}","timestamp":"{ts}","cwd":"{cwd}"}}"#)
    }

    fn user(id: &str, text: &str, ts_ms: i64) -> String {
        format!(
            r#"{{"type":"message","id":"{id}","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}},{{"type":"image","data":"AAAA","mimeType":"image/png"}}],"timestamp":{ts_ms}}}}}"#
        )
    }

    fn write_session(dir: &Path, file: &str, lines: &[String]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(file);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    #[test]
    fn parses_synthetic_session() {
        let tmp = TempDir::new("parse");
        let lines = vec![
            header("abc-123", "/work/proj", "2024-12-03T14:00:00.000Z"),
            r#"{"type":"message","id":"s1","parentId":null,"timestamp":"2024-12-03T14:00:00.500Z","message":{"role":"system","content":"","sections":{"preamble":"x"},"timestamp":1733234400500}}"#.to_string(),
            "not json at all".to_string(),
            String::new(),
            // JSON-escaped newline: the stored text is "Fix the login bug\nplease".
            user("u1", "Fix the login bug\\nplease", 1_733_234_401_000),
            r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2024-12-03T14:00:02.000Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"c","name":"bash","arguments":{"model":"decoy"}}],"api":"anthropic-messages","provider":"anthropic","model":"claude-sonnet-4-5","stopReason":"stop","timestamp":1733234402000}}"#.to_string(),
            r#"{"type":"message","id":"t1","parentId":"a1","timestamp":"2024-12-03T14:00:03.000Z","message":{"role":"toolResult","toolCallId":"c","toolName":"bash","content":[{"type":"text","text":"ok"}],"isError":false,"timestamp":1733234499000}}"#.to_string(),
            r#"{"type":"session_info","id":"i1","parentId":"t1","timestamp":"2024-12-03T14:01:00.000Z","name":"  Auth fixes  "}"#.to_string(),
            user("u2", "second question", 1_733_234_410_000),
        ];
        let path = write_session(&tmp.0, "2024-12-03T14-00-00-000Z_abc-123.jsonl", &lines);
        let s = read_summary(&path).expect("session parses");
        assert_eq!(s.id, "abc-123");
        assert_eq!(s.cwd, PathBuf::from("/work/proj"));
        assert_eq!(s.name.as_deref(), Some("Auth fixes"));
        assert_eq!(
            s.first_message.as_deref(),
            Some("Fix the login bug\nplease")
        );
        assert_eq!(s.message_count, 5);
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-5"));
        // Tool results do not count as activity; the last user message does.
        assert_eq!(s.modified_ms, 1_733_234_410_000);
        assert!(!s.truncated);
        assert_eq!(s.title(), "Auth fixes");
        assert!(s.matches("login"));
        assert!(s.matches("auth"));
        assert!(s.matches("work/proj"));
        assert!(!s.matches("kubernetes"));
    }

    #[test]
    fn later_empty_name_clears_and_model_change_counts() {
        let tmp = TempDir::new("clear");
        let lines = vec![
            header("x", "/p", "2024-12-03T14:00:00.000Z"),
            r#"{"type":"session_info","id":"i1","parentId":null,"timestamp":"2024-12-03T14:01:00.000Z","name":"Old"}"#.to_string(),
            r#"{"type":"session_info","id":"i2","parentId":"i1","timestamp":"2024-12-03T14:02:00.000Z","name":""}"#.to_string(),
            r#"{"type":"model_change","id":"m1","parentId":"i2","timestamp":"2024-12-03T14:05:00.000Z","provider":"openai","modelId":"gpt-4o"}"#.to_string(),
        ];
        let s = read_summary(&write_session(&tmp.0, "a.jsonl", &lines)).unwrap();
        assert_eq!(s.name, None);
        assert_eq!(s.first_message, None);
        assert_eq!(s.model.as_deref(), Some("gpt-4o"));
        assert_eq!(s.message_count, 0);
        // No activity: falls back to the header timestamp.
        assert_eq!(s.modified_ms, 1_733_234_400_000);
        assert_eq!(s.title(), "(no messages)");
    }

    #[test]
    fn rejects_files_without_header() {
        let tmp = TempDir::new("noheader");
        let path = write_session(&tmp.0, "b.jsonl", &[user("u1", "hi", 1)]);
        assert_eq!(read_summary(&path), None);
        let empty = write_session(&tmp.0, "c.jsonl", &[]);
        assert_eq!(read_summary(&empty), None);
    }

    #[test]
    fn lists_project_and_all_sorted_newest_first() {
        let tmp = TempDir::new("list");
        let agent = tmp.0.join("agent");
        let cwd = PathBuf::from("/work/proj");
        let project = agent.join("sessions").join(session_dir_name(&cwd));
        let other = agent.join("sessions").join("--work-other--");
        write_session(
            &project,
            "old.jsonl",
            &[
                header("old", "/work/proj", "2024-01-01T00:00:00Z"),
                user("u", "older", 1_000_000),
            ],
        );
        write_session(
            &project,
            "new.jsonl",
            &[
                header("new", "/work/proj", "2024-01-01T00:00:00Z"),
                user("u", "newer", 3_000_000),
            ],
        );
        write_session(&project, "notes.txt", &["ignored".to_string()]);
        write_session(
            &project,
            "junk.jsonl",
            &["{\"type\":\"message\"}".to_string()],
        );
        write_session(
            &other,
            "x.jsonl",
            &[
                header("other", "/work/other", "2024-01-01T00:00:00Z"),
                user("u", "elsewhere", 2_000_000),
            ],
        );

        let ids = |v: Vec<SessionSummary>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids(list_sessions(&agent, &cwd, Scope::Project)),
            ["new", "old"]
        );
        assert_eq!(
            ids(list_sessions(&agent, &cwd, Scope::All)),
            ["new", "other", "old"]
        );
        assert!(list_sessions(&agent, Path::new("/nowhere"), Scope::Project).is_empty());
    }

    #[test]
    fn custom_session_dir_filters_by_header_cwd() {
        let tmp = TempDir::new("custom");
        let agent = tmp.0.join("agent");
        let flat = tmp.0.join("flat");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("settings.json"),
            format!(
                "{{\"sessionDir\": {}}}",
                serde_json::to_string(&flat.to_string_lossy()).unwrap()
            ),
        )
        .unwrap();
        write_session(
            &flat,
            "a.jsonl",
            &[header("mine", "/work/proj", "2024-01-01T00:00:00Z")],
        );
        write_session(
            &flat,
            "b.jsonl",
            &[header("theirs", "/work/other", "2024-01-02T00:00:00Z")],
        );
        let cwd = Path::new("/work/proj");
        let ids = |v: Vec<SessionSummary>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(ids(list_sessions(&agent, cwd, Scope::Project)), ["mine"]);
        assert_eq!(
            ids(list_sessions(&agent, cwd, Scope::All)),
            ["theirs", "mine"]
        );
    }

    #[test]
    fn encodes_cwd_like_pi() {
        assert_eq!(
            session_dir_name(Path::new(
                "/Users/ismaelosuna/Documents/Git/personal/gentle-shell-gpui"
            )),
            "--Users-ismaelosuna-Documents-Git-personal-gentle-shell-gpui--"
        );
        assert_eq!(session_dir_name(Path::new("C:\\work\\x")), "--C--work-x--");
    }

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(
            parse_iso_ms("2024-12-03T14:00:00.000Z"),
            Some(1_733_234_400_000)
        );
        assert_eq!(
            parse_iso_ms("2024-12-03T14:00:00Z"),
            Some(1_733_234_400_000)
        );
        assert_eq!(
            parse_iso_ms("2024-12-03T14:00:00.5Z"),
            Some(1_733_234_400_500)
        );
        assert_eq!(
            parse_iso_ms("2024-12-03T16:00:00+02:00"),
            Some(1_733_234_400_000)
        );
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_ms("yesterday"), None);
    }

    #[test]
    fn formats_relative_time() {
        let now = 1_000_000_000_000;
        assert_eq!(relative_time(now, now + 5_000), "just now");
        assert_eq!(relative_time(now, now - 30_000), "just now");
        assert_eq!(relative_time(now, now - 5 * 60_000), "5m ago");
        assert_eq!(relative_time(now, now - 3 * 3_600_000), "3h ago");
        assert_eq!(relative_time(now, now - 2 * 86_400_000), "2d ago");
        assert_eq!(relative_time(now, now - 14 * 86_400_000), "2w ago");
        assert_eq!(relative_time(now, now - 400 * 86_400_000), "1y ago");
    }
}
