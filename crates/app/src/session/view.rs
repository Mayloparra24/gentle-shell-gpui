//! Rendering for [`SessionView`]: the conversation (tool cards, thinking,
//! messages), the composer with its pickers, extension dialogs and the rail.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::{UiAnswer, UiMethod};
use serde_json::Value;

use super::attachments;
use super::{Backend, CONTEXT, ModelInfo, RailVisible, SessionView};
use crate::gentle_state::PermissionMode;
use crate::tools;
use crate::transcript::{Block, Item, NoticeLevel, ToolStatus, format_tokens};
use crate::ui;

// ── Display-only state ─────────────────────────────────────────────────────

/// What the view tracks beside the transcript: when running things started
/// (live timers), the prompt Pi has not picked up yet, picker and dialog
/// highlights. Reset with the conversation.
#[derive(Default)]
pub(super) struct Live {
    turn: Option<Instant>,
    /// Tool and shell items by transcript index.
    tools: HashMap<usize, Span>,
    /// Thinking blocks by (item, block).
    thinking: HashMap<(usize, usize), Span>,
    pending: Option<Pending>,
    /// Keyboard highlight of the front dialog, by dialog id.
    dialog_cursor: Option<(String, usize)>,
    model_picker: bool,
    model_cursor: usize,
    model_query: String,
    model_scroll: ScrollHandle,
    /// Re-renders running timers while something is live.
    ticker: Option<Task<()>>,
}

#[derive(Clone, Copy)]
struct Span {
    start: Instant,
    took: Option<Duration>,
}

impl Span {
    fn elapsed(&self) -> Duration {
        self.took.unwrap_or_else(|| self.start.elapsed())
    }
}

/// A prompt sent while idle, shown right away until Pi's own events arrive.
struct Pending {
    text: String,
    /// The attachments it carries, shown until Pi echoes them.
    images: Vec<Arc<Image>>,
    /// Transcript length when it was sent: Pi's user message lands here.
    base: usize,
    since: Instant,
    /// Pi answered the `prompt` request.
    acked: Option<Instant>,
    /// Pi reported the agent running.
    started: bool,
}

impl Live {
    pub(super) fn expect_reply(&mut self, text: &str, images: Vec<Arc<Image>>, base: usize) {
        self.pending = Some(Pending {
            text: text.trim().to_string(),
            images,
            base,
            since: Instant::now(),
            acked: None,
            started: false,
        });
    }

    pub(super) fn reply_acknowledged(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.acked.get_or_insert_with(Instant::now);
        }
    }

    pub(super) fn reply_failed(&mut self) {
        self.pending = None;
    }
}

/// Rate-limit usage an extension reported (`51% used (seven_day)`).
pub(super) struct RateLimit {
    pub(super) percent: u32,
    pub(super) text: String,
}

fn track<K: Eq + Hash>(map: &mut HashMap<K, Span>, key: K, live: bool, now: Instant) {
    match map.get_mut(&key) {
        Some(span) if live && span.took.is_some() => {
            *span = Span {
                start: now,
                took: None,
            }
        }
        Some(span) if !live && span.took.is_none() => span.took = Some(now - span.start),
        Some(_) => {}
        None if live => {
            map.insert(
                key,
                Span {
                    start: now,
                    took: None,
                },
            );
        }
        None => {}
    }
}

// Keys into `SessionView::expanded`: one per foldable region.
const OUTPUT: usize = 0;
const INPUT: usize = 1;
const THINKING: usize = 2;

fn fold_key(ix: usize, sub: usize, slot: usize) -> usize {
    (ix << 16) | (sub << 2) | slot
}

/// Lines of a collapsed tool input and output.
const INPUT_LINES: usize = 6;
const DIFF_LINES: usize = 12;
const OUTPUT_LINES: usize = 8;
/// Pane widths below which the composer sheds secondary chrome.
const NARROW: f32 = 520.;
const TINY: f32 = 420.;

impl SessionView {
    /// A prompt is in flight: Pi is working or has not picked it up yet.
    pub(super) fn busy(&self) -> bool {
        self.transcript.running || self.live.pending.is_some()
    }

    /// Record start/end instants of live items and keep a ticker running
    /// for their timers.
    fn sync_live(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let running = self.transcript.running;
        if running {
            self.live.turn.get_or_insert(now);
        } else {
            self.live.turn = None;
        }
        let mut shells = false;
        for (ix, item) in self.transcript.items.iter().enumerate() {
            match item {
                Item::Tool { status, .. } => track(
                    &mut self.live.tools,
                    ix,
                    *status == ToolStatus::Running,
                    now,
                ),
                Item::Shell { running, .. } => {
                    shells |= *running;
                    track(&mut self.live.tools, ix, *running, now);
                }
                Item::Assistant {
                    blocks, streaming, ..
                } => {
                    for (bi, block) in blocks.iter().enumerate() {
                        if matches!(block, Block::Thinking(_)) {
                            let live = *streaming && bi + 1 == blocks.len();
                            track(&mut self.live.thinking, (ix, bi), live, now);
                        }
                    }
                }
                _ => {}
            }
        }

        let ready = matches!(self.backend, Backend::Ready(_));
        let items = &self.transcript.items;
        let dialogs = !self.dialogs.is_empty();
        if let Some(pending) = &mut self.live.pending {
            pending.started |= running;
            let answered = items
                .get(pending.base..)
                .is_some_and(|rest| rest.iter().any(|i| matches!(i, Item::Assistant { .. })));
            // An extension command that never starts a turn: Pi acknowledged
            // and nothing followed (or it opened a dialog).
            let grace = if pending.text.starts_with('/') {
                Duration::from_millis(1500)
            } else {
                Duration::from_secs(8)
            };
            let idle_command =
                !pending.started && (dialogs || pending.acked.is_some_and(|t| t.elapsed() > grace));
            if answered || (pending.started && !running) || idle_command || !ready {
                self.live.pending = None;
            }
        }

        if running || shells || self.live.pending.is_some() {
            if self.live.ticker.is_none() {
                self.live.ticker = Some(cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(200))
                            .await;
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                }));
            }
        } else {
            self.live.ticker = None;
        }
    }

    fn tool_duration(
        &self,
        ix: usize,
        status: ToolStatus,
        backend_ms: Option<u64>,
    ) -> Option<Duration> {
        let span = self.live.tools.get(&ix);
        if status == ToolStatus::Running {
            return span.map(Span::elapsed);
        }
        backend_ms
            .map(Duration::from_millis)
            .or_else(|| span.and_then(|s| s.took))
            .filter(|d| d.as_millis() >= 50)
    }

    fn toggle(&mut self, key: usize, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        cx.notify();
    }

    // ── Empty and error states ─────────────────────────────────────────────

    fn render_hero(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let version = self.version.clone().unwrap_or_default();
        let needs_sign_in = matches!(self.backend, Backend::Ready(_))
            && self.models_loaded
            && self.models.is_empty();
        // The banner is ~175px per unit of scale; shrink it to fit the pane.
        let scale = ((self.pane_width.get() - 48.) / 175.).clamp(1.5, 4.0);
        v_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap_1()
            .pt(px(96.))
            .child(crate::banner::banner(self.banner, scale, self.banner_reveal))
            .child(
                div()
                    .font_family(ui::MONO)
                    .text_xs()
                    .text_color(ui::faint())
                    .child(if version.is_empty() { String::new() } else { format!("v{version}") }),
            )
            .when(needs_sign_in, |this| {
                this.child(ui::appear(
                    "hero-sign-in-card",
                    ui::card()
                        .mt_8()
                        .w_full()
                        .max_w(px(420.))
                        .p_5()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .size(px(36.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_xl()
                                .bg(ui::accent().opacity(0.12))
                                .child(Icon::new(IconName::KeyRound).small().text_color(ui::accent())),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child("Sign in to start cooking"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_center()
                                .text_color(ui::muted())
                                .child("No models are available yet. Sign in with a subscription or add an API key."),
                        )
                        .child(
                            Button::new("hero-sign-in")
                                .primary()
                                .small()
                                .icon(Icon::new(IconName::LogIn))
                                .label("Sign in to a provider")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(super::SessionEvent::OpenProviders))),
                        ),
                ))
            })
    }

    fn render_backend_state(&self) -> Option<AnyElement> {
        let (title, body): (&str, String) = match &self.backend {
            Backend::Starting => {
                return Some(
                    h_flex()
                        .gap_2()
                        .pt(px(24.))
                        .justify_center()
                        .text_size(px(13.))
                        .text_color(ui::muted())
                        .child(Spinner::new().small().color(ui::accent()))
                        .child(ShimmerText::new("Starting gentle-shell…").highlight_color(ui::fg()))
                        .into_any_element(),
                );
            }
            Backend::Ready(_) => return None,
            Backend::Failed(error) => ("Could not start gentle-shell", error.clone()),
            Backend::Exited(code) => (
                "gentle-shell stopped",
                format!(
                    "Exit code {}.\n{}",
                    code.map_or("unknown".into(), |c| c.to_string()),
                    self.stderr
                        .iter()
                        .rev()
                        .take(12)
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            ),
        };
        Some(
            ui::card()
                .mt_6()
                .mx_auto()
                .w(px(560.))
                .p_4()
                .gap_3()
                .border_color(ui::danger().opacity(0.4))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Icon::new(IconName::CircleX)
                                .small()
                                .text_color(ui::danger()),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::danger())
                                .child(title),
                        ),
                )
                .child(
                    ui::well_box()
                        .p_3()
                        .text_color(ui::muted())
                        .whitespace_normal()
                        .child(body),
                )
                .into_any_element(),
        )
    }

    // ── Transcript ─────────────────────────────────────────────────────────

    fn render_item(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        match item {
            Item::User { text, images } => {
                let images = images.iter().map(|i| self.image_cache.get(i)).collect();
                user_bubble(text, images).into_any_element()
            }
            Item::Assistant {
                blocks,
                streaming,
                error,
            } => {
                let mut column = v_flex().w_full().gap_2();
                let has_text = blocks
                    .iter()
                    .any(|b| matches!(b, Block::Text(t) if !t.is_empty()));
                let mut thinking_live = false;
                for (block_ix, block) in blocks.iter().enumerate() {
                    match block {
                        Block::Text(_) => {
                            if let Some((state, _)) = self.markdown.get(&(ix, block_ix)) {
                                column = column.child(
                                    div()
                                        .text_size(px(14.))
                                        .text_color(ui::fg())
                                        .child(TextView::new(state).selectable(true)),
                                );
                            }
                        }
                        Block::Thinking(text) if !text.trim().is_empty() => {
                            let live = *streaming && block_ix + 1 == blocks.len();
                            thinking_live |= live;
                            column =
                                column.child(self.render_thinking(ix, block_ix, text, live, cx));
                        }
                        _ => {}
                    }
                }
                if *streaming && !has_text && !thinking_live {
                    let since = self.live.turn.map(|t| t.elapsed());
                    column = column.child(working_indicator(("working", ix), "Working…", since));
                }
                if let Some(error) = error {
                    column = column.child(notice_card(NoticeLevel::Error, error));
                }
                column.into_any_element()
            }
            Item::Tool {
                name,
                args,
                status,
                output,
                details,
                duration_ms,
                ..
            } => {
                let spec = tool_spec(name, args, *status, output, details);
                let duration = self.tool_duration(ix, *status, *duration_ms);
                self.render_card(ix, spec, duration, cx)
            }
            Item::Shell {
                command,
                output,
                exit_code,
                running,
            } => {
                let status = match (running, exit_code) {
                    (true, _) => ToolStatus::Running,
                    (false, Some(0)) => ToolStatus::Done,
                    _ => ToolStatus::Failed,
                };
                let spec = CardSpec {
                    icon: IconName::CodeXml,
                    title: "Shell".into(),
                    subject: String::new(),
                    chip: Some(command_language(command).to_string()),
                    added: None,
                    removed: None,
                    input: Some((command.clone(), false)),
                    output: output.clone(),
                    always_output: true,
                    status,
                };
                let duration = self.tool_duration(ix, status, None);
                self.render_card(ix, spec, duration, cx)
            }
            Item::Custom { kind, text } => self.render_custom(ix, kind, text, cx),
            Item::Notice { level, text } => notice_card(*level, text).into_any_element(),
        }
    }

    fn render_thinking(
        &self,
        ix: usize,
        block_ix: usize,
        text: &str,
        live: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let key = fold_key(ix, block_ix, THINKING);
        // Live thoughts read along; finished ones fold away. A click flips either.
        let open = live != self.expanded.contains(&key);
        let elapsed = self.live.thinking.get(&(ix, block_ix)).map(Span::elapsed);
        let star = div()
            .text_color(if live { ui::info() } else { ui::faint() })
            .child("✳");
        v_flex()
            .w_full()
            .gap_1p5()
            .child(
                h_flex()
                    .id(("thinking", key))
                    .gap_1p5()
                    .cursor_pointer()
                    .text_size(px(13.))
                    .italic()
                    .text_color(ui::muted())
                    .hover(|s| s.text_color(ui::fg()))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(key, cx)))
                    .child(if live {
                        ui::pulse(("thinking-star", key), star).into_any_element()
                    } else {
                        star.into_any_element()
                    })
                    .child(if live { "Thinking…" } else { "Thought" })
                    .when_some(elapsed, |this, d| {
                        this.child(
                            div()
                                .font_family(ui::MONO)
                                .text_size(px(11.))
                                .text_color(ui::faint())
                                .child(ui::format_duration(d.as_millis() as u64)),
                        )
                    })
                    .child(
                        Icon::new(if open {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall()
                        .text_color(ui::faint()),
                    ),
            )
            .when(open, |this| {
                this.child(
                    div()
                        .ml(px(5.))
                        .pl_3()
                        .border_l_2()
                        .border_color(ui::border_strong())
                        .text_size(px(13.))
                        .italic()
                        .text_color(ui::muted())
                        .child(text.trim().to_string()),
                )
            })
    }

    fn render_custom(
        &self,
        ix: usize,
        kind: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (icon, title, color) = match kind {
            "gentle-agents.result" => (IconName::Bot, "Agent answer".to_string(), ui::success()),
            "gentle-agents.message" | "gentle-agents.orchestrator-message" => {
                (IconName::MessageSquare, "Message".to_string(), ui::info())
            }
            "gentle-jobs.notice" => (IconName::Activity, "Job".to_string(), ui::warning()),
            other => (IconName::Info, other.to_string(), ui::muted()),
        };
        let key = fold_key(ix, 0, OUTPUT);
        let open = self.expanded.contains(&key);
        let (shown, hidden) = head(text, if open { 400 } else { INPUT_LINES });
        let more = text.trim_end().lines().count().saturating_sub(INPUT_LINES);
        ui::card()
            .w_full()
            .overflow_hidden()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .child(Icon::new(icon).small().text_color(color))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::champagne())
                            .child(title),
                    ),
            )
            .when(!shown.is_empty(), |this| {
                this.child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_size(px(13.))
                        .text_color(ui::muted())
                        .whitespace_normal()
                        .child(shown),
                )
            })
            .when(open && hidden > 0, |this| {
                this.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_size(px(11.))
                        .text_color(ui::faint())
                        .child(format!("… {hidden} more lines not shown")),
                )
            })
            .when(more > 0, |this| {
                this.child(div().pb_1().child(self.fold_toggle(
                    key,
                    if open {
                        "Show less".into()
                    } else {
                        format!("{more} more lines")
                    },
                    open,
                    cx,
                )))
            })
            .into_any_element()
    }

    fn render_card(
        &self,
        ix: usize,
        spec: CardSpec,
        duration: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let out_key = fold_key(ix, 0, OUTPUT);
        let open = self.expanded.contains(&out_key);
        let has_output = !spec.output.trim().is_empty();
        let show_output =
            has_output && (spec.always_output || open || spec.status != ToolStatus::Done);
        let foldable = has_output && !spec.always_output;
        ui::card()
            .w_full()
            .overflow_hidden()
            .child(
                h_flex()
                    .id(("tool", ix))
                    .px_3()
                    .py_2()
                    .gap_2()
                    .when(foldable, |this| {
                        this.cursor_pointer()
                            .hover(|s| s.bg(ui::surface_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(out_key, cx)))
                    })
                    .child(Icon::new(spec.icon).small().text_color(ui::muted()))
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(1.)
                            .max_w(px(420.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::champagne())
                            .child(spec.title),
                    )
                    .when_some(spec.chip, |this, chip| {
                        this.child(ui::chip(chip, ui::info()).flex_shrink_0())
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family(ui::MONO)
                            .text_size(px(12.))
                            .text_color(ui::muted())
                            .child(spec.subject),
                    )
                    .when_some(spec.added, |this, n| {
                        this.child(
                            div()
                                .font_family(ui::MONO)
                                .text_size(px(12.))
                                .text_color(ui::success())
                                .child(format!("+{n}")),
                        )
                    })
                    .when_some(spec.removed, |this, n| {
                        this.child(
                            div()
                                .font_family(ui::MONO)
                                .text_size(px(12.))
                                .text_color(ui::danger())
                                .child(format!("−{n}")),
                        )
                    })
                    .when(foldable, |this| {
                        this.child(
                            Icon::new(if open {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .xsmall()
                            .text_color(ui::faint()),
                        )
                    }),
            )
            .when_some(spec.input, |this, (text, diff)| {
                this.child(div().px_3().pb_2().child(self.code_well(
                    fold_key(ix, 0, INPUT),
                    &text,
                    diff,
                    cx,
                )))
            })
            .when(show_output, |this| {
                this.child(self.output_block(out_key, &spec.output, cx))
            })
            .child(status_footer(spec.status, duration))
            .into_any_element()
    }

    /// A command, file body or diff, clipped from the bottom with a fade.
    fn code_well(&self, key: usize, text: &str, diff: bool, cx: &mut Context<Self>) -> Div {
        let max = if diff { DIFF_LINES } else { INPUT_LINES };
        let open = self.expanded.contains(&key);
        let (shown, hidden) = head(text, if open { 400 } else { max });
        let more = text.trim_end().lines().count().saturating_sub(max);
        let clipped = !open && hidden > 0;
        let lines = v_flex()
            .relative()
            .py_2()
            .overflow_hidden()
            .children(shown.lines().map(|line| {
                if diff {
                    diff_row(line)
                } else {
                    div().px_3().whitespace_nowrap().text_color(ui::fg()).child(
                        if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        },
                    )
                }
            }))
            .when(clipped, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(28.))
                        .bg(linear_gradient(
                            180.,
                            linear_color_stop(ui::well().opacity(0.), 0.),
                            linear_color_stop(ui::well(), 1.),
                        )),
                )
            });
        ui::well_box()
            .w_full()
            .overflow_hidden()
            .child(lines)
            .when(open && hidden > 0, |this| {
                this.child(
                    div()
                        .px_3()
                        .pb_1()
                        .text_color(ui::faint())
                        .child(format!("… {hidden} more lines not shown")),
                )
            })
            .when(more > 0, |this| {
                this.child(div().pb_1().child(self.fold_toggle(
                    key,
                    if open {
                        "Show less".into()
                    } else {
                        format!("{more} more lines")
                    },
                    open,
                    cx,
                )))
            })
    }

    /// Command output, clipped from the top so the latest lines show.
    fn output_block(&self, key: usize, body: &str, cx: &mut Context<Self>) -> Div {
        let open = self.expanded.contains(&key);
        let (shown, hidden) = ui::tail(body, if open { 200 } else { OUTPUT_LINES });
        let more = body.trim_end().lines().count().saturating_sub(OUTPUT_LINES);
        v_flex()
            .w_full()
            .border_t_1()
            .border_color(ui::border())
            .py_1()
            .when(more > 0, |this| {
                this.child(self.fold_toggle(
                    key,
                    if open {
                        "Show fewer lines".into()
                    } else {
                        format!("{more} earlier lines")
                    },
                    !open,
                    cx,
                ))
            })
            .child(
                v_flex()
                    .px_3()
                    .py_1()
                    .overflow_hidden()
                    .font_family(ui::MONO)
                    .text_xs()
                    .text_color(ui::muted())
                    .when(open && hidden > 0, |this| {
                        this.child(
                            div()
                                .text_color(ui::faint())
                                .child(format!("… {hidden} earlier lines not shown")),
                        )
                    })
                    .children(shown.lines().map(|line| {
                        div().whitespace_nowrap().child(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        })
                    })),
            )
    }

    fn fold_toggle(
        &self,
        key: usize,
        label: String,
        up: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        h_flex()
            .id(("fold", key))
            .gap_1()
            .px_3()
            .py_0p5()
            .font_family(cx.theme().font_family.clone())
            .text_size(px(11.))
            .text_color(ui::muted())
            .cursor_pointer()
            .hover(|s| s.text_color(ui::fg()))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(key, cx)))
            .child(label)
            .child(
                Icon::new(if up {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .xsmall(),
            )
    }

    // ── Dialogs ────────────────────────────────────────────────────────────

    fn dialog_cursor(&self) -> usize {
        let Some(dialog) = self.dialogs.front() else {
            return 0;
        };
        if let Some((id, cursor)) = &self.live.dialog_cursor
            && *id == dialog.id
        {
            return *cursor;
        }
        match &dialog.method {
            UiMethod::Select { title, options } => split_current(title)
                .1
                .and_then(|current| options.iter().position(|o| option_matches(o, current)))
                .unwrap_or(0),
            _ => 0,
        }
    }

    /// ↑/↓, Enter and 1–9 for select dialogs; Enter/y/n for confirms. Only
    /// while the session itself holds focus (the dialog takes it on arrival),
    /// so typing in the composer is never hijacked.
    fn on_dialog_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let m = &event.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        let key = event.keystroke.key.as_str();
        let cursor = self.dialog_cursor();
        let Some(dialog) = self.dialogs.front() else {
            return;
        };
        let answer = match &dialog.method {
            UiMethod::Select { options, .. } if !options.is_empty() => {
                let n = options.len();
                let cursor = cursor.min(n - 1);
                match key {
                    "up" => {
                        self.live.dialog_cursor = Some((dialog.id.clone(), (cursor + n - 1) % n));
                        None
                    }
                    "down" => {
                        self.live.dialog_cursor = Some((dialog.id.clone(), (cursor + 1) % n));
                        None
                    }
                    "enter" => Some(UiAnswer::Value(options[cursor].clone())),
                    digit => match digit.parse::<usize>() {
                        Ok(d) if (1..=9).contains(&d) && d <= n => {
                            Some(UiAnswer::Value(options[d - 1].clone()))
                        }
                        _ => return,
                    },
                }
            }
            UiMethod::Confirm { .. } => match key {
                "enter" | "y" => Some(UiAnswer::Confirmed(true)),
                "n" => Some(UiAnswer::Confirmed(false)),
                _ => return,
            },
            _ => return,
        };
        cx.stop_propagation();
        match answer {
            Some(answer) => self.answer_dialog(answer, window, cx),
            None => cx.notify(),
        }
    }

    fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.dialogs.front()?;
        let cursor = self.dialog_cursor();
        let (icon, raw_title) = match &dialog.method {
            UiMethod::Select { title, .. } => (IconName::ListTodo, title.as_str()),
            UiMethod::Confirm { title, .. } => (IconName::ShieldCheck, title.as_str()),
            UiMethod::Input { title, .. } => (IconName::SquareTerminal, title.as_str()),
            UiMethod::Editor { title, .. } => (IconName::FilePen, title.as_str()),
            _ => (IconName::Info, "Extension request"),
        };
        let (title, current) = split_current(raw_title);
        let title = if title.is_empty() { "Choose" } else { title };

        let header = h_flex()
            .gap_2p5()
            .child(
                div()
                    .size(px(28.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_lg()
                    .bg(ui::accent().opacity(0.12))
                    .child(Icon::new(icon).small().text_color(ui::accent())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui::fg())
                    .child(title.to_string()),
            )
            .when_some(current, |this, current| {
                this.child(ui::chip(format!("current: {current}"), ui::accent()).flex_shrink_0())
            });

        let mut hints: Vec<(&str, &str)> = Vec::new();
        let body: AnyElement = match &dialog.method {
            UiMethod::Select { options, .. } => {
                hints = vec![("↑↓", "navigate"), ("↵", "choose"), ("esc", "cancel")];
                let multi = options.iter().any(|o| checkbox_option(o).is_some());
                let mut rows = v_flex().w_full().gap_0p5();
                let mut actions = h_flex().gap_2().justify_end();
                let mut primary_used = false;
                for (i, option) in options.iter().enumerate() {
                    let value = option.clone();
                    let choose = cx.listener(move |this, _, window, cx| {
                        this.answer_dialog(UiAnswer::Value(value.clone()), window, cx)
                    });
                    let selected = i == cursor;
                    let checkbox = checkbox_option(option);
                    if multi && checkbox.is_none() {
                        // `Done` and friends of a multi-select are actions, not rows.
                        let primary = !primary_used;
                        primary_used = true;
                        actions = actions.child(
                            Button::new(("dialog-action", i))
                                .small()
                                .when(primary, |b| b.primary())
                                .when(!primary, |b| b.ghost())
                                .selected(selected)
                                .label(option.clone())
                                .on_click(choose),
                        );
                        continue;
                    }
                    let label = checkbox
                        .map_or(option.as_str(), |(_, rest)| rest)
                        .to_string();
                    let is_current = current.is_some_and(|c| option_matches(option, c));
                    rows = rows.child(
                        h_flex()
                            .id(("option", i))
                            .relative()
                            .w_full()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .cursor_pointer()
                            .text_size(px(13.))
                            .text_color(ui::fg())
                            .hover(|s| s.bg(ui::surface_hover()))
                            .when(selected, |this| {
                                this.bg(ui::surface_hover()).child(accent_bar())
                            })
                            .when_some(checkbox, |this, (checked, _)| {
                                this.child(checkbox_box(checked))
                            })
                            .child(div().flex_1().min_w_0().child(label))
                            .when(is_current, |this| {
                                this.child(
                                    Icon::new(IconName::Check).xsmall().text_color(ui::accent()),
                                )
                            })
                            .when(i < 9, |this| this.child(ui::kbd(format!("{}", i + 1))))
                            .on_click(choose),
                    );
                }
                v_flex()
                    .gap_2()
                    .child(rows)
                    .when(primary_used, |this| this.child(actions))
                    .into_any_element()
            }
            UiMethod::Confirm { message, .. } => {
                hints = vec![("↵", "yes"), ("n", "no"), ("esc", "cancel")];
                v_flex()
                    .gap_3()
                    .when(!message.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(13.))
                                .text_color(ui::muted())
                                .whitespace_normal()
                                .child(message.clone()),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("confirm-yes")
                                    .primary()
                                    .small()
                                    .icon(Icon::new(IconName::Check))
                                    .label("Yes")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.answer_dialog(UiAnswer::Confirmed(true), window, cx)
                                    })),
                            )
                            .child(
                                Button::new("confirm-no")
                                    .ghost()
                                    .small()
                                    .label("No")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.answer_dialog(UiAnswer::Confirmed(false), window, cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            UiMethod::Input { .. } | UiMethod::Editor { .. } => {
                let editor = matches!(dialog.method, UiMethod::Editor { .. });
                hints = vec![
                    (if editor { "⌘↵" } else { "↵" }, "submit"),
                    ("esc", "cancel"),
                ];
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .w_full()
                            .px_3()
                            .py_2()
                            .rounded_lg()
                            .bg(ui::well())
                            .border_1()
                            .border_color(ui::border_strong())
                            .text_size(px(13.))
                            .child(Textarea::new(&self.dialog_input).appearance(false)),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("dialog-submit")
                                .primary()
                                .small()
                                .label("Submit")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let value = this.dialog_input.read(cx).value().to_string();
                                    this.answer_dialog(UiAnswer::Value(value), window, cx)
                                })),
                        ),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        };

        let waiting = self.dialogs.len() - 1;
        let footer = h_flex()
            .gap_3()
            .pt_2()
            .border_t_1()
            .border_color(ui::border())
            .text_size(px(11.))
            .text_color(ui::faint())
            .children(
                hints
                    .into_iter()
                    .map(|(key, what)| h_flex().gap_1().child(ui::kbd(key)).child(what)),
            )
            .child(div().flex_1())
            .when(waiting > 0, |this| {
                this.child(ui::chip(format!("{waiting} more waiting"), ui::warning()))
            })
            .child(
                Button::new("dialog-cancel")
                    .ghost()
                    .xsmall()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.answer_dialog(UiAnswer::Cancelled, window, cx)
                    })),
            );

        let card = v_flex()
            .w_full()
            .gap_3()
            .p_4()
            .rounded_2xl()
            .border_1()
            .border_color(ui::accent().opacity(0.45))
            .bg(ui::surface())
            .shadow_lg()
            .child(header)
            .child(body)
            .child(footer);
        Some(
            ui::pop_in(
                ElementId::Name(format!("dialog-{}", dialog.id).into()),
                card,
            )
            .into_any_element(),
        )
    }

    // ── Composer ───────────────────────────────────────────────────────────

    fn render_slash_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.composer.read(cx).value();
        let query = text.strip_prefix('/')?;
        if query.contains(char::is_whitespace) {
            return None;
        }
        let matches: Vec<&(String, String)> = self
            .commands
            .iter()
            .filter(|(name, _)| name.contains(query))
            .take(8)
            .collect();
        if matches.is_empty() {
            return None;
        }
        Some(
            ui::pop_in(
                "slash-menu",
                v_flex()
                    .w_full()
                    .p_1()
                    .rounded_xl()
                    .border_1()
                    .border_color(ui::border_strong())
                    .bg(ui::surface())
                    .shadow_lg()
                    .font_family(cx.theme().font_family.clone())
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .pb_1()
                            .child(ui::section_label("Commands")),
                    )
                    .children(
                        matches
                            .into_iter()
                            .enumerate()
                            .map(|(i, (name, description))| {
                                let name = name.clone();
                                h_flex()
                                    .id(("command", i))
                                    .w_full()
                                    .px_2()
                                    .py_1p5()
                                    .gap_3()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(ui::surface_hover()))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .font_family(ui::MONO)
                                            .text_size(px(12.))
                                            .text_color(ui::accent())
                                            .child(format!("/{name}")),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_size(px(12.))
                                            .text_color(ui::muted())
                                            .child(description.clone()),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.insert_command(&name, window, cx)
                                    }))
                            }),
                    ),
            )
            .into_any_element(),
        )
    }

    fn filtered_models(&self, cx: &App) -> Vec<ModelInfo> {
        let query = self.model_search.read(cx).value().trim().to_lowercase();
        self.models
            .iter()
            .filter(|m| {
                query.is_empty()
                    || m.name.to_lowercase().contains(&query)
                    || m.id.to_lowercase().contains(&query)
                    || m.provider.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    pub(super) fn set_model_picker(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.live.model_picker == open {
            return;
        }
        self.live.model_picker = open;
        if open {
            self.live.model_query.clear();
            self.model_search.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.focus(window, cx);
            });
            self.live.model_cursor = self
                .model
                .as_ref()
                .and_then(|current| self.models.iter().position(|m| m == current))
                .unwrap_or(0);
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    /// The search text changed: highlight the first match again.
    pub(super) fn model_query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.model_search.read(cx).value().to_string();
        if query != self.live.model_query {
            self.live.model_query = query;
            self.live.model_cursor = 0;
        }
        cx.notify();
    }

    /// ↑/↓/Enter/Esc while the model picker is open. Runs as a keystroke
    /// interceptor so it wins over the focused search field.
    pub(super) fn model_picker_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.live.model_picker {
            return false;
        }
        let m = &keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return false;
        }
        let models = self.filtered_models(cx);
        let n = models.len();
        let cursor = self.live.model_cursor.min(n.saturating_sub(1));
        match keystroke.key.as_str() {
            "up" | "down" if n > 0 => {
                let next = if keystroke.key == "up" {
                    (cursor + n - 1) % n
                } else {
                    (cursor + 1) % n
                };
                self.live.model_cursor = next;
                // Rows sit between provider headers in the scrolled list.
                let headers = 1 + models[1..=next]
                    .iter()
                    .zip(&models[..next])
                    .filter(|(a, b)| a.provider != b.provider)
                    .count();
                self.live.model_scroll.scroll_to_item(next + headers);
            }
            "enter" => {
                if let Some(model) = models.get(cursor) {
                    self.pick_model(model.clone(), window, cx);
                }
            }
            "escape" => self.set_model_picker(false, window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    fn pick_model(&mut self, model: ModelInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.set_model_picker(false, window, cx);
        if self.model.as_ref() != Some(&model) {
            self.set_model(model, cx);
        }
    }

    fn render_model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.entity().downgrade();
        let busy = self.busy();
        let label = self
            .model
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "No model".into());
        let dot = self
            .model
            .as_ref()
            .map_or(ui::faint(), |m| provider_color(&m.provider));
        let trigger = Button::new("model")
            .ghost()
            .xsmall()
            .text_color(ui::info())
            .tooltip("Choose model")
            .child(if busy {
                Spinner::new().xsmall().color(ui::info()).into_any_element()
            } else {
                div().size(px(7.)).rounded_full().bg(dot).into_any_element()
            })
            .child(
                div()
                    .min_w_0()
                    .when(self.pane_width.get() < NARROW, |d| d.max_w(px(120.)))
                    .truncate()
                    .child(label),
            )
            .dropdown_caret(true);

        let models = self.filtered_models(cx);
        let cursor = self.live.model_cursor.min(models.len().saturating_sub(1));
        let current = self.model.clone();
        let search = self.model_search.clone();
        let has_any = !self.models.is_empty();
        let content_weak = weak.clone();
        let scroll = self.live.model_scroll.clone();

        Popover::new("model-picker")
            .anchor(Anchor::BottomLeft)
            .trigger(trigger)
            .open(self.live.model_picker)
            .track_focus(&self.model_search.focus_handle(cx))
            .on_open_change(move |open, window, cx| {
                let _ = weak.update(cx, |this, cx| this.set_model_picker(*open, window, cx));
            })
            .w(px(380.))
            .p_0()
            .content(move |_, _, cx| {
                let weak = content_weak.clone();
                let mut list = v_flex()
                    .id("model-list")
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .p_1();
                let mut provider = "";
                for (i, model) in models.iter().enumerate() {
                    if model.provider != provider {
                        provider = &model.provider;
                        list = list.child(
                            h_flex()
                                .gap_1p5()
                                .px_2()
                                .pt_2()
                                .pb_1()
                                .child(
                                    div()
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(provider_color(provider)),
                                )
                                .child(ui::section_label(provider.to_string())),
                        );
                    }
                    let is_current = current.as_ref() == Some(model);
                    let pick = model.clone();
                    let weak = weak.clone();
                    list = list.child(
                        h_flex()
                            .id(("model-row", i))
                            .relative()
                            .w_full()
                            .gap_2()
                            .px_3()
                            .py_1p5()
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|s| s.bg(ui::surface_hover()))
                            .when(i == cursor, |this| {
                                this.bg(ui::surface_hover()).child(accent_bar())
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(13.))
                                    .text_color(if is_current { ui::accent() } else { ui::fg() })
                                    .child(model.name.clone()),
                            )
                            .when(model.context_window > 0, |this| {
                                this.child(ui::chip(
                                    format_tokens(model.context_window),
                                    ui::muted(),
                                ))
                            })
                            .when(model.reasoning, |this| {
                                this.child(ui::chip("reasoning", ui::petal()))
                            })
                            .when(is_current, |this| {
                                this.child(
                                    Icon::new(IconName::Check).xsmall().text_color(ui::accent()),
                                )
                            })
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.pick_model(pick.clone(), window, cx)
                                });
                            }),
                    );
                }
                if models.is_empty() {
                    list = list.child(
                        div()
                            .px_3()
                            .py_4()
                            .text_size(px(12.))
                            .text_color(ui::faint())
                            .child(if has_any {
                                "No models match."
                            } else {
                                "No models yet. Sign in to a provider."
                            }),
                    );
                }
                let sign_in = weak.clone();
                v_flex()
                    .w_full()
                    .font_family(cx.theme().font_family.clone())
                    .text_color(ui::fg())
                    .child(
                        div().p_2().border_b_1().border_color(ui::border()).child(
                            ui::field()
                                .child(Icon::new(IconName::Search).xsmall().text_color(ui::faint()))
                                .child(div().flex_1().child(Input::new(&search).appearance(false))),
                        ),
                    )
                    .child(list)
                    .child(
                        div().p_1().border_t_1().border_color(ui::border()).child(
                            Button::new("picker-sign-in")
                                .ghost()
                                .small()
                                .w_full()
                                .icon(Icon::new(IconName::LogIn))
                                .label(if has_any {
                                    "Sign in to another provider…"
                                } else {
                                    "Sign in to a provider…"
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = sign_in.update(cx, |this, cx| {
                                        this.set_model_picker(false, window, cx);
                                        cx.emit(super::SessionEvent::OpenProviders);
                                    });
                                }),
                        ),
                    )
            })
    }

    fn render_mode_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.mode;
        let (icon, color) = mode_style(current);
        let weak = cx.entity().downgrade();
        let trigger = Button::new("mode-picker")
            .ghost()
            .xsmall()
            .rounded(px(999.))
            .text_color(color)
            .bg(color.opacity(0.10))
            .border_1()
            .border_color(color.opacity(0.25))
            .icon(Icon::new(icon))
            .label(current.map_or("Permissions", |m| m.label()))
            .tooltip("Permission mode for new sessions");
        Popover::new("mode-popover")
            .anchor(Anchor::BottomRight)
            .trigger(trigger)
            .w(px(340.))
            .p_0()
            .content(move |_, _, cx| {
                let rows = PermissionMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(i, mode)| {
                        let (icon, color) = mode_style(Some(mode));
                        let weak = weak.clone();
                        let is_current = current == Some(mode);
                        h_flex()
                            .id(("mode", i))
                            .relative()
                            .w_full()
                            .gap_3()
                            .px_2()
                            .py_2()
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|s| s.bg(ui::surface_hover()))
                            .when(is_current, |this| {
                                this.bg(ui::surface_hover()).child(accent_bar())
                            })
                            .child(
                                div()
                                    .size(px(28.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_lg()
                                    .bg(color.opacity(0.12))
                                    .child(Icon::new(icon).small().text_color(color)),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(ui::fg())
                                            .child(mode.label()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(ui::muted())
                                            .child(mode.description()),
                                    ),
                            )
                            .when(is_current, |this| {
                                this.child(
                                    Icon::new(IconName::Check).small().text_color(ui::accent()),
                                )
                            })
                            .on_click(cx.listener(
                                move |state: &mut PopoverState, _, window, cx| {
                                    state.dismiss(window, cx);
                                    let _ = weak
                                        .update(cx, |this, cx| this.choose_mode(mode, window, cx));
                                },
                            ))
                    });
                v_flex()
                    .w_full()
                    .p_1()
                    .font_family(cx.theme().font_family.clone())
                    .child(
                        div()
                            .px_2()
                            .pt_1p5()
                            .pb_1()
                            .child(ui::section_label("New sessions start in")),
                    )
                    .children(rows)
                    .child(
                        div()
                            .mt_1()
                            .px_2()
                            .pt_2()
                            .pb_1()
                            .border_t_1()
                            .border_color(ui::border())
                            .text_size(px(11.))
                            .text_color(ui::faint())
                            .child("Applies from the next session"),
                    )
            })
    }

    /// Elapsed time, phase and current tool over the composer, with the
    /// session's live chips on the right.
    fn render_activity(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy();
        let waiting = self.live.pending.as_ref().filter(|p| !p.started);
        let since = self
            .live
            .turn
            .or_else(|| self.live.pending.as_ref().map(|p| p.since));
        let current_tool = self
            .transcript
            .items
            .iter()
            .rev()
            .find_map(|item| match item {
                Item::Tool {
                    name,
                    args,
                    details,
                    status: ToolStatus::Running,
                    ..
                } => {
                    let s = tools::summarize(name, args, details);
                    Some(if s.subject.is_empty() {
                        s.title
                    } else {
                        format!("{} {}", s.title, ui::first_line(&s.subject, 60))
                    })
                }
                _ => None,
            });
        let provider = self
            .model
            .as_ref()
            .map_or_else(|| "the agent".to_string(), |m| m.provider.clone());

        let left = h_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .overflow_hidden()
            .whitespace_nowrap()
            .when(busy, |this| {
                this.child(ui::pulse(
                    "activity-dot",
                    div().size(px(6.)).rounded_full().bg(ui::petal()),
                ))
                .when_some(since, |this, since| {
                    this.child(
                        div()
                            .font_family(ui::MONO)
                            .text_color(ui::muted())
                            .child(clock(since.elapsed())),
                    )
                })
                .child(div().h(px(10.)).w(px(1.)).bg(ui::border_strong()))
                .map(|this| match (waiting, self.phase) {
                    (Some(_), _) => this.child(
                        div()
                            .text_color(ui::petal())
                            .child(format!("✿ waiting for {provider}…")),
                    ),
                    (None, Some(phase)) => {
                        this.child(div().text_color(ui::petal()).child(format!("✿ {phase}…")))
                    }
                    (None, None) => this.child(div().text_color(ui::petal()).child("✿ working…")),
                })
                .when_some(current_tool, |this, tool| {
                    this.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(ui::muted())
                            .child(tool),
                    )
                })
            });

        let running_jobs = self.jobs.iter().filter(|j| j.is_running()).count();
        let width = self.pane_width.get();
        let statuses: Vec<(String, String)> = self
            .statuses
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "gentle:mode" | "gentle:yolo" | "gentle-jobs"))
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect();
        // Tiny panes fold every extension status (and limits) into one chip.
        let collapse = width < TINY;
        let folded: Vec<String> = if collapse {
            statuses
                .iter()
                .map(|(_, s)| s.clone())
                .chain(self.rate_limits.values().map(|l| l.text.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let right = h_flex()
            .flex_shrink_0()
            .gap_2()
            .when(!collapse, |this| {
                this.children(statuses.into_iter().map(|(key, s)| {
                    if width < NARROW {
                        // Icon-only, the text lives in the tooltip.
                        div()
                            .id(ElementId::Name(format!("status-{key}").into()))
                            .child(Icon::new(IconName::Info).xsmall().text_color(ui::faint()))
                            .tooltip(move |window, cx| Tooltip::new(s.clone()).build(window, cx))
                            .into_any_element()
                    } else {
                        div()
                            .max_w(px(220.))
                            .truncate()
                            .text_color(ui::faint())
                            .child(s)
                            .into_any_element()
                    }
                }))
            })
            .when(!folded.is_empty(), |this| {
                let tip = folded.join("\n");
                this.child(
                    div()
                        .id("status-overflow")
                        .child(ui::chip(format!("+{}", folded.len()), ui::muted()))
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)),
                )
            })
            .when(running_jobs > 0, |this| {
                this.child(ui::chip(
                    format!(
                        "⧗ {running_jobs} job{}",
                        if running_jobs == 1 { "" } else { "s" }
                    ),
                    ui::accent(),
                ))
            })
            .when(self.queued > 0, |this| {
                this.child(ui::chip(format!("{} queued", self.queued), ui::warning()))
            })
            .when_some(
                self.agents.as_ref().filter(|a| !a.tasks.is_empty()),
                |this, activity| {
                    let active = activity.summary.running
                        + activity.summary.waiting
                        + activity.summary.queued;
                    let color = if active > 0 {
                        ui::warning()
                    } else {
                        ui::muted()
                    };
                    this.child(
                        Button::new("agents-pill")
                            .ghost()
                            .xsmall()
                            .text_color(color)
                            .icon(Icon::new(IconName::Users))
                            .label(format!("{active}/{}", activity.tasks.len()))
                            .tooltip("Helpers (⌘⇧A)")
                            .on_click(cx.listener(|this, _, _, cx| this.open_agents(None, cx))),
                    )
                },
            )
            .children(
                self.rate_limits
                    .iter()
                    .filter(|_| !collapse)
                    .map(|(window, limit)| {
                        let color = if limit.percent >= 95 {
                            ui::danger()
                        } else if limit.percent >= 80 {
                            ui::warning()
                        } else {
                            ui::muted()
                        };
                        let tip = limit.text.clone();
                        div()
                            .id(ElementId::Name(format!("limit-{window}").into()))
                            .child(ui::chip(
                                format!("{} {}%", limit_label(window), limit.percent),
                                color,
                            ))
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    }),
            )
            .child(self.render_mode_picker(cx));

        h_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .h(px(24.))
            .px_1()
            .gap_3()
            .text_size(px(11.))
            .child(left)
            .child(right)
    }

    /// `NN%` — thin pink bar — window size.
    fn render_context_line(&self) -> impl IntoElement {
        let window_tokens = self.model.as_ref().map(|m| m.context_window).unwrap_or(0);
        let percent = self.context_percent.unwrap_or(0.0).clamp(0.0, 100.0) as f32;
        v_flex()
            .w_full()
            .px_3()
            .gap(px(2.))
            .child(
                h_flex()
                    .justify_between()
                    .font_family(ui::MONO)
                    .text_size(px(10.))
                    .text_color(ui::faint())
                    .child(match self.context_percent {
                        Some(p) => format!("{p:.0}%"),
                        None => "–".into(),
                    })
                    .child(if window_tokens > 0 {
                        format_tokens(window_tokens)
                    } else {
                        String::new()
                    }),
            )
            .child(
                div()
                    .w_full()
                    .h(px(2.))
                    .rounded_full()
                    .bg(ui::border())
                    .child(
                        div()
                            .h_full()
                            .w(relative(percent / 100.0))
                            .rounded_full()
                            .bg(ui::petal()),
                    ),
            )
    }

    /// 64px numbered thumbnails of the images waiting to be sent, each with ×.
    fn render_attachments(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.attachments.is_empty() {
            return None;
        }
        let tiles = self.attachments.iter().enumerate().map(|(ix, attachment)| {
            image_tile(Some(attachment.image.clone()), px(64.), px(64.))
                .child(number_badge(ix + 1))
                .child(
                    h_flex()
                        .id(("attachment-remove", ix))
                        .absolute()
                        .top(px(3.))
                        .right(px(3.))
                        .size(px(18.))
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(ui::well())
                        .cursor_pointer()
                        .hover(|s| s.bg(ui::danger()))
                        .tooltip(|window, cx| Tooltip::new("Remove").build(window, cx))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_attachment(ix, window, cx)
                        }))
                        .child(Icon::new(IconName::X).xsmall().text_color(ui::fg())),
                )
        });
        Some(h_flex().w_full().flex_wrap().gap_2().children(tiles))
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (prefix, leaf) = ui::split_path(&self.cwd);
        let empty = self.composer.read(cx).value().trim().is_empty() && self.attachments.is_empty();
        let busy = self.busy();
        let ready = matches!(self.backend, Backend::Ready(_));

        let action_button = if busy && empty {
            Button::new("stop")
                .small()
                .danger()
                .rounded(px(999.))
                .child(div().size(px(9.)).rounded_sm().bg(ui::well()))
                .tooltip("Stop (Esc)")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("send")
                .small()
                .primary()
                .rounded(px(999.))
                .icon(Icon::new(IconName::ArrowUp))
                .tooltip(if busy {
                    "Steer the running agent"
                } else {
                    "Send"
                })
                .disabled(empty || !ready)
                .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
        };

        let git = self.git.as_ref().map(|git| {
            h_flex()
                .flex_shrink_0()
                .gap_1()
                .text_size(px(12.))
                .child(
                    Icon::new(IconName::GitBranch)
                        .xsmall()
                        .text_color(ui::champagne()),
                )
                .child(
                    div()
                        .font_family(ui::MONO)
                        .text_color(ui::champagne())
                        .child(git.branch.clone().unwrap_or_else(|| "detached".into())),
                )
                .when(!git.files.is_empty(), |this| {
                    this.child(
                        div()
                            .font_family(ui::MONO)
                            .text_color(ui::muted())
                            .child(format!("±{}", git.files.len())),
                    )
                })
        });

        let width = self.pane_width.get();
        let narrow = width < NARROW;
        let bottom = h_flex()
            .w_full()
            .min_w_0()
            .gap_1p5()
            .items_center()
            .child(
                Button::new("attach")
                    .ghost()
                    .xsmall()
                    .text_color(ui::muted())
                    .icon(Icon::new(IconName::Paperclip))
                    .tooltip("Attach images (or paste / drop them)")
                    .disabled(!ready)
                    .on_click(cx.listener(|this, _, window, cx| this.pick_images(window, cx))),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .child(self.render_model_picker(cx)),
            )
            .when(self.model.as_ref().is_some_and(|m| m.reasoning), |this| {
                let thinking = self.thinking.clone();
                this.child(
                    Button::new("thinking")
                        .ghost()
                        .xsmall()
                        .text_color(ui::muted())
                        .icon(Icon::new(IconName::Brain))
                        .tooltip(format!("Thinking: {thinking} · click to cycle"))
                        .when(!narrow, |b| b.label(thinking))
                        .on_click(cx.listener(|this, _, _, cx| this.cycle_thinking(cx))),
                )
            })
            .when(!narrow, |this| {
                this.child(
                    h_flex()
                        .id("cwd")
                        .min_w_0()
                        .flex_shrink(1.)
                        .gap_1()
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(ui::surface_hover()))
                        .text_size(px(12.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .tooltip(|window, cx| Tooltip::new("Open another folder").build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx)))
                        .child(Icon::new(IconName::Folder).xsmall().text_color(ui::faint()))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis_start()
                                .font_family(ui::MONO)
                                .text_color(ui::faint())
                                .child(prefix),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_family(ui::MONO)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child(leaf),
                        ),
                )
            })
            .when(!narrow, |this| this.children(git))
            .child(div().flex_1().min_w_0())
            .when(self.cost > 0.0 && !narrow, |this| {
                this.child(
                    div()
                        .font_family(ui::MONO)
                        .text_size(px(11.))
                        .text_color(ui::faint())
                        .child(format!("${:.2}", self.cost)),
                )
            })
            .child(div().flex_shrink_0().child(action_button));

        v_flex()
            .w_full()
            .min_w_0()
            .max_w(px(720.))
            .gap_1p5()
            .children(self.render_slash_menu(cx))
            .children(self.render_dialog(cx))
            .child(self.render_activity(cx))
            .child(self.render_context_line())
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .overflow_hidden()
                    .gap_2()
                    .map(|d| if narrow { d.px_3() } else { d.px_4() })
                    .pt_3()
                    .pb_2()
                    .rounded_2xl()
                    .border_1()
                    .border_color(ui::border_strong())
                    .bg(ui::surface())
                    .shadow_lg()
                    .children(self.render_attachments(cx))
                    .child({
                        let weak = cx.entity().downgrade();
                        div().min_w_0().text_size(px(14.)).child(
                            Textarea::new(&self.composer)
                                .appearance(false)
                                .disabled(!ready)
                                .on_paste(move |item, window, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.paste_clipboard(item, window, cx)
                                    })
                                    .unwrap_or(false)
                                }),
                        )
                    })
                    .child(bottom),
            )
            .when(width >= TINY, |this| {
                this.child(
                    h_flex()
                        .h(px(16.))
                        .min_w_0()
                        .justify_center()
                        .overflow_hidden()
                        .text_size(px(10.))
                        .text_color(ui::faint())
                        .child(div().min_w_0().truncate().child(if busy {
                            "↵ steers · esc stops"
                        } else if narrow {
                            "↵ sends · ⇧↵ newline · / commands"
                        } else {
                            "↵ sends · ⇧↵ newline · !cmd runs a shell command · / for commands · ⌘N new session"
                        })),
                )
            })
    }

    // ── Rail ───────────────────────────────────────────────────────────────

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let card = || ui::card().w_full().gap_1p5().p_3();
        let mut rail = v_flex()
            .id("rail")
            .w(px(300.))
            .h_full()
            .flex_shrink_0()
            .gap_3()
            .p_3()
            .overflow_y_scroll();
        // Git changes live in the sidebar's Changes tab; the rail doesn't repeat them.

        if !self.jobs.is_empty() {
            let running = self.jobs.iter().filter(|j| j.is_running()).count();
            rail = rail.child(ui::appear(
                "rail-jobs",
                card()
                    .child(
                        h_flex()
                            .child(ui::section_label("Jobs"))
                            .child(div().flex_1())
                            .when(running > 0, |this| {
                                this.child(ui::chip(format!("{running} running"), ui::accent()))
                            }),
                    )
                    .children(self.jobs.iter().take(10).map(|job| {
                        let (glyph, color) = if job.is_running() {
                            ("⧗", ui::accent())
                        } else if job.exit_code.unwrap_or(0) == 0 && job.status != "failed" {
                            ("✓", ui::success())
                        } else {
                            ("✗", ui::danger())
                        };
                        h_flex()
                            .gap_2()
                            .text_size(px(12.))
                            .child(div().text_color(color).child(glyph))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_family(ui::MONO)
                                    .text_color(ui::fg())
                                    .child(if job.label.is_empty() {
                                        format!("job {}", job.id)
                                    } else {
                                        job.label.clone()
                                    }),
                            )
                            .child(div().text_color(ui::faint()).child(match job.exit_code {
                                Some(code) => format!("exit {code}"),
                                None => job.status.clone(),
                            }))
                    })),
            ));
        }

        if !self.todo.is_empty() {
            let done = self.todo.iter().filter(|t| t.status == "done").count();
            rail = rail.child(ui::appear(
                "rail-todo",
                card()
                    .child(
                        h_flex()
                            .child(ui::section_label("Todo"))
                            .child(div().flex_1())
                            .child(ui::chip(
                                format!("{done}/{}", self.todo.len()),
                                ui::success(),
                            )),
                    )
                    .children(self.todo.iter().map(|task| {
                        let (glyph, color) = match task.status.as_str() {
                            "done" => ("✓", ui::success()),
                            "in_progress" => ("◐", ui::accent()),
                            "blocked" => ("⊘", ui::warning()),
                            "dropped" => ("–", ui::faint()),
                            _ => ("○", ui::muted()),
                        };
                        v_flex()
                            .child(
                                h_flex()
                                    .items_start()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .child(div().flex_none().text_color(color).child(glyph))
                                    // min_w_0 lets long titles wrap inside the card
                                    // instead of pushing past its edge.
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_color(if task.status == "done" {
                                                ui::muted()
                                            } else {
                                                ui::fg()
                                            })
                                            .child(task.title.clone()),
                                    ),
                            )
                            .when_some(task.note.clone(), |this, note| {
                                this.child(
                                    div()
                                        .pl_4()
                                        .text_size(px(11.))
                                        .text_color(ui::faint())
                                        .child(note),
                                )
                            })
                    })),
            ));
        }

        if let Some(activity) = &self.agents
            && !activity.tasks.is_empty()
        {
            rail = rail.child(
                card()
                    .id("rail-helpers")
                    .cursor_pointer()
                    .hover(|s| s.border_color(ui::border_strong()))
                    .on_click(cx.listener(|this, _, _, cx| this.open_agents(None, cx)))
                    .child(
                        h_flex()
                            .child(ui::section_label("Helpers"))
                            .child(div().flex_1())
                            .when(activity.summary.running > 0, |this| {
                                this.child(ui::chip(
                                    format!("{} running", activity.summary.running),
                                    ui::warning(),
                                ))
                            }),
                    )
                    .children(activity.tasks.iter().take(12).map(|task| {
                        let s = &task.summary;
                        let color = super::agents::status_color(&s.status);
                        let dot = div().size(px(6.)).rounded_full().bg(color);
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .child(if s.status == "running" {
                                        ui::pulse(
                                            ElementId::Name(format!("helper-{}", s.id).into()),
                                            dot,
                                        )
                                        .into_any_element()
                                    } else {
                                        dot.into_any_element()
                                    })
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_color(ui::fg())
                                            .child(s.agent.clone()),
                                    )
                                    .child(
                                        div().flex_none().text_color(color).child(s.status.clone()),
                                    ),
                            )
                            .when_some(s.label.clone(), |this, label| {
                                this.child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(11.))
                                        .text_color(ui::muted())
                                        .child(label),
                                )
                            })
                            .when_some(s.last_step.clone(), |this, step| {
                                this.child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_size(px(11.))
                                        .font_family(ui::MONO)
                                        .text_color(ui::faint())
                                        .child(ui::first_line(&step, 60)),
                                )
                            })
                    })),
            );
        }

        for (key, lines) in &self.widgets {
            rail = rail.child(card().child(ui::section_label(key.clone())).children(
                lines.iter().map(|line| {
                    div()
                        .min_w_0()
                        .text_size(px(12.))
                        .font_family(ui::MONO)
                        .text_color(ui::muted())
                        .child(line.clone())
                }),
            ));
        }

        if let Some(usage) = self.transcript.last_usage {
            rail = rail.child(
                card().child(ui::section_label("Last response")).child(
                    div()
                        .text_size(px(12.))
                        .font_family(ui::MONO)
                        .text_color(ui::muted())
                        .child(format!(
                            "in {} · out {} · cache {}",
                            format_tokens(usage.input),
                            format_tokens(usage.output),
                            format_tokens(usage.cache_read)
                        )),
                ),
            );
        }
        rail
    }
}

impl Focusable for SessionView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_live(cx);
        self.image_cache.sync(
            self.transcript
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::User { images, .. } => Some(images),
                    _ => None,
                })
                .flatten(),
        );
        let empty = self.transcript.items.is_empty() && self.live.pending.is_none();
        // The rail gives way on narrow windows so the conversation keeps its width.
        let show_rail = cx.try_global::<RailVisible>().is_none_or(|r| r.0)
            && window.viewport_size().width >= px(980.);
        let count = self.transcript.items.len();
        let mut items: Vec<AnyElement> = self
            .transcript
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                // Keyed by index: items animate in once, never again.
                ui::appear(
                    ("item", ix),
                    div().w_full().child(self.render_item(ix, item, cx)),
                )
                .into_any_element()
            })
            .collect();

        // The prompt just sent, until Pi echoes it; it takes the same slot (and
        // animation id) as the real message so nothing jumps.
        if let Some(pending) = &self.live.pending {
            if count == pending.base && !pending.text.starts_with('/') {
                items.push(
                    ui::appear(
                        ("item", pending.base),
                        div().w_full().child(user_bubble(
                            &pending.text,
                            pending.images.iter().cloned().map(Some).collect(),
                        )),
                    )
                    .into_any_element(),
                );
            }
            items.push(
                ui::appear(
                    ("pending", pending.base),
                    div().w_full().child(working_indicator(
                        ("pending-petal", pending.base),
                        "Thinking…",
                        Some(pending.since.elapsed()),
                    )),
                )
                .into_any_element(),
            );
        } else if self.transcript.running {
            // Between tool calls nothing streams; keep a pulse at the end.
            let idle_tail = match self.transcript.items.last() {
                Some(Item::Assistant { streaming, .. }) => !streaming,
                Some(Item::Tool { status, .. }) => *status != ToolStatus::Running,
                Some(Item::Shell { running, .. }) => !running,
                _ => true,
            };
            if idle_tail {
                items.push(
                    ui::appear(
                        ("between", count),
                        div().w_full().child(working_indicator(
                            ("between-petal", count),
                            "Working…",
                            self.live.turn.map(|t| t.elapsed()),
                        )),
                    )
                    .into_any_element(),
                );
            }
        }

        let width = self.pane_width.clone();
        let view = cx.entity_id();
        let narrow = self.pane_width.get() < NARROW;
        let main = v_flex()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(ui::surface_hover()))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.attach_paths(paths.paths().to_vec(), window, cx);
            }))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let w = f32::from(bounds.size.width);
                        if (w - width.get()).abs() > 1. {
                            width.set(w);
                            cx.notify(view);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .id("transcript")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        h_flex().w_full().justify_center().child(
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .max_w(px(760.))
                                .map(|d| if narrow { d.px_3() } else { d.px_6() })
                                .py_6()
                                .gap_4()
                                .when(empty, |this| this.child(self.render_hero(cx)))
                                .children(self.render_backend_state())
                                .children(items),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .map(|d| if narrow { d.px_3() } else { d.px_6() })
                    .pb_2()
                    .child(self.render_composer(cx)),
            );

        h_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::abort))
            .on_action(cx.listener(Self::toggle_agents))
            .on_action(cx.listener(Self::on_new_session))
            .capture_key_down(cx.listener(Self::on_dialog_key))
            .relative()
            .size_full()
            .child(main)
            .when(show_rail, |this| this.child(self.render_rail(cx)))
            .children(self.render_agents_panel(cx))
    }
}

// ── Building blocks ────────────────────────────────────────────────────────

/// Everything a tool card shows, derived from the tool call.
struct CardSpec {
    icon: IconName,
    title: String,
    subject: String,
    chip: Option<String>,
    added: Option<usize>,
    removed: Option<usize>,
    /// The command, file body or diff (`true`) shown in a code well.
    input: Option<(String, bool)>,
    output: String,
    /// Output is the point of the tool (commands): show it without a click.
    always_output: bool,
    status: ToolStatus,
}

fn tool_spec(
    name: &str,
    args: &Value,
    status: ToolStatus,
    output: &str,
    details: &Value,
) -> CardSpec {
    let summary = tools::summarize(name, args, details);
    let arg = |key: &str| args[key].as_str().filter(|s| !s.trim().is_empty());
    let command_like = matches!(name, "bash" | "bash_background" | "monitor" | "codemode");
    let input = match name {
        "bash" | "bash_background" | "monitor" => arg("command").map(|c| (c.to_string(), false)),
        "codemode" => arg("code")
            .or(arg("command"))
            .map(|c| (c.to_string(), false)),
        "write" => arg("content").map(|c| (c.to_string(), false)),
        "edit" => details["diff"].as_str().map(|d| (d.to_string(), true)),
        _ => None,
    };
    let chip = match name {
        "bash" | "bash_background" | "monitor" => {
            Some(command_language(arg("command").unwrap_or_default()).to_string())
        }
        "codemode" => Some(arg("language").unwrap_or("code").to_string()),
        "read" | "edit" | "write" => {
            language_for_path(arg("path").unwrap_or_default()).map(str::to_string)
        }
        n if n.starts_with("subagent") => arg("agent").map(str::to_string),
        _ => None,
    };
    let title = match arg("description") {
        Some(description) if command_like => ui::first_line(description, 80),
        _ => summary.title,
    };
    let subject = if command_like && input.is_some() {
        String::new()
    } else {
        summary.subject
    };
    // A file tool's result is a one-line receipt; only failures say more.
    let output =
        if matches!(name, "edit" | "write") && input.is_some() && status != ToolStatus::Failed {
            String::new()
        } else {
            output.to_string()
        };
    CardSpec {
        icon: tool_icon(name),
        title,
        subject,
        chip,
        added: summary.added,
        removed: summary.removed,
        input,
        output,
        always_output: command_like,
        status,
    }
}

fn tool_icon(name: &str) -> IconName {
    match name {
        "bash" | "bash_background" | "monitor" | "codemode" => IconName::CodeXml,
        "read" => IconName::FileText,
        "edit" | "write" => IconName::FilePen,
        "grep" | "find" => IconName::Search,
        "ls" => IconName::Folder,
        "todo" => IconName::ListTodo,
        n if n.starts_with("subagent") => IconName::Users,
        n if n.starts_with("web")
            || n.contains("fetch")
            || n.contains("browser")
            || n.contains("search") =>
        {
            IconName::Globe
        }
        _ => IconName::Wrench,
    }
}

fn language_for_path(path: &str) -> Option<&'static str> {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, ext) = file.rsplit_once('.')?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "rs" => "rust",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "md" | "mdx" => "markdown",
        "json" | "jsonl" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" | "fish" => "bash",
        "go" => "go",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "java" => "java",
        "rb" => "ruby",
        "lua" => "lua",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "c++",
        "cs" => "c#",
        "css" | "scss" => "css",
        "html" | "htm" => "html",
        "sql" => "sql",
        _ => return None,
    })
}

/// The interpreter a shell command runs, for the card chip.
fn command_language(command: &str) -> &'static str {
    let program = command.split_whitespace().next().unwrap_or_default();
    let program = program.rsplit('/').next().unwrap_or(program);
    match program {
        p if p.starts_with("python") => "python",
        "node" | "bun" | "deno" => "javascript",
        "ruby" => "ruby",
        _ => "bash",
    }
}

/// Pi's edit diff lines: `+12 text`, `-3 text`, ` 4 ctx` → (sign, number, text).
fn parse_diff_line(line: &str) -> (char, &str, &str) {
    let sign = match line.as_bytes().first() {
        Some(b'+') => '+',
        Some(b'-') => '-',
        Some(b' ') => ' ',
        _ => return (' ', "", line),
    };
    let rest = &line[1..];
    let trimmed = rest.trim_start_matches(' ');
    let digits = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    if digits == 0 {
        return (sign, "", rest);
    }
    let text = &trimmed[digits..];
    (
        sign,
        &trimmed[..digits],
        text.strip_prefix(' ').unwrap_or(text),
    )
}

fn diff_row(line: &str) -> Div {
    let (sign, number, text) = parse_diff_line(line);
    let (color, bg, glyph) = match sign {
        '+' => (ui::success(), Some(ui::success().opacity(0.08)), "+"),
        '-' => (ui::danger(), Some(ui::danger().opacity(0.08)), "−"),
        _ => (ui::muted(), None, " "),
    };
    h_flex()
        .w_full()
        .px_3()
        .gap_2()
        .whitespace_nowrap()
        .when_some(bg, |this, bg| this.bg(bg))
        .child(
            div()
                .w(px(32.))
                .flex_shrink_0()
                .text_right()
                .text_color(ui::faint())
                .child(number.to_string()),
        )
        .child(
            div()
                .w(px(10.))
                .flex_shrink_0()
                .text_color(color)
                .child(glyph),
        )
        .child(
            div()
                .text_color(if sign == ' ' { ui::muted() } else { ui::fg() })
                .child(if text.is_empty() {
                    " ".to_string()
                } else {
                    text.to_string()
                }),
        )
}

/// The first `max` lines of `text`, plus how many were hidden.
fn head(text: &str, max: usize) -> (String, usize) {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let hidden = lines.len().saturating_sub(max);
    (lines[..lines.len() - hidden].join("\n"), hidden)
}

fn clock(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

fn status_footer(status: ToolStatus, duration: Option<Duration>) -> Div {
    let (icon, label, color) = match status {
        ToolStatus::Running => (None, "Running", ui::info()),
        ToolStatus::Done => (Some(IconName::Check), "Done", ui::success()),
        ToolStatus::Failed => (Some(IconName::X), "Failed", ui::danger()),
    };
    h_flex()
        .px_3()
        .py_1p5()
        .gap_1p5()
        .border_t_1()
        .border_color(ui::border())
        .text_size(px(12.))
        .text_color(color)
        .map(|this| match icon {
            Some(icon) => this.child(Icon::new(icon).xsmall()),
            None => this.child(Spinner::new().xsmall().color(ui::info())),
        })
        .child(label)
        .when_some(duration, |this, d| {
            this.child(
                div()
                    .font_family(ui::MONO)
                    .text_size(px(11.))
                    .text_color(ui::faint())
                    .child(ui::format_duration(d.as_millis() as u64)),
            )
        })
}

/// A user message: its images (numbered like their `[Image #N]` tokens)
/// over the text, tokens shown as `🖼 #N` chips. `None` is an image that
/// could not be decoded.
fn user_bubble(text: &str, images: Vec<Option<Arc<Image>>>) -> Div {
    let (shown, chips) = attachments::chip_text(text);
    let chip = HighlightStyle {
        color: Some(ui::info()),
        font_weight: Some(FontWeight::SEMIBOLD),
        background_color: Some(ui::info().opacity(0.14)),
        ..Default::default()
    };
    h_flex().w_full().justify_end().child(
        v_flex()
            .max_w(px(560.))
            .gap_2()
            .px_4()
            .py_2p5()
            .rounded_2xl()
            .bg(ui::raised())
            .border_1()
            .border_color(ui::border())
            .text_size(px(14.))
            .text_color(ui::fg())
            .when(!images.is_empty(), |this| {
                this.child(h_flex().flex_wrap().gap_2().children(
                    images.into_iter().enumerate().map(|(ix, image)| {
                        image_tile(image, px(160.), px(120.)).child(number_badge(ix + 1))
                    }),
                ))
            })
            .when(!shown.trim().is_empty(), |this| {
                this.child(
                    StyledText::new(shown).with_highlights(chips.into_iter().map(|r| (r, chip))),
                )
            }),
    )
}

/// A rounded, cropped image (or a placeholder) for attachment strips.
fn image_tile(image: Option<Arc<Image>>, width: Pixels, height: Pixels) -> Div {
    div()
        .relative()
        .flex_shrink_0()
        .w(width)
        .h(height)
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .border_color(ui::border_strong())
        .bg(ui::well())
        .child(match image {
            Some(image) => img(image)
                .size_full()
                .rounded_lg()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => h_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Icon::new(IconName::Image).text_color(ui::faint()))
                .into_any_element(),
        })
}

/// `#N` in an image's lower-left corner, matching its `[Image #N]` token.
fn number_badge(n: usize) -> Div {
    div()
        .absolute()
        .left(px(3.))
        .bottom(px(3.))
        .px_1()
        .rounded_sm()
        .bg(ui::well())
        .font_family(ui::MONO)
        .text_size(px(10.))
        .text_color(ui::info())
        .child(format!("#{n}"))
}

fn notice_card(level: NoticeLevel, text: &str) -> Div {
    let (icon, color) = match level {
        NoticeLevel::Info => (IconName::Info, ui::muted()),
        NoticeLevel::Warning => (IconName::TriangleAlert, ui::warning()),
        NoticeLevel::Error => (IconName::CircleX, ui::danger()),
    };
    ui::card()
        .flex_row()
        .w_full()
        .items_start()
        .gap_2()
        .px_3()
        .py_2()
        .when(level != NoticeLevel::Info, |this| {
            this.border_color(color.opacity(0.35))
        })
        .child(
            div()
                .pt(px(2.))
                .child(Icon::new(icon).xsmall().text_color(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(if level == NoticeLevel::Info {
                    ui::muted()
                } else {
                    color
                })
                .whitespace_normal()
                .child(text.to_string()),
        )
}

/// The petal-and-shimmer "agent is on it" row.
fn working_indicator(
    id: impl Into<ElementId>,
    label: &'static str,
    elapsed: Option<Duration>,
) -> Div {
    h_flex()
        .gap_2()
        .items_center()
        .child(ui::pulse(
            id,
            div().text_size(px(15.)).text_color(ui::petal()).child("✿"),
        ))
        .child(
            ShimmerText::new(label)
                .highlight_color(ui::fg())
                .text_size(px(13.))
                .italic()
                .text_color(ui::muted()),
        )
        .when_some(elapsed, |this, d| {
            this.child(
                div()
                    .font_family(ui::MONO)
                    .text_size(px(11.))
                    .text_color(ui::faint())
                    .child(clock(d)),
            )
        })
}

/// The selected-row marker of lists and pickers.
fn accent_bar() -> Div {
    div()
        .absolute()
        .left_0()
        .top(px(6.))
        .bottom(px(6.))
        .w(px(3.))
        .rounded_full()
        .bg(ui::accent())
}

fn checkbox_box(checked: bool) -> Div {
    div()
        .size(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .map(|this| {
            if checked {
                this.bg(ui::accent())
                    .border_color(ui::accent())
                    .child(Icon::new(IconName::Check).xsmall().text_color(ui::well()))
            } else {
                this.border_color(ui::border_strong()).bg(ui::well())
            }
        })
}

/// `ask_user_question` multi-select rows: `[x] label` / `[ ] label`.
fn checkbox_option(option: &str) -> Option<(bool, &str)> {
    [
        ("[x] ", true),
        ("[X] ", true),
        ("[ ] ", false),
        ("☑ ", true),
        ("☐ ", false),
    ]
    .into_iter()
    .find_map(|(prefix, checked)| option.strip_prefix(prefix).map(|rest| (checked, rest)))
}

/// `Persona (current: gentleman)` → (`Persona`, `gentleman`).
fn split_current(title: &str) -> (&str, Option<&str>) {
    if let Some(start) = title.rfind("(current: ")
        && title.ends_with(')')
    {
        let current = title[start + "(current: ".len()..title.len() - 1].trim();
        return (
            title[..start].trim_end(),
            Some(current).filter(|c| !c.is_empty()),
        );
    }
    (title, None)
}

fn option_matches(option: &str, current: &str) -> bool {
    let option = checkbox_option(option)
        .map_or(option, |(_, rest)| rest)
        .trim();
    option.eq_ignore_ascii_case(current)
        || option
            .get(..current.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(current))
            && option[current.len()..].starts_with([' ', '—', '-', '('])
}

fn mode_style(mode: Option<PermissionMode>) -> (IconName, Hsla) {
    match mode {
        Some(PermissionMode::Normal) => (IconName::Shield, ui::muted()),
        Some(PermissionMode::Auto) => (IconName::Sparkles, ui::info()),
        Some(PermissionMode::Yolo) => (IconName::Zap, ui::warning()),
        Some(PermissionMode::SuperYolo) => (IconName::Flame, ui::danger()),
        None => (IconName::Shield, ui::faint()),
    }
}

fn provider_color(provider: &str) -> Hsla {
    let palette = [
        ui::petal(),
        ui::info(),
        ui::success(),
        ui::champagne(),
        ui::warning(),
    ];
    let hash = provider
        .bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    palette[hash % palette.len()]
}

/// `Claude rate limit warning: 51% used (seven_day)` → (51, `seven_day`).
pub(super) fn parse_rate_limit(message: &str) -> Option<(u32, String)> {
    const MARK: &str = "% used (";
    let at = message.find(MARK)?;
    let before = &message[..at];
    let digits = before.len() - before.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let percent = before[at - digits..].parse().ok()?;
    let rest = &message[at + MARK.len()..];
    let window = &rest[..rest.find(')')?];
    (!window.is_empty()
        && window
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then(|| (percent, window.to_string()))
}

/// `seven_day` → `7d`, `five_hour` → `5h`.
fn limit_label(window: &str) -> String {
    let short = window
        .replace("seven_day", "7d")
        .replace("five_hour", "5h")
        .replace("one_hour", "1h")
        .replace("one_day", "1d");
    short.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::{
        checkbox_option, command_language, head, language_for_path, limit_label, option_matches,
        parse_diff_line, parse_rate_limit, split_current,
    };

    #[test]
    fn diff_lines_split_sign_number_and_text() {
        assert_eq!(parse_diff_line("+12 text"), ('+', "12", "text"));
        assert_eq!(parse_diff_line("-  3 old line"), ('-', "3", "old line"));
        assert_eq!(parse_diff_line("  4 ctx"), (' ', "4", "ctx"));
        assert_eq!(parse_diff_line("+7 "), ('+', "7", ""));
        assert_eq!(parse_diff_line("..."), (' ', "", "..."));
    }

    #[test]
    fn head_keeps_first_lines() {
        assert_eq!(head("a\nb\nc\n", 2), ("a\nb".to_string(), 1));
        assert_eq!(head("a", 5), ("a".to_string(), 0));
    }

    #[test]
    fn dialog_titles_expose_the_current_value() {
        assert_eq!(
            split_current("el Gentleman persona (current: gentleman)"),
            ("el Gentleman persona", Some("gentleman"))
        );
        assert_eq!(split_current("Pick one"), ("Pick one", None));
        assert!(option_matches("gentleman", "gentleman"));
        assert!(option_matches("Gentleman — formal", "gentleman"));
        assert!(!option_matches("gentlemanly", "gentleman"));
    }

    #[test]
    fn multi_select_options_are_checkboxes() {
        assert_eq!(checkbox_option("[x] Rust"), Some((true, "Rust")));
        assert_eq!(checkbox_option("[ ] Go"), Some((false, "Go")));
        assert_eq!(checkbox_option("Done"), None);
    }

    #[test]
    fn rate_limit_warnings_parse() {
        assert_eq!(
            parse_rate_limit("Claude rate limit warning: 51% used (seven_day)"),
            Some((51, "seven_day".to_string()))
        );
        assert_eq!(parse_rate_limit("Everything is fine"), None);
        assert_eq!(limit_label("seven_day"), "7d");
        assert_eq!(limit_label("five_hour"), "5h");
        assert_eq!(limit_label("seven_day_opus"), "7d opus");
    }

    #[test]
    fn languages_come_from_paths_and_commands() {
        assert_eq!(language_for_path("crates/app/src/ui.rs"), Some("rust"));
        assert_eq!(language_for_path("v1.2/Makefile"), None);
        assert_eq!(command_language("python3 - <<'EOF'"), "python");
        assert_eq!(command_language("/usr/bin/node x.js"), "javascript");
        assert_eq!(command_language("cargo test"), "bash");
    }
}
