//! Commands, key chords and the default key map (SPEC §3).
//!
//! Action identifiers reuse the command IDs observed in Commander One's defaults
//! (`KeyManager.keymap.pairStorage`), so a key map exported from it can be imported
//! (see [`Keymap::from_commander_one_json`]). Commands that have no Commander One
//! counterpart use an `fc.` prefix.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Preference-window grouping of commands (SPEC §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ActionGroup {
    File,
    Navigation,
    Panels,
    Selection,
    Interface,
    Other,
}

impl ActionGroup {
    pub const ALL: [ActionGroup; 6] = [
        ActionGroup::File,
        ActionGroup::Navigation,
        ActionGroup::Panels,
        ActionGroup::Selection,
        ActionGroup::Interface,
        ActionGroup::Other,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ActionGroup::File => "File operations",
            ActionGroup::Navigation => "Navigation",
            ActionGroup::Panels => "Panels and tabs",
            ActionGroup::Selection => "Selection",
            ActionGroup::Interface => "Interface",
            ActionGroup::Other => "Other",
        }
    }
}

macro_rules! define_actions {
    ($( $variant:ident => $id:literal, $group:ident, $label:literal; )*) => {
        /// Every user-invocable command.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Action { $($variant),* }

        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$variant),*];

            /// Stable identifier used in key map files.
            pub fn id(self) -> &'static str {
                match self { $(Action::$variant => $id),* }
            }

            pub fn group(self) -> ActionGroup {
                match self { $(Action::$variant => ActionGroup::$group),* }
            }

            /// Human-readable command name.
            pub fn label(self) -> &'static str {
                match self { $(Action::$variant => $label),* }
            }

            pub fn from_id(id: &str) -> Option<Action> {
                match id { $($id => Some(Action::$variant),)* _ => None }
            }
        }
    };
}

define_actions! {
    // File operations
    OpenItem => "TCXOpenItemAction", File, "Open";
    EnterItem => "TCXEnterItem", File, "Show package contents";
    View => "TCXViewAction", File, "View file";
    Edit => "TCXEditAction", File, "Edit file";
    CopyFiles => "fc.CopyToClipboard", File, "Copy";
    CutFiles => "fc.CutToClipboard", File, "Cut";
    PasteFiles => "fc.PasteFromClipboard", File, "Paste";
    MoveHere => "fc.MoveFromClipboard", File, "Move items here";
    Copy => "TCXCopyAction", File, "Copy to other panel";
    Move => "TCXMoveAction", File, "Move to other panel";
    Rename => "TCXRenameAction", File, "Rename";
    Queue => "TCXQueueAction", File, "Add to queue";
    NewFolder => "TCXNewFolderAction", File, "New folder";
    NewFile => "TCXNewFileAction", File, "New file";
    Delete => "TCXDeleteAction", File, "Move to Trash";
    DeleteForce => "TCXDeleteForceAction", File, "Delete permanently";
    Duplicate => "TCXDublicateFileAction", File, "Duplicate";
    MakeLink => "TCXMakeLinkAction", File, "Make symlink";
    MakeAlias => "TCXMakeAliasAction", File, "Make alias";
    Compress => "TCXCompressAction", File, "Compress";
    CompressWithOptions => "TCXCompressWithOptionsAction", File, "Compress with options…";
    Uncompress => "TCXUncompressAction", File, "Extract archive";
    ShowInfo => "TCXShowInfoAction", File, "Get info";
    ShowFinderInfo => "TCXShowFinderInfoAction", File, "Get Finder info";
    ShowInFinder => "TCXShowInFinderAction", File, "Show in Finder";
    QuickLook => "TCXShowQuickLookAction", File, "Quick Look";
    FolderSize => "TCXFolderSpaceAction", File, "Calculate folder size";
    // Navigation
    GoBack => "TCXGoBackAction", Navigation, "Back";
    GoForward => "TCXGoForwardAction", Navigation, "Forward";
    GoTop => "TCXGoTopAction", Navigation, "Enclosing folder";
    GoHome => "TCXGoHomeAction", Navigation, "Home";
    GoDesktop => "TCXGoDesktopAction", Navigation, "Desktop";
    GoDocuments => "TCXGoDocumentsAction", Navigation, "Documents";
    GoDownloads => "TCXGoDownloadsAction", Navigation, "Downloads";
    GoApplications => "TCXGoApplicationsAction", Navigation, "Applications";
    GoSystemApplications => "TCXGoSystemApplicationsAction", Navigation, "System applications";
    GoUtilities => "TCXGoUtilitiesAction", Navigation, "Utilities";
    GoFolder => "TCXGoFolderAction", Navigation, "Go to folder…";
    GoServer => "TCXGoServerAction", Navigation, "Connect to server…";
    ReloadContent => "TCXReloadContent", Navigation, "Reload";
    ChangeDriveLeft => "TCXChangeDriveLeft", Navigation, "Change drive (left panel)";
    ChangeDriveRight => "TCXChangeDriveRight", Navigation, "Change drive (right panel)";
    ShowFavorites => "TCXShowFavoritesAction", Navigation, "Favorites…";
    ShowFTP => "TCXShowFTPAction", Navigation, "Connections…";
    Eject => "TCXEjectAction", Navigation, "Eject volume";
    // Panels and tabs
    SwitchPanel => "TCXSwitchTab", Panels, "Switch panel";
    NewTab => "TCXNewTabAction", Panels, "New tab";
    CloneTab => "TCXCloneTabAction", Panels, "Duplicate tab";
    CloseTab => "fc.CloseTab", Panels, "Close tab";
    NextTab => "TCXNextTabAction", Panels, "Next tab";
    PrevTab => "TCXPrevTabAction", Panels, "Previous tab";
    ToggleTabLock => "fc.ToggleTabLock", Panels, "Cycle tab lock";
    SwapPanels => "TCXSwapPanelsAction", Panels, "Swap panels";
    EqualizePanels => "TCXEqualizePanelsAction", Panels, "Equalize panels";
    NewWindow => "TCXNewWindowAction", Panels, "New window";
    // Selection
    SelectItem => "TCXSelectItemAction", Selection, "Toggle selection";
    SelectItemWithSize => "TCXSelectItemWithSizeAction", Selection, "Select and calculate size";
    Select => "TCXSelectAction", Selection, "Select by pattern…";
    Unselect => "TCXUnselectAction", Selection, "Unselect by pattern…";
    InvertSelect => "TCXInvertSelectAction", Selection, "Invert selection";
    SelectAll => "TCXSelectAllAction", Selection, "Select all";
    UnselectAll => "TCXUnselectAllAction", Selection, "Unselect all";
    // Interface
    ViewModeFull => "TCXViewModeFull", Interface, "Full mode";
    ViewModeBrief => "TCXViewModeBrief", Interface, "Brief mode";
    ViewModeThumbs => "TCXViewModeThumbs", Interface, "Thumbs mode";
    SortByName => "name", Interface, "Sort by name";
    SortByExtension => "extension", Interface, "Sort by extension";
    SortBySize => "size", Interface, "Sort by size";
    SortByDate => "date", Interface, "Sort by date modified";
    SortByCreated => "created", Interface, "Sort by date created";
    SortByAdded => "added", Interface, "Sort by date added";
    SortByAccessed => "accessed", Interface, "Sort by date opened";
    SortByKind => "kind", Interface, "Sort by kind";
    SortByPermissions => "permissions", Interface, "Sort by permissions";
    SortAscending => "TCXViewSortAscending", Interface, "Ascending";
    SortDescending => "TCXViewSortDescending", Interface, "Descending";
    ToggleHidden => "TCXViewShowHidden", Interface, "Show hidden files";
    FullScreen => "TCXViewFullScreen", Interface, "Full screen";
    ToggleButtons => "TCXViewShowButtons", Interface, "Show function-key bar";
    ToggleCommandLine => "TCXViewShowCommandLine", Interface, "Show command line";
    ToggleDiskList => "TCXViewShowDiskList", Interface, "Show disk list";
    ToggleDiskInfo => "TCXViewShowDiskInfo", Interface, "Show disk info";
    TogglePathBar => "TCXViewShowPathBar", Interface, "Show path bar";
    ToggleSelectionInfo => "TCXViewShowSelectionInfo", Interface, "Show selection info";
    ShowQueue => "fc.ShowQueue", Interface, "Operations queue";
    // Other
    Search => "TCXSearchAction", Other, "Search…";
    SearchSpotlight => "TCXSearchSpotlightAction", Other, "Search with Spotlight…";
    ShowTerminal => "TCXShowTerminal", Other, "Open in Terminal";
    CopyItemName => "TCXCopyItemNameAction", Other, "Copy name to clipboard";
    CopyItemPath => "TCXCopyItemPathAction", Other, "Copy path to clipboard";
    PasteItemName => "TCXPasteItemNameAction", Other, "Paste name to command line";
    PasteItemPath => "TCXPasteItemPathAction", Other, "Paste path to command line";
    Preferences => "fc.Preferences", Other, "Preferences…";
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let id = String::deserialize(d)?;
        Action::from_id(&id).ok_or_else(|| serde::de::Error::custom(format!("unknown action {id:?}")))
    }
}

/// A key plus modifiers, written canonically as `ctrl-alt-shift-cmd-<key>`.
///
/// Key names: `a`–`z`, `0`–`9`, `f1`–`f20`, `up`, `down`, `left`, `right`, `pageup`,
/// `pagedown`, `home`, `end`, `delete` (forward delete), `backspace`, `enter`, `tab`,
/// `space`, `escape`, `insert`, punctuation as itself (`[`, `]`, `§`, …) and numeric-pad
/// keys `num0`–`num9`, `num+`, `num-`, `num*`, `num/`, `num.`, `num=`, `numenter`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub key: String,
}

const MODIFIER_PREFIXES: &[(&str, u8)] = &[
    ("ctrl-", 0), ("control-", 0),
    ("alt-", 1), ("option-", 1), ("opt-", 1),
    ("shift-", 2),
    ("cmd-", 3), ("command-", 3), ("super-", 3), ("platform-", 3),
];

impl Chord {
    pub fn new(key: &str) -> Chord {
        Chord { ctrl: false, alt: false, shift: false, cmd: false, key: key.to_string() }
    }

    /// Parses `"shift-cmd-t"`, `"alt-f7"`, `"cmd-num-"`, `"cmd--"` … Modifiers may come
    /// in any order; whatever follows the last modifier prefix is the key.
    pub fn parse(text: &str) -> Result<Chord, String> {
        let mut rest = text.trim();
        let mut chord = Chord::new("");
        'outer: loop {
            let lower = rest.to_ascii_lowercase();
            for (prefix, which) in MODIFIER_PREFIXES {
                if lower.starts_with(prefix) && rest.len() > prefix.len() {
                    match which {
                        0 => chord.ctrl = true,
                        1 => chord.alt = true,
                        2 => chord.shift = true,
                        _ => chord.cmd = true,
                    }
                    rest = &rest[prefix.len()..];
                    continue 'outer;
                }
            }
            break;
        }
        let key = normalize_key(rest);
        if key.is_empty() {
            return Err(format!("missing key in chord {text:?}"));
        }
        if !is_known_key(&key) {
            return Err(format!("unknown key {key:?} in chord {text:?}"));
        }
        chord.key = key;
        Ok(chord)
    }

    /// Glyph form for menus and preferences, e.g. `⌃⌥F5`, `⇧⌘T`, `Num+`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.ctrl { s.push('⌃'); }
        if self.alt { s.push('⌥'); }
        if self.shift { s.push('⇧'); }
        if self.cmd { s.push('⌘'); }
        s.push_str(&key_glyph(&self.key));
        s
    }

    /// Keystroke string for GPUI's key binding parser. GPUI does not report the
    /// numeric-pad flag, so numeric-pad keys map to their main-keyboard characters.
    pub fn gpui_keystroke(&self) -> String {
        let mut s = String::new();
        if self.ctrl { s.push_str("ctrl-"); }
        if self.alt { s.push_str("alt-"); }
        if self.shift { s.push_str("shift-"); }
        if self.cmd { s.push_str("cmd-"); }
        let key = match self.key.as_str() {
            "numenter" => "enter".to_string(),
            k if k.starts_with("num") && k.len() > 3 => k[3..].to_string(),
            k => k.to_string(),
        };
        s.push_str(&key);
        s
    }

    /// Decodes Commander One's stored form: `NSEvent.modifierFlags.rawValue | keyCode`.
    pub fn from_commander_one_raw(raw: u64) -> Option<Chord> {
        let keycode = (raw & 0xFFFF) as u16;
        let flags = raw & !0xFFFF;
        let key = mac_keycode_name(keycode)?;
        Some(Chord {
            shift: flags & (1 << 17) != 0,
            ctrl: flags & (1 << 18) != 0,
            alt: flags & (1 << 19) != 0,
            cmd: flags & (1 << 20) != 0,
            key: key.to_string(),
        })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl { f.write_str("ctrl-")?; }
        if self.alt { f.write_str("alt-")?; }
        if self.shift { f.write_str("shift-")?; }
        if self.cmd { f.write_str("cmd-")?; }
        f.write_str(&self.key)
    }
}

fn normalize_key(key: &str) -> String {
    let lower = key.to_lowercase();
    match lower.as_str() {
        "return" => "enter".into(),
        "esc" => "escape".into(),
        "del" | "forwarddelete" => "delete".into(),
        "pgup" => "pageup".into(),
        "pgdn" => "pagedown".into(),
        "numplus" | "kp+" => "num+".into(),
        "numminus" | "kp-" => "num-".into(),
        _ => lower,
    }
}

fn is_known_key(key: &str) -> bool {
    const NAMED: &[&str] = &[
        "up", "down", "left", "right", "pageup", "pagedown", "home", "end", "delete",
        "backspace", "enter", "tab", "space", "escape", "insert", "numenter", "num+", "num-",
        "num*", "num/", "num.", "num=",
    ];
    if NAMED.contains(&key) {
        return true;
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=20).contains(&n);
    }
    if let Some(d) = key.strip_prefix("num") {
        return d.len() == 1 && d.chars().all(|c| c.is_ascii_digit());
    }
    key.chars().count() == 1
}

fn key_glyph(key: &str) -> String {
    match key {
        "up" => "↑".into(),
        "down" => "↓".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "pageup" => "⇞".into(),
        "pagedown" => "⇟".into(),
        "home" => "↖".into(),
        "end" => "↘".into(),
        "delete" => "⌦".into(),
        "backspace" => "⌫".into(),
        "enter" => "↩".into(),
        "numenter" => "⌤".into(),
        "tab" => "⇥".into(),
        "space" => "Space".into(),
        "escape" => "⎋".into(),
        "insert" => "Ins".into(),
        k if k.starts_with("num") && k.len() > 3 => format!("Num{}", &k[3..]),
        k if k.starts_with('f') && k.len() > 1 && k[1..].chars().all(|c| c.is_ascii_digit()) => {
            k.to_uppercase()
        }
        k => k.to_uppercase(),
    }
}

/// macOS virtual key code (`kVK_*`) → canonical key name.
fn mac_keycode_name(code: u16) -> Option<&'static str> {
    Some(match code {
        0 => "a", 1 => "s", 2 => "d", 3 => "f", 4 => "h", 5 => "g", 6 => "z", 7 => "x",
        8 => "c", 9 => "v", 10 => "§", 11 => "b", 12 => "q", 13 => "w", 14 => "e", 15 => "r",
        16 => "y", 17 => "t", 18 => "1", 19 => "2", 20 => "3", 21 => "4", 22 => "6", 23 => "5",
        24 => "=", 25 => "9", 26 => "7", 27 => "-", 28 => "8", 29 => "0", 30 => "]", 31 => "o",
        32 => "u", 33 => "[", 34 => "i", 35 => "p", 36 => "enter", 37 => "l", 38 => "j",
        39 => "'", 40 => "k", 41 => ";", 42 => "\\", 43 => ",", 44 => "/", 45 => "n", 46 => "m",
        47 => ".", 48 => "tab", 49 => "space", 50 => "`", 51 => "backspace", 53 => "escape",
        65 => "num.", 67 => "num*", 69 => "num+", 75 => "num/", 76 => "numenter", 78 => "num-",
        81 => "num=", 82 => "num0", 83 => "num1", 84 => "num2", 85 => "num3", 86 => "num4",
        87 => "num5", 88 => "num6", 89 => "num7", 91 => "num8", 92 => "num9",
        96 => "f5", 97 => "f6", 98 => "f7", 99 => "f3", 100 => "f8", 101 => "f9", 103 => "f11",
        105 => "f13", 106 => "f16", 107 => "f14", 109 => "f10", 111 => "f12", 113 => "f15",
        114 => "insert", 115 => "home", 116 => "pageup", 117 => "delete", 118 => "f4",
        119 => "end", 120 => "f2", 121 => "pagedown", 122 => "f1", 123 => "left",
        124 => "right", 125 => "down", 126 => "up",
        _ => return None,
    })
}

/// The observed default key map, in its stored order (SPEC §3.1, evidence E8).
pub const DEFAULT_BINDINGS: &[(&str, Action)] = &[
    ("f2", Action::Queue),
    ("cmd-n", Action::NewWindow),
    ("f3", Action::View),
    ("f4", Action::Edit),
    ("f5", Action::Copy),
    ("f6", Action::Move),
    ("f7", Action::NewFolder),
    ("f8", Action::Delete),
    ("shift-f8", Action::DeleteForce),
    ("delete", Action::Delete),
    ("shift-delete", Action::DeleteForce),
    ("cmd-d", Action::Duplicate),
    ("shift-f4", Action::NewFile),
    ("cmd-l", Action::MakeLink),
    ("alt-cmd-l", Action::MakeAlias),
    ("shift-f6", Action::Rename),
    ("cmd-i", Action::ShowInfo),
    ("alt-cmd-i", Action::ShowFinderInfo),
    ("alt-cmd-o", Action::ShowInFinder),
    ("cmd-y", Action::QuickLook),
    ("ctrl-f5", Action::Compress),
    ("ctrl-alt-f5", Action::CompressWithOptions),
    ("ctrl-f9", Action::Uncompress),
    ("shift-cmd-t", Action::NewTab),
    ("cmd-t", Action::CloneTab),
    ("alt-f7", Action::Search),
    ("ctrl-alt-f7", Action::SearchSpotlight),
    ("alt-shift-e", Action::SwapPanels),
    ("alt-e", Action::EqualizePanels),
    ("num+", Action::Select),
    ("num-", Action::Unselect),
    ("num*", Action::InvertSelect),
    ("cmd-a", Action::SelectAll),
    ("cmd-num+", Action::SelectAll),
    ("cmd-num-", Action::UnselectAll),
    ("alt-f1", Action::ChangeDriveLeft),
    ("alt-f2", Action::ChangeDriveRight),
    ("tab", Action::SwitchPanel),
    ("alt-enter", Action::EnterItem),
    ("ctrl-enter", Action::PasteItemName),
    ("ctrl-shift-enter", Action::PasteItemPath),
    ("alt-cmd-c", Action::CopyItemName),
    ("ctrl-cmd-c", Action::CopyItemPath),
    ("ctrl-o", Action::ShowTerminal),
    ("cmd-r", Action::ReloadContent),
    ("cmd-1", Action::ViewModeFull),
    ("cmd-2", Action::ViewModeBrief),
    ("cmd-3", Action::ViewModeThumbs),
    ("alt-cmd-1", Action::SortByName),
    ("alt-cmd-2", Action::SortByExtension),
    ("alt-cmd-5", Action::SortByCreated),
    ("alt-cmd-6", Action::SortByAdded),
    ("alt-cmd-7", Action::SortByAccessed),
    ("alt-cmd-9", Action::SortByPermissions),
    ("alt-cmd-a", Action::SortAscending),
    ("alt-cmd-d", Action::SortDescending),
    ("alt-shift-cmd-h", Action::ToggleHidden),
    ("alt-cmd-f", Action::FullScreen),
    ("shift-cmd-b", Action::ToggleButtons),
    ("shift-cmd-p", Action::ToggleCommandLine),
    ("alt-cmd-s", Action::ToggleDiskList),
    ("shift-cmd-i", Action::ToggleDiskInfo),
    ("alt-cmd-p", Action::TogglePathBar),
    ("shift-cmd-s", Action::ToggleSelectionInfo),
    ("cmd-[", Action::GoBack),
    ("cmd-]", Action::GoForward),
    ("cmd-up", Action::GoTop),
    ("shift-cmd-h", Action::GoHome),
    ("shift-cmd-d", Action::GoDesktop),
    ("shift-cmd-o", Action::GoDocuments),
    ("shift-cmd-l", Action::GoDownloads),
    ("shift-cmd-a", Action::GoApplications),
    ("alt-shift-cmd-a", Action::GoSystemApplications),
    ("shift-cmd-u", Action::GoUtilities),
    ("shift-cmd-g", Action::GoFolder),
    ("cmd-k", Action::GoServer),
    ("cmd-e", Action::Eject),
    ("cmd-f", Action::ShowFTP),
    ("ctrl-cmd-f", Action::ShowFavorites),
    ("ctrl-tab", Action::NextTab),
    ("ctrl-shift-tab", Action::PrevTab),
    ("space", Action::SelectItemWithSize),
    ("enter", Action::OpenItem),
    ("§", Action::SelectItem),
    ("alt-cmd-3", Action::SortBySize),
    ("alt-cmd-4", Action::SortByDate),
    ("alt-cmd-8", Action::SortByKind),
];

/// One key → command assignment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    #[serde(rename = "key", serialize_with = "ser_chord", deserialize_with = "de_chord")]
    pub chord: Chord,
    pub action: Action,
}

fn ser_chord<S: Serializer>(c: &Chord, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&c.to_string())
}

fn de_chord<'de, D: Deserializer<'de>>(d: D) -> Result<Chord, D::Error> {
    let text = String::deserialize(d)?;
    Chord::parse(&text).map_err(serde::de::Error::custom)
}

/// An ordered list of bindings. A chord maps to at most one command; a command may
/// have several chords (the preferences UI shows the first two as primary/secondary).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
}

/// Finder-style shortcuts added on top of Commander One's defaults.
pub const EXTRA_BINDINGS: &[(&str, Action)] = &[
    ("cmd-c", Action::CopyFiles),
    ("cmd-x", Action::CutFiles),
    ("cmd-v", Action::PasteFiles),
    ("alt-cmd-v", Action::MoveHere),
    ("cmd-backspace", Action::Delete),
    ("alt-cmd-backspace", Action::DeleteForce),
];

fn parse_table(table: &[(&str, Action)]) -> Vec<Binding> {
    table.iter().map(|(k, a)| Binding { chord: Chord::parse(k).expect("valid default chord"), action: *a }).collect()
}

impl Keymap {
    /// Commander One's default key map, exactly as observed (87 bindings).
    pub fn commander_one_defaults() -> Keymap {
        Keymap { bindings: parse_table(DEFAULT_BINDINGS) }
    }

    /// This app's defaults: Commander One's map plus the Finder-style extras.
    pub fn defaults() -> Keymap {
        Keymap::commander_one_defaults().with_additions()
    }

    /// Adds each extra binding whose command has no key yet and whose chord is free.
    /// Used for saved and imported key maps so new shortcuts appear after upgrades.
    pub fn with_additions(mut self) -> Keymap {
        for b in parse_table(EXTRA_BINDINGS) {
            let action_unbound = self.chords_for(b.action).is_empty() || matches!(b.action, Action::Delete | Action::DeleteForce);
            if action_unbound && self.action_for(&b.chord).is_none() {
                self.bindings.push(b);
            }
        }
        self
    }

    pub fn action_for(&self, chord: &Chord) -> Option<Action> {
        self.bindings.iter().find(|b| &b.chord == chord).map(|b| b.action)
    }

    pub fn chords_for(&self, action: Action) -> Vec<&Chord> {
        self.bindings.iter().filter(|b| b.action == action).map(|b| &b.chord).collect()
    }

    /// Assigns `chord` to `action` in slot 0 (primary) or 1 (secondary). The chord is
    /// removed from any other command first. `None` clears the slot.
    pub fn set_binding(&mut self, action: Action, slot: usize, chord: Option<Chord>) {
        if let Some(c) = &chord {
            self.bindings.retain(|b| &b.chord != c);
        }
        let positions: Vec<usize> = self
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, b)| b.action == action)
            .map(|(i, _)| i)
            .collect();
        match (positions.get(slot), chord) {
            (Some(&i), Some(c)) => self.bindings[i].chord = c,
            (Some(&i), None) => {
                self.bindings.remove(i);
            }
            (None, Some(c)) => self.bindings.push(Binding { chord: c, action }),
            (None, None) => {}
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.bindings).expect("keymap serializes")
    }

    pub fn from_json(text: &str) -> Result<Keymap, String> {
        let bindings: Vec<Binding> = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Ok(Keymap { bindings })
    }

    /// Imports Commander One's `KeyManager.keymap` value converted to JSON, e.g. with
    /// `plutil -extract KeyManager.keymap json -o - ~/Library/Preferences/com.eltima.cmd1.plist`.
    /// Unknown command IDs or key codes are skipped and reported.
    pub fn from_commander_one_json(text: &str) -> Result<(Keymap, Vec<String>), String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Stored {
            key_storage: Vec<u64>,
            pair_storage: std::collections::HashMap<String, String>,
        }
        let stored: Stored = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut keymap = Keymap::default();
        let mut skipped = Vec::new();
        for raw in stored.key_storage {
            let Some(id) = stored.pair_storage.get(&raw.to_string()) else {
                skipped.push(format!("{raw}: no command"));
                continue;
            };
            match (Chord::from_commander_one_raw(raw), Action::from_id(id)) {
                (Some(chord), Some(action)) => keymap.bindings.push(Binding { chord, action }),
                _ => skipped.push(format!("{raw}: {id}")),
            }
        }
        Ok((keymap, skipped))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for a in Action::ALL {
            assert!(seen.insert(a.id()), "duplicate id {}", a.id());
            assert_eq!(Action::from_id(a.id()), Some(*a));
        }
    }

    #[test]
    fn chord_parsing() {
        let c = Chord::parse("shift-cmd-t").unwrap();
        assert!(c.shift && c.cmd && !c.alt && !c.ctrl);
        assert_eq!(c.key, "t");
        assert_eq!(Chord::parse("cmd-shift-t").unwrap(), c);
        assert_eq!(Chord::parse("cmd-num-").unwrap().key, "num-");
        assert_eq!(Chord::parse("cmd--").unwrap().key, "-");
        assert_eq!(Chord::parse("Option-Return").unwrap(), Chord::parse("alt-enter").unwrap());
        assert!(Chord::parse("cmd-").is_err());
        assert!(Chord::parse("ctrl-nonsense").is_err());
        assert_eq!(Chord::parse("ctrl-alt-f5").unwrap().display(), "⌃⌥F5");
        assert_eq!(Chord::parse("cmd-num+").unwrap().gpui_keystroke(), "cmd-+");
        assert_eq!(Chord::parse("alt-shift-cmd-h").unwrap().to_string(), "alt-shift-cmd-h");
    }

    #[test]
    fn defaults_have_87_unique_chords() {
        let km = Keymap::commander_one_defaults();
        assert_eq!(km.bindings.len(), 87);
        let mut chords = std::collections::HashSet::new();
        for b in &km.bindings {
            assert!(chords.insert(b.chord.clone()), "duplicate chord {}", b.chord);
        }
        assert_eq!(km.action_for(&Chord::parse("f5").unwrap()), Some(Action::Copy));
        assert_eq!(km.chords_for(Action::Delete).len(), 2);
    }

    #[test]
    fn finder_extras_sit_on_top_without_conflicts() {
        let km = Keymap::defaults();
        assert_eq!(km.bindings.len(), 87 + EXTRA_BINDINGS.len());
        let mut chords = std::collections::HashSet::new();
        for b in &km.bindings {
            assert!(chords.insert(b.chord.clone()), "duplicate chord {}", b.chord);
        }
        assert_eq!(km.action_for(&Chord::parse("cmd-c").unwrap()), Some(Action::CopyFiles));
        assert_eq!(km.action_for(&Chord::parse("cmd-v").unwrap()), Some(Action::PasteFiles));
        assert_eq!(km.action_for(&Chord::parse("cmd-backspace").unwrap()), Some(Action::Delete));
        assert_eq!(km.chords_for(Action::Delete).len(), 3);

        // A user who rebound Copy-to-clipboard keeps their choice; a taken chord is not stolen.
        let mut custom = Keymap::commander_one_defaults();
        custom.set_binding(Action::CopyFiles, 0, Some(Chord::parse("ctrl-c").unwrap()));
        custom.set_binding(Action::View, 1, Some(Chord::parse("cmd-v").unwrap()));
        let merged = custom.with_additions();
        assert_eq!(merged.chords_for(Action::CopyFiles).len(), 1);
        assert_eq!(merged.action_for(&Chord::parse("cmd-v").unwrap()), Some(Action::View));
        assert!(merged.chords_for(Action::PasteFiles).is_empty());
        assert_eq!(merged.clone().with_additions(), merged, "idempotent");
    }

    #[test]
    fn rebinding_moves_chord_and_fills_slots() {
        let mut km = Keymap::defaults();
        let f5 = Chord::parse("f5").unwrap();
        km.set_binding(Action::View, 1, Some(f5.clone()));
        assert_eq!(km.action_for(&f5), Some(Action::View));
        assert!(km.chords_for(Action::Copy).is_empty());
        km.set_binding(Action::View, 1, None);
        assert_eq!(km.chords_for(Action::View).len(), 1);
    }

    #[test]
    fn json_round_trip() {
        let km = Keymap::defaults();
        let back = Keymap::from_json(&km.to_json()).unwrap();
        assert_eq!(km, back);
        assert!(Keymap::from_json(r#"[{"key":"f5","action":"nope"}]"#).is_err());
    }
}
