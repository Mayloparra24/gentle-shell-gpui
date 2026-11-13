//! The Agents panel: Gentle Agents subagents for this session, Tern-style.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Selectable as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::SessionView;
use crate::shared::ui;
use gentle_core::activity::{AgentTask, ThreadItem};
use gentle_core::gentle_state::{Job, JobOutcome};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsTab {
    #[default]
    Agents,
    Activity,
    Jobs,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentsFilter {
    #[default]
    All,
    Active,
    Finished,
}

impl AgentsFilter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Active,
            Self::Active => Self::Finished,
            Self::Finished => Self::All,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::All => "Filter: all",
            Self::Active => "Filter: active",
            Self::Finished => "Filter: finished",
        }
    }
    fn keeps(self, task: &AgentTask) -> bool {
        match self {
            Self::All => true,
            Self::Active => task.summary.is_active(),
            Self::Finished => !task.summary.is_active(),
        }
    }
}

/// Panel state kept on the session.
#[derive(Default)]
pub struct AgentsPanel {
    pub open: bool,
    pub tab: AgentsTab,
    pub filter: AgentsFilter,
    pub selected: Option<String>,
    pub expanded_tools: std::collections::HashSet<(String, usize)>,
}

pub fn status_color(status: &str) -> Hsla {
    match status {
        "running" => ui::warning(),
        "waiting" | "queued" => ui::accent(),
        "done" | "finished" | "completed" | "succeeded" => ui::success(),
        "failed" | "error" | "aborted" | "cancelled" | "stopped" => ui::danger(),
        _ => ui::muted(),
    }
}

fn duration(ms: Option<u64>) -> String {
    ms.map(ui::format_duration).unwrap_or_else(|| "–".into())
}

/// One background job: status glyph, command and exit code. The Jobs chip's
/// card and the Jobs tab both list jobs with it.
pub(super) fn job_row(job: &Job) -> AnyElement {
    let (glyph, color) = match job.outcome() {
        JobOutcome::Running => ("⧗", ui::accent()),
        JobOutcome::Succeeded => ("✓", ui::success()),
        JobOutcome::Failed => ("✗", ui::danger()),
    };
    h_flex()
        .gap_2()
        .text_size(px(12.))
        .child(div().flex_none().text_color(color).child(glyph))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(ui::MONO)
                .text_color(ui::fg())
                .child(job.display_label()),
        )
        .child(
            div()
                .flex_none()
                .text_color(ui::faint())
                .child(job.state_text()),
        )
        .into_any_element()
}

/// What a tab shows while it has nothing to list.
fn empty_state(icon: IconName, text: &'static str) -> AnyElement {
    v_flex()
        .pt_10()
        .items_center()
        .gap_2()
        .child(Icon::new(icon).text_color(ui::faint()))
        .child(
            div()
                .max_w(px(360.))
                .text_sm()
                .text_center()
                .text_color(ui::muted())
                .child(text),
        )
        .into_any_element()
}

/// Wall-clock milliseconds, the unit of the activity document's timestamps.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl SessionView {
    pub(super) fn open_agents(&mut self, select: Option<String>, cx: &mut Context<Self>) {
        self.agents_panel.open = true;
        if select.is_some() {
            self.agents_panel.selected = select;
        }
        // Tick running times once a second while the panel is open.
        self._agents_ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |this, cx| {
                        let open = this.agents_panel.open;
                        if open
                            && this
                                .activity
                                .agents
                                .as_ref()
                                .is_some_and(|a| a.tasks.iter().any(|t| t.summary.is_active()))
                        {
                            cx.notify();
                        }
                        open
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        });
        cx.notify();
    }

    pub(super) fn close_agents(&mut self, cx: &mut Context<Self>) {
        self.agents_panel.open = false;
        self._agents_ticker = Task::ready(());
        cx.notify();
    }

    /// Gentle Shell has no host-side stop for helpers; ask the agent to cancel it.
    fn ask_to_stop_agent(
        &mut self,
        task: &gentle_core::activity::TaskSummary,
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

    fn selected_task(&self) -> Option<&AgentTask> {
        let tasks = &self.activity.agents.as_ref()?.tasks;
        self.agents_panel
            .selected
            .as_ref()
            .and_then(|id| tasks.iter().find(|t| &t.summary.id == id))
            .or_else(|| tasks.first())
    }

    pub(super) fn render_agents_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.agents_panel.open {
            return None;
        }
        let empty = Vec::new();
        let tasks = self.activity.agents.as_ref().map_or(&empty, |a| &a.tasks);
        let summary = self
            .activity
            .agents
            .as_ref()
            .map(|a| a.summary)
            .unwrap_or_default();
        let now = now_ms();
        let filter = self.agents_panel.filter;
        let visible: Vec<&AgentTask> = tasks.iter().filter(|t| filter.keeps(t)).collect();
        let selected = self.selected_task();
        let selected_id = selected.map(|t| t.summary.id.clone());
        let total_turns: u64 = tasks.iter().map(|t| t.summary.turns).sum();
        let total_tools: u64 = tasks.iter().map(|t| t.summary.tool_calls).sum();
        let tab = self.agents_panel.tab;
        let on_jobs = tab == AgentsTab::Jobs;
        let jobs = &self.activity.jobs;
        let jobs_running = jobs.iter().filter(|j| j.is_running()).count();
        let (summary_line, running_now, count) = if on_jobs {
            (
                format!(
                    "{jobs_running} running · {} finished",
                    jobs.len() - jobs_running
                ),
                jobs_running > 0,
                jobs.len(),
            )
        } else {
            (
                format!(
                    "{} running · {} waiting · {} queued · {} finished · {total_turns} turns · {total_tools} tools",
                    summary.running, summary.waiting, summary.queued, summary.finished
                ),
                summary.running > 0,
                tasks.len(),
            )
        };

        let header = ui::screen_header()
            .child(Icon::new(IconName::Users).small().text_color(ui::muted()))
            .child(
                div()
                    .flex_shrink_0()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::fg())
                    .child("Agents"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(ui::muted())
                    .child(summary_line),
            )
            .when(running_now, |d| {
                d.child(ui::pulse(
                    div()
                        .flex_shrink_0()
                        .size(px(6.))
                        .rounded_full()
                        .bg(ui::warning()),
                ))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .font_family(ui::MONO)
                    .text_color(ui::faint())
                    .child(count.to_string()),
            )
            .child(
                Button::new("agents-close")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::X))
                    .on_click(cx.listener(|this, _, _, cx| this.close_agents(cx))),
            );

        let tab_button = |id: &'static str, label: &'static str, tab: AgentsTab| {
            let active = self.agents_panel.tab == tab;
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(active, |d| d.bg(ui::raised()).text_color(ui::fg()))
                .when(!active, |d| {
                    d.text_color(ui::muted()).hover(|d| d.text_color(ui::fg()))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.agents_panel.tab = tab;
                    cx.notify();
                }))
                .child(label)
        };
        let tabs = h_flex()
            .self_start()
            .p(px(2.))
            .gap(px(2.))
            .rounded_lg()
            .bg(ui::well())
            .border_1()
            .border_color(ui::border())
            .child(tab_button("agents-tab", "Agents", AgentsTab::Agents))
            .child(tab_button("activity-tab", "Activity", AgentsTab::Activity))
            .child(tab_button("jobs-tab", "Jobs", AgentsTab::Jobs));

        let no_subagents = || {
            empty_state(
                IconName::Bot,
                "No subagents yet. When el Gentleman delegates work, helpers appear here live.",
            )
        };
        let left_body = match tab {
            AgentsTab::Agents if tasks.is_empty() => no_subagents(),
            AgentsTab::Agents => {
                self.render_agents_table(&visible, selected_id.as_deref(), now, cx)
            }
            AgentsTab::Activity => match selected {
                Some(task) => self.render_thread(task, cx),
                None => no_subagents(),
            },
            AgentsTab::Jobs if jobs.is_empty() => empty_state(
                IconName::Activity,
                "No background jobs yet. Commands the agent runs in the background show up here.",
            ),
            AgentsTab::Jobs => v_flex()
                .gap_0p5()
                .children(
                    jobs.iter()
                        .map(|job| div().px_3().py_1p5().child(job_row(job))),
                )
                .into_any_element(),
        };

        let left = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_3()
            .gap_3()
            .child(tabs)
            .child(
                div()
                    .id("agents-left")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(left_body),
            );

        let right = v_flex()
            .id("agents-detail")
            .w(relative(0.36))
            .min_w(px(240.))
            .max_w(px(420.))
            .flex_shrink_0()
            .h_full()
            .p_5()
            .gap_3()
            .border_l_1()
            .border_color(ui::border())
            .overflow_y_scroll()
            .children(selected.map(|task| self.render_agent_detail(task, now)));

        let footer = h_flex()
            .px_4()
            .py_3()
            .gap_2()
            .border_t_1()
            .border_color(ui::border())
            .when_some(selected.filter(|t| !on_jobs && t.summary.is_active()).map(|t| t.summary.clone()), |d, task| {
                d.child(
                    Button::new("agents-stop")
                        .danger()
                        .outline()
                        .small()
                        .icon(Icon::new(IconName::OctagonX))
                        .label("Ask to stop")
                        .tooltip("Gentle Shell stops helpers from its agent: this asks it to run subagent_cancel")
                        .on_click(cx.listener(move |this, _, _, cx| this.ask_to_stop_agent(&task, cx))),
                )
            })
            .when(!on_jobs, |d| {
                d.child(
                    Button::new("agents-filter")
                        .ghost()
                        .small()
                        .icon(Icon::new(IconName::Funnel))
                        .label(filter.label())
                        .selected(filter != AgentsFilter::All)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agents_panel.filter = this.agents_panel.filter.next();
                            cx.notify();
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("agents-close")
                    .ghost()
                    .small()
                    .label("Close")
                    .on_click(cx.listener(|this, _, _, cx| this.close_agents(cx))),
            )
            .when(!on_jobs, |d| {
                d.child(
                    Button::new("agents-transcript")
                        .primary()
                        .small()
                        .icon(Icon::new(if tab == AgentsTab::Activity {
                            IconName::ArrowLeft
                        } else {
                            IconName::MessageSquare
                        }))
                        .label(if tab == AgentsTab::Activity { "Back to agents" } else { "Open transcript" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agents_panel.tab = match this.agents_panel.tab {
                                AgentsTab::Agents => AgentsTab::Activity,
                                AgentsTab::Activity | AgentsTab::Jobs => AgentsTab::Agents,
                            };
                            cx.notify();
                        })),
                )
            });

        let (backdrop, panel) = ui::screen(None);
        Some(
            backdrop
                .id("agents-overlay")
                .occlude()
                .child(ui::pop_in(
                    "agents-screen",
                    panel
                        .child(header)
                        .child(
                            h_flex()
                                .flex_1()
                                .min_h_0()
                                .child(left)
                                // A job has no detail pane: the list takes the width.
                                .when(!on_jobs, |d| d.child(right)),
                        )
                        .child(footer),
                ))
                .into_any_element(),
        )
    }

    fn render_agents_table(
        &self,
        tasks: &[&AgentTask],
        selected: Option<&str>,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let column = |w: f32, text: String| div().w(px(w)).flex_shrink_0().text_right().child(text);
        let head = h_flex()
            .px_3()
            .pb_1()
            .gap_3()
            .text_xs()
            .text_color(ui::faint())
            .child(div().w(px(10.)).flex_shrink_0())
            .child(div().flex_1().min_w_0().child("Agent"))
            .child(column(72., "Status".into()))
            .child(column(56., "Time".into()))
            .child(column(40., "Turns".into()))
            .child(column(40., "Tools".into()));
        let rows = tasks.iter().enumerate().map(|(ix, task)| {
            let s = &task.summary;
            let id = s.id.clone();
            let is_selected = selected == Some(s.id.as_str());
            let color = status_color(&s.status);
            let (name, kind) = match &s.label {
                Some(label) if !label.is_empty() => (label.clone(), Some(s.agent.clone())),
                _ => (s.agent.clone(), None),
            };
            let row = h_flex()
                .id(SharedString::from(format!("agent-row-{}", s.id)))
                .px_3()
                .py_2()
                .gap_3()
                .rounded_lg()
                .cursor_pointer()
                .when(is_selected, |d| d.bg(ui::raised()))
                .when(!is_selected, |d| d.hover(|d| d.bg(ui::surface_hover())))
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    this.agents_panel.selected = Some(id.clone());
                    if event.click_count() >= 2 {
                        this.agents_panel.tab = AgentsTab::Activity;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .w(px(10.))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(if s.status == "running" {
                            ui::pulse(div().size(px(7.)).rounded_full().bg(color))
                                .into_any_element()
                        } else {
                            div()
                                .size(px(7.))
                                .rounded_full()
                                .bg(color)
                                .into_any_element()
                        }),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .min_w(px(48.))
                                .max_w(px(200.))
                                .truncate()
                                .text_sm()
                                .font_family(ui::MONO)
                                .text_color(ui::fg())
                                .child(name),
                        )
                        .when_some(kind, |d, kind| d.child(kind_chip(kind)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(ui::muted())
                                .child(s.last_step.clone().unwrap_or_default()),
                        ),
                )
                .child(
                    column(72., s.status.clone())
                        .truncate()
                        .text_xs()
                        .text_color(color),
                )
                .child(
                    column(56., duration(s.elapsed_ms(now)))
                        .text_xs()
                        .font_family(ui::MONO)
                        .text_color(ui::muted()),
                )
                .child(
                    column(40., s.turns.to_string())
                        .text_xs()
                        .font_family(ui::MONO)
                        .text_color(ui::muted()),
                )
                .child(
                    column(40., s.tool_calls.to_string())
                        .text_xs()
                        .font_family(ui::MONO)
                        .text_color(ui::muted()),
                );
            ui::appear(("agent-row-in", ix), div().child(row))
        });
        v_flex()
            .gap_0p5()
            .child(head)
            .children(rows)
            .into_any_element()
    }

    fn render_agent_detail(&self, task: &AgentTask, now: u64) -> impl IntoElement {
        let s = &task.summary;
        let field = |label: &'static str, value: String, mono: bool| {
            v_flex().gap_1().child(ui::section_label(label)).child(
                div()
                    .text_sm()
                    .text_color(ui::fg())
                    .when(mono, |d| d.font_family(ui::MONO).text_xs())
                    .child(value),
            )
        };
        let stat = |label: &'static str, value: String| {
            ui::card()
                .flex_1()
                .min_w_0()
                .px_3()
                .py_2()
                .gap_0p5()
                .child(ui::section_label(label))
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_family(ui::MONO)
                        .text_color(ui::fg())
                        .child(value),
                )
        };
        let color = status_color(&s.status);
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui::fg())
                                    .child(s.agent.clone()),
                            )
                            .child(ui::chip(s.status.clone(), color).flex_shrink_0()),
                    )
                    .when_some(s.label.clone(), |d, label| {
                        d.child(div().text_xs().text_color(ui::muted()).child(label))
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(stat("Time", duration(s.elapsed_ms(now))))
                    .child(stat("Turns", s.turns.to_string()))
                    .child(stat("Tools", s.tool_calls.to_string())),
            )
            .when_some(s.error.clone(), |d, error| {
                d.child(
                    v_flex()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(ui::danger().opacity(0.08))
                        .border_1()
                        .border_color(ui::danger().opacity(0.22))
                        .child(
                            div()
                                .text_size(px(10.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::danger())
                                .child("ERROR"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(ui::MONO)
                                .text_color(ui::danger())
                                .child(error),
                        ),
                )
            })
            .when_some(s.last_step.clone(), |d, step| {
                d.child(field("Last step", step, true))
            })
            .when_some(s.prompt.clone(), |d, prompt| {
                d.child(
                    v_flex().gap_1().child(ui::section_label("Task")).child(
                        ui::well_box()
                            .px_3()
                            .py_2()
                            .text_color(ui::fg())
                            .whitespace_normal()
                            .child(prompt),
                    ),
                )
            })
            .child(field("Id", s.id.clone(), true))
    }

    fn render_thread(&self, task: &AgentTask, cx: &mut Context<Self>) -> AnyElement {
        let id = task.summary.id.clone();
        let mut column = v_flex().gap_2().px_1();
        if task.thread.dropped > 0 {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(ui::faint())
                    .child(format!("… {} earlier items dropped", task.thread.dropped)),
            );
        }
        for (ix, item) in task.thread.items.iter().enumerate() {
            column =
                column.child(match item {
                    ThreadItem::Text { text } => div()
                        .text_sm()
                        .text_color(ui::fg())
                        .child(text.clone())
                        .into_any_element(),
                    ThreadItem::Thinking { text } => div()
                        .text_xs()
                        .italic()
                        .text_color(ui::faint())
                        .child(text.clone())
                        .into_any_element(),
                    ThreadItem::Note { text } => div()
                        .text_xs()
                        .text_color(ui::muted())
                        .child(text.clone())
                        .into_any_element(),
                    ThreadItem::Tool {
                        name,
                        args,
                        running,
                        is_error,
                        output,
                    } => {
                        let key = (id.clone(), ix);
                        let open = self.agents_panel.expanded_tools.contains(&key);
                        let status: AnyElement = if *running {
                            ui::spinner(Size::XSmall, ui::accent()).into_any_element()
                        } else if *is_error {
                            Icon::new(IconName::X)
                                .xsmall()
                                .text_color(ui::danger())
                                .into_any_element()
                        } else {
                            Icon::new(IconName::Check)
                                .xsmall()
                                .text_color(ui::success())
                                .into_any_element()
                        };
                        let body = output.clone().unwrap_or_default();
                        let (shown, hidden) = ui::tail(&body, if open { 40 } else { 3 });
                        ui::card()
                            .overflow_hidden()
                            .child(
                                h_flex()
                                    .id(SharedString::from(format!("thread-{id}-{ix}")))
                                    .px_3()
                                    .py_1p5()
                                    .gap_2()
                                    .text_xs()
                                    .cursor_pointer()
                                    .hover(|d| d.bg(ui::surface_hover()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.agents_panel.expanded_tools.remove(&key) {
                                            this.agents_panel.expanded_tools.insert(key.clone());
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        Icon::new(if open {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .xsmall()
                                        .text_color(ui::faint()),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(ui::champagne())
                                            .child(name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .font_family(ui::MONO)
                                            .text_color(ui::muted())
                                            .child(ThreadItem::tool_subject(args)),
                                    )
                                    .child(status),
                            )
                            .when(!shown.trim().is_empty(), |d| {
                                d.child(
                                    v_flex()
                                        .px_3()
                                        .py_1p5()
                                        .border_t_1()
                                        .border_color(ui::border())
                                        .bg(ui::well())
                                        .overflow_hidden()
                                        .font_family(ui::MONO)
                                        .text_xs()
                                        .text_color(ui::muted())
                                        .when(hidden > 0, |d| {
                                            d.child(
                                                div()
                                                    .text_color(ui::faint())
                                                    .child(format!("… {hidden} more lines")),
                                            )
                                        })
                                        .children(shown.lines().map(|l| {
                                            div().whitespace_nowrap().child(l.to_string())
                                        })),
                                )
                            })
                            .into_any_element()
                    }
                    ThreadItem::Other => div().into_any_element(),
                });
        }
        column.into_any_element()
    }
}

/// Tern's model pill: outlined, rounded, with a small leading dot.
fn kind_chip(text: String) -> Div {
    h_flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .gap_1()
        .px_2()
        .h(px(18.))
        .rounded_full()
        .border_1()
        .border_color(ui::border_strong())
        .bg(ui::well())
        .text_size(px(10.5))
        .text_color(ui::muted())
        .child(
            div()
                .flex_shrink_0()
                .size(px(4.))
                .rounded_full()
                .bg(ui::faint()),
        )
        .child(text)
}
