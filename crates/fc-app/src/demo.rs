//! Demo recorder (`FC_DEMO=<output dir>`): plays a scripted tour of the interface on the
//! sample data prepared by `scripts/demo.sh`, saving Retina screenshots and an MP4 of the
//! main window. Only this app's own windows are captured: never the desktop, the menu
//! bar or other apps. `FC_FFMPEG` points at the ffmpeg binary used for encoding.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{point, px, App, AsyncApp};

use fc_core::keymap::Action;
use fc_core::{Location, ViewMode};

use crate::capture;
use crate::smoke::{find_workspace, keys, pause, step, wait_jobs, wait_prompt};
use crate::state::AppState;
use crate::workspace::{dirs_home, Workspace};

const FPS: u32 = 30;

pub fn run(out: PathBuf, cx: &mut App) {
    // The tour shows both panels.
    cx.global_mut::<AppState>().settings.view.show_right_panel = true;
    cx.refresh_windows();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = tour(&out, cx).await;
        match &result {
            Ok(()) => eprintln!("DEMO OK"),
            Err(e) => eprintln!("DEMO FAIL: {e}"),
        }
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}

/// Records one window to an MP4 on a background thread.
struct Recorder {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<u64, String>>>,
}

impl Recorder {
    fn start(window_id: u32, out: PathBuf, ffmpeg: PathBuf) -> Result<Recorder, String> {
        let first = capture::capture(window_id, false).ok_or("could not capture the window")?;
        if first.is_blank() {
            return Err("the captured window is blank (is the display asleep?)".into());
        }
        let (w, h) = first.size();
        let (w, h) = (w & !1, h & !1);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || -> Result<u64, String> {
            let mut child = Command::new(&ffmpeg)
                .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "bgra"])
                .args(["-s", &format!("{w}x{h}"), "-r", &FPS.to_string(), "-i", "-"])
                .args(["-vf", "format=yuv420p", "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-movflags", "+faststart"])
                .arg(&out)
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| format!("ffmpeg: {e}"))?;
            let mut stdin = child.stdin.take().ok_or("ffmpeg stdin")?;
            let start = Instant::now();
            let mut written: u64 = 0;
            let mut last: Option<Vec<u8>> = None;
            while !flag.load(Ordering::Relaxed) {
                let frame = capture::capture(window_id, false).and_then(|f| {
                    let (fw, fh) = f.size();
                    let px = f.bgra()?;
                    // Crop to the even size fixed at start.
                    let mut out = Vec::with_capacity(w * h * 4);
                    for y in 0..h.min(fh) {
                        out.extend_from_slice(&px[y * fw * 4..y * fw * 4 + w.min(fw) * 4]);
                    }
                    (fw >= w && fh >= h).then_some(out)
                });
                if let Some(f) = frame {
                    last = Some(f);
                }
                // Keep wall-clock timing: repeat the latest frame for every elapsed tick.
                let due = (start.elapsed().as_secs_f64() * FPS as f64) as u64;
                if let Some(f) = &last {
                    while written <= due {
                        stdin.write_all(f).map_err(|e| e.to_string())?;
                        written += 1;
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            drop(stdin);
            let status = child.wait().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("ffmpeg exited with {status}"));
            }
            Ok(written)
        });
        Ok(Recorder { stop, thread: Some(thread) })
    }

    fn finish(mut self) -> Result<u64, String> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().ok_or("recorder not running")?.join().map_err(|_| "recorder panicked".to_string())?
    }
}

/// Saves a Retina PNG of every window whose content matches `which`.
async fn shot(cx: &mut AsyncApp, dir: &Path, name: &str, which: Which) -> Result<(), String> {
    pause(cx, if name.starts_with("08") { 250 } else { 450 }).await;
    let path = dir.join(format!("{name}.png"));
    let saved = cx
        .update(|cx| {
            let mut saved = false;
            for w in cx.windows() {
                let matches = match which {
                    Which::Main => w.downcast::<Workspace>().is_some(),
                    Which::Viewer => w.downcast::<crate::viewer::Viewer>().is_some(),
                    Which::Prefs => w.downcast::<crate::prefs::PrefsView>().is_some(),
                    Which::Search => w.downcast::<crate::search::SearchView>().is_some(),
                };
                if matches {
                    let _ = w.update(cx, |_, window, _| {
                        if let Some(frame) = capture::window_number(window).and_then(|id| capture::capture(id, true)) {
                            saved = !frame.is_blank() && frame.save_png(&path);
                        }
                    });
                }
            }
            saved
        })
        .map_err(|e| e.to_string())?;
    eprintln!("demo: {} {}", if saved { "saved" } else { "FAILED" }, path.display());
    if saved { Ok(()) } else { Err(format!("could not capture {name}")) }
}

#[derive(Clone, Copy)]
enum Which {
    Main,
    Viewer,
    Prefs,
    Search,
}

/// Types text into whatever has focus, one key at a time.
async fn type_text(cx: &mut AsyncApp, text: &str) -> Result<(), String> {
    for ch in text.chars() {
        let key = if ch == ' ' { "space".to_string() } else { ch.to_string() };
        keys(cx, &[key.as_str()]).await?;
        pause(cx, 70).await;
    }
    Ok(())
}

/// Moves the cursor of `side` to the entry named `name`.
fn cursor_to(name: &'static str, side: usize) -> impl FnOnce(&mut Workspace, &mut gpui::Window, &mut gpui::Context<Workspace>) {
    move |ws, w, cx| {
        ws.activate_side(side, w, cx);
        let dir = ws.panels[side].tab().working_dir().join(name);
        if let Some(ix) = ws.panels[side].tab().index_of(&dir) {
            ws.panels[side].tab_mut().set_cursor(ix);
            ws.reveal_cursor(side);
        }
    }
}

async fn slow_keys(cx: &mut AsyncApp, list: &[&str], gap_ms: u64) -> Result<(), String> {
    for k in list {
        keys(cx, &[k]).await?;
        pause(cx, gap_ms).await;
    }
    Ok(())
}

fn set_theme(cx: &mut AsyncApp, theme: &str) -> Result<(), String> {
    let theme = theme.to_string();
    cx.update(move |cx| {
        cx.global_mut::<AppState>().settings.appearance.theme = theme;
        cx.refresh_windows();
    })
    .map_err(|e| e.to_string())
}

async fn tour(out: &Path, cx: &mut AsyncApp) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let home = dirs_home();
    let ffmpeg = std::env::var_os("FC_FFMPEG").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("ffmpeg"));
    pause(cx, 1200).await;

    // Starting layout: Projects on the left, Photos on the right, both in Full mode.
    let (projects, photos) = (home.join("Projects"), home.join("Photos"));
    let main_id = step(cx, "set up panels", move |ws, w, cx| {
        cx.activate(true);
        w.activate_window();
        ws.navigate(0, projects, None, w, cx);
        ws.navigate(1, photos, None, w, cx);
        for side in 0..2 {
            ws.panels[side].tab_mut().view_mode = ViewMode::Full;
            ws.panels[side].tab_mut().set_cursor(0);
        }
        ws.activate_side(0, w, cx);
        capture::window_number(w)
    })
    .await?
    .ok_or("main window has no number")?;
    pause(cx, 900).await;

    let recorder = Recorder::start(main_id, out.join("ferrous-commander-demo.mp4"), ffmpeg)?;
    pause(cx, 1200).await;
    shot(cx, out, "01-dual-panel-dark", Which::Main).await?;

    // Glide the cursor, open a project, come back.
    slow_keys(cx, &["down", "down", "down", "up"], 260).await?;
    step(cx, "cursor to ferrous-site", cursor_to("ferrous-site", 0)).await?;
    pause(cx, 500).await;
    keys(cx, &["enter"]).await?;
    pause(cx, 1300).await;
    slow_keys(cx, &["down", "down", "down", "down"], 220).await?;
    shot(cx, out, "02-project-folder", Which::Main).await?;
    keys(cx, &["backspace"]).await?;
    pause(cx, 900).await;

    // Photos as thumbnails in the other panel.
    keys(cx, &["tab"]).await?;
    pause(cx, 700).await;
    keys(cx, &["cmd-3"]).await?;
    pause(cx, 1200).await;
    slow_keys(cx, &["right", "right", "down"], 300).await?;
    shot(cx, out, "03-thumbnails", Which::Main).await?;
    keys(cx, &["cmd-2"]).await?;
    pause(cx, 1100).await;
    keys(cx, &["cmd-1"]).await?;
    pause(cx, 800).await;

    // Mark a few files.
    step(cx, "cursor to first photo", |ws, _, cx| {
        ws.panels[1].tab_mut().set_cursor(1);
        ws.reveal_cursor(1);
        cx.notify();
    })
    .await?;
    slow_keys(cx, &["space", "space", "space"], 320).await?;
    pause(cx, 500).await;
    shot(cx, out, "04-selection", Which::Main).await?;

    // Command palette.
    keys(cx, &["cmd-p"]).await?;
    pause(cx, 600).await;
    type_text(cx, "new fold").await?;
    pause(cx, 700).await;
    shot(cx, out, "05-command-palette", Which::Main).await?;
    keys(cx, &["escape"]).await?;
    pause(cx, 600).await;

    // Context menu.
    step(cx, "context menu", |ws, w, cx| {
        let cursor = ws.panels[1].tab().cursor;
        let b = ws.list_bounds[1].get();
        let x: f32 = (b.origin.x + px(180.)).into();
        let y: f32 = (b.origin.y + px(Workspace::row_height(cx) * (cursor as f32 + 0.6))).into();
        ws.row_right_click(1, cursor, point(px(x), px(y)), w, cx);
    })
    .await?;
    pause(cx, 900).await;
    shot(cx, out, "06-context-menu", Which::Main).await?;
    step(cx, "close menu", |ws, w, cx| ws.close_menu(w, cx)).await?;
    pause(cx, 400).await;

    // Copy the marked photos to Downloads with ⌘C / ⌘V, twice to show the conflict prompt.
    let downloads = home.join("Downloads");
    step(cx, "left to Downloads", move |ws, w, cx| ws.navigate(0, downloads, None, w, cx)).await?;
    pause(cx, 700).await;
    keys(cx, &["cmd-c"]).await?;
    pause(cx, 900).await;
    keys(cx, &["tab"]).await?;
    pause(cx, 500).await;
    keys(cx, &["cmd-v"]).await?;
    wait_jobs(cx).await?;
    pause(cx, 900).await;
    keys(cx, &["cmd-v"]).await?;
    wait_prompt(cx).await?;
    pause(cx, 500).await;
    shot(cx, out, "07-conflict", Which::Main).await?;
    keys(cx, &["enter"]).await?;
    for _ in 0..3 {
        pause(cx, 400).await;
        let more = cx.update(|cx| cx.global::<AppState>().jobs.snapshot().prompt.is_some()).map_err(|e| e.to_string())?;
        if more {
            keys(cx, &["enter"]).await?;
        }
    }
    wait_jobs(cx).await?;
    pause(cx, 900).await;

    // A big file copy shows a job card with progress.
    let movie_dir = home.join("Movies");
    step(cx, "right to Movies", move |ws, w, cx| {
        ws.navigate(1, movie_dir, None, w, cx);
        ws.activate_side(1, w, cx);
    })
    .await?;
    step(cx, "cursor to the movie", cursor_to("Holiday 4K.mov", 1)).await?;
    pause(cx, 500).await;
    keys(cx, &["f5"]).await?;
    pause(cx, 700).await;
    keys(cx, &["enter"]).await?;
    shot(cx, out, "08-job-progress", Which::Main).await?;
    wait_jobs(cx).await?;
    pause(cx, 1000).await;

    // Light theme.
    set_theme(cx, "Silver")?;
    let projects = home.join("Projects");
    step(cx, "silver layout", move |ws, w, cx| {
        ws.navigate(1, projects.join("ferrous-site"), None, w, cx);
        ws.activate_side(1, w, cx);
        ws.panels[1].tab_mut().set_cursor(3);
    })
    .await?;
    pause(cx, 1400).await;
    slow_keys(cx, &["down", "down"], 260).await?;
    shot(cx, out, "09-silver-theme", Which::Main).await?;
    pause(cx, 600).await;

    let frames = recorder.finish()?;
    eprintln!("demo: video has {frames} frames ({:.1} s)", frames as f64 / FPS as f64);

    // Secondary windows (screenshots only), in the light theme, then dark.
    step(cx, "view a source file", cursor_to("app.js", 1)).await?;
    keys(cx, &["f3"]).await?;
    pause(cx, 900).await;
    shot(cx, out, "10-viewer-silver", Which::Viewer).await?;
    close_windows(cx, Which::Viewer)?;
    set_theme(cx, "Black")?;
    step(cx, "view again", cursor_to("app.js", 1)).await?;
    keys(cx, &["f3"]).await?;
    pause(cx, 900).await;
    shot(cx, out, "11-viewer-dark", Which::Viewer).await?;
    close_windows(cx, Which::Viewer)?;

    cx.update(|cx| crate::prefs::open(crate::prefs::Page::Appearance, cx)).map_err(|e| e.to_string())?;
    pause(cx, 900).await;
    shot(cx, out, "12-settings-appearance", Which::Prefs).await?;
    let prefs = cx
        .update(|cx| cx.windows().into_iter().find_map(|w| w.downcast::<crate::prefs::PrefsView>()))
        .map_err(|e| e.to_string())?;
    if let Some(p) = prefs {
        let _ = p.update(cx, |view, _, cx| {
            view.page = crate::prefs::Page::Hotkeys;
            cx.notify();
        });
        pause(cx, 700).await;
        shot(cx, out, "13-settings-hotkeys", Which::Prefs).await?;
    }
    close_windows(cx, Which::Prefs)?;

    let projects = home.join("Projects");
    step(cx, "search", move |ws, w, cx| {
        ws.navigate(0, projects, None, w, cx);
        ws.activate_side(0, w, cx);
    })
    .await?;
    let ws_window = cx.update(find_workspace).map_err(|e| e.to_string())?.ok_or("no main window")?;
    let _ = ws_window.update(cx, |ws, w, cx| {
        let roots = vec![ws.tab().working_dir().to_path_buf()];
        crate::search::open_with(roots, false, Some("*.md".into()), cx.entity().downgrade(), w.window_handle(), cx);
    });
    pause(cx, 1500).await;
    shot(cx, out, "14-search", Which::Search).await?;
    close_windows(cx, Which::Search)?;

    // Leave nothing selected or cut behind.
    let _ = step(cx, "tidy up", |ws, w, cx| {
        for side in 0..2 {
            ws.panels[side].tab_mut().unselect_all();
            if matches!(ws.panels[side].tab().location, Location::Results { .. }) {
                let _ = ws.panels[side].tab_mut().go_up(Workspace::opts(cx));
            }
        }
        ws.run(Action::ReloadContent, w, cx);
    })
    .await;
    Ok(())
}

fn close_windows(cx: &mut AsyncApp, which: Which) -> Result<(), String> {
    cx.update(|cx| {
        for w in cx.windows() {
            let matches = match which {
                Which::Main => false,
                Which::Viewer => w.downcast::<crate::viewer::Viewer>().is_some(),
                Which::Prefs => w.downcast::<crate::prefs::PrefsView>().is_some(),
                Which::Search => w.downcast::<crate::search::SearchView>().is_some(),
            };
            if matches {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
        }
    })
    .map_err(|e| e.to_string())
}
