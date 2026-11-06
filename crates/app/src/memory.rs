//! Memory: browse and search Engram, Gentle Shell's persistent memory, over
//! its local HTTP API (`gentle-engram` talks to the same server). Read-only:
//! the agent owns writes through `mem_save` / `mem_update`.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::Deserialize;

use crate::ui;

pub enum MemoryEvent {
    Close,
}

/// Engram's `store.Observation` (internal/store/store.go), the fields shown here.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Observation {
    pub id: i64,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    pub project: Option<String>,
    #[serde(default)]
    pub scope: String,
    pub topic_key: Option<String>,
    #[serde(default)]
    pub revision_count: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// `ENGRAM_URL`, else `ENGRAM_PORT` on loopback, else Engram's default port.
fn base_url() -> String {
    if let Ok(url) = std::env::var("ENGRAM_URL")
        && !url.trim().is_empty()
    {
        return url.trim_end_matches('/').to_string();
    }
    let port = std::env::var("ENGRAM_PORT")
        .ok()
        .filter(|p| p.parse::<u16>().is_ok())
        .unwrap_or_else(|| "7437".into());
    format!("http://127.0.0.1:{port}")
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn get(path: &str) -> Result<serde_json::Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into();
    let mut request = agent.get(format!("{}{path}", base_url()));
    if let Ok(token) = std::env::var("ENGRAM_HTTP_TOKEN")
        && !token.is_empty()
    {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|_| {
        "Engram is not running. Gentle Shell starts it with the first session.".to_string()
    })?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| format!("Engram answered {status} without JSON"))?;
    if !(200..300).contains(&status) {
        return Err(json["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Engram answered {status}")));
    }
    Ok(json)
}

/// The project Engram assigns to `cwd` (`GET /project/current`).
fn current_project(cwd: &std::path::Path) -> Option<String> {
    let json = get(&format!(
        "/project/current?cwd={}",
        encode(&cwd.to_string_lossy())
    ))
    .ok()?;
    json["project"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(str::to_string)
}

pub fn parse_observations(value: serde_json::Value) -> Vec<Observation> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        _ => Vec::new(),
    }
}

fn load(project: Option<&str>, query: &str) -> Result<Vec<Observation>, String> {
    let scope = match project {
        Some(p) => format!("project={}", encode(p)),
        None => "all_projects=true".into(),
    };
    let path = if query.trim().is_empty() {
        format!("/observations/recent?{scope}&limit=60")
    } else {
        format!("/search?q={}&{scope}&limit=40", encode(query.trim()))
    };
    match get(&path) {
        Ok(value) => Ok(parse_observations(value)),
        // A project Engram has never seen simply has no memories yet.
        Err(error) if project.is_some() && error.contains("not found") => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub struct MemoryView {
    cwd: PathBuf,
    project: Option<String>,
    all_projects: bool,
    query: Entity<InputState>,
    items: Vec<Observation>,
    selected: Option<i64>,
    error: Option<String>,
    loading: bool,
    _load: Task<()>,
    _subscription: Subscription,
}

impl EventEmitter<MemoryEvent> for MemoryView {}

impl MemoryView {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search memory (Enter)"));
        let subscription = cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.reload(cx);
            }
        });
        let mut this = Self {
            cwd,
            project: None,
            all_projects: false,
            query,
            items: Vec::new(),
            selected: None,
            error: None,
            loading: false,
            _load: Task::ready(()),
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let cwd = self.cwd.clone();
        let known = self.project.clone();
        let all = self.all_projects;
        let query = self.query.read(cx).value().to_string();
        self.loading = true;
        self._load = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let project = known.or_else(|| current_project(&cwd));
                    let items = load(if all { None } else { project.as_deref() }, &query);
                    (project, items)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.project = result.0;
                match result.1 {
                    Ok(items) => {
                        this.error = None;
                        if !this
                            .selected
                            .is_some_and(|id| items.iter().any(|o| o.id == id))
                        {
                            this.selected = items.first().map(|o| o.id);
                        }
                        this.items = items;
                    }
                    Err(error) => {
                        this.error = Some(error);
                        this.items.clear();
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }
}

impl Render for MemoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .selected
            .and_then(|id| self.items.iter().find(|o| o.id == id))
            .cloned();
        let list =
            v_flex()
                .gap_0p5()
                .children(self.items.iter().enumerate().map(|(i, o)| {
                    let id = o.id;
                    let active = self.selected == Some(id);
                    let row =
                        v_flex()
                            .id(("memory", id as u64))
                            .px_3()
                            .py_2()
                            .gap_1()
                            .rounded_lg()
                            .cursor_pointer()
                            .border_1()
                            .border_color(transparent_black())
                            .when(active, |d| {
                                d.bg(ui::raised()).border_color(ui::border_strong())
                            })
                            .when(!active, |d| d.hover(|d| d.bg(ui::surface_hover())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(id);
                                cx.notify();
                            }))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .child(ui::chip(o.kind.clone(), ui::petal()).flex_shrink_0())
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .text_color(ui::fg())
                                            .child(o.title.clone()),
                                    ),
                            )
                            .child(div().truncate().text_xs().text_color(ui::faint()).child(
                                format!(
                        "{}{}",
                        o.updated_at
                            .get(..16)
                            .unwrap_or(&o.updated_at)
                            .replace('T', " "),
                        o.project
                            .as_ref()
                            .filter(|_| self.all_projects)
                            .map(|p| format!(" · {p}"))
                            .unwrap_or_default()
                    ),
                            ));
                    ui::appear(("memory-row", i), div().child(row))
                }));

        let body: AnyElement = if let Some(error) = &self.error {
            div()
                .p_4()
                .child(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(ui::danger().opacity(0.08))
                        .border_1()
                        .border_color(ui::danger().opacity(0.22))
                        .child(
                            Icon::new(IconName::CircleAlert)
                                .small()
                                .text_color(ui::danger())
                                .mt_0p5(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(ui::danger())
                                .child(error.clone()),
                        ),
                )
                .into_any_element()
        } else if self.items.is_empty() && !self.loading {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(Icon::new(IconName::Brain).text_color(ui::faint()))
                .child(
                    div()
                        .max_w(px(420.))
                        .text_center()
                        .text_sm()
                        .text_color(ui::muted())
                        .child(if self.query.read(cx).value().is_empty() {
                            "Nothing remembered for this project yet. el Gentleman saves decisions, bugs and discoveries here as you work."
                        } else {
                            "No memories match."
                        }),
                )
                .into_any_element()
        } else {
            h_flex()
                .size_full()
                .items_start()
                .child(
                    div()
                        .id("memory-list")
                        .w(relative(0.4))
                        .min_w(px(220.))
                        .max_w(px(340.))
                        .h_full()
                        .flex_shrink_0()
                        .overflow_y_scroll()
                        .p_2()
                        .child(list),
                )
                .child(
                    v_flex()
                        .id("memory-detail")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .p_5()
                        .gap_3()
                        .border_l_1()
                        .border_color(ui::border())
                        .when_some(selected, |d, o| {
                            d.child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui::fg())
                                    .child(o.title.clone()),
                            )
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .child(ui::chip(format!("#{}", o.id), ui::faint()))
                                    .child(ui::chip(o.kind.clone(), ui::petal()))
                                    .when(!o.scope.is_empty(), |d| {
                                        d.child(ui::chip(o.scope.clone(), ui::info()))
                                    })
                                    .when_some(o.topic_key.clone(), |d, k| {
                                        d.child(ui::chip(k, ui::champagne()))
                                    })
                                    .when(o.revision_count > 0, |d| {
                                        d.child(ui::chip(
                                            format!("{} revisions", o.revision_count),
                                            ui::muted(),
                                        ))
                                    }),
                            )
                            .child(
                                ui::card().p_4().child(
                                    div()
                                        .text_sm()
                                        .text_color(ui::fg())
                                        .whitespace_normal()
                                        .child(o.content.clone()),
                                ),
                            )
                        }),
                )
                .into_any_element()
        };

        let all = self.all_projects;
        let (backdrop, panel) = ui::screen(None);
        backdrop.occlude().child(ui::pop_in(
            "memory-screen",
            panel
                .max_w(px(1100.))
                .child(
                    ui::screen_header()
                        .child(Icon::new(IconName::Brain).small().text_color(ui::petal()))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Memory"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(match (&self.project, all) {
                                    (_, true) => "Engram · all projects".to_string(),
                                    (Some(p), false) => format!("Engram · {p}"),
                                    (None, false) => "Engram".to_string(),
                                }),
                        )
                        .when(self.loading, |d| {
                            d.child(Spinner::new().small().color(ui::accent()))
                        })
                        .child(ui::kbd("esc"))
                        .child(
                            Button::new("memory-close")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::X))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(MemoryEvent::Close))),
                        ),
                )
                .child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(ui::border())
                        .child(
                            ui::field()
                                .flex_1()
                                .min_w_0()
                                .child(Icon::new(IconName::Search).small().text_color(ui::faint()))
                                .child(Input::new(&self.query).appearance(false).small()),
                        )
                        .child(ui::segmented(
                            "memory-scope",
                            &["This project", "All projects"],
                            Some(usize::from(all)),
                            cx.listener(|this, index: &usize, _, cx| {
                                let all = *index == 1;
                                if this.all_projects != all {
                                    this.all_projects = all;
                                    this.reload(cx);
                                }
                            }),
                        ))
                        .child(
                            Button::new("memory-refresh")
                                .ghost()
                                .small()
                                .icon(Icon::new(IconName::RefreshCw))
                                .tooltip("Reload")
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        ),
                )
                .child(div().flex_1().min_h_0().child(body)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{encode, parse_observations};

    #[test]
    fn observations_and_search_results_parse() {
        // `/search` returns SearchResult = Observation + rank, flattened.
        let value = serde_json::json!([
            {"id": 7, "sync_id": "s", "session_id": "x", "type": "decision", "title": "Use GPUI", "content": "Because…",
             "project": "gentle-shell-gpui", "scope": "project", "topic_key": "arch/ui", "revision_count": 2,
             "duplicate_count": 0, "created_at": "2026-11-06T10:00:00Z", "updated_at": "2026-11-06T10:05:00Z", "rank": -1.5},
            {"unexpected": true}
        ]);
        let items = parse_observations(value);
        assert_eq!(items.len(), 1);
        assert_eq!(
            (
                items[0].id,
                items[0].kind.as_str(),
                items[0].topic_key.as_deref()
            ),
            (7, "decision", Some("arch/ui"))
        );
        assert!(parse_observations(serde_json::json!({"error": "x"})).is_empty());
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("/Users/a b/c&d"), "%2FUsers%2Fa%20b%2Fc%26d");
    }
}
