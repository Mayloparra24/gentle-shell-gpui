//! The footer: the status line and where the profile applies (this repo,
//! the repository declaration, the session binding, Save, Make default and
//! Use in this session).

use gentle_core::profiles::PinRead;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::profiles::{Leave, ProfilesView, Tone};
use crate::ui;

impl ProfilesView {
    pub(super) fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has = self.selected.is_some();
        let disabled = self.busy || !has;
        let unsaved = self.change_count();
        let name = self.selected.clone().unwrap_or_default();
        let pinned_here =
            matches!(self.pin.as_ref().map(|p| &p.local), Some(PinRead::Valid(p)) if *p == name);
        let declared_here =
            matches!(self.pin.as_ref().map(|p| &p.repo), Some(PinRead::Valid(p)) if *p == name);
        v_flex()
            .border_t_1()
            .border_color(ui::border())
            .when_some(self.status.as_ref(), |d, (message, tone)| {
                let (icon, color) = match tone {
                    Tone::Info => (IconName::Info, ui::muted()),
                    Tone::Good => (IconName::CircleCheck, ui::success()),
                    Tone::Bad => (IconName::CircleAlert, ui::danger()),
                };
                d.child(
                    h_flex()
                        .px_4()
                        .pt_2()
                        .gap_2()
                        .items_start()
                        .text_xs()
                        .text_color(color)
                        .child(Icon::new(icon).xsmall().flex_shrink_0().mt(px(1.)))
                        .child(div().flex_1().min_w_0().child(message.clone())),
                )
            })
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .flex_wrap()
                    // Where the profile applies beyond this screen.
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("repo-pin")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::GitBranch))
                                    .label(if pinned_here { "Stop using in this repo" } else { "Use in this repo" })
                                    .tooltip("Clone-local pin in the Git common dir; subagent launches here resolve it")
                                    .disabled(disabled || self.pin.is_none())
                                    .on_click(cx.listener(|this, _, _, cx| this.toggle_repo_pin(cx))),
                            )
                            .child(
                                Button::new("publish")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Upload))
                                    .label(if declared_here { "Unpublish from repo" } else { "Publish to repo" })
                                    .tooltip("Toggle the committed .pi/gentle-ai/profile.json declaration")
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.publish(cx))),
                            )
                            .child(
                                Button::new("clear-session")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Unlink))
                                    .label("Clear session binding")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_session(cx))),
                            ),
                    )
                    .child(
                        h_flex()
                            .ml_auto()
                            .gap_2()
                            .flex_wrap()
                            .when(unsaved > 0, |d| {
                                d.child(ui::chip(format!("{unsaved} unsaved"), ui::warning()))
                            })
                            .child(
                                Button::new("save")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Check))
                                    .label("Save")
                                    .disabled(disabled || unsaved == 0)
                                    .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                            )
                            .child(
                                Button::new("make-default")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::Star))
                                    .label("Make default")
                                    .tooltip("Default for new sessions: writes models.json, agent routing and settings.json")
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.request_make_default(cx))),
                            )
                            .child(
                                Button::new("footer-close")
                                    .small()
                                    .ghost()
                                    .icon(Icon::new(IconName::X))
                                    .label("Close")
                                    .on_click(cx.listener(|this, _, window, cx| this.leave(Leave::Close, window, cx))),
                            )
                            .child(
                                Button::new("use-session")
                                    .small()
                                    .primary()
                                    .icon(Icon::new(IconName::CircleCheck))
                                    .label("Use in this session")
                                    .tooltip("Switch this session's orchestrator to the profile (saves edits first)")
                                    .disabled(disabled)
                                    .on_click(cx.listener(|this, _, _, cx| this.use_in_session(cx))),
                            ),
                    ),
            )
    }
}
