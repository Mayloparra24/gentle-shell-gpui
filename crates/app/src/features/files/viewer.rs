//! A file pane: an editable, syntax-highlighted source buffer that saves with
//! ⌘S and follows outside writes, or a file's read-only working-tree diff
//! against HEAD. Markdown files also render: in place of the buffer, or
//! beside it, live as you type.

use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{Diff, DiffFile, DiffFileStatus, DiffHoverHighlight, DiffState};
use gpui_kit::component::input::{Editor, EditorState, InputEvent, Rope, RopeExt as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{file_icon, human_size, language};
use crate::shared::ui;

actions!(file_view, [Save]);

/// Key context of a file pane, where ⌘S saves.
const CONTEXT: &str = "GentleFileView";
/// The save shortcut as the header shows it.
const SAVE_HINT: &str = if cfg!(target_os = "macos") {
    "⌘S"
} else {
    "Ctrl+S"
};
/// How often an open file is checked for outside writes while it has focus.
const POLL: Duration = Duration::from_millis(800);
/// The same for an unfocused pane, which may be in a hidden tab or
/// workspace: nobody sees it change.
const POLL_UNFOCUSED: Duration = Duration::from_secs(5);
/// How long the header says "Saved".
const SAVED_FLASH: Duration = Duration::from_millis(1600);
/// How long typing pauses before the Markdown preview catches up.
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);

/// Larger files show a notice instead of the editor.
const MAX_PREVIEW: u64 = 2 * 1024 * 1024;
/// Patches past this size are not rendered.
const MAX_PATCH: usize = 4 * 1024 * 1024;

enum Body {
    Loading,
    Code(Entity<EditorState>),
    Diff(Entity<DiffState>),
    Notice {
        icon: IconName,
        title: SharedString,
        detail: SharedString,
    },
}

/// Header facts, filled as the content loads.
#[derive(Default)]
struct Meta {
    size: Option<u64>,
    lines: Option<usize>,
    language: &'static str,
    /// Diff mode: status label, additions, deletions.
    change: Option<(&'static str, usize, usize)>,
}

pub struct FileView {
    path: PathBuf,
    /// Breadcrumb: muted prefix, bold leaf.
    crumbs: (String, String),
    diff: bool,
    body: Body,
    meta: Meta,
    /// File mode with an editable buffer; `None` for diffs and notices.
    edit: Option<Editing>,
    /// A Markdown file's view mode and rendered preview; `None` otherwise.
    markdown: Option<Markdown>,
    focus: FocusHandle,
    /// The header's width in the last frame, for fitting its facts.
    header_width: Rc<Cell<f32>>,
}

/// How a Markdown pane shows its buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MdMode {
    Edit,
    Split,
    Preview,
}

impl MdMode {
    /// In header order.
    const ALL: [MdMode; 3] = [MdMode::Edit, MdMode::Split, MdMode::Preview];
    const LABELS: [&'static str; 3] = ["Edit", "Split", "Preview"];

    /// The mode last picked this session, which new Markdown panes open in.
    fn last() -> Self {
        Self::ALL
            .get(usize::from(LAST_MD_MODE.load(Ordering::Relaxed)))
            .copied()
            .unwrap_or(MdMode::Preview)
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

/// [`MdMode::index`] of the mode last picked; files open rendered until then.
static LAST_MD_MODE: AtomicU8 = AtomicU8::new(2);

/// A Markdown file's rendered side.
struct Markdown {
    mode: MdMode,
    /// The buffer rendered by the same component as agent replies.
    preview: Entity<TextViewState>,
    /// The file's directory: relative links and images resolve against it.
    dir: Arc<Path>,
    images: Arc<EmbeddedImages>,
    mdx: bool,
    /// The editor scroll top last mirrored into the split preview.
    mirrored: Option<Pixels>,
    /// The preview's scroll position and block count when its text was last
    /// replaced: a re-parse that changes the block count resets the list to
    /// the top, and this puts it back.
    restore: Rc<Cell<Option<(ListOffset, usize)>>>,
    _debounce: Task<()>,
    _scroll: Subscription,
    _focus: Subscription,
}

/// The editable buffer against the file on disk.
struct Editing {
    /// The text last read from or written to disk.
    saved: Rope,
    /// The buffer differs from `saved`, or the file is gone from disk.
    dirty: bool,
    /// The file's metadata when `saved` was read or written; `None` once
    /// the file is gone.
    stamp: Option<Stamp>,
    saving: bool,
    /// Briefly set after a save lands, for the header's "Saved".
    just_saved: bool,
    banner: Option<Banner>,
    _changes: Subscription,
    _poll: Task<()>,
    _flash: Task<()>,
}

/// A non-blocking notice above the buffer.
enum Banner {
    /// The file changed on disk under unsaved edits; holds the disk text.
    Conflict(String),
    /// The file is gone from disk.
    Deleted,
    /// The last save failed with this error.
    SaveFailed(SharedString),
}

impl FileView {
    /// Show `path` in an editor with syntax highlighting and line numbers.
    /// ⌘S saves it; outside writes reload a clean buffer and ask before
    /// replacing unsaved edits. Markdown opens in the mode last picked this
    /// session, rendered until one is.
    pub fn open(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let crumbs = ui::split_path(&path);
        let mut meta = Meta {
            language: language(&path).1,
            ..Meta::default()
        };
        // Stat before reading: a write in between shows up as a change.
        let stamp = stamp_of(&path);
        let mut markdown = None;
        let (body, edit) = match read_text(&path) {
            Ok((text, size)) => {
                meta.size = Some(size);
                meta.lines = Some(line_count(&text));
                let token = language(&path).0;
                let preview =
                    is_markdown(&path).then(|| cx.new(|cx| TextViewState::markdown(&text, cx)));
                let editor = cx.new(|cx| {
                    let state = EditorState::new(window, cx)
                        .line_number(true)
                        .soft_wrap(false);
                    match token {
                        Some(lang) => state.language(lang),
                        None => state,
                    }
                    .default_value(text)
                });
                let edit = Editing {
                    saved: editor.read(cx).text().clone(),
                    dirty: stamp.is_none(),
                    stamp,
                    saving: false,
                    just_saved: false,
                    banner: None,
                    _changes: cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.refresh_dirty(cx);
                            this.schedule_preview(cx);
                        }
                    }),
                    _poll: Self::watch(path.clone(), window, cx),
                    _flash: Task::ready(()),
                };
                if let Some(preview) = preview {
                    markdown = Some(Markdown {
                        mode: MdMode::last(),
                        preview,
                        dir: path.parent().unwrap_or(Path::new("")).into(),
                        images: Arc::default(),
                        mdx: path
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("mdx")),
                        mirrored: None,
                        restore: Rc::default(),
                        _debounce: Task::ready(()),
                        _scroll: cx.observe(&editor, |this, _, cx| this.mirror_scroll(cx)),
                        _focus: cx.on_focus(&focus, window, |this, window, cx| {
                            this.focus_body(window, cx)
                        }),
                    });
                }
                (Body::Code(editor), Some(edit))
            }
            Err(notice) => {
                meta.size = notice.size;
                (notice.into_body(), None)
            }
        };
        Self {
            path,
            crumbs,
            diff: false,
            body,
            meta,
            edit,
            markdown,
            focus,
            header_width: Rc::default(),
        }
    }

    /// Bind ⌘S (Ctrl+S elsewhere) to saving the focused file pane.
    pub fn bind_keys(cx: &mut App) {
        let key = if cfg!(target_os = "macos") {
            "cmd-s"
        } else {
            "ctrl-s"
        };
        cx.bind_keys([KeyBinding::new(key, Save, Some(CONTEXT))]);
    }

    /// Show `git diff HEAD -- path` for a file in the repository at `root`
    /// (`path` may be absolute or relative to `root`). Untracked files show
    /// whole as added.
    pub fn diff(root: PathBuf, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let abs = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        let rel = abs.strip_prefix(&root).unwrap_or(&abs).to_path_buf();
        let rel_text = rel.to_string_lossy().replace('\\', "/");
        let crumbs = match rel_text.rsplit_once('/') {
            Some((dir, leaf)) => (format!("{dir}/"), leaf.to_string()),
            None => (String::new(), rel_text.clone()),
        };
        let job_root = root.clone();
        let job_abs = abs.clone();
        cx.spawn_in(window, async move |this, cx| {
            let patch = cx
                .background_executor()
                .spawn(async move { load_patch(&job_root, &job_abs, &rel_text) })
                .await;
            let _ = this.update(cx, |this, cx| this.install_patch(patch, cx));
        })
        .detach();
        Self {
            meta: Meta {
                language: language(&abs).1,
                size: std::fs::metadata(&abs).ok().map(|m| m.len()),
                ..Meta::default()
            },
            path: abs,
            crumbs,
            diff: true,
            body: Body::Loading,
            edit: None,
            markdown: None,
            focus: cx.focus_handle(),
            header_width: Rc::default(),
        }
    }

    /// The tab title: the file name, marked for diffs and unsaved edits.
    pub fn title(&self) -> SharedString {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());
        if self.diff {
            format!("{name} · diff").into()
        } else if self.edit.as_ref().is_some_and(|e| e.dirty) {
            format!("{name} •").into()
        } else {
            name.into()
        }
    }

    /// Edits not yet written to disk; closing the pane would lose them.
    pub fn has_unsaved(&self) -> bool {
        self.edit.as_ref().is_some_and(|e| e.dirty)
    }

    fn install_patch(&mut self, patch: Result<String, Notice>, cx: &mut Context<Self>) {
        self.body = match patch.and_then(|p| parse_patch(&p)) {
            Ok(file) => {
                self.meta.change = Some((
                    status_label(file.status()),
                    file.additions(),
                    file.deletions(),
                ));
                Body::Diff(cx.new(|cx| DiffState::new([file], cx)))
            }
            Err(notice) => notice.into_body(),
        };
        cx.notify();
    }

    /// Check the file for outside writes while the window is in front: a
    /// cheap stat off the UI thread, and a read only when it changed. Panes
    /// without focus (maybe not even on screen) are checked less often.
    fn watch(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            let mut checked = Instant::now();
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(due) = this.update_in(cx, |this, window, cx| {
                    let every = if this.focus.contains_focused(window, cx) {
                        POLL
                    } else {
                        POLL_UNFOCUSED
                    };
                    window.is_window_active()
                        && checked.elapsed() >= every
                        && this.edit.as_ref().is_some_and(|e| !e.saving)
                }) else {
                    break;
                };
                if !due {
                    continue;
                }
                checked = Instant::now();
                let job_path = path.clone();
                let stamp = cx
                    .background_executor()
                    .spawn(async move { stamp_of(&job_path) })
                    .await;
                let Ok(changed) = this.update(cx, |this, _| {
                    this.edit
                        .as_ref()
                        .is_some_and(|e| !e.saving && e.stamp != stamp)
                }) else {
                    break;
                };
                if !changed {
                    continue;
                }
                let job_path = path.clone();
                let disk = cx
                    .background_executor()
                    .spawn(async move { stamp.map(|_| read_text(&job_path)) })
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        this.on_disk_change(stamp, disk, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// Apply an outside write: `disk` is `None` when the file is gone, and a
    /// notice when it can no longer be shown as text (the buffer is kept).
    fn on_disk_change(
        &mut self,
        stamp: Option<Stamp>,
        disk: Option<Result<(String, u64), Notice>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Body::Code(editor), Some(edit)) = (&self.body, self.edit.as_mut()) else {
            return;
        };
        // A save started meanwhile: its own stamp wins.
        if edit.saving {
            return;
        }
        edit.stamp = stamp;
        match disk {
            None => edit.banner = Some(Banner::Deleted),
            Some(Err(_)) => {}
            Some(Ok((text, size))) => {
                self.meta.size = Some(size);
                if matches!(edit.banner, Some(Banner::Deleted | Banner::Conflict(_))) {
                    edit.banner = None;
                }
                let buffer = editor.read(cx).text();
                match outside_write(&text, &edit.saved, buffer) {
                    Outside::Same => {}
                    Outside::Adopt => edit.saved = buffer.clone(),
                    Outside::Reload => {
                        replace_buffer(editor, text, window, cx);
                        edit.saved = editor.read(cx).text().clone();
                    }
                    Outside::Conflict => edit.banner = Some(Banner::Conflict(text)),
                }
            }
        }
        self.refresh_dirty(cx);
        self.schedule_preview(cx);
    }

    /// Recompute the dirty flag and line count from the buffer.
    fn refresh_dirty(&mut self, cx: &mut Context<Self>) {
        let (Body::Code(editor), Some(edit)) = (&self.body, self.edit.as_mut()) else {
            return;
        };
        let buffer = editor.read(cx).text();
        edit.dirty = edit.stamp.is_none() || *buffer != edit.saved;
        self.meta.lines = Some(rope_lines(buffer));
        cx.notify();
    }

    /// Write the buffer to disk off the UI thread.
    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        let (Body::Code(editor), Some(edit)) = (&self.body, self.edit.as_mut()) else {
            return;
        };
        if edit.saving || !edit.dirty {
            return;
        }
        // Saving answers every banner: it keeps these edits over the disk.
        edit.banner = None;
        edit.saving = true;
        let text = editor.read(cx).text().clone();
        let path = self.path.clone();
        cx.spawn(async move |this, cx| {
            let job_text = text.clone();
            let result = cx
                .background_executor()
                .spawn(async move { save_rope(&path, &job_text) })
                .await;
            let _ = this.update(cx, |this, cx| this.finish_save(text, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_save(
        &mut self,
        text: Rope,
        result: io::Result<Option<Stamp>>,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        edit.saving = false;
        match result {
            Ok(stamp) => {
                self.meta.size = Some(text.len() as u64);
                edit.saved = text;
                edit.stamp = stamp;
                edit.just_saved = true;
                edit._flash = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SAVED_FLASH).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(edit) = this.edit.as_mut() {
                            edit.just_saved = false;
                        }
                        cx.notify();
                    });
                });
            }
            Err(err) => {
                edit.just_saved = false;
                edit.banner = Some(Banner::SaveFailed(err.to_string().into()));
            }
        }
        self.refresh_dirty(cx);
    }

    /// Conflict banner: drop the edits for the file on disk.
    fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Body::Code(editor), Some(edit)) = (&self.body, self.edit.as_mut()) else {
            return;
        };
        let Some(Banner::Conflict(text)) = edit.banner.take() else {
            return;
        };
        replace_buffer(editor, text, window, cx);
        edit.saved = editor.read(cx).text().clone();
        self.refresh_dirty(cx);
        self.schedule_preview(cx);
    }

    /// Conflict banner: keep editing; the disk text becomes the baseline, so
    /// the buffer stays dirty until saved over it.
    fn keep_mine(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        if let Some(Banner::Conflict(text)) = edit.banner.take() {
            edit.saved = Rope::from(text.as_str());
        }
        self.refresh_dirty(cx);
    }

    fn dismiss_banner(&mut self, cx: &mut Context<Self>) {
        if let Some(edit) = self.edit.as_mut() {
            edit.banner = None;
        }
        cx.notify();
    }

    /// Re-render the preview once typing pauses; nothing while it is hidden.
    fn schedule_preview(&mut self, cx: &mut Context<Self>) {
        let Some(md) = self.markdown.as_mut().filter(|m| m.mode != MdMode::Edit) else {
            return;
        };
        md._debounce = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.sync_preview(cx));
        });
    }

    /// Hand the buffer, unsaved edits included, to the preview.
    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        let (Body::Code(editor), Some(md)) = (&self.body, self.markdown.as_ref()) else {
            return;
        };
        let text = editor.read(cx).text().to_string();
        let list = md.preview.read(cx).list_state().clone();
        md.restore
            .set(Some((list.logical_scroll_top(), list.item_count())));
        md.preview.update(cx, |state, cx| state.set_text(&text, cx));
    }

    fn set_mode(&mut self, mode: MdMode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(md) = self.markdown.as_mut() else {
            return;
        };
        LAST_MD_MODE.store(mode.index() as u8, Ordering::Relaxed);
        if md.mode == mode {
            return;
        }
        md.mode = mode;
        md.mirrored = None;
        if mode != MdMode::Edit {
            self.sync_preview(cx);
        }
        self.focus_body(window, cx);
        cx.notify();
    }

    /// Focus what takes keys in the current mode: the editor, or the pane
    /// itself while only the preview shows, so ⌘S still reaches it.
    fn focus_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (&self.body, self.markdown.as_ref()) {
            (Body::Code(editor), Some(md)) if md.mode != MdMode::Preview => {
                editor.update(cx, |editor, cx| editor.focus(window, cx));
            }
            _ => self.focus.focus(window, cx),
        }
    }

    /// Split mode: scroll the preview to the same share of the document as
    /// the editor. One way: the preview still scrolls on its own, until the
    /// editor next moves.
    fn mirror_scroll(&mut self, cx: &mut Context<Self>) {
        let (Body::Code(editor), Some(md)) = (&self.body, self.markdown.as_mut()) else {
            return;
        };
        if md.mode != MdMode::Split {
            return;
        }
        let state = editor.read(cx);
        let top = -state.scroll_offset().y;
        if md.mirrored == Some(top) {
            return;
        }
        let (Some(line), Some(rows)) = (state.line_height(), state.visible_row_range()) else {
            return;
        };
        let share = scroll_share(
            f32::from(top) / f32::from(line),
            rope_lines(state.text()),
            rows.len(),
        );
        let list = md.preview.read(cx).list_state().clone();
        let max = list.max_offset_for_scrollbar().y;
        // Not laid out yet: try again on the editor's next update.
        if max <= px(0.) {
            return;
        }
        md.mirrored = Some(top);
        list.set_offset_from_scrollbar(point(px(0.), -(max * share)));
        md.preview.update(cx, |_, cx| cx.notify());
    }

    /// The rendered buffer, scrolling on its own.
    fn render_preview(md: &Markdown, cx: &App) -> Div {
        let restore = md.restore.clone();
        let list = md.preview.read(cx).list_state().clone();
        let image_dir = md.dir.clone();
        let images = md.images.clone();
        let link_dir = md.dir.clone();
        let view = TextView::new(&md.preview)
            .scrollable(true)
            .selectable(true)
            .stream_fade(false)
            .image_source(move |uri| image_source(&image_dir, &images, uri))
            .on_link_click(move |url, _, _, cx| open_link(&link_dir, url, cx));
        let view = if md.mdx { view.markdown_mdx() } else { view };
        div()
            .relative()
            .size_full()
            .text_size(px(14.))
            .text_color(ui::fg())
            // Ahead of the text in prepaint order: it runs once a re-parse
            // has reset the list, and before the list lays out again.
            .child(
                canvas(
                    move |_, _, _| restore_scroll(&restore, &list),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(view.px_6().py_4())
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (icon, tint) = file_icon(&self.crumbs.1);
        let mut facts = Vec::new();
        if let Some(size) = self.meta.size {
            facts.push(human_size(size));
        }
        if let Some(lines) = self.meta.lines {
            facts.push(format!(
                "{lines} {}",
                if lines == 1 { "line" } else { "lines" }
            ));
        }
        let facts = facts.join(" · ");

        // Facts join the header by priority (language, change, size) while
        // they fit beside the path, measured from the last frame's width;
        // the path prefix keeps a little room before it truncates.
        let chars = |s: &str| s.chars().count() as f32;
        let edit = self.edit.as_ref();
        let dirty = edit.is_some_and(|e| e.dirty);
        let saving = edit.is_some_and(|e| e.saving);
        let just_saved = edit.is_some_and(|e| e.just_saved) && !dirty;
        // The save button, and the hint or "Saved" beside it.
        let save_width = match edit {
            None => 0.,
            Some(_) if just_saved => 28. + 58.,
            Some(_) if dirty => 28. + chars(SAVE_HINT) * 7. + 8.,
            Some(_) => 28.,
        };
        // A Markdown pane's Edit · Split · Preview toggle.
        let mode = self.markdown.as_ref().map(|m| m.mode);
        let toggle_width = match mode {
            None => 0.,
            Some(_) => {
                MdMode::LABELS
                    .iter()
                    .map(|l| chars(l) * 6.5 + 22.)
                    .sum::<f32>()
                    + 14.
            }
        };
        let path_width = chars(&self.crumbs.1) * 7.6
            + (chars(&self.crumbs.0) * 6.8).min(90.)
            + if dirty { 13. } else { 0. };
        let mut budget =
            self.header_width.get() - 32. - 24. - 32. - save_width - toggle_width - path_width;
        let mut fits = |cost: f32| {
            let fit = cost + 8. <= budget;
            if fit {
                budget -= cost + 8.;
            }
            fit
        };
        let show_language = fits(chars(self.meta.language) * 6. + 16.);
        let show_change = self.meta.change.is_some_and(|(status, added, removed)| {
            let counts = format!("+{added} −{removed}");
            fits(chars(&counts) * 7. + chars(status) * 6. + 24.)
        });
        let show_facts = !facts.is_empty() && fits(chars(&facts) * 6.6);

        let path = self.path.clone();
        let width = self.header_width.clone();
        let view = cx.entity_id();
        h_flex()
            .relative()
            .flex_none()
            .h(px(38.))
            .px_4()
            .gap_2()
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
            .child(Icon::new(icon).small().text_color(tint))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(ui::faint())
                            .child(self.crumbs.0.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui::fg())
                            .child(self.crumbs.1.clone()),
                    )
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .ml_1p5()
                                .size(px(7.))
                                .rounded_full()
                                .bg(ui::accent()),
                        )
                    }),
            )
            .when_some(
                self.meta.change.filter(|_| show_change),
                |d, (status, added, removed)| {
                    d.child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .font_family(ui::MONO)
                            .text_size(px(11.5))
                            .when(added > 0, |d| {
                                d.child(div().text_color(ui::success()).child(format!("+{added}")))
                            })
                            .when(removed > 0, |d| {
                                d.child(div().text_color(ui::danger()).child(format!("−{removed}")))
                            }),
                    )
                    .child(ui::chip(status, status_tint(status)).flex_none())
                },
            )
            .when(show_facts, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(ui::muted())
                        .child(facts),
                )
            })
            .when(show_language, |d| {
                d.child(ui::chip(self.meta.language, ui::info()).flex_none())
            })
            .when_some(mode, |d, mode| {
                let view = cx.entity().downgrade();
                d.child(ui::segmented(
                    "file-view-mode",
                    &MdMode::LABELS,
                    Some(mode.index()),
                    move |ix, window, cx| {
                        if let Some(&mode) = MdMode::ALL.get(*ix) {
                            let _ = view.update(cx, |this, cx| this.set_mode(mode, window, cx));
                        }
                    },
                ))
            })
            .when(just_saved, |d| {
                d.child(ui::appear(
                    "file-view-saved",
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .text_size(px(12.))
                        .text_color(ui::success())
                        .child(Icon::new(IconName::Check).xsmall())
                        .child("Saved"),
                ))
            })
            .when(dirty, |d| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(ui::MONO)
                        .text_size(px(11.5))
                        .text_color(ui::faint())
                        .child(SAVE_HINT),
                )
            })
            .when(edit.is_some(), |d| {
                d.child(
                    Button::new("file-view-save")
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(IconName::Save))
                        .tooltip(format!("Save ({SAVE_HINT})"))
                        .loading(saving)
                        .disabled(!dirty || saving)
                        .on_click(cx.listener(|this, _, window, cx| this.save(&Save, window, cx))),
                )
            })
            .child(
                Button::new("file-view-reveal")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::ExternalLink))
                    .tooltip("Open with default app")
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&path))),
            )
    }

    /// The outside-write or save-error notice above the buffer, tinted, with
    /// its answers.
    fn render_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let banner = self.edit.as_ref()?.banner.as_ref()?;
        let action = |id: &'static str, label: &'static str, tooltip: &'static str| {
            Button::new(id)
                .ghost()
                .xsmall()
                .label(label)
                .tooltip(tooltip)
        };
        let (tint, icon, title, detail, actions): (_, _, _, SharedString, Vec<Button>) =
            match banner {
                Banner::Conflict(_) => (
                    ui::warning(),
                    IconName::TriangleAlert,
                    "Changed on disk",
                    "This file was changed outside the editor while you had unsaved edits.".into(),
                    vec![
                        action(
                            "file-view-reload",
                            "Reload",
                            "Discard your edits and load the file from disk",
                        )
                        .icon(Icon::new(IconName::RefreshCw))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.reload_from_disk(window, cx)),
                        ),
                        action(
                            "file-view-keep",
                            "Keep mine",
                            "Keep your edits; saving overwrites the file on disk",
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.keep_mine(cx))),
                    ],
                ),
                Banner::Deleted => (
                    ui::warning(),
                    IconName::TriangleAlert,
                    "Deleted on disk",
                    format!("Save ({SAVE_HINT}) to write it back.").into(),
                    vec![
                        action("file-view-dismiss", "Dismiss", "Hide this notice")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_banner(cx))),
                    ],
                ),
                Banner::SaveFailed(err) => (
                    ui::danger(),
                    IconName::CircleAlert,
                    "Couldn't save",
                    err.clone(),
                    vec![
                        action("file-view-dismiss", "Dismiss", "Hide this notice")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_banner(cx))),
                    ],
                ),
            };
        Some(
            h_flex()
                .flex_none()
                .mx_3()
                .mb_2()
                .px_3()
                .py_1p5()
                .gap_2()
                .rounded_lg()
                .bg(tint.opacity(0.10))
                .child(Icon::new(icon).small().text_color(tint))
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child(title),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(ui::muted())
                                .child(detail),
                        ),
                )
                .children(actions),
        )
    }
}

impl Focusable for FileView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.body {
            // The pane itself, so the mode decides where focus lands.
            Body::Code(_) if self.markdown.is_some() => self.focus.clone(),
            Body::Code(editor) => editor.focus_handle(cx),
            Body::Diff(state) => state.focus_handle(cx),
            _ => self.focus.clone(),
        }
    }
}

impl Render for FileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.body {
            Body::Loading => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small().color(ui::muted()))
                .into_any_element(),
            Body::Code(editor) => {
                let code = || {
                    Editor::new(editor)
                        .appearance(false)
                        .font_family(ui::MONO)
                        .text_size(px(12.5))
                        .size_full()
                };
                match self.markdown.as_ref() {
                    Some(md) if md.mode == MdMode::Preview => {
                        Self::render_preview(md, cx).into_any_element()
                    }
                    // Editor left, preview right on an inset tint, kept off
                    // the pane's edges.
                    Some(md) if md.mode == MdMode::Split => h_flex()
                        .size_full()
                        .gap_2()
                        .child(div().flex_1().min_w_0().h_full().child(code()))
                        .child(
                            div().flex_1().min_w_0().h_full().pr_2().pb_2().child(
                                Self::render_preview(md, cx)
                                    .rounded_xl()
                                    .bg(ui::surface())
                                    .overflow_hidden(),
                            ),
                        )
                        .into_any_element(),
                    _ => code().into_any_element(),
                }
            }
            Body::Diff(state) => Diff::new(state)
                .header_visible(false)
                .hover_highlight(DiffHoverHighlight::Line)
                .size_full()
                .into_any_element(),
            Body::Notice {
                icon,
                title,
                detail,
            } => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .p_6()
                .child(ui::appear(
                    "file-view-notice",
                    ui::card()
                        .items_center()
                        .gap_2()
                        .px_6()
                        .py_5()
                        .max_w(px(420.))
                        .child(Icon::new(*icon).text_color(ui::faint()))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(ui::fg())
                                .child(title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(ui::muted())
                                .text_center()
                                .child(detail.clone()),
                        ),
                ))
                .into_any_element(),
        };
        let banner = self.render_banner(cx);
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::save))
            .size_full()
            .min_h_0()
            .child(self.render_header(cx))
            .when_some(banner, |d, banner| {
                d.child(ui::appear("file-view-banner", banner))
            })
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
    }
}

/// Why a file or diff shows a notice instead of content.
#[derive(Debug, Clone, PartialEq)]
struct Notice {
    icon: IconName,
    title: &'static str,
    detail: String,
    size: Option<u64>,
}

impl Notice {
    fn new(icon: IconName, title: &'static str, detail: impl Into<String>) -> Self {
        Self {
            icon,
            title,
            detail: detail.into(),
            size: None,
        }
    }

    fn into_body(self) -> Body {
        Body::Notice {
            icon: self.icon,
            title: self.title.into(),
            detail: self.detail.into(),
        }
    }
}

/// The file as text with its size, or the reason it cannot be previewed.
fn read_text(path: &Path) -> Result<(String, u64), Notice> {
    let unreadable = |e: std::io::Error| {
        Notice::new(IconName::CircleAlert, "Can't read this file", e.to_string())
    };
    let meta = std::fs::metadata(path).map_err(unreadable)?;
    if meta.is_dir() {
        return Err(Notice::new(
            IconName::FolderOpen,
            "This is a folder",
            path.display().to_string(),
        ));
    }
    if meta.len() > MAX_PREVIEW {
        return Err(Notice {
            size: Some(meta.len()),
            ..Notice::new(
                IconName::TriangleAlert,
                "Too large to preview",
                format!(
                    "{} is over the {} preview limit.",
                    human_size(meta.len()),
                    human_size(MAX_PREVIEW)
                ),
            )
        });
    }
    let bytes = std::fs::read(path).map_err(unreadable)?;
    let size = bytes.len() as u64;
    let binary = || Notice {
        size: Some(size),
        ..Notice::new(
            IconName::File,
            "Binary file",
            "This file isn't text. Open it with its default app instead.",
        )
    };
    if looks_binary(&bytes) {
        return Err(binary());
    }
    String::from_utf8(bytes)
        .map(|t| (t, size))
        .map_err(|_| binary())
}

/// A NUL byte in the first 8 KB, as git decides.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

fn line_count(text: &str) -> usize {
    text.lines().count().max(usize::from(!text.is_empty()))
}

/// `line_count` of a buffer, without materializing it.
fn rope_lines(text: &Rope) -> usize {
    match text.len() {
        0 => 0,
        len => text.lines_len() - usize::from(text.byte(len - 1) == b'\n'),
    }
}

/// Files the pane can render: by extension, as editors and forges do.
fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        ["md", "markdown", "mdx", "mdown", "mkd"]
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
    })
}

/// How far down the document an editor scrolled `top_row` rows is, from 0
/// at the top to 1 where its last row shows.
fn scroll_share(top_row: f32, total_rows: usize, visible_rows: usize) -> f32 {
    let range = total_rows.saturating_sub(visible_rows).max(1) as f32;
    (top_row / range).clamp(0., 1.)
}

/// Put the preview back where it was before a re-parse changed its block
/// count, which resets the list to the top.
fn restore_scroll(restore: &Cell<Option<(ListOffset, usize)>>, list: &ListState) {
    let Some((top, count)) = restore.get() else {
        return;
    };
    let now = list.item_count();
    if now == count {
        return;
    }
    restore.set(None);
    if now > 0 {
        list.scroll_to(ListOffset {
            item_ix: top.item_ix.min(now - 1),
            ..top
        });
    }
}

/// Decoded `data:` images by URL, so a frame doesn't decode them again.
type EmbeddedImages = Mutex<HashMap<SharedUri, Option<Arc<Image>>>>;

/// Where a preview image loads from: relative paths against the file's
/// directory, `data:` URLs decoded, anything else as the URL it is.
fn image_source(dir: &Path, embedded: &EmbeddedImages, uri: &SharedUri) -> ImageSource {
    let url: &str = uri;
    if url.starts_with("data:") {
        let mut cache = embedded.lock().unwrap_or_else(PoisonError::into_inner);
        // Typing inside a data URL makes a new one per keystroke.
        if cache.len() >= 32 && !cache.contains_key(uri) {
            cache.clear();
        }
        if let Some(image) = cache.entry(uri.clone()).or_insert_with(|| data_image(url)) {
            return ImageSource::Image(image.clone());
        }
        return uri.clone().into();
    }
    match local_target(dir, url) {
        Some(path) => path.into(),
        None => uri.clone().into(),
    }
}

/// The image a base64 `data:` URL carries, if GPUI can decode its type.
fn data_image(url: &str) -> Option<Arc<Image>> {
    let (meta, data) = url.strip_prefix("data:")?.split_once(',')?;
    let format = ImageFormat::from_mime_type(meta.strip_suffix(";base64")?)?;
    let bytes = BASE64.decode(data.trim()).ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// A clicked preview link: URLs in the browser, local files (relative to
/// the Markdown file) in their default app, in-page anchors nowhere.
fn open_link(dir: &Path, url: &str, cx: &mut App) {
    let url = url.trim();
    if url.is_empty() || url.starts_with('#') {
        return;
    }
    match local_target(dir, url) {
        Some(path) if path.exists() => cx.open_with_system(&path),
        Some(_) => {}
        None => cx.open_url(url),
    }
}

/// A link or image target as a local path, relative ones against `dir`.
/// `None` for URLs with a scheme other than `file:`, and for bare anchors.
fn local_target(dir: &Path, url: &str) -> Option<PathBuf> {
    let url = url.trim();
    let rest = match url.strip_prefix("file://") {
        Some(rest) => rest,
        None if has_scheme(url) => return None,
        None => url,
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let path = percent_decode(&rest[..end]);
    // An absolute path replaces `dir`.
    (!path.is_empty()).then(|| dir.join(path))
}

/// `url` starts with a URL scheme (`https:`, `mailto:`). A single letter is
/// a Windows drive, not a scheme.
fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// `%XX` escapes decoded; text that doesn't decode to UTF-8 stays as is.
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let hex = |b: Option<&u8>| b.and_then(|b| char::from(*b).to_digit(16));
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// What a file's metadata says about its content: a change in either means
/// someone wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

/// `path`'s stamp, `None` when it is gone or unreadable.
fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// What an outside write to the file means for the buffer.
#[derive(Debug, PartialEq, Eq)]
enum Outside {
    /// The disk still holds the saved text (touched, or rewritten as is).
    Same,
    /// The disk now holds exactly the buffer: it becomes the saved text.
    Adopt,
    /// A clean buffer follows the disk.
    Reload,
    /// Unsaved edits and a different disk text: the user decides.
    Conflict,
}

fn outside_write(disk: &str, saved: &Rope, buffer: &Rope) -> Outside {
    if *buffer == disk {
        Outside::Adopt
    } else if *saved == disk {
        Outside::Same
    } else if buffer == saved {
        Outside::Reload
    } else {
        Outside::Conflict
    }
}

/// Replace the editor's text, keeping its selection and scroll where they
/// were so an outside write doesn't jump the view.
fn replace_buffer(editor: &Entity<EditorState>, text: String, window: &mut Window, cx: &mut App) {
    editor.update(cx, |state, cx| {
        let range = state.selected_range();
        let reversed = state.is_selection_reversed();
        let scroll = state.scroll_offset();
        state.set_value(text, window, cx);
        let len = state.text().len();
        state.set_selected_range_with_direction(
            range.start.min(len)..range.end.min(len),
            reversed,
            cx,
        );
        state.set_scroll_offset(scroll, cx);
    });
}

/// Save `text` to `path` and return the file's new stamp.
fn save_rope(path: &Path, text: &Rope) -> io::Result<Option<Stamp>> {
    atomic_write(path, |out| {
        text.chunks()
            .try_for_each(|chunk| out.write_all(chunk.as_bytes()))
    })?;
    Ok(stamp_of(path))
}

/// Replace the file at `path` with what `write` produces. The bytes go to a
/// sibling temp file that takes the file's permissions and is renamed over
/// it, so a failed write leaves the file as it was. A symlink is followed:
/// the link stays and its target is replaced.
fn atomic_write(
    path: &Path,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let target = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let permissions = match fs::metadata(&target) {
        Ok(meta) if meta.permissions().readonly() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the file is read-only",
            ));
        }
        Ok(meta) => Some(meta.permissions()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file path"))?;
    let temp = target.with_file_name(format!(
        ".{}.{}-{}.saving",
        name.to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let mut out = BufWriter::new(file);
        write(&mut out)?;
        let file = out.into_inner().map_err(io::IntoInnerError::into_error)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        fs::rename(&temp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// The unified diff for one file: staged and unstaged changes against HEAD,
/// or the whole file as added when it is untracked.
fn load_patch(root: &Path, abs: &Path, rel: &str) -> Result<String, Notice> {
    let target = abs.to_string_lossy();
    let diff = |extra: &[&str]| {
        let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
        args.extend_from_slice(extra);
        args.extend(["--", target.as_ref()]);
        git(root, &args)
    };
    // A repository without commits has no HEAD: staged, else unstaged.
    let tracked = diff(&["HEAD"]).or_else(|| {
        diff(&["--cached"])
            .filter(|p| !p.trim().is_empty())
            .or_else(|| diff(&[]))
    });
    let Some(patch) = tracked else {
        return Err(Notice::new(
            IconName::GitBranch,
            "Not a git repository",
            root.display().to_string(),
        ));
    };
    if !patch.trim().is_empty() {
        if patch.len() > MAX_PATCH {
            return Err(Notice::new(
                IconName::TriangleAlert,
                "Diff too large",
                human_size(patch.len() as u64),
            ));
        }
        return Ok(patch);
    }
    let untracked = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            target.as_ref(),
        ],
    )
    .is_some_and(|s| s.starts_with("??"));
    if !untracked {
        return Err(Notice::new(
            IconName::CircleCheck,
            "No changes",
            "This file matches HEAD.",
        ));
    }
    let (text, _) = read_text(abs)?;
    Ok(untracked_patch(rel, &text))
}

/// A git-style patch adding `text` as the new file `path`.
fn untracked_patch(path: &str, text: &str) -> String {
    let mut patch = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"
    );
    if text.is_empty() {
        return patch;
    }
    let lines: Vec<&str> = text.lines().collect();
    patch.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
    for line in &lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    if !text.ends_with('\n') {
        patch.push_str("\\ No newline at end of file\n");
    }
    patch
}

/// The single file of a one-path patch, or a notice for binary, empty or
/// malformed patches.
fn parse_patch(patch: &str) -> Result<DiffFile, Notice> {
    let files = DiffFile::parse(patch).map_err(|e| {
        Notice::new(
            IconName::CircleAlert,
            "Couldn't read the diff",
            e.to_string(),
        )
    })?;
    let Some(file) = files.into_iter().next() else {
        return Err(Notice::new(
            IconName::CircleCheck,
            "No changes",
            "This file matches HEAD.",
        ));
    };
    if file.is_binary() {
        return Err(Notice::new(
            IconName::File,
            "Binary file changed",
            "Binary changes can't be shown as text.",
        ));
    }
    // `has_changes` also counts mode-only changes and empty added files.
    if file.additions() == 0 && file.deletions() == 0 {
        let detail = match file.status() {
            DiffFileStatus::Added => "A new, empty file.",
            DiffFileStatus::Deleted => "An empty file was deleted.",
            DiffFileStatus::Renamed => "Renamed without content changes.",
            _ => "Only the file mode changed.",
        };
        return Err(Notice::new(
            IconName::CircleCheck,
            "No content changes",
            detail,
        ));
    }
    Ok(file)
}

fn status_label(status: DiffFileStatus) -> &'static str {
    match status {
        DiffFileStatus::Added => "Added",
        DiffFileStatus::Deleted => "Deleted",
        DiffFileStatus::Renamed => "Renamed",
        DiffFileStatus::Copied => "Copied",
        DiffFileStatus::Conflicted => "Conflict",
        _ => "Modified",
    }
}

fn status_tint(label: &str) -> Hsla {
    match label {
        "Added" => ui::success(),
        "Deleted" | "Conflict" => ui::danger(),
        "Renamed" | "Copied" => ui::info(),
        _ => ui::warning(),
    }
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = pi_rpc::command("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;

    use super::{
        Outside, Rope, atomic_write, data_image, has_scheme, is_markdown, line_count, local_target,
        looks_binary, outside_write, parse_patch, percent_decode, rope_lines, save_rope,
        scroll_share, status_label, untracked_patch,
    };

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gentle-viewer-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entries(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    const MODIFIED: &str = "diff --git a/src/lib.rs b/src/lib.rs
index 3b18e51..a1c2d3e 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,5 @@
 fn main() {
-    println!(\"hi\");
+    let name = \"gentle\";
+    println!(\"hi {name}\");
 }
 
";

    #[test]
    fn modified_patch_counts_changes() {
        let file = parse_patch(MODIFIED).expect("parses");
        assert_eq!(file.path().as_ref(), "src/lib.rs");
        assert_eq!((file.additions(), file.deletions()), (2, 1));
        assert_eq!(status_label(file.status()), "Modified");
    }

    #[test]
    fn untracked_file_is_all_added() {
        let patch = untracked_patch("notes/todo.md", "# Todo\n- one\n- two");
        assert!(patch.contains("@@ -0,0 +1,3 @@\n+# Todo\n+- one\n+- two\n\\ No newline"));
        let file = parse_patch(&patch).expect("parses");
        assert_eq!(file.path().as_ref(), "notes/todo.md");
        assert_eq!((file.additions(), file.deletions()), (3, 0));
        assert_eq!(status_label(file.status()), "Added");
    }

    #[test]
    fn empty_and_binary_patches_are_notices() {
        assert!(parse_patch("").is_err());
        assert!(parse_patch(&untracked_patch("empty.txt", "")).is_err());
        let binary = "diff --git a/logo.png b/logo.png
index 1111111..2222222 100644
Binary files a/logo.png and b/logo.png differ
";
        match parse_patch(binary) {
            Err(notice) => assert_eq!(notice.title, "Binary file changed"),
            Ok(_) => panic!("binary patches are notices"),
        }
    }

    #[test]
    fn text_detection() {
        assert!(looks_binary(b"PNG\0\x01"));
        assert!(!looks_binary("héllo".as_bytes()));
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\nb\n"), 2);
    }

    #[test]
    fn buffer_lines_match_text_lines() {
        for text in ["", "a", "a\n", "\n", "a\nb\n", "a\n\nb", "x\r\ny"] {
            assert_eq!(rope_lines(&Rope::from(text)), line_count(text), "{text:?}");
        }
    }

    #[test]
    fn save_round_trips_bytes() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("notes.md");
        fs::write(&path, "old").unwrap();
        // Large enough to span many rope chunks; CRLF, tabs, no final newline.
        let text = "línea\tuno ✓\r\n".repeat(20_000) + "end";
        save_rope(&path, &Rope::from(text.as_str())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
        assert_eq!(entries(&dir), ["notes.md"]);
        // A file that is gone is written back.
        fs::remove_file(&path).unwrap();
        save_rope(&path, &Rope::from("back")).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "back");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_write_keeps_the_file() {
        let dir = temp_dir("failed");
        let path = dir.join("main.rs");
        fs::write(&path, "fn main() {}\n").unwrap();
        let result = atomic_write(&path, |out| {
            out.write_all(b"fn ma")?;
            Err(io::Error::other("disk full"))
        });
        assert_eq!(result.unwrap_err().to_string(), "disk full");
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn main() {}\n");
        assert_eq!(entries(&dir), ["main.rs"], "the temp file is removed");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_permissions_and_symlinks() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let dir = temp_dir("perms");
        let script = dir.join("run.sh");
        fs::write(&script, "echo hi\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o750)).unwrap();
        let link = dir.join("link.sh");
        symlink(&script, &link).unwrap();
        save_rope(&link, &Rope::from("echo bye\n")).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&script).unwrap(), "echo bye\n");
        let mode = fs::metadata(&script).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750);

        // Read-only files are refused, not replaced.
        fs::set_permissions(&script, fs::Permissions::from_mode(0o444)).unwrap();
        let err = save_rope(&script, &Rope::from("nope")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_to_string(&script).unwrap(), "echo bye\n");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn outside_writes_reload_clean_buffers_and_flag_dirty_ones() {
        let saved = Rope::from("v1");
        let clean = saved.clone();
        let edited = Rope::from("v1 + mine");
        assert_eq!(outside_write("v2", &saved, &clean), Outside::Reload);
        assert_eq!(outside_write("v2", &saved, &edited), Outside::Conflict);
        assert_eq!(outside_write("v1", &saved, &edited), Outside::Same);
        assert_eq!(outside_write("v1", &saved, &clean), Outside::Adopt);
        assert_eq!(outside_write("v1 + mine", &saved, &edited), Outside::Adopt);
    }

    #[test]
    fn markdown_files_by_extension() {
        for name in ["README.md", "notes.MARKDOWN", "page.mdx", "a.mdown"] {
            assert!(is_markdown(std::path::Path::new(name)), "{name}");
        }
        for name in ["README", "main.rs", "md"] {
            assert!(!is_markdown(std::path::Path::new(name)), "{name}");
        }
    }

    #[test]
    fn preview_links_resolve_against_the_file() {
        let dir = std::path::Path::new("/repo/docs");
        let local = |url| local_target(dir, url);
        assert_eq!(
            local("img/logo.png"),
            Some(PathBuf::from("/repo/docs/img/logo.png"))
        );
        assert_eq!(
            local("../README.md#install"),
            Some(PathBuf::from("/repo/docs/../README.md"))
        );
        assert_eq!(
            local("my%20notes.md?plain=1"),
            Some(PathBuf::from("/repo/docs/my notes.md"))
        );
        assert_eq!(local("/etc/hosts"), Some(PathBuf::from("/etc/hosts")));
        assert_eq!(
            local("file:///tmp/a.png"),
            Some(PathBuf::from("/tmp/a.png"))
        );
        assert_eq!(
            local("docs/a:b.png"),
            Some(PathBuf::from("/repo/docs/docs/a:b.png"))
        );
        for url in [
            "https://example.com/a.png",
            "mailto:me@x.io",
            "data:image/png;base64,AA==",
            "#top",
            "",
        ] {
            assert_eq!(local(url), None, "{url}");
        }
        assert!(has_scheme("HTTPS://x") && has_scheme("git+ssh://x"));
        assert!(!has_scheme("C:/repo/a.md") && !has_scheme("a.md"));
        assert_eq!(percent_decode("%E2%9C%93 100%"), "✓ 100%");
        assert_eq!(percent_decode("%FF"), "%FF");
    }

    #[test]
    fn embedded_images_decode() {
        let png = data_image("data:image/png;base64,iVBORw0KGgo=").expect("decodes");
        assert_eq!(png.bytes(), b"\x89PNG\r\n\x1a\n");
        assert!(data_image("data:text/plain;base64,aGVsbG8=").is_none());
        assert!(data_image("data:image/png;base64,not*base64").is_none());
        assert!(data_image("https://example.com/logo.png").is_none());
    }

    #[test]
    fn split_scroll_share() {
        assert_eq!(scroll_share(0., 100, 40), 0.);
        assert_eq!(scroll_share(30., 100, 40), 0.5);
        assert_eq!(scroll_share(60., 100, 40), 1.);
        // Past the last line, and documents that fit.
        assert_eq!(scroll_share(90., 100, 40), 1.);
        assert_eq!(scroll_share(0., 10, 40), 0.);
    }
}
