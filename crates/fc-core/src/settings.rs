//! Preferences and session persistence (SPEC §9).
//!
//! Files live in `~/Library/Application Support/Ferrous Commander/` (override with the
//! `FC_CONFIG_DIR` environment variable): `settings.json`, `keymap.json`, `session.json`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::format::SizeMode;
use crate::keymap::Keymap;
use crate::panel::{LockState, Location, Panel, Tab, ViewMode};
use crate::sort::{SortKey, SortSpec};

pub const APP_NAME: &str = "Ferrous Commander";

/// Which viewer F3 uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewerChoice {
    #[default]
    Internal,
    QuickLook,
    Preview,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// Changing drive goes to its root instead of the last folder used there.
    pub go_root_on_drive_change: bool,
    /// Num+ / Num* / ⌘Num+ affect files only.
    pub select_files_only: bool,
    /// Space calculates folder sizes.
    pub folder_size_on_space: bool,
    /// F8 moves to Trash (⇧F8 deletes permanently).
    pub move_to_trash: bool,
    pub viewer: ViewerChoice,
    /// Application used by F4; `None` = default system text editor.
    pub editor: Option<String>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        GeneralSettings {
            go_root_on_drive_change: false,
            select_files_only: false,
            folder_size_on_space: true,
            move_to_trash: true,
            viewer: ViewerChoice::Internal,
            editor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    pub size_in_columns: SizeMode,
    pub size_in_footer: SizeMode,
    pub always_show_tab_bar: bool,
    pub folders_on_top: bool,
    pub show_hidden: bool,
    pub thumbs_in_full_mode: bool,
    pub brief_column_width: f32,
    pub show_toolbar: bool,
    pub show_sidebar: bool,
    pub show_buttons: bool,
    pub show_command_line: bool,
    pub show_disk_list: bool,
    pub show_disk_info: bool,
    pub show_path_bar: bool,
    pub show_selection_info: bool,
    /// Second (right) file panel. Off by default: one panel fills the window.
    pub show_right_panel: bool,
    /// The `..` row at the top of each folder (Backspace and ⌘↑ go up either way).
    pub show_parent_entry: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        ViewSettings {
            size_in_columns: SizeMode::Dynamic,
            size_in_footer: SizeMode::Dynamic,
            always_show_tab_bar: false,
            folders_on_top: true,
            show_hidden: false,
            thumbs_in_full_mode: false,
            brief_column_width: 220.0,
            show_toolbar: true,
            show_sidebar: true,
            show_buttons: true,
            show_command_line: true,
            show_disk_list: true,
            show_disk_info: true,
            show_path_bar: true,
            show_selection_info: true,
            show_right_panel: false,
            show_parent_entry: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// Theme name (see `theme::PRESETS`); "System" follows the macOS appearance.
    pub theme: String,
    pub font_size: f32,
    pub viewer_font_size: f32,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        AppearanceSettings { theme: "System".into(), font_size: 13.0, viewer_font_size: 13.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedSettings {
    pub confirm_delete: bool,
    pub confirm_delete_nonempty: bool,
    pub confirm_change_extension: bool,
    pub confirm_drag_drop: bool,
    pub save_panels_state: bool,
    pub save_folder_history: bool,
    pub save_command_line: bool,
}

impl Default for AdvancedSettings {
    fn default() -> Self {
        AdvancedSettings {
            confirm_delete: true,
            confirm_delete_nonempty: false,
            confirm_change_extension: true,
            confirm_drag_drop: false,
            save_panels_state: true,
            save_folder_history: true,
            save_command_line: true,
        }
    }
}

/// All preferences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub view: ViewSettings,
    pub appearance: AppearanceSettings,
    pub advanced: AdvancedSettings,
    /// User favorites (bookmarked folders).
    pub favorites: Vec<PathBuf>,
    /// Visible Full-mode columns, in display order. Name is always shown.
    pub view_columns: Vec<SortKey>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            general: GeneralSettings::default(),
            view: ViewSettings::default(),
            appearance: AppearanceSettings::default(),
            advanced: AdvancedSettings::default(),
            favorites: Vec::new(),
            view_columns: vec![SortKey::Name, SortKey::Extension, SortKey::Size, SortKey::Date],
        }
    }
}

/// Persisted state of one tab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSession {
    pub path: PathBuf,
    #[serde(default)]
    pub mode: ViewMode,
    #[serde(default)]
    pub sort: SortSpec,
    #[serde(default)]
    pub lock: LockState,
}

/// Persisted state of one panel.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSession {
    pub tabs: Vec<TabSession>,
    pub active: usize,
    pub folder_history: Vec<PathBuf>,
}

/// Window frame in points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Everything restored at launch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub left: PanelSession,
    pub right: PanelSession,
    /// 0 = left, 1 = right.
    pub active_side: usize,
    /// Fraction of the width given to the left panel.
    pub split: f32,
    pub command_history: Vec<String>,
    pub window: Option<WindowFrame>,
    /// Height of the embedded terminal in points.
    pub terminal_height: f32,
}

impl Default for Session {
    fn default() -> Self {
        Session {
            left: PanelSession::default(),
            right: PanelSession::default(),
            active_side: 0,
            split: 0.5,
            command_history: Vec::new(),
            window: None,
            terminal_height: 240.0,
        }
    }
}

impl PanelSession {
    pub fn capture(panel: &Panel, include_history: bool) -> PanelSession {
        PanelSession {
            tabs: panel
                .tabs
                .iter()
                .map(|t| TabSession {
                    path: t.working_dir().to_path_buf(),
                    mode: t.view_mode,
                    sort: t.sort,
                    lock: t.lock,
                })
                .collect(),
            active: panel.active,
            folder_history: if include_history { panel.folder_history.clone() } else { Vec::new() },
        }
    }

    /// Rebuilds a panel; missing folders fall back to their nearest existing ancestor,
    /// then to `fallback`. Entries are not loaded (call `Tab::reload`).
    pub fn restore(&self, fallback: &Path) -> Panel {
        let mut tabs: Vec<Tab> = self
            .tabs
            .iter()
            .map(|ts| {
                let mut dir = ts.path.clone();
                while !dir.is_dir() {
                    match dir.parent() {
                        Some(p) => dir = p.to_path_buf(),
                        None => {
                            dir = fallback.to_path_buf();
                            break;
                        }
                    }
                }
                let mut tab = Tab::new(dir);
                tab.view_mode = ts.mode;
                tab.sort = ts.sort;
                tab.lock = ts.lock;
                tab
            })
            .collect();
        if tabs.is_empty() {
            tabs.push(Tab::new(fallback.to_path_buf()));
        }
        let active = self.active.min(tabs.len() - 1);
        Panel { tabs, active, folder_history: self.folder_history.clone() }
    }
}

/// Configuration directory (created on demand).
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FC_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_NAME)
}

fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(value).map_err(io::Error::other)?)?;
    fs::rename(tmp, path)
}

impl Settings {
    pub fn load_from(dir: &Path) -> Settings {
        load_json(&dir.join("settings.json"))
    }

    pub fn save_to(&self, dir: &Path) -> io::Result<()> {
        save_json(&dir.join("settings.json"), self)
    }

    pub fn list_options(&self) -> crate::panel::ListOptions {
        crate::panel::ListOptions {
            show_hidden: self.view.show_hidden,
            folders_first: self.view.folders_on_top,
            show_parent: self.view.show_parent_entry,
        }
    }
}

impl Session {
    pub fn load_from(dir: &Path) -> Session {
        load_json(&dir.join("session.json"))
    }

    pub fn save_to(&self, dir: &Path) -> io::Result<()> {
        save_json(&dir.join("session.json"), self)
    }
}

/// Loads `keymap.json`, falling back to the defaults when absent or invalid.
pub fn load_keymap(dir: &Path) -> Keymap {
    fs::read_to_string(dir.join("keymap.json"))
        .ok()
        .and_then(|s| Keymap::from_json(&s).ok())
        .map(Keymap::with_additions)
        .unwrap_or_else(Keymap::defaults)
}

pub fn save_keymap(dir: &Path, keymap: &Keymap) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("keymap.json"), keymap.to_json())
}

/// Reads Commander One's key map from its preferences, if present (interoperability).
pub fn commander_one_keymap_json() -> Option<String> {
    let plist = dirs::home_dir()?.join("Library/Preferences/com.eltima.cmd1.plist");
    if !plist.exists() {
        return None;
    }
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "KeyManager.keymap", "json", "-o", "-"])
        .arg(&plist)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Location helper used when restoring: only folders are persisted.
pub fn is_persistable(location: &Location) -> bool {
    matches!(location, Location::Dir(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Action, Chord};

    #[test]
    fn settings_round_trip_and_partial_files() {
        let t = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.view.show_hidden = true;
        s.appearance.theme = "Classic Blue".into();
        s.favorites.push("/tmp".into());
        s.save_to(t.path()).unwrap();
        assert_eq!(Settings::load_from(t.path()), s);

        fs::write(t.path().join("settings.json"), r#"{"general":{"move_to_trash":false}}"#).unwrap();
        let partial = Settings::load_from(t.path());
        assert!(!partial.general.move_to_trash);
        assert!(partial.general.folder_size_on_space, "missing keys take defaults");
        assert!(!partial.view.show_right_panel && !partial.view.show_parent_entry, "one panel, no `..` row by default");
        assert!(!partial.list_options().show_parent);
        assert_eq!(Settings::load_from(&t.path().join("missing")), Settings::default());
    }

    #[test]
    fn session_round_trip_and_restore_fallback() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("a/b")).unwrap();
        let mut panel = Panel::new(r.join("a/b"));
        panel.tabs[0].view_mode = ViewMode::Brief;
        panel.tabs[0].sort = SortSpec { key: SortKey::Size, ascending: false };
        panel.tabs[0].lock = LockState::LockedDirChangesAllowed;
        panel.tabs.push(Tab::new(r.join("a")));
        panel.active = 1;
        panel.folder_history = vec![r.join("a")];

        let session = Session {
            left: PanelSession::capture(&panel, true),
            right: PanelSession::capture(&panel, false),
            active_side: 1,
            split: 0.4,
            command_history: vec!["ls -la".into()],
            window: Some(WindowFrame { x: 10.0, y: 20.0, width: 900.0, height: 600.0 }),
            terminal_height: 300.0,
        };
        session.save_to(r).unwrap();
        let back = Session::load_from(r);
        assert_eq!(back, session);
        assert!(back.right.folder_history.is_empty());

        fs::remove_dir(r.join("a/b")).unwrap();
        let restored = back.left.restore(r);
        assert_eq!(restored.tabs[0].working_dir(), r.join("a"), "missing folder falls back to ancestor");
        assert_eq!(restored.tabs[0].view_mode, ViewMode::Brief);
        assert_eq!(restored.tabs[0].lock, LockState::LockedDirChangesAllowed);
        assert_eq!(restored.active, 1);

        let json = fs::read_to_string(r.join("session.json")).unwrap();
        assert!(json.contains("\"lock\": 2"), "lock state is stored as 0/1/2");
    }

    #[test]
    fn keymap_file_fallback() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(load_keymap(t.path()), Keymap::defaults());
        let mut km = Keymap::defaults();
        km.set_binding(Action::View, 0, Some(Chord::parse("cmd-shift-v").unwrap()));
        save_keymap(t.path(), &km).unwrap();
        assert_eq!(load_keymap(t.path()), km);
        fs::write(t.path().join("keymap.json"), "garbage").unwrap();
        assert_eq!(load_keymap(t.path()), Keymap::defaults());
    }
}
