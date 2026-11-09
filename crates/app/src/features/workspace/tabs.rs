//! Tab lookup and inline tab rename.

use super::*;

impl Workspace {
    pub(super) fn alloc_id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    pub(super) fn tab(&self) -> Option<&Tab> {
        let space = self.space();
        space.tabs.get(space.active)
    }

    /// Open the inline name field on tab `index` when its focused pane is an
    /// agent; terminal and file tabs keep their titles.
    pub(super) fn start_tab_rename(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.space().tabs.get(index).map(|t| t.focused) else {
            return;
        };
        if self.panes.get(&pane).is_none_or(|p| p.session.is_none()) {
            return;
        }
        self.finish_tab_rename(false, window, cx);
        let title = self.pane_title(pane).to_string();
        let len = title.len();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                // Enter commits, and so does clicking away (like the file tree).
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.finish_tab_rename(true, window, cx)
                }
                _ => {}
            },
        );
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(0..len, cx);
        });
        self.renaming = Some(TabRename {
            pane,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn cancel_tab_rename(
        &mut self,
        _: &Escape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_tab_rename(false, window, cx);
    }

    /// Close the tab's name field; with `commit`, a changed non-empty name
    /// becomes the session's name (RPC `set_session_name`).
    pub(super) fn finish_tab_rename(
        &mut self,
        commit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(rename) = self.renaming.take() else {
            return;
        };
        let focused = rename.input.focus_handle(cx).is_focused(window);
        let name = rename.input.read(cx).value().trim().to_string();
        cx.notify();
        let Some(pane) = self.panes.get(&rename.pane) else {
            return;
        };
        // Enter and Escape hand the keyboard back to the agent; a click
        // elsewhere keeps its new focus.
        if focused {
            pane.item.focus.focus(window, cx);
        }
        if commit
            && !name.is_empty()
            && name != pane.title
            && let Some(session) = pane.session.clone()
        {
            session.update(cx, |s, cx| s.rename(name, cx));
        }
    }

    pub(super) fn tab_mut(&mut self) -> Option<&mut Tab> {
        let space = self.space_mut();
        space.tabs.get_mut(space.active)
    }

    pub(super) fn focused_pane(&self) -> Option<PaneId> {
        self.tab().map(|t| t.focused)
    }
}
