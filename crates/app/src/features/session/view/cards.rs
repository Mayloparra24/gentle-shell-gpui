//! Cards for extension custom messages, styled from their type and details.

use gpui_kit::assets::IconName;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{INPUT_LINES, OUTPUT, fold_key, head_slice, tail_slice};
use crate::features::session::SessionView;
use crate::shared::ui;

impl SessionView {
    pub(super) fn render_custom(
        &self,
        ix: usize,
        kind: &str,
        text: &str,
        details: &serde_json::Value,
        lines: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let look = custom_look(kind, details);
        let key = fold_key(ix, 0, OUTPUT);
        let open = self.expanded.contains(&key);
        let more = lines.saturating_sub(INPUT_LINES);
        let body = match look.body {
            CustomBody::Markdown => self.markdown.get(&(ix, 0)).map(|(state, _)| {
                div()
                    .px_3()
                    .pb_2()
                    .text_size(px(13.))
                    .text_color(ui::fg())
                    .when(!open && more > 0, |d| {
                        d.max_h(px(INPUT_LINES as f32 * 21.)).overflow_hidden()
                    })
                    .child(TextView::new(state).selectable(true))
                    .into_any_element()
            }),
            CustomBody::Head | CustomBody::Tail => {
                let max = if open { 400 } else { INPUT_LINES };
                let (shown, hidden) = match look.body {
                    CustomBody::Tail => tail_slice(text, lines, max),
                    _ => head_slice(text, lines, max),
                };
                // One text run: line endings normalized the way `str::lines` reads them.
                let shown = if shown.contains('\r') {
                    shown.lines().collect::<Vec<_>>().join("\n")
                } else {
                    shown.to_string()
                };
                (!shown.is_empty()).then(|| {
                    v_flex()
                        .px_3()
                        .pb_2()
                        .gap_1()
                        .when(open && hidden > 0, |this| {
                            this.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ui::faint())
                                    .child(format!("… {hidden} more lines not shown")),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(ui::muted())
                                .whitespace_normal()
                                .when(look.body == CustomBody::Tail, |d| {
                                    d.font_family(ui::MONO).text_size(px(12.))
                                })
                                .child(shown),
                        )
                        .into_any_element()
                })
            }
        };
        let color = look.tone.color();
        ui::card()
            .w_full()
            .overflow_hidden()
            .when(matches!(look.tone, Tone::Danger | Tone::Warning), |this| {
                this.border_color(color.opacity(0.35))
            })
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .min_w_0()
                    .child(Icon::new(look.icon).small().text_color(color))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::champagne())
                            .child(look.title),
                    )
                    .child(div().flex_1())
                    .when_some(look.badge, |this, (badge, tone)| {
                        this.child(ui::chip(badge, tone.color()).flex_shrink_0())
                    }),
            )
            .children(body)
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
}

/// A card's tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Success,
    Danger,
    Warning,
    Info,
    Muted,
}

impl Tone {
    fn color(self) -> Hsla {
        match self {
            Tone::Success => ui::success(),
            Tone::Danger => ui::danger(),
            Tone::Warning => ui::warning(),
            Tone::Info => ui::info(),
            Tone::Muted => ui::muted(),
        }
    }
}

/// How a custom message's text shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CustomBody {
    /// Its first lines, as plain text.
    Head,
    /// Rendered Markdown (a helper's answer).
    Markdown,
    /// Its last lines in monospace (a job's output).
    Tail,
}

/// What a custom message's card shows besides its text, from its type and
/// the extension's `details`.
#[derive(Debug, PartialEq)]
struct CustomLook {
    icon: IconName,
    title: String,
    tone: Tone,
    badge: Option<(String, Tone)>,
    body: CustomBody,
}

/// Custom messages whose text is Markdown.
pub(in crate::features::session) fn markdown_custom(kind: &str) -> bool {
    kind == "gentle-agents.result"
}

fn custom_look(kind: &str, details: &serde_json::Value) -> CustomLook {
    let look = |icon, title: &str, tone| CustomLook {
        icon,
        title: title.to_string(),
        tone,
        badge: None,
        body: CustomBody::Head,
    };
    match kind {
        // gentle-agents.ts `taskDetails`: `{gentleAgents: {agent, status, …}}`.
        "gentle-agents.result" => {
            let agent = &details["gentleAgents"];
            let status = agent["status"].as_str().filter(|s| !s.is_empty());
            let tone = match status {
                None | Some("completed") => Tone::Success,
                Some(_) => Tone::Danger,
            };
            CustomLook {
                icon: IconName::Bot,
                title: agent["agent"]
                    .as_str()
                    .filter(|a| !a.is_empty())
                    .map_or("Agent answer".into(), str::to_string),
                tone,
                badge: status.map(|s| (s.replace('_', " "), tone)),
                body: CustomBody::Markdown,
            }
        }
        "gentle-agents.message" | "gentle-agents.orchestrator-message" => {
            let agent = details["gentleAgents"]["agent"]
                .as_str()
                .filter(|a| !a.is_empty());
            let title = agent.map_or("Message".into(), |a| format!("{a} message"));
            look(IconName::MessageSquare, &title, Tone::Info)
        }
        // background-jobs-tools.ts: `{gentleJobs: {label, status, exitCode, kind, event}}`.
        "gentle-jobs.notice" => {
            let job = &details["gentleJobs"];
            let fallback = if job["kind"] == "monitor" {
                "Monitor"
            } else {
                "Background job"
            };
            let title = job["label"]
                .as_str()
                .or(job["command"].as_str())
                .filter(|l| !l.is_empty())
                .unwrap_or(fallback);
            let exit = job["exitCode"].as_i64();
            let (tone, badge) = if job["event"] == "events" {
                (Tone::Info, None)
            } else {
                match (job["status"].as_str(), exit) {
                    (Some("exited"), Some(0)) => (Tone::Success, Some("exit 0".to_string())),
                    (_, Some(code)) => (Tone::Danger, Some(format!("exit {code}"))),
                    (Some(status), None) => (Tone::Danger, Some(status.replace('_', " "))),
                    (None, None) => (Tone::Muted, None),
                }
            };
            CustomLook {
                icon: IconName::Activity,
                title: title.to_string(),
                tone,
                badge: badge.map(|b| (b, tone)),
                body: CustomBody::Tail,
            }
        }
        "gentle-pi.review-preflight" => look(IconName::ShieldCheck, "Review preflight", Tone::Info),
        "gentle-pi.exploration-budget" => look(IconName::Gauge, "Exploration budget", Tone::Info),
        "gentle-pi.verifier-settle-reminder" => look(
            IconName::TriangleAlert,
            "Verification reminder",
            Tone::Warning,
        ),
        other => look(IconName::Info, other, Tone::Muted),
    }
}

#[cfg(test)]
mod tests {
    use super::{CustomBody, Tone, custom_look};
    use serde_json::json;

    #[test]
    fn custom_cards_read_extension_details() {
        let answer = custom_look(
            "gentle-agents.result",
            &json!({"gentleAgents": {"agent": "explorer", "status": "completed"}}),
        );
        assert_eq!(answer.title, "explorer");
        assert_eq!(answer.tone, Tone::Success);
        assert_eq!(answer.badge, Some(("completed".into(), Tone::Success)));
        assert_eq!(answer.body, CustomBody::Markdown);
        let failed = custom_look(
            "gentle-agents.result",
            &json!({"gentleAgents": {"agent": "reviewer", "status": "timed_out"}}),
        );
        assert_eq!(failed.tone, Tone::Danger);
        assert_eq!(failed.badge, Some(("timed out".into(), Tone::Danger)));
        assert_eq!(
            custom_look("gentle-agents.result", &json!(null)).title,
            "Agent answer"
        );

        let job = custom_look(
            "gentle-jobs.notice",
            &json!({"gentleJobs": {"label": "npm test", "status": "exited", "exitCode": 1}}),
        );
        assert_eq!(job.title, "npm test");
        assert_eq!(job.badge, Some(("exit 1".into(), Tone::Danger)));
        assert_eq!(job.body, CustomBody::Tail);
        let ok = custom_look(
            "gentle-jobs.notice",
            &json!({"gentleJobs": {"kind": "monitor", "status": "exited", "exitCode": 0}}),
        );
        assert_eq!(ok.title, "Monitor");
        assert_eq!(ok.tone, Tone::Success);
        let events = custom_look(
            "gentle-jobs.notice",
            &json!({"gentleJobs": {"label": "watch", "event": "events", "status": "running"}}),
        );
        assert_eq!((events.tone, events.badge), (Tone::Info, None));

        assert_eq!(
            custom_look("gentle-pi.review-preflight", &json!({})).title,
            "Review preflight"
        );
        assert_eq!(
            custom_look("gentle-pi.exploration-budget", &json!({})).title,
            "Exploration budget"
        );
        let reminder = custom_look("gentle-pi.verifier-settle-reminder", &json!({}));
        assert_eq!(
            (reminder.title.as_str(), reminder.tone),
            ("Verification reminder", Tone::Warning)
        );
        assert_eq!(custom_look("some.type", &json!({})).title, "some.type");
    }
}
