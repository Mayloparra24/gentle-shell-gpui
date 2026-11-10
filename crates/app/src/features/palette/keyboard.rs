//! Keyboard: selection, running a row, cycling categories and the inline
//! workspace rename.

use gpui_kit::component::input::InputState;
use gpui_kit::*;

use super::{CATEGORIES, Line, PaletteAction, PaletteEvent, PaletteView};

impl PaletteView {
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        let line = self
            .lines
            .iter()
            .position(|l| matches!(l, Line::Row(i) if *i == self.selected))
            .unwrap_or(0);
        // Keep the group header in view above the first row.
        self.scroll
            .scroll_to_item(if self.selected == 0 { 0 } else { line });
        cx.notify();
    }

    fn run_selected(&mut self, cx: &mut Context<Self>) {
        let last = self.rows.len().saturating_sub(1);
        if let Some(row) = self.rows.get(self.selected.min(last)) {
            cx.emit(PaletteEvent::Run(row.action.clone()));
        }
    }

    fn cycle_category(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let len = CATEGORIES.len();
        let index = CATEGORIES
            .iter()
            .position(|(c, _)| *c == self.category)
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        };
        self.set_category(CATEGORIES[next].0, window, cx);
    }

    pub(super) fn start_rename(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .workspaces
            .iter()
            .find(|w| w.index == index)
            .map(|w| w.name.clone())
        else {
            return;
        };
        let state = cx.new(|cx| InputState::new(window, cx).default_value(name));
        state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.renaming = Some((index, state));
        cx.notify();
    }

    fn finish_rename(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, state)) = self.renaming.take() else {
            return;
        };
        let name = state.read(cx).value().trim().to_string();
        if commit && !name.is_empty() {
            if let Some(row) = self.workspaces.iter_mut().find(|w| w.index == index) {
                row.name = name.clone();
            }
            self.refresh_rows(cx);
            cx.emit(PaletteEvent::Run(PaletteAction::RenameWorkspace {
                index,
                name,
            }));
        }
        self.query.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// What Enter does: commit the rename in progress, else run the selected row.
    pub(super) fn press_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            self.finish_rename(true, window, cx);
        } else {
            self.run_selected(cx);
        }
    }

    /// What Escape does: cancel the rename in progress, else close.
    pub(super) fn press_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            self.finish_rename(false, window, cx);
        } else {
            cx.emit(PaletteEvent::Close);
        }
    }

    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let renaming = self.renaming.is_some();
        match keystroke.key.as_str() {
            "escape" => self.press_escape(window, cx),
            "enter" => self.press_enter(window, cx),
            "up" if !renaming => self.move_selection(-1, cx),
            "down" if !renaming => self.move_selection(1, cx),
            "p" if modifiers.control && !renaming => self.move_selection(-1, cx),
            "n" if modifiers.control && !renaming => self.move_selection(1, cx),
            "tab" if !renaming => self.cycle_category(!modifiers.shift, window, cx),
            // With a query typed, ←/→ keep moving the caret; Tab still switches.
            "left" | "right"
                if !renaming && !modifiers.modified() && self.query.read(cx).value().is_empty() =>
            {
                self.cycle_category(keystroke.key == "right", window, cx)
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}
