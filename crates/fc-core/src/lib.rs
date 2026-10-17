//! Ferrous Commander core: everything that is not drawing pixels.
//!
//! The behavior follows `SPEC.md` at the repository root. The GPUI front end lives in
//! the `fc-app` crate and only translates input and state into views.

pub mod archive;
pub mod diskmap;
pub mod entry;
pub mod fileicon;
pub mod format;
pub mod fuzzy;
pub mod keymap;
pub mod ops;
pub mod panel;
pub mod pasteboard;
pub mod pattern;
pub mod platform;
pub mod queue;
pub mod search;
pub mod settings;
pub mod sort;
pub mod theme;
pub mod viewer;
pub mod volumes;

pub use entry::{Entry, EntryKind};
pub use keymap::{Action, Chord, Keymap};
pub use panel::{ListOptions, Location, LockState, Panel, Tab, ViewMode};
pub use sort::{SortKey, SortSpec};
