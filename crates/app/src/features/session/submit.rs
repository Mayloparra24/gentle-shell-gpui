//! What the composer sends: `!` shell commands, built-ins and natively shown
//! commands run here, prompts with their image attachments.

use std::path::PathBuf;

use anyhow::Result;
use gpui_kit::component::input::Position;
use gpui_kit::*;
use serde_json::json;

use super::{SessionEvent, SessionView, attachments, builtins};

impl SessionView {
    pub(super) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().trim().to_string();
        let shell = text.starts_with('!');
        let images = !shell && !self.attachments.is_empty();
        if (text.is_empty() && !images) || self.process().is_none() {
            return;
        }
        if shell && !self.attachments.is_empty() {
            // Clearing the composer would strand the images with no tokens left.
            let message =
                "Shell commands cannot carry images. Remove the attachments or drop the leading !.";
            self.warn(message.into(), window, cx);
            return;
        }
        if images && let Some(model) = self.models.current.as_ref().filter(|m| !m.images) {
            let message = format!(
                "{} does not accept images. Switch to a model with image input, or remove the attachments.",
                model.name
            );
            self.warn(message, window, cx);
            return;
        }
        self.composer
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.list.scroll_to_end();

        if let Some(command) = text.strip_prefix('!') {
            self.run_shell(command, cx);
            cx.notify();
            return;
        }

        if self.attachments.is_empty() && self.intercept(&text, cx) {
            cx.notify();
            return;
        }

        let images = std::mem::take(&mut self.attachments);
        self.send_prompt_with(text, images, cx);
        cx.notify();
    }

    /// Text from outside the composer (`-p`, a later launch's prompt): runs
    /// the way typing it would, `!` shell commands included.
    pub(super) fn run_text(&mut self, text: String, cx: &mut Context<Self>) {
        match text.trim().strip_prefix('!') {
            Some(command) => {
                self.run_shell(command, cx);
                cx.notify();
            }
            None => self.run_prompt(text, cx),
        }
    }

    /// Pi's TUI convention: `!cmd` runs and shares output, `!!cmd` keeps it
    /// out of context. `command` is what follows the first `!`.
    fn run_shell(&mut self, command: &str, cx: &mut Context<Self>) {
        let (command, exclude) = match command.strip_prefix('!') {
            Some(rest) => (rest.trim().to_string(), true),
            None => (command.trim().to_string(), false),
        };
        if command.is_empty() {
            return;
        }
        let index = self.transcript.start_shell(command.clone());
        self.active_shell = Some(index);
        self.request(
            json!({"type": "bash", "command": command, "excludeFromContext": exclude}),
            cx,
            move |this, result, _| {
                this.active_shell = None;
                match result {
                    Ok(data) => this.transcript.finish_shell(
                        index,
                        data["exitCode"].as_i64(),
                        data["output"].as_str().map(str::to_string),
                    ),
                    Err(error) => {
                        this.transcript
                            .finish_shell(index, None, Some(format!("{error:#}")))
                    }
                }
            },
        );
    }

    /// Run a slash command the way typing it would: built-ins and natively
    /// shown commands run here, the rest goes to Pi.
    pub fn run_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        if !self.intercept(&text, cx) {
            self.send_prompt(text, cx);
        }
        cx.notify();
    }

    /// Run `text` here instead of sending it when it is a Pi built-in or a
    /// Gentle Shell command the app shows natively; false: send it.
    fn intercept(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        match builtins::classify(text) {
            builtins::Submit::Builtin(builtin) => {
                self.run_builtin(builtin, cx);
                true
            }
            builtins::Submit::Open(screen) => {
                cx.emit(SessionEvent::OpenScreen(screen));
                true
            }
            builtins::Submit::Prompt => false,
        }
    }

    fn run_builtin(&mut self, builtin: builtins::Builtin, cx: &mut Context<Self>) {
        match builtin {
            builtins::Builtin::Compact(instructions) => self.compact(instructions, cx),
            builtins::Builtin::Name(name) if name.trim().is_empty() => {
                let text = match &self.session_name {
                    Some(name) => {
                        format!("This session is named “{name}”. Rename it with /name <title>.")
                    }
                    None => "This session has no name yet. Name it with /name <title>.".into(),
                };
                self.note(text, cx);
            }
            builtins::Builtin::Name(name) => self.rename(name, cx),
            builtins::Builtin::Copy => self.copy_last_answer(cx),
            builtins::Builtin::Export(path) => self.export_html(path, cx),
            builtins::Builtin::Reload => self.reload(cx),
        }
    }

    /// `/name ` in the composer, cursor at its end.
    pub(super) fn insert_command(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = format!("/{name} ");
        let column = text.chars().map(char::len_utf16).sum::<usize>() as u32;
        self.composer.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.set_cursor_position(Position::new(0, column), window, cx);
            state.focus(window, cx);
        });
    }

    /// Attach validated images (inserting their tokens at the cursor) and warn
    /// about the rest.
    fn attach(
        &mut self,
        loaded: Vec<Result<attachments::Attachment, attachments::Rejection>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let first = self.attachments.len() + 1;
        let mut rejected = Vec::new();
        for result in loaded {
            match result {
                Ok(attachment) => self.attachments.push(attachment),
                Err(rejection) => rejected.push(rejection.to_string()),
            }
        }
        let added = first..self.attachments.len() + 1;
        if !added.is_empty() {
            self.composer.update(cx, |state, cx| {
                let value = state.value();
                let mut before = value[..state.cursor().min(value.len())].chars().next_back();
                let mut tokens = String::new();
                for n in added {
                    tokens.push_str(&attachments::insertion(before, n));
                    before = Some(' ');
                }
                state.insert(tokens, window, cx);
                state.focus(window, cx);
            });
        }
        for message in rejected {
            self.warn(message, window, cx);
        }
        cx.notify();
    }

    /// Read dropped or picked files off the UI thread, then attach them.
    pub(super) fn attach_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let loading = cx.background_executor().spawn(async move {
            paths
                .iter()
                .map(|path| attachments::load_path(path))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let loaded = loading.await;
            let _ = this.update_in(cx, |this, window, cx| this.attach(loaded, window, cx));
        })
        .detach();
    }

    /// The composer's paste hook: clipboard images and copied image files
    /// become attachments; anything else pastes as text.
    pub(super) fn paste_clipboard(
        &mut self,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut images = Vec::new();
        let mut paths = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => images.push(attachments::Attachment::from_bytes(
                    "The pasted image",
                    image.bytes.clone(),
                )),
                ClipboardEntry::ExternalPaths(external) => {
                    paths.extend(external.paths().iter().cloned())
                }
                _ => {}
            }
        }
        // Copied files that are not images paste as their names.
        if !paths.iter().any(|p| attachments::has_image_extension(p)) {
            paths.clear();
        }
        if images.is_empty() && paths.is_empty() {
            return false;
        }
        if !images.is_empty() {
            self.attach(images, window, cx);
        }
        self.attach_paths(paths, window, cx);
        true
    }

    /// The 📎 button: pick image files.
    pub(super) fn pick_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.attach_paths(paths, window, cx));
        })
        .detach();
    }

    /// Drop attachment `index` (0-based) and its token; later tokens shift down.
    pub(super) fn remove_attachment(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.attachments.len() {
            return;
        }
        self.attachments.remove(index);
        self.composer.update(cx, |state, cx| {
            let text = attachments::remove_token(&state.value(), index + 1);
            let line = text.split('\n').count().saturating_sub(1) as u32;
            let column = text
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .map(char::len_utf16)
                .sum::<usize>() as u32;
            state.replace_all(text, window, cx);
            state.set_cursor_position(Position::new(line, column), window, cx);
        });
        cx.notify();
    }
}
