//! File copy/paste through the macOS pasteboard, interoperable with Finder:
//! files copied here paste in Finder and vice versa.

// objc 0.2 macros test a `cargo-clippy` feature this crate does not declare.
#![allow(unexpected_cfgs)]

use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    type Id = *mut Object;

    /// Runs `f` inside an autorelease pool (needed off the main run loop, e.g. tests).
    fn with_pool<R>(f: impl FnOnce() -> R) -> R {
        // SAFETY: standard NSAutoreleasePool lifecycle.
        unsafe {
            let pool: Id = msg_send![class!(NSAutoreleasePool), new];
            let r = f();
            let _: () = msg_send![pool, drain];
            r
        }
    }

    fn ns_string(s: &str) -> Id {
        let c = CString::new(s).unwrap_or_default();
        // SAFETY: c is a valid NUL-terminated UTF-8 string.
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    /// A macOS pasteboard: the general one, or a private named one (tests).
    pub struct Pasteboard {
        handle: Id,
        named: bool,
    }

    impl Pasteboard {
        pub fn general() -> Pasteboard {
            // SAFETY: generalPasteboard returns a shared, long-lived object.
            let handle: Id = unsafe { msg_send![class!(NSPasteboard), generalPasteboard] };
            Pasteboard { handle, named: false }
        }

        pub fn named(name: &str) -> Pasteboard {
            with_pool(|| {
                // SAFETY: pasteboardWithName returns a shared object for that name; we retain it.
                let handle: Id = unsafe { msg_send![class!(NSPasteboard), pasteboardWithName: ns_string(name)] };
                let _: Id = unsafe { msg_send![handle, retain] };
                Pasteboard { handle, named: true }
            })
        }

        /// Increments whenever any app writes to the pasteboard.
        pub fn change_count(&self) -> i64 {
            // SAFETY: plain getter.
            let n: isize = unsafe { msg_send![self.handle, changeCount] };
            n as i64
        }

        /// Replaces the pasteboard contents with file URLs for `paths`.
        pub fn write_files(&self, paths: &[PathBuf]) -> bool {
            if paths.is_empty() {
                return false;
            }
            with_pool(|| unsafe {
                // SAFETY: building autoreleased Foundation objects and handing them to AppKit.
                let urls: Vec<Id> = paths
                    .iter()
                    .map(|p| msg_send![class!(NSURL), fileURLWithPath: ns_string(&p.to_string_lossy())])
                    .collect();
                let array: Id = msg_send![class!(NSArray), arrayWithObjects: urls.as_ptr() count: urls.len()];
                let _: isize = msg_send![self.handle, clearContents];
                let ok: BOOL = msg_send![self.handle, writeObjects: array];
                ok == YES
            })
        }

        /// File paths currently on the pasteboard (empty when it holds no files).
        pub fn read_files(&self) -> Vec<PathBuf> {
            with_pool(|| unsafe {
                // SAFETY: reading autoreleased objects returned by AppKit within the pool.
                let url_class: Id = msg_send![class!(NSURL), class];
                let classes: Id = msg_send![class!(NSArray), arrayWithObject: url_class];
                let yes: Id = msg_send![class!(NSNumber), numberWithBool: YES];
                let options: Id = msg_send![class!(NSDictionary), dictionaryWithObject: yes forKey: ns_string("NSPasteboardURLReadingFileURLsOnlyKey")];
                let items: Id = msg_send![self.handle, readObjectsForClasses: classes options: options];
                if items.is_null() {
                    return Vec::new();
                }
                let count: usize = msg_send![items, count];
                let mut out = Vec::with_capacity(count);
                for i in 0..count {
                    let url: Id = msg_send![items, objectAtIndex: i];
                    let path: Id = msg_send![url, path];
                    if path.is_null() {
                        continue;
                    }
                    let utf8: *const std::os::raw::c_char = msg_send![path, UTF8String];
                    if !utf8.is_null() {
                        let bytes = CStr::from_ptr(utf8).to_bytes();
                        out.push(Path::new(std::ffi::OsStr::from_bytes(bytes)).to_path_buf());
                    }
                }
                out
            })
        }
    }

    impl Drop for Pasteboard {
        fn drop(&mut self) {
            if self.named {
                // SAFETY: releases the private pasteboard we created and retained.
                unsafe {
                    let _: () = msg_send![self.handle, releaseGlobally];
                    let _: () = msg_send![self.handle, release];
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::Pasteboard;

/// True when both lists name the same files, in any order.
pub fn same_files(a: &[PathBuf], b: &[PathBuf]) -> bool {
    let mut x: Vec<&PathBuf> = a.iter().collect();
    let mut y: Vec<&PathBuf> = b.iter().collect();
    x.sort();
    y.sort();
    x == y
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn round_trip_on_a_private_pasteboard() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a file.txt");
        let b = t.path().join("Folder");
        std::fs::write(&a, "x").unwrap();
        std::fs::create_dir(&b).unwrap();

        let pb = Pasteboard::named(&format!("dev.ferrouscommander.test.{}", std::process::id()));
        assert!(pb.read_files().is_empty());
        let before = pb.change_count();
        assert!(pb.write_files(&[a.clone(), b.clone()]));
        assert!(pb.change_count() > before);
        let back: Vec<PathBuf> = pb.read_files().into_iter().map(|p| p.canonicalize().unwrap()).collect();
        assert!(same_files(&back, &[a.canonicalize().unwrap(), b.canonicalize().unwrap()]), "{back:?}");
        assert!(!pb.write_files(&[]));
    }

    #[test]
    fn same_files_ignores_order() {
        let p = |s: &str| PathBuf::from(s);
        assert!(same_files(&[p("/a"), p("/b")], &[p("/b"), p("/a")]));
        assert!(!same_files(&[p("/a")], &[p("/a"), p("/b")]));
    }
}
