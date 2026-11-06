//! A real terminal pane: the user's login shell in a PTY (alacritty_terminal's
//! `tty` + `EventLoop` + `Term`), painted cell-accurately with GPUI.

mod colors;
mod keys;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui;

const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT: f32 = 1.3;
const PADDING: f32 = 10.;

/// Events a terminal pane reports to its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The OSC title changed (or was reset); re-read [`TerminalView::title`].
    TitleChanged,
    /// The shell process exited; the pane keeps its last screen.
    Exited,
}

/// Forwards PTY/terminal events from the I/O thread to the view.
#[derive(Clone)]
struct Listener(async_channel::Sender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.try_send(event);
    }
}

#[derive(Clone, Copy)]
struct TermDims {
    columns: usize,
    lines: usize,
}

impl Dimensions for TermDims {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Where the grid was last painted; shared with the paint closure so mouse
/// events can map pixels to cells.
struct Geometry {
    origin: Point<Pixels>,
    cell: Size<Pixels>,
    dims: TermDims,
}

impl Geometry {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.dims.lines as u16,
            num_cols: self.dims.columns as u16,
            cell_width: f32::from(self.cell.width) as u16,
            cell_height: f32::from(self.cell.height) as u16,
        }
    }

    /// Viewport cell under `position`, clamped to the grid, and the cell half.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let cw = f32::from(self.cell.width).max(1.);
        let ch = f32::from(self.cell.height).max(1.);
        let x = f32::from(position.x - self.origin.x).max(0.);
        let y = f32::from(position.y - self.origin.y).max(0.);
        let col = ((x / cw) as usize).min(self.dims.columns.saturating_sub(1));
        let row = ((y / ch) as usize).min(self.dims.lines.saturating_sub(1));
        let side = if x - col as f32 * cw > cw / 2. {
            Side::Right
        } else {
            Side::Left
        };
        (row, col, side)
    }
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Option<EventLoopSender>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
    cwd: PathBuf,
    osc_title: Option<String>,
    exited: bool,
    geometry: Rc<RefCell<Geometry>>,
    cell: Option<Size<Pixels>>,
    selecting: bool,
    scroll_accum: f32,
    /// Dead-key / IME composition in progress.
    marked: Option<String>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        if let Some(pty) = &self.pty {
            let _ = pty.send(Msg::Shutdown);
        }
    }
}

#[cfg(unix)]
fn login_shell() -> tty::Shell {
    let program = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty() && Path::new(s).exists())
        .or_else(|| {
            Path::new("/bin/zsh")
                .exists()
                .then(|| "/bin/zsh".to_string())
        })
        .unwrap_or_else(|| "/bin/sh".to_string());
    tty::Shell::new(program, vec!["-l".to_string()])
}

#[cfg(windows)]
fn login_shell() -> tty::Shell {
    let program = std::env::var("COMSPEC")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "powershell.exe".to_string());
    tty::Shell::new(program, Vec::new())
}

fn spawn_pty(
    cwd: &Path,
    term: Arc<FairMutex<Term<Listener>>>,
    listener: Listener,
    size: WindowSize,
) -> anyhow::Result<EventLoopSender> {
    let working_directory = if cwd.is_dir() {
        cwd.to_path_buf()
    } else {
        dirs::home_dir().unwrap_or_else(|| cwd.to_path_buf())
    };
    let env = HashMap::from([
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".to_string(), "truecolor".to_string()),
        ("TERM_PROGRAM".to_string(), "GentleShell".to_string()),
    ]);
    // Windows has extra fields (`escape_args`); the update keeps one literal for every OS.
    #[allow(clippy::needless_update)]
    let options = tty::Options {
        shell: Some(login_shell()),
        working_directory: Some(working_directory),
        drain_on_exit: true,
        env,
        ..Default::default()
    };
    let pty = tty::new(&options, size, 0)?;
    let event_loop = EventLoop::new(term, listener, pty, true, false)?;
    let sender = event_loop.channel();
    event_loop.spawn();
    Ok(sender)
}

fn mono_font() -> Font {
    font(ui::MONO)
}

impl TerminalView {
    pub fn new(cwd: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::unbounded::<TermEvent>();
        let listener = Listener(tx);
        let dims = TermDims {
            columns: 80,
            lines: 24,
        };
        let term = Arc::new(FairMutex::new(Term::new(
            TermConfig::default(),
            &dims,
            listener.clone(),
        )));
        let cell = Self::measure_cell(window);
        let geometry = Geometry {
            origin: Point::default(),
            cell,
            dims,
        };
        let (pty, error) = match spawn_pty(&cwd, term.clone(), listener, geometry.window_size()) {
            Ok(sender) => (Some(sender), None),
            Err(err) => (
                None,
                Some(SharedString::from(format!(
                    "Could not start the shell: {err}"
                ))),
            ),
        };

        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let mut batch = vec![event];
                while let Ok(event) = rx.try_recv() {
                    batch.push(event);
                }
                let alive = this.update(cx, |this, cx| {
                    for event in batch {
                        this.on_term_event(event, cx);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            term,
            pty,
            error,
            focus_handle: cx.focus_handle(),
            cwd,
            osc_title: None,
            exited: false,
            geometry: Rc::new(RefCell::new(geometry)),
            cell: Some(cell),
            selecting: false,
            scroll_accum: 0.,
            marked: None,
        }
    }

    /// OSC title if the shell set one, otherwise `user@cwd-leaf`.
    pub fn title(&self) -> SharedString {
        if let Some(title) = self.osc_title.as_ref().filter(|t| !t.trim().is_empty()) {
            return title.clone().into();
        }
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        let leaf = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.cwd.display().to_string());
        if user.is_empty() {
            leaf.into()
        } else {
            format!("{user}@{leaf}").into()
        }
    }

    fn measure_cell(window: &mut Window) -> Size<Pixels> {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&mono_font());
        let width = text_system
            .advance(font_id, px(FONT_SIZE), 'm')
            .map(|s| s.width)
            .unwrap_or(px(FONT_SIZE * 0.6));
        size(width, px((FONT_SIZE * LINE_HEIGHT).round()))
    }

    fn cell_size(&mut self, window: &mut Window) -> Size<Pixels> {
        *self.cell.get_or_insert_with(|| Self::measure_cell(window))
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return;
        }
        if let Some(pty) = &self.pty {
            let _ = pty.send(Msg::Input(bytes));
        }
    }

    /// User input: jump back to the live screen, drop the selection, send.
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
        cx.notify();
    }

    fn on_term_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Title(title) => {
                self.osc_title = Some(title);
                cx.emit(TerminalEvent::TitleChanged);
            }
            TermEvent::ResetTitle => {
                self.osc_title = None;
                cx.emit(TerminalEvent::TitleChanged);
            }
            TermEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write(format(&text).into_bytes());
            }
            TermEvent::ColorRequest(index, format) => {
                let rgb = colors::for_index(index, self.term.lock().colors());
                self.write(format(rgb).into_bytes());
            }
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            TermEvent::TextAreaSizeRequest(format) => {
                let size = self.geometry.borrow().window_size();
                self.write(format(size).into_bytes());
            }
            TermEvent::ChildExit(_) | TermEvent::Exit => {
                if !self.exited {
                    self.exited = true;
                    cx.emit(TerminalEvent::Exited);
                }
            }
            TermEvent::Wakeup
            | TermEvent::Bell
            | TermEvent::MouseCursorDirty
            | TermEvent::CursorBlinkingChange => {}
        }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        if let Some(text) = self
            .term
            .lock()
            .selection_to_string()
            .filter(|t| !t.is_empty())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            // Strip ESC so pasted text cannot end the bracket early.
            let body = text.replace('\x1b', "");
            format!("\x1b[200~{body}\x1b[201~").into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.input(bytes, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        let key = ks.key.as_str();
        let copy = if cfg!(target_os = "macos") {
            m.platform && !m.control && !m.alt && key == "c"
        } else {
            m.control && m.shift && key == "c"
        };
        let paste = if cfg!(target_os = "macos") {
            m.platform && !m.control && !m.alt && key == "v"
        } else {
            (m.control && m.shift && key == "v")
                || (cfg!(windows) && m.control && !m.alt && key == "v")
        };
        if copy {
            self.copy_selection(cx);
            cx.stop_propagation();
            return;
        }
        if paste {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.paste(&text, cx);
            }
            cx.stop_propagation();
            return;
        }
        // Composition in progress or plain text: the input handler delivers it.
        if self.marked.is_some() || keys::is_text_input(ks) {
            return;
        }
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = keys::key_to_bytes(ks, app_cursor) {
            self.input(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn grid_point(&self, position: Point<Pixels>) -> (GridPoint, Side) {
        let (row, col, side) = self.geometry.borrow().cell_at(position);
        let offset = self.term.lock().grid().display_offset() as i32;
        (GridPoint::new(Line(row as i32 - offset), Column(col)), side)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let (point, side) = self.grid_point(event.position);
        let mut term = self.term.lock();
        match term.selection.as_mut() {
            Some(selection) if event.modifiers.shift => selection.update(point, side),
            _ => {
                let ty = match event.click_count {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                term.selection = Some(Selection::new(ty, point, side));
            }
        }
        drop(term);
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (point, side) = self.grid_point(event.position);
        if let Some(selection) = self.term.lock().selection.as_mut() {
            selection.update(point, side);
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut term = self.term.lock();
        if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            term.selection = None;
        }
        drop(term);
        cx.notify();
    }

    fn on_scroll_event(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = f32::from(self.cell_size(window).height).max(1.);
        self.scroll_accum += f32::from(event.delta.pixel_delta(px(line_height)).y);
        let lines = (self.scroll_accum / line_height).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32 * line_height;
        let mode = *self.term.lock().mode();
        if mode.intersects(TermMode::MOUSE_MODE) && mode.contains(TermMode::SGR_MOUSE) {
            let (row, col, _) = self.geometry.borrow().cell_at(event.position);
            let button = if lines > 0 { 64 } else { 65 };
            let report = format!("\x1b[<{button};{};{}M", col + 1, row + 1);
            self.write(report.repeat(lines.unsigned_abs() as usize).into_bytes());
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let app_cursor = mode.contains(TermMode::APP_CURSOR);
            let arrow = match (lines > 0, app_cursor) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            self.write(arrow.repeat(lines.unsigned_abs() as usize).into_bytes());
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }

    /// Pixel bounds of the cursor cell, for IME candidate windows.
    fn cursor_bounds(&self) -> Bounds<Pixels> {
        let geometry = self.geometry.borrow();
        let term = self.term.lock();
        let point = term.grid().cursor.point;
        let row = point.line.0 + term.grid().display_offset() as i32;
        let origin = geometry.origin
            + gpui_kit::point(
                geometry.cell.width * point.column.0 as f32,
                geometry.cell.height * row.max(0) as f32,
            );
        Bounds::new(origin, geometry.cell)
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.marked.as_ref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        self.input(text.as_bytes().to_vec(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.cursor_bounds())
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

// ── Painting ───────────────────────────────────────────────────────────────

/// A horizontal run of text painted from one origin.
struct Batch {
    row: usize,
    col: usize,
    text: String,
    runs: Vec<TextRun>,
    /// Undecorated spaces seen after the last glyph; only materialized when
    /// another glyph follows on the same row.
    pending_spaces: usize,
    next_col: usize,
    ascii: bool,
}

struct CursorPaint {
    bounds: Bounds<Pixels>,
    shape: CursorShape,
    focused: bool,
}

struct Frame {
    backgrounds: Vec<(Bounds<Pixels>, Hsla)>,
    batches: Vec<Batch>,
    cursor: Option<CursorPaint>,
    origin: Point<Pixels>,
    cell: Size<Pixels>,
}

#[derive(Clone, PartialEq)]
struct RunStyle {
    color: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strike: bool,
}

impl RunStyle {
    fn run(&self, len: usize) -> TextRun {
        let mut font = mono_font();
        if self.bold {
            font.weight = FontWeight::BOLD;
        }
        if self.italic {
            font.style = FontStyle::Italic;
        }
        TextRun {
            len,
            font,
            color: self.color,
            background_color: None,
            underline: (self.underline || self.undercurl).then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(self.color),
                wavy: self.undercurl,
            }),
            strikethrough: self.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: Some(self.color),
            }),
        }
    }

    fn matches(&self, run: &TextRun) -> bool {
        run.color == self.color
            && (run.font.weight == FontWeight::BOLD) == self.bold
            && (run.font.style == FontStyle::Italic) == self.italic
            && run.underline.is_some() == (self.underline || self.undercurl)
            && run.underline.is_some_and(|u| u.wavy) == self.undercurl
            && run.strikethrough.is_some() == self.strike
    }
}

impl Batch {
    fn new(row: usize, col: usize, ascii: bool) -> Self {
        Self {
            row,
            col,
            text: String::new(),
            runs: Vec::new(),
            pending_spaces: 0,
            next_col: col,
            ascii,
        }
    }

    fn push(&mut self, text: &str, style: &RunStyle, width: usize) {
        if self.pending_spaces > 0 {
            // Blank cells between glyphs: never underlined or struck through.
            let plain = RunStyle {
                underline: false,
                undercurl: false,
                strike: false,
                ..style.clone()
            };
            self.push_run(&" ".repeat(self.pending_spaces), &plain);
            self.next_col += self.pending_spaces;
            self.pending_spaces = 0;
        }
        self.push_run(text, style);
        self.next_col += width;
    }

    fn push_run(&mut self, text: &str, style: &RunStyle) {
        self.text.push_str(text);
        match self.runs.last_mut() {
            Some(last) if style.matches(last) => last.len += text.len(),
            _ => self.runs.push(style.run(text.len())),
        }
    }
}

fn build_frame(
    term: &Term<Listener>,
    origin: Point<Pixels>,
    cell: Size<Pixels>,
    focused: bool,
) -> Frame {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let overrides = content.colors;
    let selection = content.selection;
    let cursor = content.cursor;
    let cursor_visible = cursor.shape != CursorShape::Hidden;
    let solid_cursor = focused && cursor_visible && cursor.shape == CursorShape::Block;
    let selection_bg = {
        let mut c = ui::petal();
        c.a = 0.32;
        c
    };
    let cursor_text = colors::hsla(colors::background_default());

    let mut frame = Frame {
        backgrounds: Vec::new(),
        batches: Vec::new(),
        cursor: None,
        origin,
        cell,
    };
    let mut bg_run: Option<(usize, usize, usize, Hsla)> = None; // row, start col, end col, color
    let mut batch: Option<Batch> = None;
    let cell_origin = |row: usize, col: usize| {
        origin + gpui_kit::point(cell.width * col as f32, cell.height * row as f32)
    };

    let flush_bg = |frame: &mut Frame, run: Option<(usize, usize, usize, Hsla)>| {
        if let Some((row, start, end, color)) = run {
            let bounds = Bounds::new(
                cell_origin(row, start),
                size(cell.width * (end - start) as f32, cell.height),
            );
            frame.backgrounds.push((bounds, color));
        }
    };

    for indexed in content.display_iter {
        let point = indexed.point;
        let cell_ref = indexed.cell;
        let flags = cell_ref.flags;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let row = (point.line.0 + offset).max(0) as usize;
        let col = point.column.0;
        let width = if flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        };

        let mut fg = colors::hsla(colors::foreground(
            cell_ref.fg,
            overrides,
            flags.contains(Flags::DIM),
        ));
        let mut bg = colors::background(cell_ref.bg, overrides).map(colors::hsla);
        if flags.contains(Flags::INVERSE) {
            let new_bg = fg;
            fg = bg.unwrap_or(cursor_text);
            bg = Some(new_bg);
        }
        if selection.is_some_and(|s| s.contains(point)) {
            bg = Some(selection_bg);
        }
        let is_cursor = cursor_visible && point == cursor.point;
        if is_cursor {
            let row_px = cell_origin(row, col);
            frame.cursor = Some(CursorPaint {
                bounds: Bounds::new(row_px, size(cell.width * width as f32, cell.height)),
                shape: cursor.shape,
                focused,
            });
            if solid_cursor {
                fg = cursor_text;
            }
        }
        if flags.contains(Flags::HIDDEN) {
            fg.a = 0.;
        }

        // Backgrounds, merged per row.
        match (&mut bg_run, bg) {
            (Some((r, _, end, color)), Some(c)) if *r == row && *end == col && *color == c => {
                *end = col + width
            }
            (_, Some(c)) => {
                flush_bg(&mut frame, bg_run.take());
                bg_run = Some((row, col, col + width, c));
            }
            (_, None) => flush_bg(&mut frame, bg_run.take()),
        }

        // Text.
        let decorated = flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
        let c = cell_ref.c;
        let blank = (c == ' ' || c == '\0' || c == '\t') && !decorated;
        let contiguous = batch
            .as_ref()
            .is_some_and(|b| b.row == row && b.next_col + b.pending_spaces == col);
        if blank {
            if let Some(b) = batch.as_mut().filter(|_| contiguous) {
                b.pending_spaces += width;
            }
            continue;
        }
        let mut text = String::new();
        text.push(if c == '\0' { ' ' } else { c });
        if let Some(zerowidth) = cell_ref.zerowidth() {
            text.extend(zerowidth.iter());
        }
        let ascii = text.is_ascii() && width == 1;
        let style = RunStyle {
            color: fg,
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(
                Flags::UNDERLINE
                    | Flags::DOUBLE_UNDERLINE
                    | Flags::DOTTED_UNDERLINE
                    | Flags::DASHED_UNDERLINE,
            ),
            undercurl: flags.contains(Flags::UNDERCURL),
            strike: flags.contains(Flags::STRIKEOUT),
        };
        // Non-ASCII glyphs (wide chars, box drawing, fallback fonts) get their
        // own origin so they never push the rest of the row off the grid.
        let joins = contiguous && ascii && batch.as_ref().is_some_and(|b| b.ascii);
        if !joins {
            if let Some(done) = batch.take() {
                frame.batches.push(done);
            }
            batch = Some(Batch::new(row, col, ascii));
        }
        if let Some(b) = batch.as_mut() {
            b.push(&text, &style, width);
        }
    }
    flush_bg(&mut frame, bg_run.take());
    if let Some(done) = batch.take() {
        frame.batches.push(done);
    }
    frame
}

fn paint_frame(frame: Frame, window: &mut Window, cx: &mut App) {
    for (bounds, color) in &frame.backgrounds {
        window.paint_quad(fill(*bounds, *color));
    }
    let cursor_color = ui::petal_active();
    if let Some(cursor) = frame
        .cursor
        .as_ref()
        .filter(|c| c.focused && c.shape == CursorShape::Block)
    {
        window.paint_quad(quad(
            cursor.bounds,
            px(2.),
            cursor_color,
            px(0.),
            transparent_black(),
            BorderStyle::default(),
        ));
    }
    let text_system = window.text_system().clone();
    for batch in &frame.batches {
        let origin = frame.origin
            + gpui_kit::point(
                frame.cell.width * batch.col as f32,
                frame.cell.height * batch.row as f32,
            );
        let line =
            text_system.shape_line(batch.text.clone().into(), px(FONT_SIZE), &batch.runs, None);
        let _ = line.paint(origin, frame.cell.height, TextAlign::Left, None, window, cx);
    }
    if let Some(cursor) = frame.cursor {
        let b = cursor.bounds;
        match (cursor.focused, cursor.shape) {
            (true, CursorShape::Block) | (_, CursorShape::Hidden) => {}
            (true, CursorShape::Beam) => {
                window.paint_quad(fill(
                    Bounds::new(b.origin, size(px(2.), b.size.height)),
                    cursor_color,
                ));
            }
            (true, CursorShape::Underline) => {
                let origin = b.origin + gpui_kit::point(px(0.), b.size.height - px(2.));
                window.paint_quad(fill(
                    Bounds::new(origin, size(b.size.width, px(2.))),
                    cursor_color,
                ));
            }
            _ => {
                window.paint_quad(outline(b, cursor_color, BorderStyle::default()));
            }
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cell = self.cell_size(window);
        let focused = self.focus_handle.is_focused(window);
        self.term.lock().is_focused = focused;
        let term = self.term.clone();
        let pty = self.pty.clone();
        let geometry = self.geometry.clone();
        let focus_handle = self.focus_handle.clone();
        let entity = cx.entity();

        let grid = canvas(
            move |bounds, _window, _cx| {
                let columns = ((bounds.size.width / cell.width).floor() as usize).max(2);
                let lines = ((bounds.size.height / cell.height).floor() as usize).max(1);
                let mut geometry = geometry.borrow_mut();
                geometry.origin = bounds.origin;
                geometry.cell = cell;
                let resized = geometry.dims.columns != columns || geometry.dims.lines != lines;
                let mut term = term.lock();
                if resized {
                    geometry.dims = TermDims { columns, lines };
                    term.resize(geometry.dims);
                    if let Some(pty) = &pty {
                        let _ = pty.send(Msg::Resize(geometry.window_size()));
                    }
                }
                build_frame(&term, bounds.origin, cell, focused)
            },
            move |bounds, frame, window, cx| {
                window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity), cx);
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    paint_frame(frame, window, cx)
                });
            },
        )
        .size_full();

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .p(px(PADDING))
            .font_family(ui::MONO)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll(cx.listener(Self::on_scroll_event))
            .child(grid)
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .absolute()
                        .top(px(PADDING))
                        .left(px(PADDING))
                        .right(px(PADDING))
                        .child(
                            ui::well_box()
                                .text_size(px(12.))
                                .text_color(ui::danger())
                                .child(error),
                        ),
                )
            })
            .when(self.exited, |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(PADDING))
                        .right(px(PADDING))
                        .child(ui::chip("Process exited", ui::faint())),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Geometry, Listener, TermDims, build_frame};
    use alacritty_terminal::index::Side;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
    use gpui_kit::{point, px, size};

    fn term_with(bytes: &[u8]) -> Term<Listener> {
        let (tx, _rx) = async_channel::unbounded();
        let mut term = Term::new(
            Config::default(),
            &TermDims {
                columns: 20,
                lines: 4,
            },
            Listener(tx),
        );
        let mut parser: Processor<StdSyncHandler> = Processor::new();
        parser.advance(&mut term, bytes);
        term
    }

    #[test]
    fn frame_batches_keep_glyphs_on_the_grid() {
        let term = term_with("\x1b[4mab\x1b[0m  cd 日x\r\n\x1b[41m  \x1b[0m".as_bytes());
        let frame = build_frame(&term, point(px(0.), px(0.)), size(px(8.), px(16.)), true);
        let batches: Vec<_> = frame
            .batches
            .iter()
            .map(|b| (b.row, b.col, b.text.as_str()))
            .collect();
        assert_eq!(batches, vec![(0, 0, "ab  cd"), (0, 7, "日"), (0, 9, "x")]);
        // Underline stops at the glyphs; the gap is a plain run.
        let first = &frame.batches[0].runs;
        assert_eq!(first.iter().map(|r| r.len).collect::<Vec<_>>(), vec![2, 4]);
        assert!(first[0].underline.is_some() && first[1].underline.is_none());
        // One merged red background quad on row 1, two cells wide.
        assert_eq!(frame.backgrounds.len(), 1);
        let (bounds, _) = frame.backgrounds[0];
        assert_eq!((bounds.origin.y, bounds.size.width), (px(16.), px(16.)));
        // Cursor sits after the background on row 1.
        let cursor = frame.cursor.expect("cursor");
        assert_eq!(cursor.bounds.origin, point(px(16.), px(16.)));
    }

    #[test]
    fn mouse_positions_map_to_clamped_cells() {
        let geometry = Geometry {
            origin: point(px(10.), px(10.)),
            cell: size(px(8.), px(17.)),
            dims: TermDims {
                columns: 80,
                lines: 24,
            },
        };
        assert!(matches!(
            geometry.cell_at(point(px(10.), px(10.))),
            (0, 0, Side::Left)
        ));
        assert!(matches!(
            geometry.cell_at(point(px(10. + 8. * 3. + 6.), px(10. + 17. * 2.))),
            (2, 3, Side::Right)
        ));
        assert!(matches!(
            geometry.cell_at(point(px(5000.), px(5000.))),
            (23, 79, Side::Right)
        ));
        assert!(matches!(
            geometry.cell_at(point(px(0.), px(0.))),
            (0, 0, Side::Left)
        ));
    }
}
