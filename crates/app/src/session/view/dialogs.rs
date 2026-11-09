//! Extension dialogs (`select`, `confirm`, `input`, `editor`): the card over
//! the composer and its keyboard handling.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::{Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_rpc::{UiAnswer, UiMethod};

use crate::session::SessionView;
use crate::ui;

impl SessionView {
    fn dialog_cursor(&self) -> usize {
        let Some(dialog) = self.extension.dialogs.front() else {
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
    pub(super) fn on_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        let m = &event.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        let key = event.keystroke.key.as_str();
        let cursor = self.dialog_cursor();
        let Some(dialog) = self.extension.dialogs.front() else {
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

    pub(super) fn render_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.extension.dialogs.front()?;
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

        let waiting = self.extension.dialogs.len() - 1;
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

pub(super) fn checkbox_box(checked: bool) -> Div {
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

#[cfg(test)]
mod tests {
    use super::{checkbox_option, option_matches, split_current};

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
}
