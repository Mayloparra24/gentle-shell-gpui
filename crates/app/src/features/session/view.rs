//! Rendering for [`SessionView`]: the composition of the pane and the
//! conversation's items (tool cards, thinking, messages, notices). The
//! composer, its pickers, dialogs and overlays live in the child modules.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::attachments;
use super::state::BackendStatus;
use super::{CONTEXT, SessionView};
use crate::shared::ui;
use gentle_core::tools;
use gentle_core::transcript::{Block, Item, NoticeLevel, ToolStatus};

mod activity;
mod cards;
mod composer;
mod dialogs;
mod effort;
mod live;
mod model_picker;
mod permissions;
mod slash_menu;
mod todo_island;
mod transcript_list;

pub(super) use activity::parse_rate_limit;
pub(super) use cards::markdown_custom;
pub(super) use live::{Live, RateLimit};
use live::{Span, TICK, label_stamp, ticker_needed, track};
pub(super) use transcript_list::TranscriptList;
use transcript_list::{HeaderKey, Prepared, Row, Tail};

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

/// Bring the Markdown rendered for `key` up to `text`, appending when it
/// only grew (streaming).
fn sync_markdown_block(
    markdown: &mut HashMap<(usize, usize), (Entity<TextViewState>, usize)>,
    key: (usize, usize),
    text: &str,
    cx: &mut Context<SessionView>,
) {
    match markdown.get_mut(&key) {
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
            markdown.insert(key, (state, text.len()));
        }
    }
}

impl SessionView {
    /// A prompt is in flight: Pi is working or has not picked it up yet.
    pub(super) fn busy(&self) -> bool {
        self.transcript.running || self.live.pending.is_some()
    }

    /// The empty state (hero and banner) shows.
    pub(super) fn hero_shown(&self) -> bool {
        self.transcript.items.is_empty() && self.live.pending.is_none()
    }

    /// Bring everything derived from the transcript up to date: Markdown,
    /// live spans, decoded images, card data and the list's measurements.
    /// Only items from the first changed one on are revisited.
    pub(super) fn sync_view(&mut self, cx: &mut Context<Self>) {
        let dirty = self.transcript.take_dirty();
        if let Some(dirty) = dirty {
            if dirty.reset {
                self.markdown.clear();
                self.list.forget_items();
            }
            let from = dirty.from.min(self.transcript.items.len());
            self.sync_markdown(from, cx);
            self.track_live(from);
            self.list.prepare(&self.transcript.items, from);
            self.request_images(from, dirty.reset, cx);
        }
        self.sync_live(cx);
        let tail = self.tail_rows();
        let header = HeaderKey {
            hero: self.hero_shown(),
            backend: match self.backend.status {
                BackendStatus::Starting => 0,
                BackendStatus::Ready(_) => 1,
                BackendStatus::Failed(_) => 2,
                BackendStatus::Exited(_) => 3,
            },
        };
        self.list
            .sync(self.transcript.items.len(), tail, header, dirty);
    }

    /// Bring the rendered Markdown up to date for items `from..` (earlier
    /// items have not changed since the last sync).
    fn sync_markdown(&mut self, from: usize, cx: &mut Context<Self>) {
        let items = self.transcript.items.get(from..).unwrap_or_default();
        for (item_ix, item) in (from..).zip(items) {
            match item {
                Item::Assistant { blocks, .. } => {
                    for (block_ix, block) in blocks.iter().enumerate() {
                        if let Block::Text(text) = block {
                            sync_markdown_block(&mut self.markdown, (item_ix, block_ix), text, cx);
                        }
                    }
                }
                // Helpers answer in Markdown, like the assistant.
                Item::Custom { kind, text, .. } if markdown_custom(kind) => {
                    sync_markdown_block(&mut self.markdown, (item_ix, 0), text, cx);
                }
                _ => {}
            }
        }
    }

    /// Record start/end instants of live items `from..`.
    fn track_live(&mut self, from: usize) {
        let now = Instant::now();
        let mut opened = 0;
        let items = self.transcript.items.get(from..).unwrap_or_default();
        for (ix, item) in (from..).zip(items) {
            match item {
                Item::Tool { status, .. } => {
                    opened += track(
                        &mut self.live.tools,
                        ix,
                        *status == ToolStatus::Running,
                        now,
                    )
                }
                Item::Shell { running, .. } => {
                    opened += track(&mut self.live.tools, ix, *running, now)
                }
                Item::Assistant {
                    blocks, streaming, ..
                } => {
                    for (bi, block) in blocks.iter().enumerate() {
                        if matches!(block, Block::Thinking(_)) {
                            let live = *streaming && bi + 1 == blocks.len();
                            opened += track(&mut self.live.thinking, (ix, bi), live, now);
                        }
                    }
                }
                _ => {}
            }
        }
        self.live.open_spans = self.live.open_spans.saturating_add_signed(opened);
    }

    /// Update the turn clock and the prompt in flight; whether live timers
    /// still need ticking.
    fn refresh_live(&mut self) -> bool {
        let now = Instant::now();
        let running = self.transcript.running;
        if running {
            self.live.turn.get_or_insert(now);
        } else {
            self.live.turn = None;
        }

        let ready = self.backend.is_ready();
        let items = &self.transcript.items;
        let dialogs = !self.extension.dialogs.is_empty();
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
        ticker_needed(
            running,
            self.transcript.shells_running(),
            self.live.pending.is_some(),
        )
    }

    /// Keep a ticker running exactly while something is live. Runs on every
    /// state change (not only on render), so hidden tabs stop ticking too.
    fn sync_live(&mut self, cx: &mut Context<Self>) {
        if !self.refresh_live() {
            self.live.ticker = None;
            self.live.ticking = false;
            return;
        }
        if self.live.ticking {
            return;
        }
        self.live.ticking = true;
        self.live.stamp = None;
        self.live.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                match this.update(cx, |this, cx| this.tick(cx)) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        }));
    }

    /// One ticker step: repaint only when a timer label reads differently
    /// (or the prompt in flight resolved); false once nothing is live.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let had_pending = self.live.pending.is_some();
        let live = self.refresh_live();
        self.live.ticks += 1;
        let stamp = label_stamp(
            (self.live.open_spans > 0).then_some(self.live.ticks),
            self.live.turn.map(|t| t.elapsed()),
            self.live.pending.as_ref().map(|p| p.since.elapsed()),
        );
        if self.live.stamp != Some(stamp) || had_pending != self.live.pending.is_some() {
            self.live.stamp = Some(stamp);
            cx.notify();
        }
        if !live {
            self.live.ticking = false;
        }
        live
    }

    /// Decode images of user items `from..` off the UI thread (all shown
    /// images when the transcript was replaced, forgetting the rest).
    fn request_images(&mut self, from: usize, prune: bool, cx: &mut Context<Self>) {
        let start = if prune { 0 } else { from };
        let shown = self
            .transcript
            .items
            .get(start..)
            .unwrap_or_default()
            .iter()
            .filter_map(|item| match item {
                Item::User { images, .. } => Some(images),
                _ => None,
            })
            .flatten();
        let jobs = self.image_cache.request(shown, prune);
        if jobs.is_empty() {
            return;
        }
        let decoding = cx
            .background_executor()
            .spawn(async move { attachments::decode_all(jobs) });
        cx.spawn(async move |this, cx| {
            let decoded = decoding.await;
            let _ = this.update(cx, |this, cx| {
                this.image_cache.finish(decoded);
                cx.notify();
            });
        })
        .detach();
    }

    /// The rows after the transcript: the prompt just sent (until Pi echoes
    /// it) and a working indicator.
    fn tail_rows(&self) -> Vec<Tail> {
        let count = self.transcript.items.len();
        let mut tail = Vec::new();
        if let Some(pending) = &self.live.pending {
            // It takes the same slot (and animation id) as the real message
            // so nothing jumps.
            if count == pending.base && !pending.text.starts_with('/') {
                tail.push(Tail::PendingBubble(pending.base));
            }
            tail.push(Tail::Pending(pending.base));
        } else if self.transcript.running {
            // Between tool calls nothing streams; keep a pulse at the end.
            let idle_tail = match self.transcript.items.last() {
                Some(Item::Assistant { streaming, .. }) => !streaming,
                Some(Item::Tool { status, .. }) => *status != ToolStatus::Running,
                Some(Item::Shell { running, .. }) => !running,
                _ => true,
            };
            if idle_tail {
                tail.push(Tail::Between(count));
            }
        }
        tail
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
        // Fold keys carry their item index.
        self.list.remeasure_item(key >> 16);
        cx.notify();
    }

    // ── Empty and error states ─────────────────────────────────────────────

    fn render_hero(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let version = self.backend.version.clone().unwrap_or_default();
        let needs_sign_in =
            self.backend.is_ready() && self.models.loaded && self.models.available.is_empty();
        // The banner is ~175px per unit of scale; shrink it to fit the pane.
        let scale = ((self.pane_width.get() - 48.) / 175.).clamp(1.5, 4.0);
        v_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap_1()
            .pt(px(96.))
            .child(crate::shared::banner::banner(self.banner, scale, self.banner_reveal))
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
        let (title, body): (&str, String) = match &self.backend.status {
            BackendStatus::Starting => {
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
            BackendStatus::Ready(_) => return None,
            BackendStatus::Failed(error) => ("Could not start gentle-shell", error.clone()),
            BackendStatus::Exited(code) => (
                "gentle-shell stopped",
                format!(
                    "Exit code {}.\n{}",
                    code.map_or("unknown".into(), |c| c.to_string()),
                    self.backend
                        .stderr
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
                if !*streaming && has_text {
                    column = column
                        .group("assistant")
                        .child(self.render_answer_actions(ix, cx));
                }
                column.into_any_element()
            }
            Item::Tool { .. } | Item::Shell { .. } => {
                let computed;
                let spec = match self.list.prepared(ix) {
                    Some(Prepared::Card(spec)) => spec,
                    _ => {
                        computed = card_spec(item);
                        match &computed {
                            Some(spec) => spec,
                            None => return div().into_any_element(),
                        }
                    }
                };
                let backend_ms = match item {
                    Item::Tool { duration_ms, .. } => *duration_ms,
                    _ => None,
                };
                let duration = self.tool_duration(ix, spec.status, backend_ms);
                self.render_card(ix, item, spec, duration, cx)
            }
            Item::Custom {
                kind,
                text,
                details,
            } => {
                let lines = match self.list.prepared(ix) {
                    Some(Prepared::Lines(lines)) => *lines,
                    _ => line_count(text),
                };
                self.render_custom(ix, kind, text, details, lines, cx)
            }
            Item::CommandOutput { command, notices } => {
                let lines = match self.list.prepared(ix) {
                    Some(Prepared::Lines(lines)) => *lines,
                    _ => notices.iter().map(|(_, text)| line_count(text)).sum(),
                };
                self.render_command_output(ix, command, notices, lines, cx)
            }
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

    /// An extension command's `notify` output as one card: the command as
    /// typed, then each notice in order, warnings and errors tinted.
    fn render_command_output(
        &self,
        ix: usize,
        command: &str,
        notices: &[(NoticeLevel, String)],
        lines: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const SHOWN_LINES: usize = 16;
        let key = fold_key(ix, 0, OUTPUT);
        let open = self.expanded.contains(&key);
        let foldable = lines > SHOWN_LINES;
        let worst = notices
            .iter()
            .map(|(level, _)| *level)
            .max_by_key(|level| match level {
                NoticeLevel::Info => 0,
                NoticeLevel::Warning => 1,
                NoticeLevel::Error => 2,
            })
            .unwrap_or(NoticeLevel::Info);
        let accent = match worst {
            NoticeLevel::Info => None,
            NoticeLevel::Warning => Some(ui::warning()),
            NoticeLevel::Error => Some(ui::danger()),
        };
        let rows = notices.iter().map(|(level, text)| {
            let (icon, color) = match level {
                NoticeLevel::Info => (None, ui::fg()),
                NoticeLevel::Warning => (Some(IconName::TriangleAlert), ui::warning()),
                NoticeLevel::Error => (Some(IconName::CircleX), ui::danger()),
            };
            h_flex()
                .w_full()
                .items_start()
                .gap_1p5()
                .when_some(icon, |this, icon| {
                    this.child(
                        div()
                            .pt(px(2.))
                            .child(Icon::new(icon).xsmall().text_color(color)),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(ui::MONO)
                        .text_size(px(12.))
                        .text_color(color)
                        .whitespace_normal()
                        .child(text.trim_end().to_string()),
                )
        });
        ui::card()
            .w_full()
            .overflow_hidden()
            .when_some(accent, |this, color| this.border_color(color.opacity(0.35)))
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .small()
                            .text_color(accent.unwrap_or(ui::muted())),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(ui::MONO)
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::champagne())
                            .child(command.to_string()),
                    ),
            )
            .child(
                div().px_3().pb_2().child(
                    ui::well_box()
                        .p_3()
                        .gap_1p5()
                        .when(foldable && !open, |d| {
                            d.max_h(px(SHOWN_LINES as f32 * 18.)).overflow_hidden()
                        })
                        .children(rows),
                ),
            )
            .when(foldable, |this| {
                this.child(div().pb_1().child(self.fold_toggle(
                    key,
                    if open {
                        "Show less".into()
                    } else {
                        format!("{} more lines", lines - SHOWN_LINES)
                    },
                    open,
                    cx,
                )))
            })
            .into_any_element()
    }

    /// Under a finished answer, shown on hover: copy it.
    fn render_answer_actions(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        h_flex()
            .h(px(20.))
            .mt(px(-4.))
            .invisible()
            .group_hover("assistant", |s| s.visible())
            .child(
                Button::new(("copy-answer", ix))
                    .ghost()
                    .xsmall()
                    .text_color(ui::faint())
                    .icon(Icon::new(IconName::Copy))
                    .tooltip("Copy answer")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(Item::Assistant { blocks, .. }) = this.transcript.items.get(ix)
                        else {
                            return;
                        };
                        let text = blocks
                            .iter()
                            .filter_map(|b| match b {
                                Block::Text(t) if !t.trim().is_empty() => Some(t.trim()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        this.copy_text(text, cx);
                    })),
            )
    }

    fn render_card(
        &self,
        ix: usize,
        item: &Item,
        spec: &CardSpec,
        duration: Option<Duration>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (input, output) = card_texts(item);
        let out_key = fold_key(ix, 0, OUTPUT);
        let open = self.expanded.contains(&out_key);
        let has_output = spec.has_output;
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
                            .child(spec.title.clone()),
                    )
                    .when_some(spec.chip.clone(), |this, chip| {
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
                            .child(spec.subject.clone()),
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
            .when_some(
                input.zip(spec.input_lines),
                |this, ((text, diff), lines)| {
                    this.child(div().px_3().pb_2().child(self.code_well(
                        fold_key(ix, 0, INPUT),
                        text,
                        lines,
                        diff,
                        cx,
                    )))
                },
            )
            .when(show_output, |this| {
                this.child(self.output_block(out_key, output, spec.output_lines, cx))
            })
            .child(status_footer(spec.status, duration))
            .into_any_element()
    }

    /// A command, file body or diff (`lines` long), clipped from the bottom with a fade.
    fn code_well(
        &self,
        key: usize,
        text: &str,
        lines: usize,
        diff: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let max = if diff { DIFF_LINES } else { INPUT_LINES };
        let open = self.expanded.contains(&key);
        let (shown, hidden) = head_slice(text, lines, if open { 400 } else { max });
        let more = lines.saturating_sub(max);
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

    /// Command output (`lines` long), clipped from the top so the latest lines show.
    fn output_block(&self, key: usize, body: &str, lines: usize, cx: &mut Context<Self>) -> Div {
        let open = self.expanded.contains(&key);
        let (shown, hidden) = tail_slice(body, lines, if open { 200 } else { OUTPUT_LINES });
        let more = lines.saturating_sub(OUTPUT_LINES);
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
}

impl Focusable for SessionView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SessionView {
    /// One row of the transcript list; `cx.processor` calls it for the
    /// rows on screen (and in the overdraw) only.
    fn render_row(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.list.row(ix) else {
            return div().into_any_element();
        };
        let hero = self.hero_shown();
        // `render_backend_state` shows something unless the backend is ready.
        let header = hero || !self.backend.is_ready();
        let now = Instant::now();
        let content = match row {
            Row::Header => {
                if !header {
                    return div().into_any_element();
                }
                v_flex()
                    .w_full()
                    .gap_4()
                    .when(hero, |this| this.child(self.render_hero(cx)))
                    .children(self.render_backend_state())
                    .into_any_element()
            }
            Row::Item(i) => {
                let Some(item) = self.transcript.items.get(i) else {
                    return div().into_any_element();
                };
                let element = self.render_item(i, item, cx);
                // Keyed by index: items animate in once, never again.
                self.list
                    .appear(("item", i), now, div().w_full().child(element))
            }
            Row::Tail(Tail::PendingBubble(base)) => {
                let Some(pending) = &self.live.pending else {
                    return div().into_any_element();
                };
                let bubble = user_bubble(
                    &pending.text,
                    pending.images.iter().cloned().map(Some).collect(),
                );
                self.list
                    .appear(("item", base), now, div().w_full().child(bubble))
            }
            Row::Tail(Tail::Pending(base)) => {
                let since = self.live.pending.as_ref().map(|p| p.since.elapsed());
                let indicator = working_indicator(("pending-petal", base), "Thinking…", since);
                self.list
                    .appear(("pending", base), now, div().w_full().child(indicator))
            }
            Row::Tail(Tail::Between(count)) => {
                let since = self.live.turn.map(|t| t.elapsed());
                let indicator = working_indicator(("between-petal", count), "Working…", since);
                self.list
                    .appear(("between", count), now, div().w_full().child(indicator))
            }
            // The column's bottom padding.
            Row::Footer => return div().h(px(24.)).into_any_element(),
        };
        // The column's top padding before the first row, its gap after.
        let first = matches!(row, Row::Header) || (ix == 1 && !header);
        let narrow = self.pane_width.get() < NARROW;
        h_flex()
            .w_full()
            .justify_center()
            .child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .max_w(px(760.))
                    .map(|d| if narrow { d.px_3() } else { d.px_6() })
                    .pt(px(if first { 24. } else { 16. }))
                    .child(content),
            )
            .into_any_element()
    }
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A tab starts its backend once shown.
        self.ensure_started(window, cx);
        self.sync_view(cx);
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
                div().id("transcript").flex_1().min_h_0().child(
                    list(
                        self.list.state(),
                        cx.processor(|this, ix, window, cx| this.render_row(ix, window, cx)),
                    )
                    .size_full(),
                ),
            )
            .child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .map(|d| if narrow { d.px_3() } else { d.px_6() })
                    .pb_2()
                    .child(self.render_composer(cx)),
            )
            .when_some(self.render_todo_island(cx), |this, island| {
                this.child(island)
            });

        h_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::abort))
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(Self::toggle_agents))
            .on_action(cx.listener(Self::on_new_session))
            .capture_key_down(cx.listener(Self::on_dialog_key))
            .relative()
            .size_full()
            .child(main)
            .children(self.render_agents_panel(cx))
    }
}

// ── Building blocks ────────────────────────────────────────────────────────

/// Everything a tool or shell card shows except its text bodies, derived
/// once per change of its item; the bodies are borrowed from the item when
/// drawn ([`card_texts`]).
struct CardSpec {
    icon: IconName,
    title: SharedString,
    subject: SharedString,
    chip: Option<SharedString>,
    added: Option<usize>,
    removed: Option<usize>,
    /// Lines of the code well (command, file body or diff), when there is one.
    input_lines: Option<usize>,
    output_lines: usize,
    has_output: bool,
    /// Output is the point of the tool (commands): show it without a click.
    always_output: bool,
    status: ToolStatus,
}

/// The card data of a tool or shell item.
fn card_spec(item: &Item) -> Option<CardSpec> {
    let (input, output) = card_texts(item);
    let (input_lines, output_lines) = (input.map(|(text, _)| line_count(text)), line_count(output));
    let has_output = !output.trim().is_empty();
    match item {
        Item::Tool {
            name,
            args,
            status,
            details,
            ..
        } => {
            let summary = tools::summarize(name, args, details);
            let arg = |key: &str| args[key].as_str().filter(|s| !s.trim().is_empty());
            let command_like = matches!(
                name.as_str(),
                "bash" | "bash_background" | "monitor" | "codemode"
            );
            let chip = match name.as_str() {
                "bash" | "bash_background" | "monitor" => {
                    Some(command_language(arg("command").unwrap_or_default()).into())
                }
                "codemode" => Some(arg("language").unwrap_or("code").to_string().into()),
                "read" | "edit" | "write" => {
                    language_for_path(arg("path").unwrap_or_default()).map(SharedString::from)
                }
                n if n.starts_with("subagent") => arg("agent").map(|a| a.to_string().into()),
                _ => None,
            };
            let title = match arg("description") {
                Some(description) if command_like => gentle_core::text::first_line(description, 80),
                _ => summary.title,
            };
            let subject = if command_like && input.is_some() {
                String::new()
            } else {
                summary.subject
            };
            Some(CardSpec {
                icon: tool_icon(name),
                title: title.into(),
                subject: subject.into(),
                chip,
                added: summary.added,
                removed: summary.removed,
                input_lines,
                output_lines,
                has_output,
                always_output: command_like,
                status: *status,
            })
        }
        Item::Shell {
            command,
            exit_code,
            running,
            ..
        } => Some(CardSpec {
            icon: IconName::CodeXml,
            title: "Shell".into(),
            subject: SharedString::default(),
            chip: Some(command_language(command).into()),
            added: None,
            removed: None,
            input_lines,
            output_lines,
            has_output,
            always_output: true,
            status: match (running, exit_code) {
                (true, _) => ToolStatus::Running,
                (false, Some(0)) => ToolStatus::Done,
                _ => ToolStatus::Failed,
            },
        }),
        _ => None,
    }
}

/// A card's text bodies, borrowed from its item: the code well (and
/// whether it is a diff) and the output.
fn card_texts(item: &Item) -> (Option<(&str, bool)>, &str) {
    match item {
        Item::Tool {
            name,
            args,
            status,
            output,
            details,
            ..
        } => {
            let arg = |key: &str| args[key].as_str().filter(|s| !s.trim().is_empty());
            let input = match name.as_str() {
                "bash" | "bash_background" | "monitor" => arg("command").map(|c| (c, false)),
                "codemode" => arg("code").or(arg("command")).map(|c| (c, false)),
                "write" => arg("content").map(|c| (c, false)),
                "edit" => details["diff"].as_str().map(|d| (d, true)),
                _ => None,
            };
            // A file tool's result is a one-line receipt; only failures say more.
            let output = if matches!(name.as_str(), "edit" | "write")
                && input.is_some()
                && *status != ToolStatus::Failed
            {
                ""
            } else {
                output.as_str()
            };
            (input, output)
        }
        Item::Shell {
            command, output, ..
        } => (Some((command.as_str(), false)), output.as_str()),
        _ => (None, ""),
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

/// How many lines `str::lines` reads in `text` with its trailing whitespace trimmed.
fn line_count(text: &str) -> usize {
    let text = text.trim_end();
    if text.is_empty() {
        0
    } else {
        text.bytes().filter(|&b| b == b'\n').count() + 1
    }
}

/// The first `max` of the `lines` lines of `text` ([`line_count`]) as one
/// slice, plus how many were hidden. Read it back with `str::lines`.
fn head_slice(text: &str, lines: usize, max: usize) -> (&str, usize) {
    let text = text.trim_end();
    if lines <= max {
        return (text, 0);
    }
    if max == 0 {
        return ("", lines);
    }
    let end = text
        .match_indices('\n')
        .nth(max - 1)
        .map_or(text.len(), |(at, _)| at);
    let head = &text[..end];
    (head.strip_suffix('\r').unwrap_or(head), lines - max)
}

/// The last `max` of the `lines` lines of `text` ([`line_count`]) as one
/// slice, plus how many were hidden. Read it back with `str::lines`.
fn tail_slice(text: &str, lines: usize, max: usize) -> (&str, usize) {
    let text = text.trim_end();
    if lines <= max {
        return (text, 0);
    }
    let hidden = lines - max;
    if max == 0 {
        return ("", hidden);
    }
    let start = text
        .match_indices('\n')
        .nth(hidden - 1)
        .map_or(text.len(), |(at, _)| at + 1);
    (&text[start..], hidden)
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
    // min_w_0 on both levels: without it the bubble's minimum width is its
    // unwrapped text, so in a narrow pane it overflows (clipped on the left).
    h_flex().w_full().min_w_0().justify_end().child(
        v_flex()
            .min_w_0()
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

#[cfg(test)]
mod tests {
    use super::{
        command_language, head_slice, language_for_path, line_count, parse_diff_line, tail_slice,
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
    fn head_and_tail_slices_match_line_splitting() {
        // What the old helpers' joined strings read back as (`str::lines`).
        fn head(text: &str, max: usize) -> (Vec<&str>, usize) {
            let lines: Vec<&str> = text.trim_end().lines().collect();
            let hidden = lines.len().saturating_sub(max);
            let read = lines[..lines.len() - hidden].join("\n").lines().count();
            (lines[..read].to_vec(), hidden)
        }
        fn tail(text: &str, max: usize) -> (Vec<&str>, usize) {
            let lines: Vec<&str> = text.trim_end().lines().collect();
            let hidden = lines.len().saturating_sub(max);
            let joined = lines[hidden..].join("\n");
            let read = joined.lines().count();
            (lines[hidden..hidden + read].to_vec(), hidden)
        }
        let texts = [
            "",
            "  \n",
            "a",
            "a\nb\nc\n",
            "a\n\nb\n\n\n",
            "one\r\ntwo\r\nthree\r\n",
            "x\ny\nz\nw\nv",
        ];
        for text in texts {
            let lines = line_count(text);
            assert_eq!(lines, text.trim_end().lines().count(), "{text:?}");
            for max in 0..7 {
                let (shown, hidden) = head_slice(text, lines, max);
                let shown: Vec<&str> = shown.lines().collect();
                assert_eq!((shown, hidden), head(text, max), "{text:?} {max}");
                let (shown, hidden) = tail_slice(text, lines, max);
                let shown: Vec<&str> = shown.lines().collect();
                assert_eq!((shown, hidden), tail(text, max), "{text:?} {max}");
            }
        }
        assert_eq!(head_slice("a\nb\nc\n", 3, 2), ("a\nb", 1));
        assert_eq!(tail_slice("a\nb\nc\n", 3, 2), ("b\nc", 1));
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
