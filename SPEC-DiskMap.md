# Disk Map: behavioral specification

Clean-room specification of a disk-usage analyzer for Ferrous Commander, modelled on
**DaisyDisk 4.34.2** by Software Ambience Corp. It was derived with the REA loop from the
shipped artifact and the vendor's public user guide. It describes *behavior only*. No code,
artwork, nibs, strings files or other assets were copied. The implementation
(`crates/fc-core/src/diskmap.rs`, `crates/fc-app/src/diskmap.rs`) is written from this
document alone.

Disk Map is an **additional tool**. It lives in its own window and does not change how the
file panels, dialogs or key map of the main window behave (§1).

The target was `DaisyDisk.zip` from the vendor site (`daisydiskapp.com/download/DaisyDisk.zip`,
sha256 `45ef4639…4257`). The main executable is universal (x86_64 + arm64), sha256
`b82db65a…417c`.

---

## 0. Evidence index

| ID | Source | Tool | What it established |
|----|--------|------|---------------------|
| D1 | `Contents/Info.plist` | `plutil` | Bundle id `com.daisydiskapp.DaisyDiskStandAlone`, version 4.34.2, category utilities, document type "Folders" (role Viewer): folders and disks can be dropped on the app to scan them |
| D2 | `Contents/MacOS/DaisyDisk` (arm64 slice) | `rea inspect-native-dispatch-metadata` | 479 Objective-C classes. The report page is `ReportPageController`, which owns a `TreePieViewController` (the sunburst, drawn as `TPSegment`s with `ring` and `span`), a `SummaryViewController` (the list beside it) and a `CollectorViewController`, all synchronized by a `SelectionController` (`docs/evidence/daisydisk/classes.txt`) |
| D3 | Same, `DirectoryItem`, `Item`, `SpecialItemFactory` | same | The model is a size tree. `sortChildItemsBySizeRecursively`, `calculateSizeInBytesWithoutCollected`, `isPackage` / `setPackageExpanded:`. Pseudo-items: free, available, hidden, purgeable, reserved ("other volumes"), other users, unknown ("still hidden"), snapshots, and a *smaller items group* per directory (`factorySmallerItemsGroupItemWithParentDirectory:smallerItems:`) |
| D4 | `+[TPUtils nextVisibleItemThroughLevelsFromItem:uptoDirectory:includingCollected:minRelativeSize:]` at `0x10006d1f8` | `objdump` (no decompiler provider was available to REA) | The map shows an item only if its size relative to the map root is at least `minRelativeSize`. The walk goes to the next sibling, skips collected items unless asked to include them, and climbs to the parent after the last sibling, never above the map root |
| D5 | `NavigationHistory` ivars and selectors | D2 tool | Separate undo/redo stacks of directories, plus `pruneDeletedDirectories` and `pruneHiddenDirectories`. History survives rescans by path (`savePathStacksToAscendentItem:` / `resolveSavedPathStacks…`) |
| D6 | `Collector`, `CanBeCollectedItemVisitor`, `FileDeletionRequest` | D2 tool | The collector holds items and restores them by path after a rescan. Whether an item can be collected depends on its system type. Deletion runs in the background with progress phases |
| D7 | `TweakInspectorController` (debug panel) | D2 tool | Sixteen ring-width and ring-spacing fields (0–15), so ring geometry is per ring; zoom depth and duration, expand/collapse animation durations, a smaller-items fill alpha |
| D8 | `en.lproj/MainMenu.nib`, `ReportPageView.nib` | `rea decode-interface-builder` | Menu titles, key equivalents and action selectors (`docs/evidence/daisydisk/menu-wiring.txt`) |
| D9 | `en.lproj/*.strings` | `plutil` | UI vocabulary: scanning states, error titles, collector states, special-space explanations, size units B–PB (`docs/evidence/daisydisk/string-keys.txt`, keys only) |
| D10 | Vendor user guide, "Understanding the disk map", "Deleting files", "Keyboard shortcuts" | WebFetch | Coloured segments are folders and grey ones are files. Pointing at a folder shows its contents in the sidebar. Click drills down and the centre goes up. Items too thin to show are merged into semi-transparent segments. The collector refuses system folders and the home folder. Deletion starts after a 5-second countdown and is permanent. Hotkeys (§6) |

REA's runtime capture of the native UI was not used: the app is trial-gated and no screen
grant was given. Visual layout comes from D2/D8/D10, not from screenshots.

---

## 1. Integration with Ferrous Commander

* New command **`DiskMap`** (id `fc.DiskMap`, label "Disk map…", group *Other*). Default key
  **⇧⌘M** (a Ferrous Commander addition; Commander One has no such command). It can be
  rebound in Settings ▸ Hotkeys like any other command.
* It is reachable from **Commands ▸ Disk Map…**, the command palette (featured), and a
  title-bar button with a sunburst glyph.
* Running it opens the **Disk Map window**, or brings it to the front if it is already open.
  There is at most one Disk Map window. The folder of the active panel becomes the
  window's suggested scan target ("This folder"). If exactly one folder is marked, that
  folder is suggested instead.
* Nothing else in the main window changes. Disk Map never changes a panel's folder unless
  the user picks **Show in Panel** (§7).

## 2. Window and pages

One window (default 1080×720, minimum 760×520) with three pages:

1. **Disks and Folders** (start page).
2. **Scanning** (progress).
3. **Map** (results).

The title bar shows the Disk Map glyph and title. On the Map page it also shows back,
forward and up buttons, the breadcrumb path from the scan root to the current folder (each
crumb navigates), a **Rescan** button and a **Disks and Folders** button.

### 2.1 Disks and Folders
* **Folders** section:
  * *This folder*: the suggested folder from §1, if any.
  * *Home*.
  * *Choose Folder…*: the system open panel, directories only.
* **Disks** section: every mounted volume (as listed by the main window's drive list). Each
  shows its name, a capacity gauge, and "<used> used of <total>". With purgeable space
  known, the label also says "<free> free + <purgeable> purgeable" (D9 "free + purgeable").
* Every row has a **Scan** button. Clicking the row also scans.
* Dropping a folder or volume on the page (from Finder or from a Ferrous Commander panel)
  scans it (D1, D9 "drop the folder here to scan it").
* A failed scan shows an error banner with its reason (§9).

### 2.2 Scanning
* Shows the target's name, an animated progress ring, and live counters: items scanned,
  bytes counted, elapsed time. It also shows the folder currently being read.
* When the target is a volume, the ring fills by `bytes counted / bytes used on the volume`
  (capped at 99% until done). For folders the ring spins without a fraction.
* **Cancel** (or Esc) stops the scan and returns to Disks and Folders.
* When the scan finishes, the Map page opens on the scan root and the map blooms in (§5.5).

### 2.3 Map
* Left: the sunburst, as large as fits, centred.
* Right: the **sidebar** (300 pt): title item, list of children, footer.
* Bottom: the **collector** strip (§8).

## 3. Scanning

Input: a directory path. Output: a size tree.

* The tree contains every file system object under the root: files, folders, packages and
  symlinks. Hidden (dot) files are included, because disk usage counts them.
* **Size** is allocated size on disk (`st_blocks × 512`), the same figure Finder calls "on
  disk". A directory's size is its own allocation plus the sizes of its descendants.
* **Symlinks** are never followed. They count their own allocation.
* **Hard links**: a file with a link count above 1 is counted once per (device, inode). Later
  paths to the same inode are kept with size 0.
* **Mount points** below the root are not entered and are left out of the tree: anything in
  the system mount table other than the root itself, such as `/Volumes/*` and
  `/System/Volumes/*`. APFS firmlinks (`/Users`, `/Applications` on the system volume) are
  not mount points, so they are scanned.
* **Restricted folders**: a folder that cannot be listed (permission denied, or no Full Disk
  Access) is kept with its own size and the *restricted* flag (D9 "Restricted folder").
* **Packages** (`.app`, `.bundle`, `.photoslibrary`… the same list the panels use) are
  scanned completely but marked as packages (D3 `isPackage`).
* Children are sorted by size, largest first; ties sort by name (D3
  `sortChildItemsBySizeRecursively`).
* Each node also records its number of descendant files ("items").
* The scan runs on background threads (one per core, at most 8). Progress (items, bytes,
  current folder) can be read at any time. Cancellation takes effect within one directory
  listing.
* **Rescan current folder** (⌘R) rescans only the current folder's subtree and grafts the
  result into the tree, updating ancestor sizes by the difference. **Rescan** (⇧⌘R)
  rescans from the root. Both keep the current folder and history when those paths still
  exist (D5).

### 3.1 Volume summary
When the scan root is a volume's mount point, the map root gets pseudo-items (D3):

| Item | Value | Shown |
|------|-------|-------|
| free space | `f_bavail × f_bsize` | sidebar footer gauge only |
| purgeable space | important-usage free space (`NSURLVolumeAvailableCapacityForImportantUsageKey`) minus free space, if positive | ring 1 and list, after real items |
| hidden space | used − purgeable − scanned, if ≥ 0.5% of capacity | ring 1 and list, after real items |

where used = capacity − free. Pseudo-items cannot be opened or collected. Selecting one
shows its explanation in the sidebar footer (D9 `Report_Hidden_*`, `Report_Purgeable_*`,
paraphrased). For example, hidden space is taken by other volumes in the same container,
system files, snapshots, or folders the app could not read, and granting Full Disk Access
reveals more.

## 4. Tree model

* Node: name, parent, children (sorted), kind (folder, package, file, symlink), `size`,
  `shown` (size minus collected descendants, D3 `calculateSizeInBytesWithoutCollected`),
  `items`, flags (restricted, hard-link duplicate, collected).
* Path of a node = the scan root path joined with the names from the root down.
* **Lookup by path** returns the node, if present.
* **Remove** (after deletion) detaches the node and subtracts its size and items from every
  ancestor.
* **Collect / uncollect** set the collected flag and subtract or re-add the node's `shown`
  size along its ancestors' `shown`. Collected nodes are skipped by the map and the list.

## 5. Sunburst

### 5.1 Geometry
* The map root is the current folder. It is drawn as the **centre disc**, whose label shows
  the root's name and size.
* Ring *k* (1-based) shows the descendants at depth *k* below the root. At most **7 rings**
  are drawn.
* The centre radius is 22% of the outer radius *R*. The geometry is laid out for as many
  rings as the deepest one shown, but never fewer than 3, so shallow folders don't balloon.
  Ring widths shrink outward, each 0.86 of the previous one, and the rings fill *R*
  exactly. A 1.5 pt gap separates rings (D7: per-ring widths and spacings).
* Angles start at 12 o'clock and run clockwise.
* A child's sweep = parent's sweep × child.shown / parent.shown. Children are laid out in
  sorted order from the parent's start angle. The children may not fill the parent's sweep
  (a folder's own allocation, or collected items), and the gap stays empty.
* At ring 1 the reference is the root: sweep = 360° × child.shown / root.shown. With a
  volume summary (§3.1), the denominator is root.shown + purgeable + hidden, and those
  pseudo-items take their sweep after the real children.

### 5.2 Visibility and "smaller objects"
* An item is drawn as its own segment only if its sweep is at least **0.6°**, that is, at
  least 1/600 of the map root (D4 `minRelativeSize`).
* Because children are sorted by size, the hidden ones form a run at the end of their
  parent's span. Together they become one **smaller objects** segment, drawn translucent
  grey (D10, D7 `smallerItemsSegmentFillAlpha`), when that run's total sweep is at least
  0.1°.
* Recursion stops at files, symlinks, packages, restricted folders, empty folders,
  smaller-objects segments and ring 7.

### 5.3 Colour
* Folders are coloured. Files, symlinks and hard-link duplicates are grey (D10).
* A folder segment's hue comes from the angle of its midpoint around the circle, so a
  subtree stays one colour family and neighbours differ. Saturation falls and lightness rises
  with depth.
* Packages take their hue the same way but are less saturated.
* Restricted folders are violet and drawn with lower opacity.
* Smaller objects are grey at 45% opacity. Purgeable space is pale green and hidden space is
  dark grey.
* The hovered segment and its descendants are brightened. In the list, each row's swatch
  uses the same colour as its segment.

### 5.4 Pointer
* **Hover** over a folder or package segment: the sidebar shows *that* folder's contents.
  Over a file segment it shows the file's parent with the file highlighted (D10 "point to
  any folder to see its content on the sidebar"). The centre label shows the hovered
  item's name and size. Leaving the map returns the sidebar to the current folder.
* **Click** a folder or package segment to make it the current folder (drill down). Click a
  file segment to select it. Click a smaller-objects segment to drill into its parent and
  expand that list group.
* **Click the centre disc** to go up one level. At the scan root this does nothing.
* **⌘-click** a segment: Show in Finder (D10 hotkeys).
* **Right-click**: context menu (§7).
* **Drag** a segment onto the collector to collect it (D10).

### 5.5 Animation
* On every change of the current folder, and when a scan finishes, the map **blooms**. Over
  320 ms with ease-out, each segment's start angle and sweep scale from 0 to their final
  values (D2 `blossomOut`, D7 expand/collapse durations).
* Hover highlights change immediately.

## 6. Sidebar list

* The **title item** at the top shows the shown folder (current, or hovered per §5.4): its
  name, size, and item count.
* Rows are its children in size order, skipping collected ones. Each row has a colour
  swatch, the file-type tile used in the panels, the name, and the size.
* Children under 1/600 of the shown folder are grouped into one row, **smaller objects…**,
  with their count and total (D3, D10). That is the map's own rule (§5.2) applied as if the
  shown folder were the map root, so for the current folder the list and ring 1 agree. If
  the group would hold a single item, that item is listed instead.
* Activating the smaller-objects row expands it in place. It stays expanded for that folder
  until the folder changes (D8 "Expand 'smaller objects'").
* At the volume root, purgeable and hidden space rows follow the real items (§3.1).
* Restricted folders show a lock badge.
* **Footer**: at a volume root, a gauge with used, purgeable and free space. Otherwise,
  the scan root's total and the time of the scan. An explanation appears when a pseudo-item
  is selected.

**Selection**: one item at a time, shared between the map and the list (D2
`SelectionController`). Selecting a row highlights its segment, and the reverse.

| Key | Action (D8, D10) |
|-----|------------------|
| ↑ / ↓ | previous / next row |
| PageUp / PageDown, Home / End | page, first, last |
| ↩ | open the selected folder, or expand smaller objects |
| ⌘↑, ⌫ | enclosing folder |
| ⌘[ / ⌘] | back / forward in history |
| Space | Quick Look of the selected item |
| ⌘⌫ | move the selected item to the collector, or out of it when the collector is focused |
| ⌘R | rescan current folder |
| ⇧⌘R | rescan from the root |
| ⇧⌘↑ | back to Disks and Folders |
| Esc | collapse the collector if expanded, otherwise clear the selection |

Double-clicking a folder row opens it. Double-clicking a file row opens it with its default
application.

## 7. Navigation and commands

* **History**: separate back and forward stacks (D5). Opening a folder pushes the current
  one onto *back* and clears *forward*. Going up counts as opening the parent. Back and
  forward skip folders that were deleted.
* **Context menu** (map or list): Open (folders), Show in Panel (reveals the item in the
  main window's active panel and brings that window forward), Show in Finder, Quick Look,
  Open in Terminal (folders), Move to Collector / Remove from Collector, Rescan This Folder.
  Items that do not apply are disabled.

## 8. Collector

* **Collect** an item by dragging it onto the collector strip, with ⌘⌫, or from the context
  menu (D10). Collected items disappear from the map and list, and their ancestors shrink
  (§4).
* **Cannot be collected** (the strip explains why while dragging, and ⌘⌫ shows a toast):
  * the scan root;
  * pseudo-items and smaller-objects groups (D10: expand them first);
  * protected paths: `/`, `/System`, `/Library`, `/Applications`, `/Users`, `/usr`, `/bin`,
    `/sbin`, `/private`, `/etc`, `/var`, `/tmp`, `/Volumes`, `/opt`, `/cores`, the home
    folder, and `~/Library` (D10: "/System, /Library and the current user's home
    folder", D9 `Report_Collector_Message_System`).
* Collecting a folder releases any of its descendants already in the collector. The
  collector only ever holds top-most items.
* **Strip**: when empty it reads "Drag items here to collect them". Otherwise it shows the
  item count and total, a **Delete** button, a mode toggle, and an expand chevron. Expanded,
  it lists the collected items with their size, a × button to release each, and **Remove
  All**.
* **Mode**: *Move to Trash* (default) or *Delete immediately*. Only *Delete immediately*
  frees space at once. The choice is remembered for the session.
* **Delete** starts a **5-second countdown** ("Deleting in 5…") with a **Stop** button
  (D9, D10). When it reaches zero, deletion runs in the background with a progress bar.
* After deletion:
  * Items that were deleted are removed from the tree (§4), and free space is re-read.
  * Items that failed stay in the collector, marked with their error.
  * A toast reports what was freed or moved to the Trash ("Freed 1.2 GB").
* Releasing an item restores it to the map and list.

## 9. Errors and edge cases

| Case | Behavior |
|------|----------|
| Target does not exist or is not a folder | Disks and Folders shows "Cannot scan <name>: <reason>" |
| Root folder cannot be listed | Same, plus a "Full Disk Access…" button that opens System Settings ▸ Privacy & Security ▸ Full Disk Access |
| Empty folder | Centre reads "This folder is empty", and the list is empty |
| Folder deleted outside the app | Rescan drops it. History and collector prune it |
| Scan cancelled | Back to Disks and Folders, no result kept |
| Delete fails for some items | They stay in the collector with the error text, and the rest are removed |
| Window closed during a scan | The scan is cancelled |
| Window closed during the countdown | Nothing is deleted |
| Window closed during deletion | Deletion finishes in the background |

## 10. Deliberate differences from DaisyDisk

* Deletion defaults to the Trash, consistent with the rest of Ferrous Commander. Permanent
  deletion, DaisyDisk's only mode, is one toggle away.
* No privileged helper ("Scan as Administrator"), cloud drives, snapshot listing, purge
  command, saved scan results, starred folders, sharing or score messages, or licensing.
* No live refresh watchdog. Use ⌘R.
* Colours, geometry constants, thresholds and the bloom animation are this project's own
  choices, guided by D4/D7/D10. They are not copied values.

## 11. Acceptance tests

Core (`crates/fc-core/src/diskmap.rs`, unit tests on generated folder trees):

1. A scan sums allocated sizes bottom-up. The root equals the sum of its children plus its
   own allocation. Children are sorted by size, largest first.
2. Hard-linked files count once. Symlinks are not followed.
3. An unreadable folder is kept and flagged restricted. The scan still completes.
4. Cancelling a scan stops it and reports cancellation.
5. Layout: ring-1 sweeps are proportional to size and sum to 360° for a folder with no own
   allocation. Children sit inside their parent's span. Ring depth is capped at 7.
6. Items under 1/600 of the root merge into one smaller-objects segment per parent, placed
   after the visible siblings.
7. Hit testing maps a point to the centre, to the right segment, or to nothing.
8. Collect, uncollect and remove keep `shown` and `size` consistent along ancestors.
   Collecting a folder releases collected descendants.
9. Protected paths, the scan root and pseudo-items cannot be collected.
10. History: back and forward skip deleted folders. Opening a folder clears forward.
11. Rescanning a subtree grafts it and corrects ancestor sizes.

Front end (smoke run, `FC_SMOKE=1`): open Disk Map from the command, scan the sandbox,
see the map, drill down and up, collect a file, delete it to the Trash after the countdown,
check it is gone from disk and from the tree, and close the window.
