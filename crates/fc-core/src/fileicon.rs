//! Finder icons rendered to PNG, so the panels can show applications with their real icon.

// objc 0.2 macros test a `cargo-clippy` feature this crate does not declare.
#![allow(unexpected_cfgs)]

use std::path::Path;

/// The icon Finder shows for `path`, rendered as a `px`×`px` PNG. Safe to call from any
/// thread; every AppKit object involved lives in a local autorelease pool.
#[cfg(target_os = "macos")]
pub fn icon_png(path: &Path, px: u32) -> Option<Vec<u8>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    use objc::runtime::{Object, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSRect {
        origin: NSPoint,
        size: NSSize,
    }

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        static NSDeviceRGBColorSpace: *mut Object;
    }

    // NSCompositingOperationSourceOver, NSBitmapImageFileTypePNG.
    const SOURCE_OVER: usize = 2;
    const PNG: usize = 4;

    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let side = px.clamp(16, 1024) as f64;
    // SAFETY: plain AppKit calls on objects created here; the bitmap context is made
    // current only on this thread and restored before returning.
    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let out = (|| {
            let s: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()];
            let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
            let image: *mut Object = msg_send![workspace, iconForFile: s];
            if image.is_null() {
                return None;
            }
            let rep: *mut Object = msg_send![class!(NSBitmapImageRep), alloc];
            let rep: *mut Object = msg_send![rep,
                initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>()
                pixelsWide: side as isize
                pixelsHigh: side as isize
                bitsPerSample: 8isize
                samplesPerPixel: 4isize
                hasAlpha: YES
                isPlanar: NO
                colorSpaceName: NSDeviceRGBColorSpace
                bytesPerRow: 0isize
                bitsPerPixel: 0isize];
            if rep.is_null() {
                return None;
            }
            let context: *mut Object = msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: rep];
            let _: () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
            let _: () = msg_send![class!(NSGraphicsContext), setCurrentContext: context];
            let rect = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: side, height: side } };
            let whole = NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 0.0, height: 0.0 } };
            let _: () = msg_send![image, drawInRect: rect fromRect: whole operation: SOURCE_OVER fraction: 1.0f64];
            let _: () = msg_send![class!(NSGraphicsContext), restoreGraphicsState];
            let properties: *mut Object = msg_send![class!(NSDictionary), dictionary];
            let data: *mut Object = msg_send![rep, representationUsingType: PNG properties: properties];
            let png = if data.is_null() {
                None
            } else {
                let len: usize = msg_send![data, length];
                let bytes: *const u8 = msg_send![data, bytes];
                (len > 0 && !bytes.is_null()).then(|| std::slice::from_raw_parts(bytes, len).to_vec())
            };
            let _: () = msg_send![rep, release];
            png
        })();
        let _: () = msg_send![pool, drain];
        out
    }
}

#[cfg(not(target_os = "macos"))]
pub fn icon_png(_path: &Path, _px: u32) -> Option<Vec<u8>> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn renders_application_icons_off_the_main_thread() {
        // Tests run on worker threads, like the panels' background loader.
        let app = ["/System/Applications/Calculator.app", "/System/Applications/Utilities/Terminal.app", "/System/Library/CoreServices/Finder.app"]
            .into_iter()
            .find(|p| Path::new(p).exists())
            .expect("a stock application");
        let png = icon_png(Path::new(app), 64).expect("an icon");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // IHDR: width and height are the requested size.
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 64);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 64);
    }
}
