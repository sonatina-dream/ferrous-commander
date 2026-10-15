# Ferrous Commander: behavioral specification

Clean-room specification of the dual-panel file manager **Commander One 3.18 (build 4006)**
by Electronic Team, Inc. It was derived with the REA loop from the shipped artifact and its
observed runtime behavior. It describes *behavior only*. No code, artwork, nibs, strings files or
other assets were copied. The implementation in this repository (`crates/`) is written from
this document alone.

The target was the notarized DMG from the vendor CDN
(`cdn.electronic.us/products/commander/mac/download/commander.dmg`,
sha256 `ccdada39…c9c9`). The main executable is universal (x86_64 + arm64), sha256 `56dc8044…88e5`.

---

## 0. Evidence index

| ID | Source | Tool | What it established |
|----|--------|------|---------------------|
| E1 | `Contents/Info.plist` | `rea inspect-plist` | Bundle id `com.eltima.cmd1`, version 3.18/4006, min macOS 10.13 (plist) / 11.0 (arm64 build metadata), document types `public.data` + `public.directory` (role Viewer), NSServices "reveal-in-commander1" for `public.item` |
| E2 | `Contents/MacOS/Commander One` (arm64 slice) | `rea inspect-macho`, `rea inspect-native-dispatch-metadata` | 407 ObjC classes. Most belong to an embedded VT100 terminal (`VT100Terminal`, `PTYSession`, `ETerm*`). The file manager is Swift module `TCX` (83 classes, e.g. `TCX.TCXPanel`, `TCX.TCXPanelTab`, `TCX.TCXPanelTabFull/Brief/Thumbs`, `TCX.FileQueue`, `TCX.TCXSearchPlugin`, `TCX.TCXOpenWith`, `TCX.CopyFile`) |
| E3 | Same binary, string pool + Swift reflection | `strings`, `swift-demangle` | Command inventory: 62 `TCX…Action` IDs plus the `KeyManager` enum (see `docs/evidence/action-ids.txt`) |
| E4 | `Frameworks/TCXCore.framework` | `rea inspect-native-dispatch-metadata` | Pluggable file-system layer: `FSPluginBase`, `FSPluginArchive`, `FSPluginMountable`, `folderContent`, `fileAttributes`, `fileSystemAttributes`, `baseOperation` |
| E5 | `Contents/PlugIns/*.bundle` | listing | FS plugins: 7z/zip, RAR, TAR, FTP/SFTP, WebDAV, S3, Backblaze, OpenStack, Dropbox, Google Drive, OneDrive, pCloud, Mega, Bonjour, iOS, MTP, Processes |
| E6 | `Resources/*.nib` (NIBArchive) | `rea decode-interface-builder` | Window, panel, tab, dialog structure and target/action wiring (`docs/evidence/nib-*.txt`) |
| E7 | `Resources/en.lproj/Localizable.strings` | `plutil` | 826 UI keys: hotkey group names, preference options, dialog semantics, status-line formats |
| E8 | Runtime: defaults domain `com.eltima.cmd1` after first launch | `defaults export` | **Exact default key map**, `KeyManager.keymap` = `{keyStorage:[Int], pairStorage:{Int:String}}`, 87 entries (`docs/evidence/default-keymap.observed.json`) |
| E9 | Runtime: same domain after a graceful quit | `defaults export` | Persisted session shape: `SavedLeftTabs`/`SavedRightTabs`, `historyLeft`/`historyRight`, `CommandsHistory`, window frame (`docs/evidence/persisted-state.observed.json`) |
| E10 | Vendor knowledge base, "Preferences – Commander One" | WebFetch | F8/Delete trash semantics, Shift+F8 permanent delete, Num+ "files only" option, Space computes folder size |

REA's native UI capture (`capture-native-ui-scenario`) and direct AX reads were **blocked**:
this host has no Screen Recording or Accessibility grant. Visual layout therefore comes from
the nib graphs (E6), not from screenshots.

---

## 1. Product shape

* A single-window, two-panel ("orthodox") file manager with one **active** panel.
* Each panel holds an ordered list of **tabs**. Each tab is one folder location with its own
  view mode, sort, history and lock state (E9: `SavedLeftTabs[i]` holds `path`, `mode`,
  `sortKey2`, `sortOrder`, `lockState`, `isActive`, `viewOptions.columns order2/size`).
* Window chrome, top to bottom (E6 `TCXWindow.nib`):
  1. Toolbar: history back/forward segmented control (`historyAction:`), view-mode segmented
     control (`switchViewAction:`, tags 1=full, 2=brief, 3=thumbs), Compress (`createArchiveAction:`),
     Connections (`showFTPConnections:`), Get Info (`showInfoAction:`),
     Quick Look (`switchQuicklLookPanel:`), Search (`searchAction:`) and the operations-queue
     button (`showQueueList:`).
  2. Two panels side by side. Each panel has a **drive bar**: a volume popup
     (`diskInfoListAction:`), an eject button (`ejectDriveAction:`) and a free-space label in the
     format `"<free> of <total> free"` (E7 `TCXPanel.diskInfoLabel`).
  3. Inside each panel: a tab bar, then the tab content. The tab content holds a path control
     whose segments are clickable (`pathControlClick:`), Favorites and History buttons
     (`favoriteActions:`, `historyActions:`), the file list, and a footer status line.
  4. Command line: an editable combo box with history (`commandPromtAction:`).
  5. Function-key button bar with six buttons wired, in order, to View, Edit, Copy, Move,
     New Folder and Delete (`viewAction:` … `deleteAction:`). Their default keys are F3–F8 (E8).
* Every chrome element can be toggled (E7 hotkey names `TCXViewShowButtons`,
  `TCXViewShowCommandLine`, `TCXViewShowDiskList`, `TCXViewShowDiskInfo`, `TCXViewShowPathBar`,
  `TCXViewShowSelectionInfo`).

## 2. File list

### 2.1 Entries
* The first row is a synthetic `..` parent entry, unless the folder is the file-system root.
* Each entry has: name, extension, size, modified ("date"), created, added, accessed (opened),
  permissions, kind (E6 `TCXPanelTabFull.nib` column identifiers; E7 `TCXPanelTab.column.*`).
* Directories show no byte size until it has been computed (§5.2). Packages such as `.app`
  behave like files: Return opens them, and Alt+Return enters them (E7 `TCXEnterItem` =
  "Show package contents").
* Hidden items are dot-files and items with the BSD `UF_HIDDEN` flag. They are shown only when
  "Show Hidden Files" is on (`TCXViewShowHidden`, toolbar tooltip E7).

### 2.2 View modes (E7, E6)
| Mode | Hotkey | Behavior |
|------|--------|----------|
| Full | ⌘1 | Table with resizable, reorderable columns. Default order is name, ext, size, date (modified), created, added, accessed, permissions, kind. Default widths come from E9 `columns size`: name 127.5, ext 40, size 65, dates 130, permissions 80, kind 80 |
| Brief | ⌘2 | Names only, flowing top-to-bottom then left-to-right in columns. Column width can be fixed, limited or maximum (E7 `view.brief-mode.*`) |
| Thumbs | ⌘3 | Icon grid with thumbnails |

### 2.3 Sorting
* Keys: name, extension, size, date (modified), created, added, accessed, kind, permissions
  (E7 `TCXPreferencesWindow.<key>` = "Sort By …").
* Hotkeys (E8): ⌥⌘1 name, ⌥⌘2 extension, ⌥⌘3 size, ⌥⌘4 date, ⌥⌘5 created, ⌥⌘6 added,
  ⌥⌘7 accessed, ⌥⌘8 kind, ⌥⌘9 permissions; ⌥⌘A ascending, ⌥⌘D descending.
* Clicking a column header sorts by that column. Clicking it again reverses the direction.
* "Show folders on top" (E7 `view.folders-top`) puts directories first, each group sorted on its
  own. `..` always stays first.
* Name comparison is case-insensitive and natural: `file2` sorts before `file10`. Ties fall
  back to name.
* The default sort is name ascending (E9 `sortKey2:"name"`, `sortOrder:true`).

### 2.4 Status lines
* Footer (selection info), E7 `TCXPanelTab.contentInfoLabel`:
  `"<sel bytes> / <total bytes> in <sel files> / <total files> file(s). <sel dirs> / <total dirs> dir(s)"`.
* Header (disk info): `"<free> of <total> free"` for the volume holding the current folder.
* Size display is either `bytes` (grouped digits) or `dynamic` (B, KB, MB, GB, TB at 1024
  steps, two decimals above KB). Columns and header/footer each have their own setting
  (E7 `view.size.*`).

## 3. Keyboard model

### 3.1 Default key map (observed, E8)
On disk each entry is `NSEvent.modifierFlags.rawValue | virtualKeyCode`. The flag bits are
shift `1<<17`, control `1<<18`, option `1<<19`, command `1<<20`, numeric pad `1<<21` and
function `1<<23`. The full table, 87 entries, is in `docs/evidence/default-keymap.observed.json`.
Grouped:

* **File operations:** F2 add to queue · F3 view · F4 edit · ⇧F4 new file · F5 copy ·
  F6 move · ⇧F6 rename · F7 new folder · F8 / ⌦ delete · ⇧F8 / ⇧⌦ permanent delete ·
  ⌘D duplicate · ⌘L symlink · ⌥⌘L alias · ⌃F5 compress · ⌃⌥F5 compress with options ·
  ⌃F9 extract · ⌘I get info · ⌥⌘I Finder info · ⌥⌘O show in Finder · ⌘Y Quick Look ·
  Return open · ⌥Return enter package.
* **Selection:** Num+ select by pattern · Num− unselect by pattern · Num* invert ·
  ⌘A / ⌘Num+ select all · ⌘Num− unselect all · Space select item and compute its size ·
  § (ISO section key, kVK 10) toggle selection of the item.
* **Navigation:** ⌘[ back · ⌘] forward · ⌘↑ enclosing folder · ⇧⌘H home · ⇧⌘D desktop ·
  ⇧⌘O documents · ⇧⌘L downloads · ⇧⌘A applications · ⌥⇧⌘A system applications ·
  ⇧⌘U utilities · ⇧⌘G go to folder · ⌘K connect to server · ⌘R reload · ⌥F1 / ⌥F2 change
  drive in the left / right panel.
* **Panels and tabs:** Tab switches the active panel · ⌘T duplicate tab · ⇧⌘T new tab ·
  ⌃Tab / ⌃⇧Tab next / previous tab · ⌥⇧E swap panels · ⌥E equalize panels · ⌘N new window ·
  ⌘E eject.
* **Interface:** ⌘1/⌘2/⌘3 view modes · sort keys as in §2.3 · ⌥⇧⌘H hidden files ·
  ⌥⌘F full screen · ⇧⌘B buttons bar · ⇧⌘P command line · ⌥⌘S disk list · ⇧⌘I disk info ·
  ⌥⌘P path bar · ⇧⌘S selection info · ⌘F connections · ⌃⌘F favorites · ⌃O terminal.
* **Command line:** ⌃Return pastes the item name · ⌃⇧Return pastes the item path ·
  ⌥⌘C / ⌃⌘C copy name / path to the clipboard. Selecting several items joins their names
  with newlines (E7 `tcxpanel-copy-file-name-plural`).
* **Search:** ⌥F7 search · ⌃⌥F7 Spotlight search.

Actions are grouped in Preferences ▸ Hotkeys as *File operations, Navigation, Panels and tabs,
Selection, Interface, Other* (E7 `keys.groups.*`). Each action has a primary and a secondary
key (E7 `key`, `key2`), and "Reset to defaults" restores the table. "Ignore system settings for
F1–F12" lets F-keys work without fn (E7, E10).

### 3.2 Built-in list keys (not remappable)
↑/↓ move the cursor. PgUp/PgDn move one page. Home/End jump to the first or last row. In Brief
mode ←/→ move one column. Shift+movement extends the selection. Typing printable characters
jumps to the first entry whose name starts with the typed prefix; the prefix resets after about
1 s of inactivity. Escape cancels the type-ahead or closes dialogs.

## 4. Selection model
* There is one **cursor** (the focused row) and a separate **selection** (marked) set. An
  operation acts on the selection, or on the cursor item when nothing is selected. `..` is
  never selected and never operated on. An operation with nothing to act on reports
  "Selection is empty" (E7 `select-error`).
* *Select by pattern* (Num+) asks for a mask (default `*`) and adds the matches. *Unselect by
  pattern* (Num−) removes them (E7 `select-file-expand-title` / `shrink-title`). A mask is a
  `;`-separated list of shell wildcards (`*`, `?`, `[…]`), case-insensitive. With "Use '+' on
  numeric pad to select files only" on, directories are skipped (E7, E10).
* Invert selection flips every non-`..` entry. With the files-only option on, it flips files only.
* Space marks or unmarks the cursor item, moves the cursor down, and computes a directory's
  recursive size (E7 `TCXSelectItemWithSizeAction` "Select file and check size", E10).
* Mouse: click moves the cursor, ⌘-click toggles selection, ⇧-click selects a range from the
  cursor, double-click opens, right-click opens the context menu (E7 `tcxpanel-*` items). The
  default pointer actions also map three-finger swipes left/right to previous/next tab (E9
  `PointerActions`).

## 5. Operations

### 5.1 Open / view / edit
* **Open** (Return, double-click): enters directories, follows symlinks to directories, and on
  `..` goes up, leaving the cursor on the folder just left. Files open in their default app.
  Packages open as files.
* **View** (F3): opens the configured viewer: Internal (default), Quick Look or Preview
  (E7 `general.view.app1..3`). The internal viewer has text, hex, binary, image, media and
  folder modes, plus find in text or hex (E7 `PreviewController-*`). On a folder it shows
  "Total space occupied … in N file(s), in M directories".
* **Edit** (F4): opens the file in the configured editor, "Default System Editor" by default
  (E7 `general.edit.*`).

### 5.2 Folder size
Computes the total byte count recursively without following symlinks. It shows the result in
the size column and the footer. Running it again recomputes.

### 5.3 Copy (F5) and Move/Rename (F6)
* The dialog reads "Copy N file(s) to" (or "Rename/move N file(s) to"), with an editable
  destination that defaults to the **other panel's** folder. Buttons: Copy/Move, **Queue**,
  Cancel (E7 `copy-dialog-label`, `move-dialog-label`, `button-queue`).
* Rename (⇧F6) acts on the cursor item in place. The destination field defaults to its current
  name.
* Destination resolution: an existing directory receives the items under their own names. If
  the path does not exist and exactly one item is involved, it becomes the new name (copy-as or
  rename). If it does not exist and several items are involved, a directory is created.
* Errors and edge cases (E7):
  * copying or moving a directory into itself or its own subdirectory is refused
    (`copy-error-own`);
  * copying into the same folder under the same name is refused (`copy-same-error`);
  * a read-only target is refused (`copy-same-nonwritable`).
* Name conflict prompt "File already exists": **Overwrite, Overwrite all, Skip, Skip all,
  Keep both, Cancel** (`file-exists-*`, `overwrite*`, `skip*`). For folders: Merge.
  *Keep both* picks `name 2.ext`, `name 3.ext`, ….
* Per-item error prompt: **Retry, Skip, Skip all, Abort** (`retry`, `skip`, `skipall`, `abort`).
* Changing an extension during rename asks for confirmation when that option is on
  (`rename-change-extension-warning-*`).
* Moves within one volume use rename(2). Across volumes they copy and then delete.
  Copies preserve modification times and permissions, and recreate symlinks as symlinks.
* Progress shows "Copying files i out of n (done/total) time left". Quitting with jobs in the
  queue asks for confirmation (`application-quit-queue-not-empty`).

### 5.4 Delete (F8, ⇧F8)
* Confirmation: "Are you sure you want to delete N selected files/directories?"
* With "Move to trash" on (the default), F8 and ⌦ move items to the Trash, and ⇧F8 and ⇧⌦
  delete permanently. With the option off, both delete permanently (E10).
* An optional confirmation before deleting non-empty folders (E7 `misc.confirm.del.nonempty`).

### 5.5 Create
* **New folder** (F7): asks for a name and creates the folder. With a selection, a "New Folder
  with Selection (N items)" variant moves the selected items into it (E7
  `newfolder-with-selection`). A name that already exists triggers "Folder already exists".
* **New file** (⇧F4): asks for a name and creates an empty file.
* **Duplicate** (⌘D): copies each item next to itself as `name copy.ext`, then
  `name copy 2.ext`, ….
* **Symlink** (⌘L) / **Alias** (⌥⌘L): created in the **other** panel's folder under the
  item's name.

### 5.6 Archives (E5, E7 `TCXCreateArchive.*`)
* **Compress** (⌃F5) packs the selection into an archive in the other panel, named after the
  single item or after the current folder. **Compress with options** (⌃⌥F5) asks for name,
  format, compression level (None / Fast / Normal / Max) and optional password.
* **Extract** (⌃F9) unpacks the archive at the cursor into the other panel.
* Commander One can also browse archives as folders. That is out of scope here (§10).

### 5.7 Clipboard and integration
Copy names or paths to the clipboard. Paste a name or path into the command line. Show in
Finder (reveal). Finder's Get Info window. Quick Look panel. Open in Terminal (⌃O). Eject the
volume (⌘E). Go to folder (prompt "Enter valid local path"). Connect to server (prompt with
example `smb://192.168.0.3`), E7 `TCXPanel.*`.

### 5.8 Get Info (⌘I)
A sheet with icon, name, Kind, Size (calculated recursively for folders, "N bytes" and "on
disk"), Where, Created and Modified. A permissions grid (owner/group/public × read/write/execute)
that applies chmod immediately, the numeric value (e.g. `755`), owner and group, and a "Locked"
checkbox (`uchg` flag), E6 `TCXFileInfo.nib` (9 × `permitionsChanged:`, `lockedChanged:`), E7 `fileinfo-*`.

## 6. Tabs
* New tab (⇧⌘T) opens at the current folder. Duplicate tab (⌘T) clones location, mode and sort.
  Close tab (⌘W) closes it, and the last tab in a panel cannot be closed. "Close Other Tabs" is
  also available. Next/previous tab cycles with wrap-around.
* Lock states (E7 `TCXPanel.lockTab*`, E9 `lockState`): **0 unlocked**, **1 locked** (navigating
  to another folder opens a new tab instead), **2 locked with directory changes allowed**.
  Closing a locked tab asks for confirmation (`alert-locked-tab-close`).
* The tab title is the folder's display name. "Always show tab bar" shows the bar even with a
  single tab.

## 7. History and favorites
* Each tab keeps a back/forward stack (⌘[ / ⌘]). Navigating somewhere new clears the forward
  stack. Each panel keeps a folder-history list (E9 `historyLeft/Right`) shown in a popup.
* Favorites are a user-editable list of folders ("Add to Favorites", "Edit Favorites",
  "Use full path"), E7 `TCXPanelTab.menu*Bookmark`, `TCXBookmarksWindow.*`.

## 8. Search (⌥F7), E6 `TCXSearchWindow.nib`, E7 `TCXSearchWindow.*`
* **Name search:** "Search for" mask (wildcards, or RegEx when ticked), case sensitive, depth
  (current folder / N levels / all), search in selection.
* **Text search:** find text in file contents with case-sensitive, RegEx, Entire Words and
  encoding options.
* **Spotlight:** query, optionally in contents, limited to the current or selected folders.
* Results are listed live ("Search in progress. N file(s) already found") and the search can be
  cancelled. Actions: Go to File (navigate the active panel and put the cursor on the hit), View,
  and **Feed to Active Panel**, which shows the results as a virtual folder (E7
  `TCXSearchPlugin.description` = "Search Results").
* An invalid regex shows an error and does not search (`search-error-*`, `findtext-error-*`).

## 9. Preferences and persistence
* **General:** go to root when changing drive; Num+ selects files only; calculate folder size
  on Space; move to trash; view-with (Internal/Quick Look/Preview); edit-with (default/custom
  app); language.
* **View:** size in columns (bytes/dynamic); size in header/footer (bytes/dynamic); always show
  tab bar; show folders on top; show full file name; thumbnails in full and brief modes; brief
  column mode and width.
* **Appearance:** file-list font, viewer font, colour presets and seven colours: background,
  alternate background, text, cursor, cursor text, selection, text in selection (E7 `font.*`).
* **Hotkeys:** see §3. **Extensions:** enable or disable FS plugins.
* **Advanced:** confirm before deleting non-empty folders, changing an extension, or
  drag & drop. Save on quit: panel state, opened folders, folder history, command line.
* The session (tabs, history, command history, window frame) is saved on quit and restored on
  launch (E9).

## 10. Scope of this implementation

**In scope:** everything in §1–§9 except the items below, with the stated substitutions.

| Commander One feature | This build |
|---|---|
| Embedded VT100 terminal | ⌃O / command line hand off to Terminal.app in the panel's folder |
| FTP/SFTP/WebDAV/S3/cloud/MTP/iOS plugins, encryption, process viewer | Not implemented. The core has a VFS seam for them |
| Archive browsing as folder, RAR/7z, archive passwords | Compress to ZIP (no password); extract ZIP and TAR(.gz) |
| Finder Sync extension menu items, licensing/PRO gating, telemetry, Sparkle | Not implemented (deliberately) |
| Num+/Num−/Num* distinct from main-keyboard + − * | GPUI does not expose the numeric-pad flag, so these bind to `+`, `-`, `*` in the panel context. While type-ahead is active they fall through to it |
| (additions) | Finder-style clipboard on the macOS pasteboard: ⌘C copy, ⌘X cut, ⌘V paste (duplicates in place), ⌥⌘V paste as move; ⌘⌫ / ⌥⌘⌫ trash / delete (remappable, added on top of the 87 observed defaults). ⌘P = command palette, Backspace = enclosing folder, ⌘⇧. = hidden files, ⌘W = close tab, Esc = clear type-ahead/selection (these last ones are fixed keys) |

## 11. Acceptance tests (from observed behavior)
1. The default key map contains exactly the 87 observed bindings, and decoding each raw
   E8 integer gives the documented chord (e.g. `0x820064` → ⇧F8 → Permanent delete).
2. The footer string matches the §2.4 format, and the disk label matches `"X of Y free"`.
3. Sorting: `..` first, folders on top when enabled, natural case-insensitive names, all nine
   keys in both directions.
4. Pattern selection, unselection and inversion honour masks, `;` lists and files-only.
5. Copy and move: destination resolution rules, refusal into own subtree, same-folder refusal,
   every conflict policy, Keep-both naming, metadata preservation.
6. Duplicate naming (`copy`, `copy 2`), new-folder-already-exists, delete-to-trash vs permanent.
7. Tabs: lock states 0/1/2, duplicate, close-last refused, wrap-around cycling.
8. History back/forward and forward-stack truncation. Going up leaves the cursor on the folder
   just left.
9. Search: wildcard and regex names, case sensitivity, depth limits, content search with
   whole-word matching, cancellation, invalid regex.
10. ZIP compress/extract round-trip. Size formatting in bytes and dynamic modes. Permission
    strings and octal values.
11. Session JSON round-trip.
