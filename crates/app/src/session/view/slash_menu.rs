//! The `/` command menu over the composer.

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::session::SessionView;
use crate::ui;

impl SessionView {
    /// The commands the `/` menu offers for the composer's text: built-ins
    /// first, then Gentle Shell's (hidden ones left out). Empty when the text
    /// isn't a bare `/name` being typed.
    fn slash_matches(&self, text: &str) -> Vec<(String, String)> {
        let Some(query) = text.strip_prefix('/') else {
            return Vec::new();
        };
        if query.contains(char::is_whitespace) {
            return Vec::new();
        }
        let builtins = crate::session::builtins::BUILTINS.iter().copied();
        let backend = self
            .commands
            .iter()
            .filter(|(name, _)| !gentle_core::commands::hidden(name))
            .map(|(name, description)| (name.as_str(), description.as_str()));
        builtins
            .chain(backend)
            .filter(|(name, _)| name.contains(query))
            .map(|(name, description)| (name.to_string(), description.to_string()))
            .collect()
    }

    /// The highlighted row of the `/` menu showing `count` rows for `text`.
    fn slash_cursor(&self, text: &str, count: usize) -> usize {
        if self.live.slash_query == text {
            self.live.slash_cursor.min(count.saturating_sub(1))
        } else {
            0
        }
    }

    /// ↑/↓ move through the `/` menu, Tab or Enter fill in the highlighted
    /// command (Enter still sends a command typed in full), Esc closes the
    /// menu. Runs as a keystroke interceptor so it wins over the composer.
    pub(in crate::session) fn slash_menu_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let m = &keystroke.modifiers;
        if m.platform || m.control || m.alt || m.shift {
            return false;
        }
        if !self.composer.focus_handle(cx).is_focused(window) {
            return false;
        }
        let text = self.composer.read(cx).value().to_string();
        if self.live.slash_dismissed.as_deref() == Some(text.as_str()) {
            return false;
        }
        let matches = self.slash_matches(&text);
        let n = matches.len();
        if n == 0 {
            return false;
        }
        let cursor = self.slash_cursor(&text, n);
        match keystroke.key.as_str() {
            "up" | "down" => {
                let next = if keystroke.key == "up" {
                    (cursor + n - 1) % n
                } else {
                    (cursor + 1) % n
                };
                self.live.slash_query = text;
                self.live.slash_cursor = next;
                self.live.slash_scroll.scroll_to_item(next);
            }
            "tab" => self.insert_command(&matches[cursor].0, window, cx),
            "enter" => {
                let picked = &matches[cursor].0;
                if text.trim() == format!("/{picked}") {
                    return false;
                }
                self.insert_command(picked, window, cx);
            }
            "escape" => self.live.slash_dismissed = Some(text),
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_slash_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.composer.read(cx).value().to_string();
        if self.live.slash_dismissed.as_deref() == Some(text.as_str()) {
            return None;
        }
        let matches = self.slash_matches(&text);
        if matches.is_empty() {
            return None;
        }
        let cursor = self.slash_cursor(&text, matches.len());
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
                    .child(
                        v_flex()
                            .id("slash-list")
                            .max_h(px(SLASH_MENU_H))
                            .overflow_y_scroll()
                            .track_scroll(&self.live.slash_scroll)
                            .children(matches.into_iter().enumerate().map(
                                |(i, (name, description))| {
                                    h_flex()
                                        .id(("command", i))
                                        .w_full()
                                        .px_2()
                                        .py_1p5()
                                        .gap_3()
                                        .rounded_lg()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(ui::surface_hover()))
                                        .when(i == cursor, |s| s.bg(ui::surface_hover()))
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
                                                .child(description),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.insert_command(&name, window, cx)
                                        }))
                                },
                            )),
                    ),
            )
            .into_any_element(),
        )
    }
}

/// The `/` menu shows about eight commands; the rest scroll.
const SLASH_MENU_H: f32 = 256.;
