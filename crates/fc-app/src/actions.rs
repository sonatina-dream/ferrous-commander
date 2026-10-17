//! Bridges `fc_core::Action` (data) to GPUI actions (types).
//!
//! GPUI binds keys and menu items to action *types*, while the core models commands as
//! an enum so key maps can be stored and edited. A macro generates one unit action per
//! command plus the glue in both directions.

use gpui::{Context, InteractiveElement, KeyBinding, MenuItem, Window};

use fc_core::Action;

macro_rules! bridge {
    ($($name:ident),* $(,)?) => {
        /// One GPUI action type per `fc_core::Action` variant.
        pub mod cmd {
            gpui::actions!(fc, [$($name),*]);
        }

        /// Key binding for a core command.
        pub fn key_binding(action: Action, keystroke: &str, context: Option<&str>) -> KeyBinding {
            match action {
                $(Action::$name => KeyBinding::new(keystroke, cmd::$name, context),)*
            }
        }

        /// Menu item for a core command, labelled with its name.
        pub fn menu_item(action: Action) -> MenuItem {
            match action {
                $(Action::$name => MenuItem::action(action.label(), cmd::$name),)*
            }
        }

        /// Menu item with a custom label.
        pub fn menu_item_named(label: &str, action: Action) -> MenuItem {
            match action {
                $(Action::$name => MenuItem::action(label.to_string(), cmd::$name),)*
            }
        }

        /// Routes every command type to `handler` on the element.
        pub fn route<T: 'static, E: InteractiveElement>(
            el: E,
            cx: &Context<T>,
            handler: fn(&mut T, Action, &mut Window, &mut Context<T>),
        ) -> E {
            el $(.on_action(cx.listener(move |this, _: &cmd::$name, window, cx| handler(this, Action::$name, window, cx))))*
        }

        /// Boxed GPUI action for a core command.
        #[cfg(test)]
        pub fn boxed(action: Action) -> Box<dyn gpui::Action> {
            match action {
                $(Action::$name => Box::new(cmd::$name),)*
            }
        }
    };
}

bridge!(
    OpenItem, EnterItem, View, Edit, CopyFiles, CutFiles, PasteFiles, MoveHere, Copy, Move, Rename, Queue, NewFolder, NewFile, Delete,
    DeleteForce, Duplicate, MakeLink, MakeAlias, Compress, CompressWithOptions, Uncompress,
    ShowInfo, ShowFinderInfo, ShowInFinder, QuickLook, FolderSize, GoBack, GoForward, GoTop,
    GoHome, GoDesktop, GoDocuments, GoDownloads, GoApplications, GoSystemApplications,
    GoUtilities, GoFolder, GoServer, ReloadContent, ChangeDriveLeft, ChangeDriveRight,
    ShowFavorites, ShowFTP, Eject, SwitchPanel, NewTab, CloneTab, CloseTab, NextTab, PrevTab,
    ToggleTabLock, SwapPanels, EqualizePanels, ToggleRightPanel, NewWindow, SelectItem, SelectItemWithSize,
    Select, Unselect, InvertSelect, SelectAll, UnselectAll, ViewModeFull, ViewModeBrief,
    ViewModeThumbs, SortByName, SortByExtension, SortBySize, SortByDate, SortByCreated,
    SortByAdded, SortByAccessed, SortByKind, SortByPermissions, SortAscending, SortDescending,
    ToggleHidden, FullScreen, ToggleButtons, ToggleCommandLine, ToggleDiskList, ToggleDiskInfo,
    TogglePathBar, ToggleSelectionInfo, ShowQueue, Search, SearchSpotlight, ShowTerminal,
    OpenTerminalApp, CopyItemName, CopyItemPath, PasteItemName, PasteItemPath, Preferences,
);

/// Fixed (non-remappable) UI actions.
pub mod ui {
    gpui::actions!(
        fc_ui,
        [
            CursorUp, CursorDown, CursorLeft, CursorRight, PageUp, PageDown, CursorHome, CursorEnd,
            ExtendUp, ExtendDown, ExtendPageUp, ExtendPageDown, ExtendHome, ExtendEnd, GoUpKey,
            ClearTypeAhead, Confirm, Cancel, FocusCommandLine, HistoryPrev, HistoryNext,
            FocusPanels, Quit, CloseWindow, Minimize, Zoom, Hide, HideOthers, ShowAll, About,
            ShowHotkeys, ImportCommanderOneKeymap, ResetKeymap, RevealConfig, CloseViewer,
            ViewerText, ViewerHex, ViewerFind, ViewerFindNext, MenuUp, MenuDown, OpenPalette,
            ToggleTerminal,
        ]
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_action_is_bridged() {
        for a in Action::ALL {
            let boxed = boxed(*a);
            assert!(boxed.name().ends_with(&format!("{a:?}")), "{} vs {:?}", boxed.name(), a);
        }
    }

    #[test]
    fn every_default_chord_parses_in_gpui() {
        for b in fc_core::Keymap::defaults().bindings {
            let ks = b.chord.gpui_keystroke();
            assert!(gpui::Keystroke::parse(&ks).is_ok(), "GPUI rejects {ks}");
        }
    }
}
