//! Application-wide state, key bindings and the menu bar.

use std::path::PathBuf;

use gpui::{App, Global, KeyBinding, Menu, MenuItem, SystemMenuType};

use fc_core::keymap::Action;
use fc_core::queue::JobManager;
use fc_core::settings::{self, Settings};
use fc_core::Keymap;

use crate::actions::{self, menu_item, menu_item_named, ui};

/// Key context of the file-panel area. GPUI shows menu shortcuts for bindings whose
/// context matches its default stack (Workspace › Pane › Editor), so panel commands bind
/// under "Pane" to get native menu key equivalents.
pub const PANEL_CONTEXT: &str = "Pane";

/// Shared by every window.
pub struct AppState {
    pub settings: Settings,
    pub keymap: Keymap,
    pub jobs: JobManager,
    pub config_dir: PathBuf,
    /// The macOS pasteboard used for ⌘C / ⌘X / ⌘V (a private one for smoke and demo runs).
    pub pasteboard: fc_core::pasteboard::Pasteboard,
    /// Files cut with ⌘X and the pasteboard change count at that moment.
    pub cut: Option<(Vec<PathBuf>, i64)>,
}

impl Global for AppState {}

impl AppState {
    pub fn load() -> AppState {
        let config_dir = settings::config_dir();
        AppState {
            settings: Settings::load_from(&config_dir),
            keymap: settings::load_keymap(&config_dir),
            jobs: JobManager::new(),
            config_dir,
            pasteboard: if std::env::var_os("FC_SMOKE").is_some() || std::env::var_os("FC_DEMO").is_some() {
                fc_core::pasteboard::Pasteboard::named(&format!("dev.ferrouscommander.smoke.{}", std::process::id()))
            } else {
                fc_core::pasteboard::Pasteboard::general()
            },
            cut: None,
        }
    }

    pub fn save_settings(&self) {
        if let Err(e) = self.settings.save_to(&self.config_dir) {
            eprintln!("could not save settings: {e}");
        }
    }

    pub fn save_keymap(&self) {
        if let Err(e) = settings::save_keymap(&self.config_dir, &self.keymap) {
            eprintln!("could not save keymap: {e}");
        }
    }
}

/// Open workspace windows (for refreshing them after preference changes).
#[derive(Default)]
pub struct Registry {
    pub workspaces: Vec<gpui::WeakEntity<crate::workspace::Workspace>>,
}

impl Global for Registry {}

/// Reloads every open workspace (after listing options changed).
pub fn reload_all_workspaces(cx: &mut App) {
    let list: Vec<_> = cx.global::<Registry>().workspaces.clone();
    for ws in list {
        let _ = ws.update(cx, |ws, cx| ws.reload_all(cx));
    }
    cx.global_mut::<Registry>().workspaces.retain(|w| w.upgrade().is_some());
}

/// Saves the session of every open workspace.
pub fn persist_all_sessions(cx: &mut App) {
    let list: Vec<_> = cx.global::<Registry>().workspaces.clone();
    for ws in list {
        let _ = ws.update(cx, |ws, cx| ws.persist_session(cx));
    }
}

/// Commands that work regardless of focus (bound without a context).
const GLOBAL_ACTIONS: &[Action] = &[Action::NewWindow, Action::Preferences];

/// (Re)installs all key bindings from the current key map.
pub fn install_key_bindings(cx: &mut App) {
    let keymap = cx.global::<AppState>().keymap.clone();
    cx.clear_key_bindings();
    let mut bindings: Vec<KeyBinding> = Vec::new();

    // Fixed list navigation inside panels.
    let p = Some(PANEL_CONTEXT);
    bindings.extend([
        KeyBinding::new("up", ui::CursorUp, p),
        KeyBinding::new("down", ui::CursorDown, p),
        KeyBinding::new("left", ui::CursorLeft, p),
        KeyBinding::new("right", ui::CursorRight, p),
        KeyBinding::new("pageup", ui::PageUp, p),
        KeyBinding::new("pagedown", ui::PageDown, p),
        KeyBinding::new("home", ui::CursorHome, p),
        KeyBinding::new("end", ui::CursorEnd, p),
        KeyBinding::new("shift-up", ui::ExtendUp, p),
        KeyBinding::new("shift-down", ui::ExtendDown, p),
        KeyBinding::new("shift-pageup", ui::ExtendPageUp, p),
        KeyBinding::new("shift-pagedown", ui::ExtendPageDown, p),
        KeyBinding::new("shift-home", ui::ExtendHome, p),
        KeyBinding::new("shift-end", ui::ExtendEnd, p),
        KeyBinding::new("backspace", ui::GoUpKey, p),
        KeyBinding::new("escape", ui::ClearTypeAhead, p),
        KeyBinding::new("cmd-shift-.", actions::cmd::ToggleHidden, p),
        KeyBinding::new("cmd-w", actions::cmd::CloseTab, p),
        // Dialogs, menus, command line.
        KeyBinding::new("enter", ui::Confirm, Some("Dialog")),
        KeyBinding::new("escape", ui::Cancel, Some("Dialog")),
        KeyBinding::new("up", ui::MenuUp, Some("Menu")),
        KeyBinding::new("down", ui::MenuDown, Some("Menu")),
        KeyBinding::new("enter", ui::Confirm, Some("Menu")),
        KeyBinding::new("escape", ui::Cancel, Some("Menu")),
        KeyBinding::new("cmd-p", ui::OpenPalette, Some("Workspace")),
        KeyBinding::new("up", ui::MenuUp, Some("Palette")),
        KeyBinding::new("down", ui::MenuDown, Some("Palette")),
        KeyBinding::new("enter", ui::Confirm, Some("Palette")),
        KeyBinding::new("escape", ui::Cancel, Some("Palette")),
        KeyBinding::new("enter", ui::Confirm, Some("CommandLine")),
        KeyBinding::new("escape", ui::FocusPanels, Some("CommandLine")),
        KeyBinding::new("up", ui::HistoryPrev, Some("CommandLine")),
        KeyBinding::new("down", ui::HistoryNext, Some("CommandLine")),
        KeyBinding::new("tab", ui::FocusPanels, Some("CommandLine")),
        // Viewer window.
        KeyBinding::new("escape", ui::CloseViewer, Some("Viewer")),
        KeyBinding::new("f3", ui::CloseViewer, Some("Viewer")),
        KeyBinding::new("f10", ui::CloseViewer, Some("Viewer")),
        KeyBinding::new("cmd-f", ui::ViewerFind, Some("Viewer")),
        KeyBinding::new("cmd-g", ui::ViewerFindNext, Some("Viewer")),
        KeyBinding::new("enter", ui::ViewerFindNext, Some("ViewerFind")),
        KeyBinding::new("escape", ui::Cancel, Some("ViewerFind")),
        // Search and preferences windows.
        KeyBinding::new("escape", ui::CloseWindow, Some("Search")),
        KeyBinding::new("enter", ui::Confirm, Some("Search")),
        KeyBinding::new("escape", ui::CloseWindow, Some("Preferences")),
        // Application.
        KeyBinding::new("cmd-q", ui::Quit, None),
        KeyBinding::new("cmd-m", ui::Minimize, None),
        KeyBinding::new("cmd-h", ui::Hide, None),
        KeyBinding::new("alt-cmd-h", ui::HideOthers, None),
        KeyBinding::new("cmd-,", actions::cmd::Preferences, None),
        KeyBinding::new("shift-cmd-w", ui::CloseWindow, None),
    ]);

    for b in &keymap.bindings {
        let context = if GLOBAL_ACTIONS.contains(&b.action) { None } else { p };
        bindings.push(actions::key_binding(b.action, &b.chord.gpui_keystroke(), context));
    }
    cx.bind_keys(bindings);
}

/// Builds the menu bar. Shortcuts are filled in by GPUI from the key bindings.
pub fn install_menus(cx: &mut App) {
    use Action as A;
    let items = |list: &[Option<A>]| -> Vec<MenuItem> {
        list.iter().map(|a| a.map(menu_item).unwrap_or_else(MenuItem::separator)).collect()
    };
    cx.set_menus(vec![
        Menu {
            name: settings::APP_NAME.into(),
            items: vec![
                MenuItem::action("About Ferrous Commander", ui::About),
                MenuItem::separator(),
                menu_item_named("Settings…", A::Preferences),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Ferrous Commander", ui::Hide),
                MenuItem::action("Hide Others", ui::HideOthers),
                MenuItem::action("Show All", ui::ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Ferrous Commander", ui::Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: {
                let mut v = items(&[
                    Some(A::OpenItem), Some(A::EnterItem), Some(A::View), Some(A::Edit), None,
                    Some(A::Copy), Some(A::Move), Some(A::Rename), Some(A::Queue), None,
                    Some(A::NewFolder), Some(A::NewFile), Some(A::Duplicate), Some(A::MakeLink), Some(A::MakeAlias), None,
                    Some(A::Delete), Some(A::DeleteForce), None,
                    Some(A::Compress), Some(A::CompressWithOptions), Some(A::Uncompress), None,
                    Some(A::ShowInfo), Some(A::ShowFinderInfo), Some(A::ShowInFinder), Some(A::QuickLook), Some(A::FolderSize), None,
                    Some(A::NewWindow), Some(A::NewTab), Some(A::CloseTab),
                ]);
                v.push(MenuItem::action("Close Window", ui::CloseWindow));
                v
            },
        },
        Menu {
            name: "Edit".into(),
            items: items(&[
                Some(A::CopyFiles), Some(A::CutFiles), Some(A::PasteFiles), Some(A::MoveHere), None,
                Some(A::SelectAll), Some(A::UnselectAll), Some(A::Select), Some(A::Unselect), Some(A::InvertSelect),
                Some(A::SelectItem), Some(A::SelectItemWithSize), None,
                Some(A::CopyItemName), Some(A::CopyItemPath), Some(A::PasteItemName), Some(A::PasteItemPath),
            ]),
        },
        Menu {
            name: "View".into(),
            items: {
                let mut v = items(&[Some(A::ViewModeFull), Some(A::ViewModeBrief), Some(A::ViewModeThumbs), None]);
                v.push(MenuItem::submenu(Menu {
                    name: "Sort By".into(),
                    items: items(&[
                        Some(A::SortByName), Some(A::SortByExtension), Some(A::SortBySize), Some(A::SortByDate),
                        Some(A::SortByCreated), Some(A::SortByAdded), Some(A::SortByAccessed), Some(A::SortByKind),
                        Some(A::SortByPermissions), None, Some(A::SortAscending), Some(A::SortDescending),
                    ]),
                }));
                v.extend(items(&[
                    None, Some(A::ToggleHidden), None,
                    Some(A::ToggleButtons), Some(A::ToggleCommandLine), Some(A::ToggleDiskList), Some(A::ToggleDiskInfo),
                    Some(A::TogglePathBar), Some(A::ToggleSelectionInfo), None,
                    Some(A::ReloadContent), Some(A::FullScreen),
                ]));
                v
            },
        },
        Menu {
            name: "Go".into(),
            items: items(&[
                Some(A::GoBack), Some(A::GoForward), Some(A::GoTop), None,
                Some(A::GoHome), Some(A::GoDesktop), Some(A::GoDocuments), Some(A::GoDownloads),
                Some(A::GoApplications), Some(A::GoSystemApplications), Some(A::GoUtilities), None,
                Some(A::GoFolder), Some(A::GoServer), Some(A::ShowFavorites), None,
                Some(A::ChangeDriveLeft), Some(A::ChangeDriveRight), Some(A::Eject),
            ]),
        },
        Menu {
            name: "Commands".into(),
            items: std::iter::once(MenuItem::action("Command Palette…", ui::OpenPalette)).chain(items(&[
                Some(A::Search), Some(A::SearchSpotlight), Some(A::ShowTerminal), None,
                Some(A::SwitchPanel), Some(A::SwapPanels), Some(A::EqualizePanels), None,
                Some(A::CloneTab), Some(A::NextTab), Some(A::PrevTab), Some(A::ToggleTabLock), None,
                Some(A::ShowQueue), Some(A::ShowFTP),
            ])).collect(),
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", ui::Minimize),
                MenuItem::action("Zoom", ui::Zoom),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("Keyboard Shortcuts…", ui::ShowHotkeys),
                MenuItem::action("Import Commander One Shortcuts", ui::ImportCommanderOneKeymap),
                MenuItem::action("Reset Shortcuts to Defaults", ui::ResetKeymap),
                MenuItem::action("Show Settings Folder", ui::RevealConfig),
            ],
        },
    ]);
}
