//! Embedded terminal (⌃O or the title-bar button): the user's shell on a pseudo-terminal,
//! emulated by `alacritty_terminal` and drawn with GPUI text runs.
//!
//! [`Session`] owns the shell and its screen and has no UI, so tests can drive it.
//! [`TerminalView`] draws the screen and turns keys, mouse and clipboard into input.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use gpui::{
    canvas, div, prelude::*, px, App, Bounds, ClipboardItem, Context, FocusHandle, Font, FontStyle, FontWeight, Hsla,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollWheelEvent, StrikethroughStyle, StyledText,
    Task, TextRun, UnderlineStyle, Window,
};

use crate::colors::{hsla, Colors};

const FONT_FAMILY: &str = "Menlo";
const FONT_SIZE: f32 = 12.5;
/// Inner padding around the character grid.
const PAD: f32 = 8.0;

/// Forwards emulator events to the UI thread.
#[derive(Clone)]
pub struct Listener(UnboundedSender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.unbounded_send(event);
    }
}

/// Grid size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// A shell on a pseudo-terminal plus its emulated screen.
pub struct Session {
    pub term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    /// The shell's process id (also its process group: the shell leads its session).
    pub pid: u32,
    /// Controlling side of the pseudo-terminal, kept open by the I/O thread.
    pty_fd: i32,
    size: GridSize,
}

impl Session {
    /// Starts `program args…` in `cwd`; screen changes and requests arrive on `events`.
    pub fn spawn(program: &str, args: &[&str], cwd: &Path, size: GridSize, events: UnboundedSender<TermEvent>) -> io::Result<Session> {
        let listener = Listener(events);
        let config = Config { scrolling_history: 5000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let mut env = HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "FerrousCommander".to_string());
        if std::env::var_os("LANG").is_none() {
            // Apps started from Finder get no locale; shells then mangle UTF-8.
            env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
        }
        let options = tty::Options {
            shell: Some(tty::Shell::new(program.to_string(), args.iter().map(|a| a.to_string()).collect())),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&options, window_size(size, 8.0, 16.0), 0)?;
        let pid = pty.child().id();
        let pty_fd = pty.file().as_raw_fd();
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        let _io_thread = event_loop.spawn();
        Ok(Session { term, notifier, pid, pty_fd, size })
    }

    /// The user's login shell in `cwd`.
    pub fn shell(cwd: &Path, size: GridSize, events: UnboundedSender<TermEvent>) -> io::Result<Session> {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
        Session::spawn(&shell, &["-l"], cwd, size, events)
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    pub fn resize(&mut self, size: GridSize, cell_width: f32, cell_height: f32) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        self.notifier.on_resize(window_size(size, cell_width, cell_height));
    }

    /// The shell's current folder.
    pub fn cwd(&self) -> Option<PathBuf> {
        fc_core::platform::process_cwd(self.pid)
    }

    /// True while the shell itself (not a program it started) owns the terminal, i.e. it
    /// sits at a prompt and typed commands reach it.
    pub fn shell_in_foreground(&self) -> bool {
        // SAFETY: `pty_fd` stays open while the I/O thread runs; afterwards the call fails.
        let group = unsafe { libc::tcgetpgrp(self.pty_fd) };
        group > 0 && group as u32 == self.pid
    }

    /// The visible screen as text, one line per row, without trailing blanks.
    pub fn screen_text(&self) -> String {
        let term = self.term.lock();
        let content = term.renderable_content();
        let mut lines = vec![String::new(); term.screen_lines()];
        for item in content.display_iter {
            let row = (item.point.line.0 + content.display_offset as i32) as usize;
            if let Some(line) = lines.get_mut(row)
                && !item.cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    line.push(item.cell.c);
                }
        }
        lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n").trim_end().to_string()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Ends the I/O thread, which hangs up on the shell.
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

fn window_size(size: GridSize, cell_width: f32, cell_height: f32) -> WindowSize {
    WindowSize {
        num_lines: size.rows as u16,
        num_cols: size.cols as u16,
        cell_width: cell_width.round().max(1.0) as u16,
        cell_height: cell_height.round().max(1.0) as u16,
    }
}

// ----- keyboard ------------------------------------------------------------------------

/// Modifier state for [`key_bytes`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Bytes a key sends to the shell (xterm conventions), or `None` for keys left to the app.
/// `key` is GPUI's key name, `key_char` the text it types; `app_cursor` is DECCKM.
pub fn key_bytes(key: &str, key_char: Option<&str>, m: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
    let csi = |final_byte: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if modifier > 1 { format!("\x1b[{code};{modifier}~").into_bytes() } else { format!("\x1b[{code}~").into_bytes() }
    };
    let bytes = match key {
        "up" => csi('A'),
        "down" => csi('B'),
        "right" if m.alt && !m.ctrl && !m.shift => b"\x1bf".to_vec(),
        "left" if m.alt && !m.ctrl && !m.shift => b"\x1bb".to_vec(),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "enter" => if m.alt { b"\x1b\r".to_vec() } else { b"\r".to_vec() },
        "tab" => if m.shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
        "escape" => b"\x1b".to_vec(),
        "backspace" => match (m.alt, m.ctrl) {
            (true, _) => b"\x1b\x7f".to_vec(),
            (_, true) => vec![0x08],
            _ => vec![0x7f],
        },
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        _ if m.ctrl => {
            let k = if key == "space" { ' ' } else { key.chars().next().filter(|_| key.chars().count() == 1)? };
            let byte = match k.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '/' | '7' => 0x1f,
                '8' | '?' => 0x7f,
                _ => return None,
            };
            if m.alt { vec![0x1b, byte] } else { vec![byte] }
        }
        "space" => b" ".to_vec(),
        _ => key_char.filter(|t| !t.is_empty())?.as_bytes().to_vec(),
    };
    Some(bytes)
}

/// Text pasted into the shell: line breaks become carriage returns; with bracketed paste
/// the program can tell pasted text from typing.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // A pasted end marker must not end the paste early.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

// ----- colours -------------------------------------------------------------------------

const ANSI_DARK: [u32; 16] = [
    0x3b4048, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xc8ccd4,
    0x5c6370, 0xff7b86, 0xb5e890, 0xffd68a, 0x82c4ff, 0xdb9cf0, 0x7fd6e0, 0xffffff,
];
const ANSI_LIGHT: [u32; 16] = [
    0x24292f, 0xc62828, 0x2e7d32, 0x9a6700, 0x1565c0, 0x8e24aa, 0x00838f, 0x6e7781,
    0x57606a, 0xe53935, 0x43a047, 0xb08800, 0x1e88e5, 0xab47bc, 0x00acc1, 0x8c959f,
];

/// RGB of xterm palette entry `index` (0–255).
pub fn palette_rgb(index: u8, dark: bool) -> u32 {
    match index {
        0..=15 => (if dark { ANSI_DARK } else { ANSI_LIGHT })[index as usize],
        16..=231 => {
            let i = index as u32 - 16;
            let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36) << 16) | (level(i / 6 % 6) << 8) | level(i % 6)
        }
        _ => {
            let v = 8 + (index as u32 - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    }
}

/// Default text and background colours of the terminal.
#[derive(Clone, Copy)]
struct Theme {
    fg: Hsla,
    bg: Hsla,
    cursor: Hsla,
    selection: Hsla,
    dark: bool,
}

impl Theme {
    fn new(c: &Colors) -> Theme {
        Theme { fg: c.text, bg: c.bg, cursor: c.accent, selection: c.accent.opacity(0.35), dark: c.dark }
    }

    fn resolve(&self, color: AnsiColor, overrides: &alacritty_terminal::term::color::Colors) -> Hsla {
        let rgb = |r: Rgb| hsla(((r.r as u32) << 16) | ((r.g as u32) << 8) | r.b as u32);
        match color {
            AnsiColor::Spec(r) => rgb(r),
            AnsiColor::Indexed(i) => overrides[i as usize].map(rgb).unwrap_or_else(|| hsla(palette_rgb(i, self.dark))),
            AnsiColor::Named(n) => {
                if let Some(r) = overrides[n] {
                    return rgb(r);
                }
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::DimForeground => self.fg.opacity(0.7),
                    NamedColor::Background => self.bg,
                    NamedColor::Cursor => self.cursor,
                    n if (n as usize) < 16 => hsla(palette_rgb(n as u8, self.dark)),
                    n => {
                        // Dim variants follow the eight base colours.
                        let base = (n as usize).saturating_sub(NamedColor::DimBlack as usize).min(7);
                        hsla(palette_rgb(base as u8, self.dark)).opacity(0.7)
                    }
                }
            }
        }
    }
}

// ----- view ----------------------------------------------------------------------------

/// Emitted when the shell exits.
pub struct Exited;

impl gpui::EventEmitter<Exited> for TerminalView {}

pub struct TerminalView {
    pub focus: FocusHandle,
    pub session: Session,
    /// Where the grid was drawn last frame (for sizing and mouse hit-testing).
    bounds: Rc<Cell<Bounds<Pixels>>>,
    cell: (f32, f32),
    dark: bool,
    selecting: bool,
    /// Scroll-wheel motion not yet worth a whole line.
    scroll_rest: f32,
    exited: bool,
    _events: Task<()>,
}

/// Starts the user's shell in `cwd`.
pub fn open(cwd: &Path, cx: &mut App) -> io::Result<gpui::Entity<TerminalView>> {
    let (tx, rx) = unbounded();
    let session = Session::shell(cwd, GridSize { cols: 100, rows: 12 }, tx)?;
    Ok(cx.new(|cx| TerminalView::with_session(session, rx, cx)))
}

impl TerminalView {
    fn with_session(session: Session, mut rx: UnboundedReceiver<TermEvent>, cx: &mut Context<Self>) -> TerminalView {
        let events = cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(more) = rx.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |this, cx| batch.into_iter().for_each(|e| this.on_term_event(e, cx))).is_err() {
                    break;
                }
            }
        });
        TerminalView {
            focus: cx.focus_handle(),
            session,
            bounds: Rc::new(Cell::new(Bounds::default())),
            cell: (7.5, 16.0),
            dark: true,
            selecting: false,
            scroll_rest: 0.0,
            exited: false,
            _events: events,
        }
    }

    fn on_term_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange | TermEvent::Title(_) | TermEvent::ResetTitle => cx.notify(),
            TermEvent::PtyWrite(text) => self.session.write(text.into_bytes()),
            TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            TermEvent::ClipboardLoad(_, format) => {
                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                self.session.write(format(&text).into_bytes());
            }
            TermEvent::ColorRequest(index, format) => {
                let theme = if self.dark { (0xc8ccd4, 0x1b1d22) } else { (0x24292f, 0xffffff) };
                let v = match index {
                    256 => theme.0,
                    257 => theme.1,
                    i => palette_rgb(i.min(255) as u8, self.dark),
                };
                let rgb = Rgb { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8 };
                self.session.write(format(rgb).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let size = window_size(self.session.size(), self.cell.0, self.cell.1);
                self.session.write(format(size).into_bytes());
            }
            TermEvent::ChildExit(_) | TermEvent::Exit => {
                if !self.exited {
                    self.exited = true;
                    cx.emit(Exited);
                }
            }
            TermEvent::Bell => {}
        }
    }

    /// Sends text as if typed (scrolls back to the prompt first).
    pub fn type_text(&mut self, text: &str) {
        self.session.term.lock().scroll_display(Scroll::Bottom);
        self.session.write(text.as_bytes().to_vec());
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if m.platform {
            match ks.key.as_str() {
                "c" => {
                    if let Some(text) = self.session.term.lock().selection_to_string().filter(|t| !t.is_empty()) {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        let bracketed = self.session.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
                        self.session.term.lock().scroll_display(Scroll::Bottom);
                        self.session.write(paste_bytes(&text, bracketed));
                    }
                }
                "a" => self.session.term.lock().selection = Some(select_all(&self.session)),
                "backspace" => self.session.write(vec![0x15]),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let mode = *self.session.term.lock().mode();
        if m.shift && matches!(ks.key.as_str(), "pageup" | "pagedown" | "home" | "end") && !mode.contains(TermMode::ALT_SCREEN) {
            let scroll = match ks.key.as_str() {
                "pageup" => Scroll::PageUp,
                "pagedown" => Scroll::PageDown,
                "home" => Scroll::Top,
                _ => Scroll::Bottom,
            };
            self.session.term.lock().scroll_display(scroll);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let mods = Mods { shift: m.shift, alt: m.alt, ctrl: m.control };
        let Some(bytes) = key_bytes(&ks.key, ks.key_char.as_deref(), mods, mode.contains(TermMode::APP_CURSOR)) else { return };
        {
            let mut term = self.session.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.session.write(bytes);
        cx.stop_propagation();
        cx.notify();
    }

    /// Grid cell (and which half of it) under a window position.
    fn grid_point(&self, position: gpui::Point<Pixels>) -> (GridPoint, Side) {
        let b = self.bounds.get();
        let (cw, ch) = self.cell;
        let x: f32 = (position.x - b.origin.x).into();
        let y: f32 = (position.y - b.origin.y).into();
        let size = self.session.size();
        let col = ((x - PAD) / cw).floor().clamp(0.0, size.cols as f32 - 1.0);
        let row = ((y - PAD) / ch).floor().clamp(0.0, size.rows as f32 - 1.0);
        let side = if (x - PAD) / cw - col > 0.5 { Side::Right } else { Side::Left };
        let offset = self.session.term.lock().grid().display_offset() as i32;
        (GridPoint::new(Line(row as i32 - offset), Column(col as usize)), side)
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let (point, side) = self.grid_point(ev.position);
        let kind = match ev.click_count {
            2 => SelectionType::Semantic,
            n if n >= 3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.session.term.lock().selection = Some(Selection::new(kind, point, side));
        self.selecting = true;
        cx.stop_propagation();
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || ev.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (point, side) = self.grid_point(ev.position);
        if let Some(sel) = self.session.term.lock().selection.as_mut() {
            sel.update(point, side);
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        let mut term = self.session.term.lock();
        if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            term.selection = None;
        }
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let dy: f32 = ev.delta.pixel_delta(px(self.cell.1)).y.into();
        self.scroll_rest += dy / self.cell.1;
        let lines = self.scroll_rest.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_rest -= lines as f32;
        let mode = *self.session.term.lock().mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // Full-screen programs (less, man, vim) scroll with the arrow keys.
            let key = if lines > 0 { "up" } else { "down" };
            let bytes = key_bytes(key, None, Mods::default(), mode.contains(TermMode::APP_CURSOR)).unwrap_or_default();
            self.session.write(bytes.repeat(lines.unsigned_abs() as usize));
        } else {
            self.session.term.lock().scroll_display(Scroll::Delta(lines));
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Fits the grid to the space it was given last frame.
    fn fit(&mut self, window: &mut Window) {
        let font = gpui::font(FONT_FAMILY);
        let ts = window.text_system();
        let id = ts.resolve_font(&font);
        let cw: f32 = ts.advance(id, px(FONT_SIZE), 'm').map(|s| s.width.into()).unwrap_or(7.5);
        let ch = (FONT_SIZE * 1.35).round();
        self.cell = (cw, ch);
        let b = self.bounds.get();
        let (w, h): (f32, f32) = (b.size.width.into(), b.size.height.into());
        if w < 20.0 || h < 10.0 {
            return;
        }
        let size = GridSize {
            cols: (((w - 2.0 * PAD) / cw).floor() as usize).max(alacritty_terminal::term::MIN_COLUMNS),
            rows: (((h - 2.0 * PAD) / ch).floor() as usize).max(alacritty_terminal::term::MIN_SCREEN_LINES),
        };
        self.session.resize(size, cw, ch);
    }
}

fn select_all(session: &Session) -> Selection {
    let term = session.term.lock();
    let mut sel = Selection::new(SelectionType::Lines, GridPoint::new(term.topmost_line(), Column(0)), Side::Left);
    sel.update(GridPoint::new(term.bottommost_line(), term.last_column()), Side::Right);
    sel
}

/// Character attributes that end a text run when they change.
#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

/// One screen row: its text and styled runs.
fn build_row(cells: &[(char, Style)], extra: &[(usize, Vec<char>)]) -> (String, Vec<(usize, Style)>) {
    let mut text = String::with_capacity(cells.len());
    let mut runs: Vec<(usize, Style)> = Vec::new();
    let mut zero = extra.iter().peekable();
    for (i, (ch, style)) in cells.iter().enumerate() {
        let start = text.len();
        text.push(*ch);
        while let Some((_, chars)) = zero.next_if(|(at, _)| *at == i) {
            text.extend(chars.iter());
        }
        let len = text.len() - start;
        match runs.last_mut() {
            Some((n, s)) if s == style => *n += len,
            _ => runs.push((len, *style)),
        }
    }
    (text, runs)
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get(window, cx);
        self.dark = c.dark;
        self.fit(window);
        let theme = Theme::new(&c);
        let focused = self.focus.is_focused(window);
        let (cw, ch) = self.cell;
        let size = self.session.size();
        let base_font = gpui::font(FONT_FAMILY);

        let (rows, cursor) = {
            let term = self.session.term.lock();
            let content = term.renderable_content();
            let offset = content.display_offset as i32;
            let default = Style { fg: theme.fg, bg: None, bold: false, italic: false, underline: false, strike: false };
            let mut grid: Vec<Vec<(char, Style)>> = vec![Vec::with_capacity(size.cols); size.rows];
            let mut extra: Vec<Vec<(usize, Vec<char>)>> = vec![Vec::new(); size.rows];
            for item in content.display_iter {
                let row = (item.point.line.0 + offset) as usize;
                let Some(line) = grid.get_mut(row) else { continue };
                let cell = &item.cell;
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let mut fg = theme.resolve(cell.fg, content.colors);
                let mut bg = match cell.bg {
                    AnsiColor::Named(NamedColor::Background) => None,
                    other => Some(theme.resolve(other, content.colors)),
                };
                if cell.flags.contains(Flags::INVERSE) {
                    let old_fg = fg;
                    fg = bg.unwrap_or(theme.bg);
                    bg = Some(old_fg);
                }
                if cell.flags.contains(Flags::DIM) {
                    fg = fg.opacity(0.7);
                }
                if cell.flags.contains(Flags::HIDDEN) {
                    fg = bg.unwrap_or(theme.bg);
                }
                if content.selection.is_some_and(|s| s.contains(item.point)) {
                    bg = Some(theme.selection);
                }
                if let Some(chars) = cell.zerowidth() {
                    extra[row].push((line.len(), chars.to_vec()));
                }
                let style = Style {
                    fg,
                    bg,
                    bold: cell.flags.contains(Flags::BOLD),
                    italic: cell.flags.contains(Flags::ITALIC),
                    underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                    strike: cell.flags.contains(Flags::STRIKEOUT),
                };
                line.push((if cell.c == '\0' { ' ' } else { cell.c }, style));
            }
            // Drop trailing blanks that draw nothing.
            for line in &mut grid {
                while line.last().is_some_and(|(ch, s)| *ch == ' ' && *s == default) {
                    line.pop();
                }
            }
            let cur = content.cursor;
            let cursor_row = cur.point.line.0 + offset;
            let cursor = (cur.shape != CursorShape::Hidden && (0..size.rows as i32).contains(&cursor_row))
                .then_some((cursor_row as usize, cur.point.column.0, cur.shape));
            let rows: Vec<(String, Vec<(usize, Style)>)> = grid.iter().zip(&extra).map(|(cells, extra)| build_row(cells, extra)).collect();
            (rows, cursor)
        };

        let mut lines = div().absolute().top(px(PAD)).left(px(PAD)).flex().flex_col();
        for (text, runs) in rows {
            let runs: Vec<TextRun> = runs
                .into_iter()
                .map(|(len, s)| TextRun {
                    len,
                    font: Font {
                        weight: if s.bold { FontWeight::BOLD } else { FontWeight::NORMAL },
                        style: if s.italic { FontStyle::Italic } else { FontStyle::Normal },
                        ..base_font.clone()
                    },
                    color: s.fg,
                    background_color: s.bg,
                    underline: s.underline.then(|| UnderlineStyle { thickness: px(1.), color: Some(s.fg), wavy: false }),
                    strikethrough: s.strike.then(|| StrikethroughStyle { thickness: px(1.), color: Some(s.fg) }),
                })
                .collect();
            let line = div().h(px(ch)).line_height(px(ch)).whitespace_nowrap();
            lines = lines.child(if text.is_empty() { line } else { line.child(StyledText::new(text).with_runs(runs)) });
        }

        let cursor_el = cursor.map(|(row, col, shape)| {
            let (left, top) = (PAD + col as f32 * cw, PAD + row as f32 * ch);
            let base = div().absolute().left(px(left)).top(px(top));
            match (focused, shape) {
                (false, _) | (_, CursorShape::HollowBlock) => base.w(px(cw)).h(px(ch)).border_1().border_color(theme.cursor),
                (true, CursorShape::Beam) => base.w(px(2.)).h(px(ch)).bg(theme.cursor),
                (true, CursorShape::Underline) => base.top(px(top + ch - 2.)).w(px(cw)).h(px(2.)).bg(theme.cursor),
                _ => base.w(px(cw)).h(px(ch)).bg(theme.cursor.opacity(0.55)),
            }
        });

        let bounds = self.bounds.clone();
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(theme.bg)
            .text_color(theme.fg)
            .font_family(FONT_FAMILY)
            .text_size(px(FONT_SIZE))
            .cursor_text()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, w, cx| this.on_mouse_up(w, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, w, cx| this.on_mouse_up(w, cx)))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |b, window, _| {
                        if bounds.get().size != b.size {
                            window.refresh();
                        }
                        bounds.set(b);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(cursor_el)
            .child(lines)
    }
}

impl gpui::Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Processor;

    #[test]
    fn keys_map_to_xterm_sequences() {
        let none = Mods::default();
        let k = |key: &str, ch: Option<&str>, m: Mods| key_bytes(key, ch, m, false);
        assert_eq!(k("a", Some("a"), none), Some(b"a".to_vec()));
        assert_eq!(k("enter", None, none), Some(b"\r".to_vec()));
        assert_eq!(k("backspace", None, none), Some(vec![0x7f]));
        assert_eq!(k("c", Some("c"), Mods { ctrl: true, ..none }), Some(vec![3]), "⌃C interrupts");
        assert_eq!(k("d", None, Mods { ctrl: true, ..none }), Some(vec![4]));
        assert_eq!(k("up", None, none), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes("up", None, none, true), Some(b"\x1bOA".to_vec()), "application cursor mode");
        assert_eq!(k("right", None, Mods { shift: true, ..none }), Some(b"\x1b[1;2C".to_vec()));
        assert_eq!(k("left", None, Mods { alt: true, ..none }), Some(b"\x1bb".to_vec()), "⌥← jumps a word");
        assert_eq!(k("tab", None, Mods { shift: true, ..none }), Some(b"\x1b[Z".to_vec()));
        assert_eq!(k("delete", None, none), Some(b"\x1b[3~".to_vec()));
        assert_eq!(k("f5", None, none), Some(b"\x1b[15~".to_vec()));
        assert_eq!(k("space", Some(" "), none), Some(b" ".to_vec()));
        assert_eq!(k("é", Some("é"), none), Some("é".as_bytes().to_vec()));
        assert_eq!(k("shift", None, none), None, "lone modifiers send nothing");
    }

    #[test]
    fn paste_converts_newlines_and_brackets() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc".to_vec());
        assert_eq!(paste_bytes("ls\n", true), b"\x1b[200~ls\r\x1b[201~".to_vec());
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~".to_vec());
    }

    #[test]
    fn xterm_palette() {
        assert_eq!(palette_rgb(16, true), 0x000000);
        assert_eq!(palette_rgb(21, true), 0x0000ff);
        assert_eq!(palette_rgb(196, true), 0xff0000);
        assert_eq!(palette_rgb(231, false), 0xffffff);
        assert_eq!(palette_rgb(232, true), 0x080808);
        assert_eq!(palette_rgb(255, true), 0xeeeeee);
    }

    #[test]
    fn rows_merge_runs_and_keep_combining_marks() {
        let a = Style { fg: gpui::black(), bg: None, bold: false, italic: false, underline: false, strike: false };
        let b = Style { bold: true, ..a };
        let cells = [('h', a), ('i', a), ('!', b)];
        let (text, runs) = build_row(&cells, &[(1, vec!['\u{301}'])]);
        assert_eq!(text, "hi\u{301}!");
        assert_eq!(runs.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [4, 1], "lengths are in bytes");
        assert!(runs[1].1.bold);
    }

    /// The emulator alone: escape sequences end up on the screen grid.
    #[test]
    fn emulator_draws_output() {
        let (tx, _rx) = unbounded();
        let mut term = Term::new(Config::default(), &GridSize { cols: 20, rows: 4 }, Listener(tx));
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"hello\r\n\x1b[31mred\x1b[0m\x1b[2;8Hxy");
        let rows: Vec<String> = (0..2)
            .map(|r| (0..20).map(|c| term.grid()[GridPoint::new(Line(r), Column(c))].c).collect::<String>().trim_end().to_string())
            .collect();
        assert_eq!(rows, ["hello", "red    xy"]);
        assert_eq!(term.grid()[GridPoint::new(Line(1), Column(0))].fg, AnsiColor::Named(NamedColor::Red));
    }

    /// A real process on a real pseudo-terminal.
    #[test]
    fn session_runs_a_program_in_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker.txt"), "").unwrap();
        let (tx, _rx) = unbounded();
        let session = Session::spawn("/bin/sh", &["-c", "ls; printf 'cols=%s' \"$(tput cols)\"; sleep 3"], dir.path(), GridSize { cols: 50, rows: 6 }, tx).unwrap();
        let mut text = String::new();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(60));
            text = session.screen_text();
            if text.contains("cols=") {
                break;
            }
        }
        assert!(text.contains("marker.txt"), "screen: {text:?}");
        assert!(text.contains("cols=50"), "the program sees the grid size: {text:?}");
        assert_eq!(session.cwd().map(|p| p.canonicalize().unwrap()), Some(dir.path().canonicalize().unwrap()));
    }

    /// The panel only types `cd` into a shell that is waiting at its prompt.
    #[test]
    fn foreground_detection_with_job_control() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, _rx) = unbounded();
        let session = Session::spawn("/bin/sh", &["-i"], dir.path(), GridSize { cols: 60, rows: 6 }, tx).unwrap();
        let wait_for = |want: bool| (0..60).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            session.shell_in_foreground() == want
        });
        assert!(wait_for(true), "the idle shell owns the terminal");
        session.write(b"sleep 2\r".to_vec());
        assert!(wait_for(false), "a running program owns the terminal");
        session.write(b"\x03".to_vec());
        assert!(wait_for(true), "back at the prompt after ⌃C");
    }
}
