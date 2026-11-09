//! The composer: context line, attachments, text area and its bottom bar.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::activity::chip_popover;
use super::dialogs::checkbox_box;
use super::{NARROW, TINY, image_tile, number_badge};
use crate::features::session::SessionView;
use crate::shared::ui;
use gentle_core::transcript::format_tokens;

impl SessionView {
    /// `NN%` (a chip: compact now, auto-compaction) — thin pink bar — window size.
    fn render_context_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let window_tokens = self
            .models
            .current
            .as_ref()
            .map(|m| m.context_window)
            .unwrap_or(0);
        let percent = self.usage.context_percent.unwrap_or(0.0).clamp(0.0, 100.0) as f32;
        let high = self
            .usage
            .context_percent
            .is_some_and(|p| p >= crate::features::alerts::context_threshold(cx));
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
                    .child(self.render_context_chip(high, cx))
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

    /// The context percentage as a chip: compact now, toggle auto-compaction.
    fn render_context_chip(&self, high: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let compacting = self.usage.compacting;
        let auto = self.usage.auto_compaction;
        let ready = self.backend.is_ready();
        let used = match self.usage.context_percent {
            Some(p) => format!("{p:.0}%"),
            None => "–".into(),
        };
        let window = self
            .models
            .current
            .as_ref()
            .map(|m| m.context_window)
            .filter(|w| *w > 0)
            .map(format_tokens);
        let label = if compacting {
            "compacting…".to_string()
        } else {
            used.clone()
        };
        let count = match &window {
            Some(window) => format!("{used} of {window}"),
            None => used,
        };
        let color = if high { ui::warning() } else { ui::faint() };
        let weak = cx.entity().downgrade();
        chip_popover(
            "context",
            IconName::Gauge,
            color,
            label,
            "Context",
            count,
            move || {
                let compact = weak.clone();
                let toggle = weak.clone();
                vec![
                    div()
                        .text_size(px(12.))
                        .text_color(ui::muted())
                        .whitespace_normal()
                        .child(if high {
                            "Context is nearly full. Compacting summarizes the conversation so far to free room."
                        } else {
                            "Compacting summarizes the conversation so far to free room. Add what to keep with /compact <instructions>."
                        })
                        .into_any_element(),
                    h_flex()
                        .pt_1()
                        .child(
                            Button::new("context-compact")
                                .small()
                                .primary()
                                .icon(Icon::new(IconName::ChevronsDownUp))
                                .label(if compacting { "Compacting…" } else { "Compact now" })
                                .loading(compacting)
                                .disabled(compacting || !ready)
                                .on_click(move |_, _, cx| {
                                    let _ = compact.update(cx, |this, cx| this.compact(None, cx));
                                }),
                        )
                        .into_any_element(),
                    h_flex()
                        .id("context-auto-compaction")
                        .gap_2()
                        .pt_1()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(ui::fg())
                        .when(auto.is_none() || !ready, |d| d.opacity(0.5))
                        .child(checkbox_box(auto.unwrap_or(false)))
                        .child("Compact automatically when nearly full")
                        .when_some(auto.filter(|_| ready), |d, enabled| {
                            d.on_click(move |_, _, cx| {
                                let _ = toggle.update(cx, |this, cx| {
                                    this.set_auto_compaction(!enabled, cx)
                                });
                            })
                        })
                        .into_any_element(),
                ]
            },
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

    pub(super) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (prefix, leaf) = ui::split_path(&self.cwd);
        let empty = self.composer.read(cx).value().trim().is_empty() && self.attachments.is_empty();
        let busy = self.busy();
        let ready = self.backend.is_ready();

        let action_button = if busy && empty {
            Button::new("stop")
                .small()
                .danger()
                .rounded(px(999.))
                .child(div().size(px(9.)).rounded_sm().bg(ui::well()))
                .tooltip(if cfg!(target_os = "macos") {
                    "Stop (⌘.)"
                } else {
                    "Stop (Ctrl+.)"
                })
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

        let git = self.git.snapshot.as_ref().map(|git| {
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
            .when(
                self.models.current.as_ref().is_some_and(|m| m.reasoning),
                |this| this.child(self.render_effort_picker(narrow, cx)),
            )
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
            .when(self.usage.cost > 0.0 && !narrow, |this| {
                this.child(
                    div()
                        .font_family(ui::MONO)
                        .text_size(px(11.))
                        .text_color(ui::faint())
                        .child(format!("${:.2}", self.usage.cost)),
                )
            })
            .child(div().flex_shrink_0().child(action_button));

        let composer_box = v_flex()
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
                            weak.update(cx, |this, cx| this.paste_clipboard(item, window, cx))
                                .unwrap_or(false)
                        }),
                )
            })
            .child(bottom);

        v_flex()
            .w_full()
            .min_w_0()
            .max_w(px(720.))
            .gap_1p5()
            .children(self.render_slash_menu(cx))
            .children(self.render_dialog(cx))
            .child(self.render_activity(cx))
            .child(self.render_context_line(cx))
            .child(composer_box)
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
                            if cfg!(target_os = "macos") {
                                "↵ steers · ⌘. stops"
                            } else {
                                "↵ steers · Ctrl+. stops"
                            }
                        } else if narrow {
                            "↵ sends · ⇧↵ newline · / commands"
                        } else {
                            "↵ sends · ⇧↵ newline · !cmd runs a shell command · / for commands · ⌘N new session"
                        })),
                )
            })
    }
}
