//! Modal dialogs and job prompts.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::{div, prelude::*, px, AnyElement, Context, Entity, Hsla, MouseButton, SharedString, Window};

use fc_core::archive::CompressionLevel;
use fc_core::format::{self, SizeMode};
use fc_core::ops::{self, ConflictChoice, ErrorChoice, TransferKind};
use fc_core::queue::{Answer, JobState, Prompt};
use fc_core::Entry;

use crate::actions::ui;
use crate::colors::Colors;
use crate::{icons, look};
use gpui::{ease_out_quint, Animation, AnimationExt};
use std::time::Duration;
use crate::text_input::{self, InputStyle, TextInput};
use crate::workspace::Workspace;

/// Get Info state; the folder size arrives from a background thread.
pub struct InfoState {
    pub entry: Entry,
    pub owner: String,
    pub group: String,
    pub mode: u32,
    pub locked: bool,
    pub size: Arc<Mutex<Option<(u64, u64, u64)>>>,
}

impl InfoState {
    pub fn new(entry: Entry) -> InfoState {
        let (owner, group) = ops::owner_names(entry.uid, entry.gid);
        InfoState {
            mode: entry.mode,
            locked: entry.flags & libc_uf_immutable() != 0,
            owner,
            group,
            size: Arc::new(Mutex::new(None)),
            entry,
        }
    }
}

const fn libc_uf_immutable() -> u32 {
    0x0000_0002
}

pub enum Dialog {
    Transfer { kind: TransferKind, sources: Vec<PathBuf>, base_dir: PathBuf, input: Entity<TextInput>, enqueue: bool },
    ConfirmRenameExt { path: PathBuf, new_name: String },
    Delete { paths: Vec<PathBuf>, permanent: bool, nonempty_dirs: usize },
    NewFolder { dir: PathBuf, input: Entity<TextInput>, with: Vec<PathBuf> },
    NewFile { dir: PathBuf, input: Entity<TextInput> },
    Pattern { select: bool, input: Entity<TextInput> },
    GoTo { input: Entity<TextInput> },
    Server { input: Entity<TextInput> },
    Compress { items: Vec<PathBuf>, input: Entity<TextInput>, level: CompressionLevel },
    CloseLockedTab { side: usize, ix: usize },
    Info(InfoState),
    Queue,
    QuitWithJobs,
    #[allow(dead_code)]
    Message { title: String, body: String },
}

impl Dialog {
    pub fn input(&self) -> Option<Entity<TextInput>> {
        match self {
            Dialog::Transfer { input, .. }
            | Dialog::NewFolder { input, .. }
            | Dialog::NewFile { input, .. }
            | Dialog::Pattern { input, .. }
            | Dialog::GoTo { input }
            | Dialog::Server { input }
            | Dialog::Compress { input, .. } => Some(input.clone()),
            _ => None,
        }
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn count_label(paths: &[PathBuf]) -> String {
    match paths {
        [one] => format!("\"{}\"", name_of(one)),
        many => format!("{} items", many.len()),
    }
}

/// Button styles.
#[derive(Clone, Copy, PartialEq)]
pub enum Btn {
    Primary,
    Normal,
    Danger,
}

pub fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    kind: Btn,
    c: &Colors,
    on_click: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let (bg, fg, border): (Hsla, Hsla, Hsla) = match kind {
        Btn::Primary => (c.accent, c.on_accent, c.accent),
        Btn::Danger => (c.danger, gpui::white(), c.danger),
        Btn::Normal => (c.sunken, c.text, c.border),
    };
    let id: SharedString = id.into();
    div()
        .id(id)
        .px(px(15.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(9.))
        .bg(bg)
        .text_color(fg)
        .border_1()
        .border_color(border)
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .when(kind != Btn::Normal, |d| d.shadow(vec![gpui::BoxShadow { color: bg.opacity(0.35), offset: gpui::point(px(0.), px(4.)), blur_radius: px(12.), spread_radius: px(-4.) }]))
        .hover(|s| s.opacity(0.88))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
            cx.stop_propagation();
            on_click(this, w, cx)
        }))
        .child(label.into())
        .into_any_element()
}

/// Dialog card with an icon tile and title.
fn card(glyph: &'static str, tint: Hsla, title: impl Into<SharedString>, c: &Colors) -> gpui::Div {
    div()
        .w(px(520.))
        .max_w(px(640.))
        .p(px(22.))
        .rounded(px(16.))
        .bg(c.surface)
        .border_1()
        .border_color(c.border)
        .shadow(vec![
            gpui::BoxShadow { color: c.shadow, offset: gpui::point(px(0.), px(24.)), blur_radius: px(60.), spread_radius: px(-12.) },
            gpui::BoxShadow { color: tint.opacity(0.12), offset: gpui::point(px(0.), px(0.)), blur_radius: px(40.), spread_radius: px(0.) },
        ])
        .text_color(c.text)
        .text_size(px(13.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().size(px(38.)).flex_none().rounded(px(11.)).bg(tint.opacity(0.16)).flex().items_center().justify_center().child(look::icon(glyph, 19., tint)))
                .child(div().flex_1().min_w(px(0.)).text_size(px(15.5)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.into())),
        )
}

fn buttons_row(children: Vec<AnyElement>) -> gpui::Div {
    div().flex().flex_row().justify_end().gap(px(8.)).pt(px(4.)).children(children)
}

fn overlay(content: impl IntoElement, c: &Colors) -> gpui::Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(c.scrim)
        .flex()
        .items_start()
        .justify_center()
        .pt(px(96.))
        .child(content)
}

/// Pop-in animation for dialog bodies.
fn pop(el: gpui::Stateful<gpui::Div>, generation: u64) -> AnyElement {
    el.with_animation(
        ("dialog-in", generation as usize),
        Animation::new(Duration::from_millis(230)).with_easing(ease_out_quint()),
        |el, t| el.opacity(t).mt(px((1.0 - t) * 18.0)),
    )
    .into_any_element()
}

fn input_style(c: &Colors) -> InputStyle {
    InputStyle { text: c.text, muted: c.muted, bg: c.bg, border: c.border, accent: c.accent, selection: c.accent.opacity(0.28) }
}

/// Renders the open dialog.
pub fn render(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let c = Colors::get(window, cx);
    text_input::set_style(input_style(&c));
    let dialog = ws.dialog.as_ref()?;
    let cancel = |cx: &mut Context<Workspace>, c: &Colors| button("cancel", "Cancel", Btn::Normal, c, |this, w, cx| this.close_dialog(w, cx), cx);
    let ok = |label: &str, kind: Btn, cx: &mut Context<Workspace>, c: &Colors| {
        button("ok", label.to_string(), kind, c, |this, w, cx| this.confirm_dialog(w, cx), cx)
    };

    let body: gpui::Div = match dialog {
        Dialog::Transfer { kind, sources, input, enqueue, .. } => {
            let verb = if *kind == TransferKind::Copy { "Copy" } else { "Move" };
            let enqueue = *enqueue;
            card(if *kind == TransferKind::Copy { icons::COPY } else { icons::MOVE }, c.accent, format!("{verb} {} to:", count_label(sources)), &c)
                .child(input.clone())
                .child(div().text_size(px(12.)).text_color(c.muted).child(
                    "An existing folder receives the items. A new name renames a single item; several items go into a new folder.",
                ))
                .child(buttons_row(vec![
                    cancel(cx, &c),
                    button("queue", "Add to Queue", if enqueue { Btn::Primary } else { Btn::Normal }, &c, move |this, w, cx| {
                        if let Some(Dialog::Transfer { enqueue, .. }) = this.dialog.as_mut() {
                            *enqueue = true;
                        }
                        this.confirm_dialog(w, cx)
                    }, cx),
                    button("ok", verb, if enqueue { Btn::Normal } else { Btn::Primary }, &c, move |this, w, cx| {
                        if let Some(Dialog::Transfer { enqueue, .. }) = this.dialog.as_mut() {
                            *enqueue = false;
                        }
                        this.confirm_dialog(w, cx)
                    }, cx),
                ]))
        }
        Dialog::ConfirmRenameExt { path, new_name } => {
            let old_name = name_of(path);
            let (_, a) = fc_core::entry::split_extension(&old_name);
            let (_, b) = fc_core::entry::split_extension(new_name);
            card(icons::WARNING, c.accent2, format!("Change the extension from \".{a}\" to \"{}\"?", if b.is_empty() { String::from("(none)") } else { format!(".{b}") }), &c)
                .child("The document may open in a different application.")
                .child(buttons_row(vec![cancel(cx, &c), ok("Change", Btn::Primary, cx, &c)]))
        }
        Dialog::Delete { paths, permanent, nonempty_dirs } => {
            let title = if *permanent {
                format!("Delete {} permanently?", count_label(paths))
            } else {
                format!("Move {} to the Trash?", count_label(paths))
            };
            let mut card = card(icons::TRASH, if *permanent { c.danger } else { c.accent }, title, &c);
            if *permanent {
                card = card.child(div().text_color(c.danger).child("This can't be undone."));
            }
            if *nonempty_dirs > 0 {
                card = card.child(format!("{nonempty_dirs} of the folders are not empty."));
            }
            card.child(buttons_row(vec![
                cancel(cx, &c),
                ok(if *permanent { "Delete" } else { "Move to Trash" }, if *permanent { Btn::Danger } else { Btn::Primary }, cx, &c),
            ]))
        }
        Dialog::NewFolder { input, with, .. } => {
            let title = if with.is_empty() { "New folder".to_string() } else { format!("New folder with {} selected item(s)", with.len()) };
            card(icons::FOLDER_PLUS, c.folder, title, &c)
                .child(input.clone())
                .child(div().text_size(px(12.)).text_color(c.muted).child("Use a/b/c to create nested folders."))
                .child(buttons_row(vec![cancel(cx, &c), ok("Create", Btn::Primary, cx, &c)]))
        }
        Dialog::NewFile { input, .. } => card(icons::FILE_PLUS, c.accent, "New file", &c)
            .child(input.clone())
            .child(buttons_row(vec![cancel(cx, &c), ok("Create", Btn::Primary, cx, &c)])),
        Dialog::Pattern { select, input } => card(icons::CHECK, c.accent, if *select { "Select items matching" } else { "Unselect items matching" }, &c)
            .child(input.clone())
            .child(div().text_size(px(12.)).text_color(c.muted).child("Wildcards: * any text, ? one character, [a-z] a range. Separate masks with ;"))
            .child(buttons_row(vec![cancel(cx, &c), ok(if *select { "Select" } else { "Unselect" }, Btn::Primary, cx, &c)])),
        Dialog::GoTo { input } => card(icons::FOLDER_OPEN, c.folder, "Go to folder", &c)
            .child(input.clone())
            .child(div().text_size(px(12.)).text_color(c.muted).child("Absolute, relative or ~ paths. A file path opens its folder and selects it."))
            .child(buttons_row(vec![cancel(cx, &c), ok("Go", Btn::Primary, cx, &c)])),
        Dialog::Server { input } => card(icons::SERVER, c.accent2, "Connect to server", &c)
            .child(input.clone())
            .child(div().text_size(px(12.)).text_color(c.muted).child("smb://, afp://, nfs:// or WebDAV (https://) addresses are mounted by macOS and appear in the drive list."))
            .child(buttons_row(vec![cancel(cx, &c), ok("Connect", Btn::Primary, cx, &c)])),
        Dialog::Compress { items, input, level } => {
            let level = *level;
            let mut levels = div().flex().gap(px(6.));
            for (i, l) in CompressionLevel::ALL.into_iter().enumerate() {
                levels = levels.child(button(
                    SharedString::from(format!("level-{i}")),
                    l.label(),
                    if l == level { Btn::Primary } else { Btn::Normal },
                    &c,
                    move |this, _w, cx| {
                        if let Some(Dialog::Compress { level, .. }) = this.dialog.as_mut() {
                            *level = l;
                        }
                        cx.notify();
                    },
                    cx,
                ));
            }
            card(icons::ZIP, c.accent, format!("Compress {} to ZIP", count_label(items)), &c)
                .child(input.clone())
                .child(div().flex().items_center().gap(px(10.)).child(div().text_color(c.muted).child("Compression level")).child(levels))
                .child(buttons_row(vec![cancel(cx, &c), ok("Compress", Btn::Primary, cx, &c)]))
        }
        Dialog::CloseLockedTab { .. } => card(icons::LOCK, c.accent, "This tab is locked", &c)
            .child("Close it anyway?")
            .child(buttons_row(vec![cancel(cx, &c), ok("Close", Btn::Primary, cx, &c)])),
        Dialog::QuitWithJobs => card(icons::WARNING, c.danger, "File operations are still running", &c)
            .child("Quitting now stops them. Quit anyway?")
            .child(buttons_row(vec![cancel(cx, &c), ok("Quit", Btn::Danger, cx, &c)])),
        Dialog::Message { title, body } => card(icons::SPARKLE, c.accent, title.clone(), &c)
            .child(body.clone())
            .child(buttons_row(vec![ok("OK", Btn::Primary, cx, &c)])),
        Dialog::Queue => render_queue(ws, &c, cx),
        Dialog::Info(state) => render_info(state, &c, cx),
    };

    Some(
        overlay(
            pop(
                div()
                    .id("dialog")
                    .key_context("Dialog")
                    .track_focus(&ws.dialog_focus)
                    .on_action(cx.listener(|this, _: &ui::Confirm, w, cx| this.confirm_dialog(w, cx)))
                    .on_action(cx.listener(|this, _: &ui::Cancel, w, cx| this.close_dialog(w, cx)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
                ws.dialog_gen,
            ),
            &c,
        )
        .into_any_element(),
    )
}

fn render_queue(ws: &Workspace, c: &Colors, cx: &mut Context<Workspace>) -> gpui::Div {
    let mut list = div().flex().flex_col().gap(px(8.)).max_h(px(380.)).overflow_hidden();
    if ws.jobs.jobs.is_empty() {
        list = list.child(div().text_color(c.muted).child("The queue is empty."));
    }
    for job in &ws.jobs.jobs {
        let id = job.id;
        let state = match &job.state {
            JobState::Queued => "Waiting…".to_string(),
            JobState::Running => format!("{:.0}%", job.progress.fraction() * 100.0),
            JobState::WaitingForAnswer => "Needs your answer".to_string(),
            JobState::Done(r) => format!("Done ({} item(s){})", r.done, if r.skipped + r.failed > 0 { format!(", {} skipped", r.skipped + r.failed) } else { String::new() }),
            JobState::Failed(e) => format!("Failed: {e}"),
            JobState::Cancelled => "Cancelled".to_string(),
        };
        let active = !job.state.is_finished();
        list = list.child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(c.border)
                .child(div().flex().justify_between().gap(px(8.)).child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().child(job.title.clone())).child(div().text_color(c.muted).text_size(px(12.)).child(state)))
                .when(active, |d| {
                    d.child(progress_bar(job.progress.fraction(), c)).child(
                        div().flex().justify_end().child(button(SharedString::from(format!("cancel-job-{id}")), "Cancel", Btn::Normal, c, move |_this, _w, cx| {
                            cx.global::<crate::state::AppState>().jobs.cancel(id);
                        }, cx)),
                    )
                }),
        );
    }
    card(icons::QUEUE, c.accent, "Operations queue", c).child(list).child(buttons_row(vec![
        button("clear", "Clear Finished", Btn::Normal, c, |this, _w, cx| {
            cx.global::<crate::state::AppState>().jobs.clear_finished();
            this.jobs = cx.global::<crate::state::AppState>().jobs.snapshot();
            cx.notify();
        }, cx),
        button("ok", "Close", Btn::Primary, c, |this, w, cx| this.close_dialog(w, cx), cx),
    ]))
}

pub fn progress_bar(fraction: f32, c: &Colors) -> gpui::Div {
    div()
        .h(px(6.))
        .w_full()
        .rounded(px(3.))
        .bg(c.border)
        .child(div().h_full().rounded(px(3.)).w(gpui::relative(fraction.clamp(0.0, 1.0))).bg(gpui::linear_gradient(90., gpui::linear_color_stop(c.accent, 0.), gpui::linear_color_stop(c.accent2, 1.))))
}

fn render_info(state: &InfoState, c: &Colors, cx: &mut Context<Workspace>) -> gpui::Div {
    let e = &state.entry;
    let size_line = if e.is_dir_like() {
        match *state.size.lock().unwrap() {
            Some((bytes, files, dirs)) => format!(
                "{} ({} bytes) — {files} file(s), {dirs} folder(s)",
                format::format_size(bytes, SizeMode::Dynamic),
                format::group_digits(bytes)
            ),
            None => "Calculating…".into(),
        }
    } else {
        format!("{} ({} bytes)", format::format_size(e.size, SizeMode::Dynamic), format::group_digits(e.size))
    };
    let row = |label: &str, value: String| {
        div()
            .flex()
            .gap(px(10.))
            .child(div().w(px(90.)).flex_none().text_color(c.muted).child(label.to_string()))
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().child(value))
    };
    let where_ = e.path.parent().map(|p| p.display().to_string()).unwrap_or_default();

    // Permission grid: owner/group/everyone × read/write/execute.
    let mut grid = div().flex().flex_col().gap(px(4.));
    for (row_ix, who) in ["Owner", "Group", "Everyone"].iter().enumerate() {
        let mut line = div().flex().items_center().gap(px(8.)).child(div().w(px(90.)).text_color(c.muted).child(who.to_string()));
        for (col_ix, what) in ["Read", "Write", "Execute"].iter().enumerate() {
            let bit = 0o400u32 >> (row_ix * 3 + col_ix);
            let on = state.mode & bit != 0;
            line = line.child(toggle(SharedString::from(format!("perm-{row_ix}-{col_ix}")), what, on, c, move |this, _w, cx| {
                if let Some(Dialog::Info(s)) = this.dialog.as_mut() {
                    let mode = s.mode ^ bit;
                    match ops::set_mode(&s.entry.path, mode) {
                        Ok(()) => s.mode = (s.mode & !0o7777) | (mode & 0o7777),
                        Err(e) => this.notify_toast(format!("Couldn't change permissions: {e}"), true),
                    }
                }
                cx.notify();
            }, cx));
        }
        grid = grid.child(line);
    }
    let locked = state.locked;

    card(look::kind_of(e).icon(), look::kind_of(e).tint(c), format!("Info — {}", e.name), c)
        .child(row("Kind", e.kind_label()))
        .child(row("Size", size_line))
        .child(row("Where", where_))
        .child(row("Created", format::format_time_long(e.created)))
        .child(row("Modified", format::format_time_long(e.modified)))
        .child(row("Opened", format::format_time_long(e.accessed)))
        .child(row("Owner", format!("{} (group {})", state.owner, state.group)))
        .child(row("Permissions", format!("{}  ({})", format::permissions_string(state.mode, e.kind), format::octal_mode(state.mode))))
        .child(grid)
        .child(toggle("locked", "Locked", locked, c, |this, _w, cx| {
            if let Some(Dialog::Info(s)) = this.dialog.as_mut() {
                match ops::set_locked(&s.entry.path, !s.locked) {
                    Ok(()) => s.locked = !s.locked,
                    Err(e) => this.notify_toast(format!("Couldn't change the lock: {e}"), true),
                }
            }
            cx.notify();
        }, cx))
        .child(buttons_row(vec![button("ok", "Done", Btn::Primary, c, |this, w, cx| {
            this.close_dialog(w, cx);
            this.reload_all(cx);
        }, cx)]))
}

pub fn toggle(
    id: impl Into<SharedString>,
    label: &str,
    on: bool,
    c: &Colors,
    on_click: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let id: SharedString = id.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
            cx.stop_propagation();
            on_click(this, w, cx)
        }))
        .child(
            div()
                .size(px(14.))
                .rounded(px(3.))
                .border_1()
                .border_color(if on { c.accent } else { c.border })
                .bg(if on { c.accent } else { c.bg })
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.on_accent)
                .text_size(px(10.))
                .child(if on { "✓" } else { "" }),
        )
        .child(label.to_string())
        .into_any_element()
}

/// Renders the pending job prompt (conflict or error), if any.
pub fn render_prompt(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let (job_id, prompt) = ws.jobs.prompt.clone()?;
    // Animate each distinct question once.
    let prompt_key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        job_id.hash(&mut h);
        format!("{prompt:?}").hash(&mut h);
        h.finish() >> 1
    };
    let c = Colors::get(window, cx);
    let ans = |id: &'static str, label: &'static str, kind: Btn, answer: Answer, cx: &mut Context<Workspace>, c: &Colors| {
        button(id, label, kind, c, move |this, w, cx| this.answer_prompt(answer, w, cx), cx)
    };
    let (body, default_answer, cancel_answer) = match prompt {
        Prompt::Conflict { src, dst, is_dir } => {
            let now = std::time::SystemTime::now();
            let (src_time, dst_time) = (
                std::fs::metadata(&src).and_then(|m| m.modified()).ok(),
                std::fs::metadata(&dst).and_then(|m| m.modified()).ok(),
            );
            let (newer_src, newer_dst) = (src_time > dst_time, dst_time > src_time);
            // One side of the comparison: tile, label, size and date.
            let side = |label: &str, p: &Path, highlight: bool| {
                let meta = std::fs::symlink_metadata(p).ok();
                let detail = meta
                    .as_ref()
                    .map(|m| {
                        let when = format::format_time_friendly(m.modified().ok(), now);
                        if m.is_dir() { format!("Folder · {when}") } else { format!("{} · {when}", format::format_size(m.len(), SizeMode::Dynamic)) }
                    })
                    .unwrap_or_default();
                let entry = fc_core::entry::read_entry(p).ok();
                let kind = entry.as_ref().map(look::kind_of).unwrap_or(look::FileKind::Other);
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(10.))
                    .bg(c.sunken)
                    .border_1()
                    .border_color(if highlight { c.success.opacity(0.6) } else { c.border })
                    .child(look::file_tile(kind, 32., &c, false))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(0.))
                            .child(div().flex().items_center().gap(px(6.)).text_size(px(11.)).text_color(c.muted).child(label.to_uppercase()).when(highlight, |d| d.child(look::chip("newer", &c, true))))
                            .child(div().text_size(px(12.5)).child(detail)),
                    )
            };
            let mut buttons = vec![
                ans("p-cancel", "Cancel", Btn::Normal, Answer::Conflict(ConflictChoice::Cancel), cx, &c),
                ans("p-skip", "Skip", Btn::Normal, Answer::Conflict(ConflictChoice::Skip), cx, &c),
                ans("p-skipall", "Skip All", Btn::Normal, Answer::Conflict(ConflictChoice::SkipAll), cx, &c),
                ans("p-both", "Keep Both", Btn::Primary, Answer::Conflict(ConflictChoice::KeepBoth), cx, &c),
            ];
            if is_dir {
                buttons.push(ans("p-merge", "Merge", Btn::Normal, Answer::Conflict(ConflictChoice::Merge), cx, &c));
            }
            buttons.push(ans("p-over", "Replace", Btn::Danger, Answer::Conflict(ConflictChoice::Overwrite), cx, &c));
            buttons.push(ans("p-overall", "Replace All", Btn::Danger, Answer::Conflict(ConflictChoice::OverwriteAll), cx, &c));
            (
                card(icons::COPY, c.accent2, format!("\"{}\" already exists", name_of(&dst)), &c)
                    .w(px(640.))
                    .max_w(px(720.))
                    .child(div().text_color(c.muted).child(format!("in {}", dst.parent().map(|p| p.display().to_string()).unwrap_or_default())))
                    .child(div().flex().gap(px(10.)).child(side("Existing", &dst, newer_dst)).child(side("Incoming", &src, newer_src)))
                    .child(div().flex().flex_wrap().justify_end().gap(px(6.)).children(buttons)),
                Answer::Conflict(ConflictChoice::KeepBoth),
                Answer::Conflict(ConflictChoice::Cancel),
            )
        }
        Prompt::Error { path, message } => (
            card(icons::WARNING, c.danger, "Couldn't complete the operation", &c)
                .child(format!("\"{}\": {message}", path.display()))
                .child(buttons_row(vec![
                    ans("e-abort", "Abort", Btn::Normal, Answer::Error(ErrorChoice::Abort), cx, &c),
                    ans("e-skipall", "Skip All", Btn::Normal, Answer::Error(ErrorChoice::SkipAll), cx, &c),
                    ans("e-skip", "Skip", Btn::Normal, Answer::Error(ErrorChoice::Skip), cx, &c),
                    ans("e-retry", "Retry", Btn::Primary, Answer::Error(ErrorChoice::Retry), cx, &c),
                ])),
            Answer::Error(ErrorChoice::Retry),
            Answer::Error(ErrorChoice::Abort),
        ),
    };
    Some(
        overlay(
            pop(
                div()
                    .id("job-prompt")
                    .key_context("Dialog")
                    .track_focus(&ws.prompt_focus)
                    .on_action(cx.listener(move |this, _: &ui::Confirm, w, cx| this.answer_prompt(default_answer, w, cx)))
                    .on_action(cx.listener(move |this, _: &ui::Cancel, w, cx| this.answer_prompt(cancel_answer, w, cx)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
                prompt_key,
            ),
            &c,
        )
        .into_any_element(),
    )
}
