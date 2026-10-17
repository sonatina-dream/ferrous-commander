//! Ferrous Commander: a dual-panel file manager for macOS, written in Rust with GPUI.
//!
//! Behavior is specified in `SPEC.md`; platform-independent logic lives in `fc-core`.

mod actions;
mod anim;
#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod demo;
mod colors;
mod dialogs;
mod icons;
mod look;
mod marquee;
mod menus;
mod palette;
mod prefs;
mod probe;
mod render;
mod search;
mod smoke;
mod state;
mod terminal;
mod text_input;
mod viewer;
mod workspace;

use gpui::{App, Application};

use fc_core::settings::Session;

use crate::actions::ui;
use crate::state::{AppState, Registry};

fn main() {
    Application::new().with_assets(icons::Assets).run(|cx: &mut App| {
        cx.set_global(AppState::load());
        cx.set_global(Registry::default());
        state::install_key_bindings(cx);
        state::install_menus(cx);

        // Fallbacks for when no window has focus.
        cx.on_action(|_: &ui::Quit, cx| {
            state::persist_all_sessions(cx);
            cx.quit();
        });
        cx.on_action(|_: &actions::cmd::NewWindow, cx| workspace::open_window(None, cx));
        cx.on_action(|_: &actions::cmd::Preferences, cx| prefs::open(prefs::Page::General, cx));
        cx.on_action(|_: &ui::ShowHotkeys, cx| prefs::open(prefs::Page::Hotkeys, cx));
        cx.on_action(|_: &ui::Hide, cx| cx.hide());
        cx.on_action(|_: &ui::HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ui::ShowAll, cx| cx.unhide_other_apps());

        cx.on_app_quit(|cx| {
            state::persist_all_sessions(cx);
            async {}
        })
        .detach();

        let state = cx.global::<AppState>();
        let session = state
            .settings
            .advanced
            .save_panels_state
            .then(|| Session::load_from(&state.config_dir))
            .filter(|s| !s.left.tabs.is_empty());
        workspace::open_window(session, cx);
        if std::env::var_os("FC_SMOKE").is_some() {
            smoke::run(cx);
        }
        #[cfg(target_os = "macos")]
        if let Some(dir) = std::env::var_os("FC_DEMO") {
            demo::run(std::path::PathBuf::from(dir), cx);
        }

        cx.on_window_closed(|cx| {
            cx.global_mut::<Registry>().workspaces.retain(|w| w.upgrade().is_some());
            if cx.global::<Registry>().workspaces.is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
}
