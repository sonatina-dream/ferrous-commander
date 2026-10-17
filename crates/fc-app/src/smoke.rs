//! End-to-end smoke run (`FC_SMOKE=1`): drives the real window through view modes,
//! dialogs, menus, jobs, prompts and secondary windows inside a sandbox folder,
//! checks the results and quits. Output goes to stderr; the last line is
//! `SMOKE OK` or `SMOKE FAIL: …`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{point, px, App, AsyncApp, WindowHandle};

use fc_core::keymap::Action;
use fc_core::queue::Answer;
use fc_core::ops::ConflictChoice;
use fc_core::ViewMode;

use crate::dialogs::Dialog;
use crate::state::AppState;
use crate::workspace::Workspace;

fn sandbox() -> PathBuf {
    let base = std::env::var_os("FC_SMOKE_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("fc-smoke"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("left/sub")).unwrap();
    std::fs::create_dir_all(base.join("right")).unwrap();
    std::fs::write(base.join("left/a.txt"), "alpha\nbeta\n").unwrap();
    std::fs::write(base.join("left/b.txt"), "bravo").unwrap();
    std::fs::write(base.join("left/c.md"), "# charlie").unwrap();
    std::fs::write(base.join("left/.hidden"), "h").unwrap();
    std::fs::write(base.join("left/sub/deep.txt"), "deep").unwrap();
    base
}

pub fn run(cx: &mut App) {
    cx.set_global(crate::probe::Probes::default());
    // The script works with both panels (settings live in a throw-away folder).
    cx.global_mut::<AppState>().settings.view.show_right_panel = true;
    cx.refresh_windows();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = script(cx).await;
        match &result {
            Ok(()) => eprintln!("SMOKE OK"),
            Err(e) => eprintln!("SMOKE FAIL: {e}"),
        }
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}

pub(crate) async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

pub(crate) fn find_workspace(cx: &mut App) -> Option<WindowHandle<Workspace>> {
    cx.windows().into_iter().find_map(|w| w.downcast::<Workspace>())
}

/// Runs `f` against the workspace and lets a frame render afterwards.
pub(crate) async fn step<R: 'static>(
    cx: &mut AsyncApp,
    name: &str,
    f: impl FnOnce(&mut Workspace, &mut gpui::Window, &mut gpui::Context<Workspace>) -> R,
) -> Result<R, String> {
    eprintln!("smoke: {name}");
    let handle = cx.update(find_workspace).map_err(|e| e.to_string())?.ok_or("no workspace window")?;
    let r = handle
        .update(cx, |ws, window, cx| {
            let r = f(ws, window, cx);
            cx.notify();
            r
        })
        .map_err(|e| format!("{name}: {e}"))?;
    pause(cx, 220).await;
    Ok(r)
}

pub(crate) async fn wait_jobs(cx: &mut AsyncApp) -> Result<(), String> {
    for _ in 0..100 {
        pause(cx, 100).await;
        let idle = cx.update(|cx| !cx.global::<AppState>().jobs.has_active()).map_err(|e| e.to_string())?;
        if idle {
            pause(cx, 300).await; // let the tick reload panels
            return Ok(());
        }
    }
    Err("jobs did not finish".into())
}

pub(crate) async fn wait_prompt(cx: &mut AsyncApp) -> Result<(), String> {
    for _ in 0..100 {
        pause(cx, 100).await;
        let has = cx.update(|cx| cx.global::<AppState>().jobs.snapshot().prompt.is_some()).map_err(|e| e.to_string())?;
        if has {
            pause(cx, 300).await;
            return Ok(());
        }
    }
    Err("no conflict prompt appeared".into())
}

fn check(cond: bool, what: &str) -> Result<(), String> {
    if cond { Ok(()) } else { Err(what.to_string()) }
}

fn exists(p: &Path) -> bool {
    p.exists()
}

async fn script(cx: &mut AsyncApp) -> Result<(), String> {
    pause(cx, 800).await;
    // The first frames render synchronously when the window opens (home folder). Later
    // frames need the display link, which macOS pauses while the display sleeps.
    let probe = step(cx, "layout probe", |ws, _, cx| {
        let b = ws.list_bounds[0].get();
        let (w, h): (f32, f32) = (b.size.width.into(), b.size.height.into());
        let visible = (h / Workspace::row_height(cx)).ceil() as usize;
        let expected = [0, 1].map(|s| ws.panels[s].tab().entries.len().min(visible));
        (w, h, ws.rendered_rows.get(), ws.frames.get(), expected)
    })
    .await?;
    eprintln!("smoke: left list {:.0}x{:.0} px, rows rendered {:?} (expected {:?}), frames {}", probe.0, probe.1, probe.2, probe.4, probe.3);
    check(probe.0 > 200.0 && probe.1 > 150.0, "file list has no usable size")?;
    check(probe.2.iter().zip(probe.4.iter()).all(|(got, want)| got.abs_diff(*want) <= 1), "rendered row count does not match the viewport")?;
    let base = sandbox();
    let (left, right) = (base.join("left"), base.join("right"));

    {
        let (l, r) = (left.clone(), right.clone());
        step(cx, "navigate both panels", move |ws, w, cx| {
            // GPUI stops drawing occluded windows; bring this one forward.
            cx.activate(true);
            w.activate_window();
            ws.active = 0;
            ws.navigate(0, l, None, w, cx);
            ws.navigate(1, r, None, w, cx);
        })
        .await?;
    }
    let names = step(cx, "list", |ws, _, _| ws.panels[0].tab().entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>()).await?;
    check(names == ["sub", "a.txt", "b.txt", "c.md"], &format!("unexpected listing {names:?}"))?;
    for (action, mode) in [(Action::ViewModeBrief, ViewMode::Brief), (Action::ViewModeThumbs, ViewMode::Thumbs), (Action::ViewModeFull, ViewMode::Full)] {
        let m = step(cx, &format!("{action:?}"), move |ws, w, cx| {
            ws.run(action, w, cx);
            ws.panels[0].tab().view_mode
        })
        .await?;
        check(m == mode, "view mode did not change")?;
    }

    let n = step(cx, "select all", |ws, w, cx| {
        ws.run(Action::SelectAll, w, cx);
        ws.panels[0].tab().targets().len()
    })
    .await?;
    check(n == 4, &format!("select all selected {n}"))?;
    step(cx, "unselect all", |ws, w, cx| ws.run(Action::UnselectAll, w, cx)).await?;

    let tabs = step(cx, "tabs", |ws, w, cx| {
        ws.run(Action::NewTab, w, cx);
        ws.run(Action::NextTab, w, cx);
        ws.run(Action::PrevTab, w, cx);
        let n = ws.panels[0].tabs.len();
        ws.run(Action::CloseTab, w, cx);
        (n, ws.panels[0].tabs.len())
    })
    .await?;
    check(tabs == (2, 1), &format!("tab counts {tabs:?}"))?;

    let hidden = step(cx, "toggle hidden", |ws, w, cx| {
        ws.run(Action::ToggleHidden, w, cx);
        let shown = ws.panels[0].tab().entries.iter().any(|e| e.name == ".hidden");
        ws.run(Action::ToggleHidden, w, cx);
        shown
    })
    .await?;
    check(hidden, "hidden file not shown after toggle")?;

    step(cx, "sort by size and back", |ws, w, cx| {
        ws.run(Action::SortBySize, w, cx);
        ws.run(Action::SortByName, w, cx);
    })
    .await?;

    let selected = step(cx, "select by pattern", |ws, w, cx| {
        ws.run(Action::Select, w, cx);
        if let Some(Dialog::Pattern { input, .. }) = &ws.dialog {
            input.update(cx, |i, cx| i.set_text("*.txt", cx));
        }
        ws.confirm_dialog(w, cx);
        ws.panels[0].tab().targets().len()
    })
    .await?;
    check(selected == 2, &format!("pattern selected {selected}"))?;

    step(cx, "copy to other panel", |ws, w, cx| {
        ws.run(Action::Copy, w, cx);
        ws.confirm_dialog(w, cx);
    })
    .await?;
    wait_jobs(cx).await?;
    check(exists(&right.join("a.txt")) && exists(&right.join("b.txt")), "copy did not produce files")?;

    step(cx, "copy again → conflict", |ws, w, cx| {
        let ix = ws.panels[0].tab().index_of(&ws.panels[0].tab().working_dir().join("a.txt")).unwrap_or(0);
        ws.panels[0].tab_mut().unselect_all();
        ws.panels[0].tab_mut().set_cursor(ix);
        ws.run(Action::Copy, w, cx);
        ws.confirm_dialog(w, cx);
    })
    .await?;
    wait_prompt(cx).await?;
    step(cx, "answer keep both", |ws, w, cx| ws.answer_prompt(Answer::Conflict(ConflictChoice::KeepBoth), w, cx)).await?;
    wait_jobs(cx).await?;
    check(exists(&right.join("a 2.txt")), "keep both did not create 'a 2.txt'")?;

    step(cx, "context menu", |ws, w, cx| {
        let cursor = ws.panels[0].tab().cursor;
        ws.row_right_click(0, cursor, point(px(200.), px(200.)), w, cx);
    })
    .await?;
    step(cx, "close menu", |ws, w, cx| ws.close_menu(w, cx)).await?;
    step(cx, "drive menu", |ws, w, cx| ws.run(Action::ChangeDriveLeft, w, cx)).await?;
    step(cx, "favorites menu", |ws, w, cx| {
        ws.close_menu(w, cx);
        ws.run(Action::ShowFavorites, w, cx);
    })
    .await?;
    step(cx, "queue dialog", |ws, w, cx| {
        ws.close_menu(w, cx);
        ws.run(Action::ShowQueue, w, cx);
    })
    .await?;
    step(cx, "get info", |ws, w, cx| {
        ws.close_dialog(w, cx);
        ws.run(Action::ShowInfo, w, cx);
    })
    .await?;
    step(cx, "go to folder dialog", |ws, w, cx| {
        ws.close_dialog(w, cx);
        ws.run(Action::GoFolder, w, cx);
    })
    .await?;

    step(cx, "rename", |ws, w, cx| {
        ws.close_dialog(w, cx);
        ws.run(Action::Rename, w, cx);
        if let Some(r) = &ws.rename {
            r.input.update(cx, |i, cx| i.set_text("renamed.txt", cx));
        }
        ws.commit_rename(w, cx);
    })
    .await?;
    check(exists(&left.join("renamed.txt")) && !exists(&left.join("a.txt")), "rename failed")?;

    step(cx, "new nested folder", |ws, w, cx| {
        ws.run(Action::NewFolder, w, cx);
        if let Some(Dialog::NewFolder { input, .. }) = &ws.dialog {
            input.update(cx, |i, cx| i.set_text("made/nested", cx));
        }
        ws.confirm_dialog(w, cx);
    })
    .await?;
    check(left.join("made/nested").is_dir(), "new folder failed")?;

    step(cx, "delete permanently", |ws, w, cx| {
        let tab = ws.panels[0].tab_mut();
        let ix = tab.index_of(&tab.working_dir().join("made")).unwrap_or(0);
        tab.set_cursor(ix);
        ws.run(Action::DeleteForce, w, cx);
        ws.confirm_dialog(w, cx);
    })
    .await?;
    wait_jobs(cx).await?;
    check(!exists(&left.join("made")), "delete failed")?;

    step(cx, "compress and extract", |ws, w, cx| {
        let tab = ws.panels[0].tab_mut();
        let ix = tab.index_of(&tab.working_dir().join("sub")).unwrap_or(0);
        tab.set_cursor(ix);
        ws.run(Action::Compress, w, cx);
    })
    .await?;
    wait_jobs(cx).await?;
    check(exists(&right.join("sub.zip")), "compress failed")?;

    step(cx, "view file in internal viewer", |ws, w, cx| {
        let tab = ws.panels[0].tab_mut();
        let ix = tab.index_of(&tab.working_dir().join("b.txt")).unwrap_or(0);
        tab.set_cursor(ix);
        ws.run(Action::View, w, cx);
    })
    .await?;
    pause(cx, 400).await;
    step(cx, "search window", |ws, w, cx| ws.run(Action::Search, w, cx)).await?;
    pause(cx, 400).await;
    step(cx, "feed results", |ws, w, cx| {
        let root = ws.panels[0].tab().working_dir().to_path_buf();
        ws.feed_results(vec![root.join("b.txt"), root.join("sub/deep.txt")], root, w, cx);
        ws.panels[0].tab().entries.len()
    })
    .await
    .and_then(|n| check(n == 2, &format!("feed shows {n} entries")))?;
    step(cx, "back from results", |ws, w, cx| ws.run(Action::GoTop, w, cx)).await?;
    step(cx, "preferences", |_ws, _w, cx| crate::prefs::open(crate::prefs::Page::Hotkeys, cx)).await?;
    pause(cx, 400).await;
    let swapped = step(cx, "swap and equalize", |ws, w, cx| {
        ws.run(Action::SwapPanels, w, cx);
        ws.run(Action::EqualizePanels, w, cx);
        ws.panels[1].tab().working_dir().to_path_buf()
    })
    .await?;
    check(swapped == left, "swap panels failed")?;
    step(cx, "type-ahead", |ws, w, cx| {
        ws.activate_side(1, w, cx);
        ws.panels[1].tab_mut().type_ahead("ren");
        ws.panels[1].tab().cursor_entry().map(|e| e.name.clone())
    })
    .await
    .and_then(|n| check(n.as_deref() == Some("renamed.txt"), &format!("type-ahead landed on {n:?}")))?;

    palette_phase(cx).await?;
    keyboard_phase(cx, &left).await?;
    clipboard_phase(cx, &left, &right).await?;
    rename_phase(cx, &left).await?;
    mouse_phase(cx, &left).await?;
    panels_phase(cx).await?;
    terminal_phase(cx, &left).await?;
    layout_phase(cx).await?;

    let windows = cx.update(|cx| cx.windows().len()).map_err(|e| e.to_string())?;
    eprintln!("smoke: {windows} window(s) open");
    let _ = std::fs::remove_dir_all(&base);
    Ok(())
}

/// Sends real keystrokes through GPUI's dispatcher (bindings, contexts, focus).
pub(crate) async fn keys(cx: &mut AsyncApp, list: &[&str]) -> Result<(), String> {
    let handle = cx.update(find_workspace).map_err(|e| e.to_string())?.ok_or("no workspace window")?;
    // The untyped handle does not lease the root view, so actions can update it.
    let handle: gpui::AnyWindowHandle = handle.into();
    for k in list {
        let ks = gpui::Keystroke::parse(k).map_err(|e| format!("bad keystroke {k}: {e:?}"))?;
        handle
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(ks, cx);
            })
            .map_err(|e| e.to_string())?;
        pause(cx, 120).await;
    }
    pause(cx, 150).await;
    Ok(())
}

async fn keyboard_phase(cx: &mut AsyncApp, left: &Path) -> Result<(), String> {
    eprintln!("smoke: keyboard phase");
    // Left sandbox is now in the right panel (after swap); focus it.
    let l = left.to_path_buf();
    step(cx, "focus panels", move |ws, w, cx| {
        ws.activate_side(1, w, cx);
        ws.navigate(1, l, None, w, cx);
        ws.panels[1].tab_mut().set_cursor(0);
    })
    .await?;

    keys(cx, &["down", "down"]).await?;
    let cursor = step(cx, "read cursor", |ws, _, _| ws.panels[1].tab().cursor).await?;
    check(cursor == 2, &format!("arrow keys moved cursor to {cursor}"))?;

    keys(cx, &["space"]).await?;
    let (sel, cursor) = step(cx, "read selection", |ws, _, _| (ws.panels[1].tab().selected.len(), ws.panels[1].tab().cursor)).await?;
    check(sel == 1 && cursor == 3, &format!("space: selected {sel}, cursor {cursor}"))?;

    keys(cx, &["tab"]).await?;
    let active = step(cx, "read active", |ws, _, _| ws.active).await?;
    check(active == 0, "tab did not switch panels")?;
    keys(cx, &["tab", "cmd-2"]).await?;
    let mode = step(cx, "read mode", |ws, _, _| ws.panels[1].tab().view_mode).await?;
    check(mode == ViewMode::Brief, "cmd-2 did not switch to brief")?;
    keys(cx, &["cmd-1"]).await?;

    keys(cx, &["f7"]).await?;
    let open = step(cx, "f7 opens dialog", |ws, _, _| matches!(ws.dialog, Some(Dialog::NewFolder { .. }))).await?;
    check(open, "F7 did not open the new-folder dialog")?;
    keys(cx, &["escape"]).await?;
    let closed = step(cx, "escape closes dialog", |ws, _, _| ws.dialog.is_none()).await?;
    check(closed, "Escape did not close the dialog")?;

    keys(cx, &["+"]).await?;
    let open = step(cx, "num+ opens pattern dialog", |ws, _, _| matches!(ws.dialog, Some(Dialog::Pattern { select: true, .. }))).await?;
    check(open, "+ did not open the select-by-pattern dialog")?;
    keys(cx, &["*", ".", "m", "d", "enter"]).await?;
    let names = step(cx, "pattern result", |ws, _, _| {
        let mut v: Vec<String> = ws.panels[1].tab().targets().into_iter().map(|e| e.name).collect();
        v.sort();
        (ws.dialog.is_none(), v)
    })
    .await?;
    check(names.0 && names.1.contains(&"c.md".to_string()), &format!("typed mask selected {:?}", names.1))?;
    keys(cx, &["escape", "escape"]).await?; // clear type-ahead, then selection

    keys(cx, &["r", "e", "n"]).await?;
    let name = step(cx, "type-ahead via keys", |ws, _, _| ws.panels[1].tab().cursor_entry().map(|e| e.name.clone())).await?;
    check(name.as_deref() == Some("renamed.txt"), &format!("type-ahead keys landed on {name:?}"))?;

    keys(cx, &["home", "enter"]).await?;
    let dir = step(cx, "enter folder", |ws, _, _| ws.panels[1].tab().working_dir().to_path_buf()).await?;
    check(dir.ends_with("left/sub"), &format!("Return entered {}", dir.display()))?;
    keys(cx, &["backspace"]).await?;
    let (dir, focus) = step(cx, "backspace goes up", |ws, _, _| (ws.panels[1].tab().working_dir().to_path_buf(), ws.panels[1].tab().cursor_entry().map(|e| e.name.clone()))).await?;
    check(dir.ends_with("left") && focus.as_deref() == Some("sub"), "Backspace did not go up to the parent with the cursor on the child")?;

    keys(cx, &["shift-cmd-t", "ctrl-tab"]).await?;
    let tabs = step(cx, "new tab via keys", |ws, _, _| ws.panels[1].tabs.len()).await?;
    check(tabs == 2, &format!("shift-cmd-T made {tabs} tabs"))?;
    keys(cx, &["cmd-w"]).await?;
    Ok(())
}

async fn palette_phase(cx: &mut AsyncApp) -> Result<(), String> {
    use crate::palette::PaletteItem;
    let first = step(cx, "palette: fuzzy command", |ws, w, cx| {
        ws.open_palette(w, cx);
        if let Some(p) = &ws.palette {
            p.input.update(cx, |i, cx| i.set_text("new fol", cx));
        }
        ws.refresh_palette(cx);
        ws.palette.as_ref().and_then(|p| p.items.first().cloned())
    })
    .await?;
    check(first == Some(PaletteItem::Action(Action::NewFolder)), &format!("'new fol' ranked {first:?} first"))?;
    let opened = step(cx, "palette: run command", |ws, w, cx| {
        ws.palette_confirm(w, cx);
        (ws.palette.is_none(), matches!(ws.dialog, Some(Dialog::NewFolder { .. })))
    })
    .await?;
    check(opened == (true, true), "running a palette command did not open its dialog")?;
    step(cx, "close dialog", |ws, w, cx| ws.close_dialog(w, cx)).await?;

    let items = step(cx, "palette: path and shell modes", |ws, w, cx| {
        ws.open_palette(w, cx);
        let mut out = Vec::new();
        for q in ["~", ">echo hi", "zzzz-no-match"] {
            if let Some(p) = &ws.palette {
                p.input.update(cx, |i, cx| i.set_text(q, cx));
            }
            ws.refresh_palette(cx);
            out.push(ws.palette.as_ref().and_then(|p| p.items.first().cloned()));
        }
        ws.close_palette(w, cx);
        out
    })
    .await?;
    check(matches!(&items[0], Some(PaletteItem::OpenPath(_))), &format!("'~' gave {:?}", items[0]))?;
    check(items[1] == Some(PaletteItem::Shell("echo hi".into())), &format!("'>echo hi' gave {:?}", items[1]))?;
    check(matches!(&items[2], Some(PaletteItem::Find(_))), &format!("no-match query gave {:?}", items[2]))?;

    // Through real keystrokes: ⌘P, type, ↩.
    let hidden_before = cx.update(|cx| cx.global::<AppState>().settings.view.show_hidden).map_err(|e| e.to_string())?;
    keys(cx, &["cmd-p"]).await?;
    let open = step(cx, "cmd-p opens palette", |ws, _, _| ws.palette.is_some()).await?;
    check(open, "⌘P did not open the palette")?;
    keys(cx, &["h", "i", "d", "d", "e", "n"]).await?;
    let top = step(cx, "palette ranks typed query", |ws, _, _| ws.palette.as_ref().and_then(|p| p.items.first().cloned())).await?;
    check(top == Some(PaletteItem::Action(Action::ToggleHidden)), &format!("typing 'hidden' ranked {top:?} first"))?;
    keys(cx, &["enter"]).await?;
    let hidden_after = cx.update(|cx| cx.global::<AppState>().settings.view.show_hidden).map_err(|e| e.to_string())?;
    check(hidden_after != hidden_before, "↩ in the palette did not run the command")?;
    step(cx, "restore hidden", |ws, w, cx| ws.run(Action::ToggleHidden, w, cx)).await?;
    keys(cx, &["cmd-p", "escape"]).await?;
    let closed = step(cx, "escape closes palette", |ws, _, _| ws.palette.is_none()).await?;
    check(closed, "Esc did not close the palette")?;
    Ok(())
}

/// ⌘C / ⌘X / ⌘V / ⌥⌘⌫ through real keystrokes (on a private pasteboard).
async fn clipboard_phase(cx: &mut AsyncApp, left: &Path, right: &Path) -> Result<(), String> {
    eprintln!("smoke: clipboard phase");
    let (l, r) = (left.to_path_buf(), right.to_path_buf());
    let put_cursor = |side: usize, path: PathBuf| {
        move |ws: &mut Workspace, w: &mut gpui::Window, cx: &mut gpui::Context<Workspace>| {
            let _ = ws.panels[side].tab_mut().reload(Workspace::opts(cx));
            ws.activate_side(side, w, cx);
            let tab = ws.panels[side].tab_mut();
            tab.unselect_all();
            let ix = tab.index_of(&path);
            if let Some(ix) = ix {
                tab.set_cursor(ix);
            }
            ix.is_some()
        }
    };
    {
        let (l2, r2) = (l.clone(), r.clone());
        step(cx, "clipboard: prepare panels", move |ws, w, cx| {
            ws.navigate(1, l2, None, w, cx);
            ws.navigate(0, r2, None, w, cx);
        })
        .await?;
    }
    check(step(cx, "cursor on c.md", put_cursor(1, l.join("c.md"))).await?, "c.md not listed")?;
    keys(cx, &["cmd-c", "tab", "cmd-v"]).await?;
    wait_jobs(cx).await?;
    check(exists(&r.join("c.md")) && exists(&l.join("c.md")), "⌘C then ⌘V in the other panel did not copy")?;

    keys(cx, &["tab", "cmd-v"]).await?;
    wait_jobs(cx).await?;
    check(exists(&l.join("c copy.md")), "⌘V in the file's own folder did not duplicate it")?;

    check(step(cx, "cursor on the copy", put_cursor(1, l.join("c copy.md"))).await?, "'c copy.md' not listed")?;
    keys(cx, &["cmd-x"]).await?;
    let cut = cx.update(|cx| cx.global::<AppState>().cut.is_some()).map_err(|e| e.to_string())?;
    check(cut, "⌘X did not mark items as cut")?;
    keys(cx, &["tab", "cmd-v"]).await?;
    wait_jobs(cx).await?;
    check(exists(&r.join("c copy.md")) && !exists(&l.join("c copy.md")), "⌘X then ⌘V did not move the file")?;
    let cut = cx.update(|cx| cx.global::<AppState>().cut.is_some()).map_err(|e| e.to_string())?;
    check(!cut, "cut state not cleared after the move")?;

    check(step(cx, "cursor on moved file", put_cursor(0, r.join("c copy.md"))).await?, "moved file not listed")?;
    keys(cx, &["alt-cmd-backspace"]).await?;
    let asked = step(cx, "delete asks first", |ws, _, _| matches!(ws.dialog, Some(Dialog::Delete { permanent: true, .. }))).await?;
    check(asked, "⌥⌘⌫ did not ask to delete permanently")?;
    keys(cx, &["enter"]).await?;
    wait_jobs(cx).await?;
    check(!exists(&r.join("c copy.md")), "⌥⌘⌫ + ↩ did not delete the file")?;
    Ok(())
}

/// With `FC_SMOKE_SHOTS=<dir>`, saves the main window as `<dir>/<name>.png` for review.
async fn shot(cx: &mut AsyncApp, name: &str) {
    let Some(dir) = std::env::var_os("FC_SMOKE_SHOTS").map(PathBuf::from) else { return };
    pause(cx, 350).await;
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{name}.png"));
    #[cfg(target_os = "macos")]
    let _ = cx.update(|cx| {
        if let Some(h) = find_workspace(cx) {
            let _ = h.update(cx, |_, window, _| {
                if let Some(frame) = crate::capture::window_number(window).and_then(|id| crate::capture::capture(id, true)) {
                    frame.save_png(&path);
                }
            });
        }
    });
    eprintln!("smoke:   screenshot {}", path.display());
}

/// Puts the cursor of `side` on `dir/name` (navigating there first); false if not listed.
fn cursor_to(side: usize, dir: PathBuf, name: &'static str) -> impl FnOnce(&mut Workspace, &mut gpui::Window, &mut gpui::Context<Workspace>) -> bool {
    move |ws, w, cx| {
        if ws.panels[side].tab().working_dir() != dir {
            ws.navigate(side, dir.clone(), None, w, cx);
        }
        let _ = ws.panels[side].tab_mut().reload(Workspace::opts(cx));
        ws.activate_side(side, w, cx);
        let tab = ws.panels[side].tab_mut();
        tab.unselect_all();
        let ix = tab.index_of(&dir.join(name));
        if let Some(ix) = ix {
            tab.set_cursor(ix);
        }
        ix.is_some()
    }
}

/// F2 renames in place: typing (including keys that are panel shortcuts), ↩, Esc, blur.
async fn rename_phase(cx: &mut AsyncApp, left: &Path) -> Result<(), String> {
    eprintln!("smoke: rename phase");
    let l = left.to_path_buf();
    check(step(cx, "cursor on b.txt", cursor_to(0, l.clone(), "b.txt")).await?, "b.txt not listed")?;
    keys(cx, &["f2"]).await?;
    let editing = step(cx, "f2 edits in place", |ws, _, _| ws.rename.is_some() && ws.dialog.is_none()).await?;
    check(editing, "F2 did not start renaming in place")?;
    shot(cx, "rename-in-place").await;
    // The stem is selected, so typing replaces it; Space and ⌫ must edit, not select or go up.
    keys(cx, &["n", "e", "w", "space", "n", "a", "m", "e", "backspace", "e", "enter"]).await?;
    check(exists(&l.join("new name.txt")) && !exists(&l.join("b.txt")), "typing a name and ↩ did not rename b.txt to 'new name.txt'")?;
    let (done, focused, cursor, selected) = step(cx, "rename finished", |ws, w, _| {
        let tab = ws.panels[0].tab();
        (ws.rename.is_none(), ws.focus.is_focused(w), tab.cursor_entry().map(|e| e.name.clone()), tab.selected.len())
    })
    .await?;
    check(done && focused, "the panels did not get the focus back after renaming")?;
    check(cursor.as_deref() == Some("new name.txt") && selected == 0, &format!("after renaming the cursor is on {cursor:?}, {selected} selected"))?;

    keys(cx, &["f2", "z", "z", "escape"]).await?;
    let open = step(cx, "escape cancels", |ws, _, _| ws.rename.is_some()).await?;
    check(!open && exists(&l.join("new name.txt")) && !exists(&l.join("zz.txt")), "Esc did not cancel the rename")?;

    check(step(cx, "cursor on c.md", cursor_to(0, l.clone(), "c.md")).await?, "c.md not listed")?;
    keys(cx, &["f2", "d", "o", "c"]).await?;
    // GPUI reports focus changes only in the key window; other windows of this run
    // (Settings, Search) may have taken that role.
    step(cx, "activate window", |_, w, cx| {
        cx.activate(true);
        w.activate_window();
    })
    .await?;
    pause(cx, 300).await;
    let active = step(cx, "focus moves away", |ws, w, _| {
        let active = w.is_window_active();
        w.focus(&ws.focus);
        active
    })
    .await?;
    if active {
        check(exists(&l.join("doc.md")) && !exists(&l.join("c.md")), "moving the focus away did not commit the new name")?;
    } else {
        eprintln!("smoke:   window is not key; committing directly");
        step(cx, "commit", |ws, w, cx| ws.commit_rename(w, cx)).await?;
        check(exists(&l.join("doc.md")), "committing the edited name failed")?;
    }
    Ok(())
}

fn mouse_down(at: gpui::Point<gpui::Pixels>) -> gpui::MouseDownEvent {
    gpui::MouseDownEvent { button: gpui::MouseButton::Left, position: at, modifiers: Default::default(), click_count: 1, first_mouse: false }
}

/// Where a Full-mode list was drawn.
#[derive(Clone, Copy)]
struct ListGeo {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    row: f32,
}

impl ListGeo {
    /// A point on the blank part of row `ix` (between the name and the columns).
    fn at(&self, ix: usize) -> gpui::Point<gpui::Pixels> {
        point(px(self.x + (self.w * 0.42).min(230.0)), px(self.y + (ix as f32 + 0.5) * self.row))
    }
}

/// Mouse behaviour through the handlers the list's mouse events call: drag-select,
/// clicking empty space, a plain click, and dropping a file onto a folder row.
async fn mouse_phase(cx: &mut AsyncApp, left: &Path) -> Result<(), String> {
    eprintln!("smoke: mouse phase");
    std::fs::write(left.join("drag-me.txt"), "d").map_err(|e| e.to_string())?;
    let l = left.to_path_buf();
    step(cx, "prepare list", move |ws, w, cx| {
        ws.navigate(0, l, None, w, cx);
        let _ = ws.panels[0].tab_mut().reload(Workspace::opts(cx));
        ws.activate_side(0, w, cx);
        ws.run(Action::ViewModeFull, w, cx);
        ws.panels[0].tab_mut().unselect_all();
        ws.panels[0].tab_mut().set_cursor(0);
        ws.reveal_cursor(0);
    })
    .await?;
    pause(cx, 300).await;
    let (geo, names) = step(cx, "read list geometry", |ws, _, cx| {
        let b = ws.list_bounds[0].get();
        let geo = ListGeo { x: b.origin.x.into(), y: b.origin.y.into(), w: b.size.width.into(), h: b.size.height.into(), row: Workspace::row_height(cx) };
        (geo, ws.panels[0].tab().entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>())
    })
    .await?;
    eprintln!("smoke:   list at {:.0},{:.0} {:.0}x{:.0}: {names:?}", geo.x, geo.y, geo.w, geo.h);
    check(names.len() >= 5 && names[0] == "sub", &format!("unexpected listing {names:?}"))?;

    // Press on the blank part of row 1, drag down to row 3.
    step(cx, "drag a selection band", move |ws, w, cx| {
        ws.row_mouse_down(0, 1, false, &mouse_down(geo.at(1)), w, cx);
        ws.mouse_moved(geo.at(2), true, w, cx);
        ws.mouse_moved(point(px(geo.x + 60.0), px(geo.y + 3.4 * geo.row)), true, w, cx);
    })
    .await?;
    shot(cx, "selection-band").await;
    let selected = step(cx, "release the band", move |ws, _, cx| {
        let dragging = ws.marquee.is_some();
        ws.mouse_released(cx);
        (dragging, ws.panels[0].tab().targets().into_iter().map(|e| e.name).collect::<Vec<_>>())
    })
    .await?;
    check(selected.0 && selected.1 == names[1..=3], &format!("dragging over rows 1–3 selected {:?}", selected.1))?;

    // Clicking empty space under the rows clears the selection.
    let below = point(px(geo.x + 120.0), px(geo.y + (names.len() as f32 + 1.5) * geo.row));
    check(f32::from(below.y) < geo.y + geo.h, "no empty space under the rows")?;
    let n = step(cx, "click on empty space", move |ws, w, cx| {
        ws.background_mouse_down(0, &mouse_down(below), w, cx);
        ws.mouse_released(cx);
        ws.panels[0].tab().selected.len()
    })
    .await?;
    check(n == 0, &format!("clicking empty space left {n} item(s) selected"))?;

    // A band dragged out from empty space selects what it crosses.
    let n = step(cx, "band from empty space", move |ws, w, cx| {
        ws.background_mouse_down(0, &mouse_down(below), w, cx);
        ws.mouse_moved(point(px(geo.x + 60.0), px(geo.y + 2.5 * geo.row)), true, w, cx);
        ws.mouse_released(cx);
        ws.panels[0].tab().selected.len()
    })
    .await?;
    check(n == names.len() - 2, &format!("a band from below up to row 2 selected {n} of {}", names.len()))?;

    // Pressing a selected row keeps the selection (to drag it); releasing without moving
    // leaves just that row. Pressing an unselected row selects only it right away.
    let (held, n, cursor) = step(cx, "click a selected row", move |ws, w, cx| {
        ws.run(Action::SelectAll, w, cx);
        ws.row_mouse_down(0, 2, true, &mouse_down(geo.at(2)), w, cx);
        let held = ws.panels[0].tab().selected.len();
        ws.mouse_released(cx);
        (held, ws.panels[0].tab().selected.len(), ws.panels[0].tab().cursor)
    })
    .await?;
    check(held == names.len() && n == 0 && cursor == 2, &format!("click on a selected row: {held} kept while pressed, {n} after release, cursor {cursor}"))?;
    let n = step(cx, "drag a selected row", move |ws, w, cx| {
        ws.run(Action::SelectAll, w, cx);
        ws.row_mouse_down(0, 2, true, &mouse_down(geo.at(2)), w, cx);
        ws.mouse_moved(point(px(geo.x + 60.0), px(geo.y + 4.0 * geo.row)), true, w, cx);
        ws.mouse_released(cx);
        let n = ws.panels[0].tab().selected.len();
        ws.panels[0].tab_mut().select_band(&Default::default(), [1]);
        ws.row_mouse_down(0, 3, true, &mouse_down(geo.at(3)), w, cx);
        (n, ws.panels[0].tab().selected.len())
    })
    .await?;
    check(n == (names.len(), 0), &format!("dragging a selected row kept {} selected; pressing another row kept {}", n.0, n.1))?;

    // Drop a file onto the folder in row 0, and a folder onto itself (ignored).
    let (l, sub) = (left.to_path_buf(), left.join("sub"));
    step(cx, "drop onto a folder", move |ws, w, cx| {
        let files = crate::workspace::DraggedFiles { side: 0, paths: vec![l.join("drag-me.txt")] };
        ws.drop_into_folder(0, sub.clone(), &files, w, cx);
        let itself = crate::workspace::DraggedFiles { side: 0, paths: vec![sub.clone()] };
        ws.drop_into_folder(0, sub.clone(), &itself, w, cx);
    })
    .await?;
    wait_jobs(cx).await?;
    check(exists(&left.join("sub/drag-me.txt")) && !exists(&left.join("drag-me.txt")), "dropping a file onto a folder did not move it there")?;
    check(exists(&left.join("sub")) && !exists(&left.join("sub/sub")), "dropping a folder onto itself did something")?;
    Ok(())
}

/// The right panel can be closed and brought back; the `..` row is optional.
async fn panels_phase(cx: &mut AsyncApp) -> Result<(), String> {
    eprintln!("smoke: panels phase");
    let state = step(cx, "hide right panel", |ws, w, cx| {
        ws.activate_side(1, w, cx);
        ws.run(Action::ToggleRightPanel, w, cx);
        (Workspace::dual(cx), ws.active)
    })
    .await?;
    check(state == (false, 0), &format!("after hiding the right panel: shown {}, active side {}", state.0, state.1))?;
    redraw_probes(cx).await?;
    check(probe(cx, "main.card0").is_ok() && probe(cx, "main.card1").is_err(), "the right panel is still drawn after hiding it")?;
    shot(cx, "one-panel").await;
    keys(cx, &["tab"]).await?;
    let active = step(cx, "tab with one panel", |ws, _, _| ws.active).await?;
    check(active == 0, "Tab switched to the hidden panel")?;

    let (first, any_parent) = step(cx, "parent row setting", |ws, _, cx| {
        cx.global_mut::<AppState>().settings.view.show_parent_entry = true;
        ws.reload_all(cx);
        let first = ws.panels[0].tab().entries.first().map(|e| e.name.clone());
        cx.global_mut::<AppState>().settings.view.show_parent_entry = false;
        ws.reload_all(cx);
        (first, ws.panels[0].tab().entries.iter().any(|e| e.is_parent()))
    })
    .await?;
    check(first.as_deref() == Some("..") && !any_parent, "the '..' row does not follow its setting")?;

    let dual = step(cx, "show right panel", |ws, w, cx| {
        ws.run(Action::ToggleRightPanel, w, cx);
        Workspace::dual(cx)
    })
    .await?;
    check(dual, "the right panel did not come back")?;
    redraw_probes(cx).await?;
    check(probe(cx, "main.card1").is_ok(), "the right panel is not drawn after showing it again")
}

async fn terminal_text(cx: &mut AsyncApp) -> Result<String, String> {
    let handle = cx.update(find_workspace).map_err(|e| e.to_string())?.ok_or("no workspace window")?;
    handle
        .update(cx, |ws, _, cx| match &ws.terminal {
            Some(t) => t.read(cx).session.screen_text(),
            None => format!("(no terminal; shown: {})", ws.terminal_open),
        })
        .map_err(|e| e.to_string())
}

async fn wait_terminal(cx: &mut AsyncApp, what: &str, ok: impl Fn(&str) -> bool) -> Result<(), String> {
    let mut text = String::new();
    for _ in 0..120 {
        pause(cx, 100).await;
        text = terminal_text(cx).await?;
        if ok(&text) {
            return Ok(());
        }
    }
    Err(format!("the terminal never showed {what}; screen:\n{text}"))
}

/// The embedded terminal: opens in the panel's folder, runs command-line commands, takes
/// typing, hides and comes back with ⌃` / ⌃O, and closes when the shell exits.
async fn terminal_phase(cx: &mut AsyncApp, left: &Path) -> Result<(), String> {
    eprintln!("smoke: terminal phase");
    let l = left.to_path_buf();
    check(step(cx, "cursor in left folder", cursor_to(0, l.clone(), "sub")).await?, "sub not listed")?;
    keys(cx, &["ctrl-o"]).await?;
    let opened = step(cx, "ctrl-o opens the terminal", |ws, w, cx| (ws.terminal_open, ws.terminal.as_ref().is_some_and(|t| t.read(cx).focus.is_focused(w)))).await?;
    check(opened == (true, true), &format!("⌃O: terminal shown/focused = {opened:?}"))?;
    let mut idle = false;
    for _ in 0..100 {
        pause(cx, 100).await;
        idle = step(cx, "shell ready", |ws, _, cx| ws.terminal.as_ref().is_some_and(|t| t.read(cx).session.shell_in_foreground())).await?;
        if idle {
            break;
        }
    }
    check(idle, "the shell never became ready")?;
    let cwd = step(cx, "shell folder", |ws, _, cx| ws.terminal.as_ref().and_then(|t| t.read(cx).session.cwd())).await?;
    check(cwd.and_then(|p| p.canonicalize().ok()) == left.canonicalize().ok(), "the shell did not start in the panel's folder")?;

    step(cx, "run from the command line", |ws, w, cx| {
        ws.command.update(cx, |c, cx| c.set_text("echo fc-smoke-$((6*7))", cx));
        ws.execute_command(w, cx);
    })
    .await?;
    wait_terminal(cx, "the command's output", |t| t.contains("fc-smoke-42")).await?;

    keys(cx, &["e", "c", "h", "o", "space", "t", "y", "p", "e", "d", "enter"]).await?;
    wait_terminal(cx, "typed input", |t| t.lines().any(|l| l.trim() == "typed")).await?;
    step(cx, "colours", |ws, _, cx| {
        if let Some(t) = &ws.terminal {
            t.update(cx, |t, _| t.type_text("printf '\\033[1;32mgreen\\033[0m \\033[7minverse\\033[0m \\033[4munderline\\033[0m\\n'; ls -la\r"));
        }
    })
    .await?;
    wait_terminal(cx, "colour output", |t| t.contains("renamed.txt")).await?;
    shot(cx, "terminal").await;
    redraw_probes(cx).await?;
    inside(cx, "main.terminal", "main.root", "terminal")?;

    keys(cx, &["ctrl-`"]).await?;
    let hidden = step(cx, "ctrl-` hides", |ws, w, _| (ws.terminal_open, ws.focus.is_focused(w))).await?;
    check(hidden == (false, true), &format!("⌃` from the terminal: shown/panels focused = {hidden:?}"))?;
    keys(cx, &["ctrl-o"]).await?;
    let back = step(cx, "ctrl-o shows it again", |ws, _, _| ws.terminal_open).await?;
    check(back, "⌃O did not bring the terminal back")?;
    wait_terminal(cx, "the earlier output", |t| t.contains("renamed.txt")).await?;

    keys(cx, &["e", "x", "i", "t", "enter"]).await?;
    let mut closed = false;
    for _ in 0..50 {
        pause(cx, 100).await;
        closed = step(cx, "shell exited", |ws, w, _| ws.terminal.is_none() && !ws.terminal_open && ws.focus.is_focused(w)).await?;
        if closed {
            break;
        }
    }
    check(closed, "typing exit did not close the terminal")
}

fn right(b: gpui::Bounds<gpui::Pixels>) -> f32 {
    (b.origin.x + b.size.width).into()
}

fn left_edge(b: gpui::Bounds<gpui::Pixels>) -> f32 {
    b.origin.x.into()
}

/// Reads a probe recorded during the last frame.
fn probe(cx: &mut AsyncApp, name: &str) -> Result<gpui::Bounds<gpui::Pixels>, String> {
    let n = name.to_string();
    cx.update(move |cx| crate::probe::get(cx, &n))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("probe {name} was never laid out (no frame rendered?)"))
}

/// `inner` must end at or before the right edge of `outer`.
fn inside(cx: &mut AsyncApp, inner: &str, outer: &str, context: &str) -> Result<(), String> {
    let (i, o) = (probe(cx, inner)?, probe(cx, outer)?);
    let (ir, or) = (right(i), right(o));
    eprintln!("smoke:   {context}: {inner} ends at {ir:.0}, {outer} at {or:.0}");
    check(ir <= or + 1.0, &format!("{context}: {inner} overflows {outer} ({ir:.0} > {or:.0})"))
}

async fn clear_probes(cx: &mut AsyncApp) {
    let _ = cx.update(|cx| cx.global::<crate::probe::Probes>().0.borrow_mut().clear());
}

/// Clears the probes and draws a fresh frame of the main window.
async fn redraw_probes(cx: &mut AsyncApp) -> Result<(), String> {
    clear_probes(cx).await;
    step(cx, "redraw", |_, w, _| w.refresh()).await?;
    pause(cx, 250).await;
    Ok(())
}

/// Every window at its minimum size: nothing may extend past its window's edge.
async fn layout_phase(cx: &mut AsyncApp) -> Result<(), String> {
    eprintln!("smoke: layout phase");
    let main = cx.update(find_workspace).map_err(|e| e.to_string())?.ok_or("no workspace window")?;
    for (w, h) in [(880.0, 520.0), (1240.0, 780.0)] {
        clear_probes(cx).await;
        main.update(cx, |ws, window, cx| {
            window.resize(gpui::size(gpui::px(w), gpui::px(h)));
            ws.split = 0.5;
            cx.notify();
        })
        .map_err(|e| e.to_string())?;
        pause(cx, 600).await;
        let ctx = format!("main {w}x{h}");
        inside(cx, "main.gear", "main.root", &ctx)?;
        inside(cx, "main.card1", "main.root", &ctx)?;
        inside(cx, "main.fkey.delete", "main.root", &ctx)?;
        let (c0, c1) = (probe(cx, "main.card0")?, probe(cx, "main.card1")?);
        check(right(c0) <= left_edge(c1), &format!("{ctx}: panels overlap"))?;
        for side in 0..2 {
            if probe(cx, &format!("main.footer.free{side}")).is_ok() {
                inside(cx, &format!("main.footer.free{side}"), &format!("main.card{side}"), &ctx)?;
            }
        }
    }

    // Settings: every page at the window's minimum size.
    let _ = cx.update(|cx| crate::prefs::open(crate::prefs::Page::General, cx));
    pause(cx, 500).await;
    let prefs = cx
        .update(|cx| cx.windows().into_iter().find_map(|w| w.downcast::<crate::prefs::PrefsView>()))
        .map_err(|e| e.to_string())?
        .ok_or("settings window did not open")?;
    for page in crate::prefs::Page::ALL {
        clear_probes(cx).await;
        prefs
            .update(cx, move |view, window, cx| {
                window.resize(gpui::size(gpui::px(680.), gpui::px(460.)));
                view.page = page;
                cx.notify();
            })
            .map_err(|e| e.to_string())?;
        pause(cx, 500).await;
        let ctx = format!("settings/{} at 680x460", page.title());
        if let (Ok(sb), Ok(ct)) = (probe(cx, "prefs.sidebar"), probe(cx, "prefs.content")) {
            eprintln!("smoke:   {ctx}: sidebar {:.0}..{:.0}, content {:.0}..{:.0}", left_edge(sb), right(sb), left_edge(ct), right(ct));
        }
        inside(cx, "prefs.content", "prefs.root", &ctx)?;
        match page {
            crate::prefs::Page::Hotkeys => {
                inside(cx, "prefs.hotkey.cell", "prefs.content", &ctx)?;
                inside(cx, "prefs.hint", "prefs.content", &ctx)?;
            }
            crate::prefs::Page::General => inside(cx, "prefs.editor", "prefs.content", &ctx)?,
            _ => {}
        }
    }
    let _ = prefs.update(cx, |_, window, _| window.remove_window());

    // Search window at its minimum size.
    step(cx, "open search", |ws, w, cx| ws.run(Action::Search, w, cx)).await?;
    pause(cx, 400).await;
    let search = cx
        .update(|cx| cx.windows().into_iter().find_map(|w| w.downcast::<crate::search::SearchView>()))
        .map_err(|e| e.to_string())?
        .ok_or("search window did not open")?;
    clear_probes(cx).await;
    search
        .update(cx, |_, window, cx| {
            window.resize(gpui::size(gpui::px(560.), gpui::px(420.)));
            cx.notify();
        })
        .map_err(|e| e.to_string())?;
    pause(cx, 500).await;
    inside(cx, "search.feed", "search.root", "search at 560x420")?;
    let _ = search.update(cx, |_, window, _| window.remove_window());
    Ok(())
}
