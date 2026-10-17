//! Window capture for the demo recorder (`FC_DEMO`): grabs this app's own windows,
//! never the desktop or other apps.

// objc 0.2 macros test a `cargo-clippy` feature this crate does not declare.
#![allow(unexpected_cfgs)]

use std::ffi::c_void;
use std::path::Path;

use gpui::Window;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

type CFTypeRef = *mut c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCreateImage(rect: CGRect, list_option: u32, window: u32, image_option: u32) -> CFTypeRef;
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGImageGetBytesPerRow(image: CFTypeRef) -> usize;
    fn CGImageGetBitsPerPixel(image: CFTypeRef) -> usize;
    fn CGImageGetDataProvider(image: CFTypeRef) -> CFTypeRef;
    fn CGDataProviderCopyData(provider: CFTypeRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFDataGetLength(data: CFTypeRef) -> isize;
    fn CFRelease(obj: CFTypeRef);
    fn CFURLCreateFromFileSystemRepresentation(alloc: CFTypeRef, buf: *const u8, len: isize, is_dir: u8) -> CFTypeRef;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const std::os::raw::c_char, encoding: u32) -> CFTypeRef;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithURL(url: CFTypeRef, uti: CFTypeRef, count: usize, options: CFTypeRef) -> CFTypeRef;
    fn CGImageDestinationAddImage(dest: CFTypeRef, image: CFTypeRef, props: CFTypeRef);
    fn CGImageDestinationFinalize(dest: CFTypeRef) -> u8;
}

/// The NSWindow number behind a GPUI window.
pub fn window_number(window: &Window) -> Option<u32> {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
    let view = h.ns_view.as_ptr() as *mut Object;
    // SAFETY: ns_view is a live NSView owned by GPUI; -window and -windowNumber are getters.
    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return None;
        }
        let number: isize = msg_send![ns_window, windowNumber];
        u32::try_from(number).ok()
    }
}

/// An owned CGImage of one window.
pub struct Frame(CFTypeRef);

// SAFETY: CGImage is immutable and thread-safe once created.
unsafe impl Send for Frame {}

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: we own one reference from CGWindowListCreateImage.
        unsafe { CFRelease(self.0) }
    }
}

/// Captures window `id`. `retina` selects the backing resolution (else 1×).
pub fn capture(id: u32, retina: bool) -> Option<Frame> {
    let null_rect = CGRect { x: f64::INFINITY, y: f64::INFINITY, w: 0.0, h: 0.0 };
    // kCGWindowListOptionIncludingWindow; kCGWindowImageBoundsIgnoreFraming | (Best|Nominal)Resolution.
    let options = (1 << 0) | if retina { 1 << 3 } else { 1 << 4 };
    // SAFETY: plain CoreGraphics call; the result is owned by Frame.
    let image = unsafe { CGWindowListCreateImage(null_rect, 1 << 3, id, options) };
    (!image.is_null()).then_some(Frame(image))
}

impl Frame {
    pub fn size(&self) -> (usize, usize) {
        // SAFETY: valid CGImage.
        unsafe { (CGImageGetWidth(self.0), CGImageGetHeight(self.0)) }
    }

    /// Tightly packed BGRA rows (stride removed).
    pub fn bgra(&self) -> Option<Vec<u8>> {
        // SAFETY: reading the image's backing data, released before returning.
        unsafe {
            if CGImageGetBitsPerPixel(self.0) != 32 {
                return None;
            }
            let (w, h) = self.size();
            let stride = CGImageGetBytesPerRow(self.0);
            let data = CGDataProviderCopyData(CGImageGetDataProvider(self.0));
            if data.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize);
            let mut out = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                out.extend_from_slice(&bytes[y * stride..y * stride + w * 4]);
            }
            CFRelease(data);
            Some(out)
        }
    }

    /// True when the frame is (almost) uniformly black, i.e. nothing was composited.
    pub fn is_blank(&self) -> bool {
        self.bgra().is_none_or(|px| px.chunks(4).step_by(97).all(|p| p[0] < 4 && p[1] < 4 && p[2] < 4))
    }

    pub fn save_png(&self, path: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: ImageIO calls with objects we create and release here.
        unsafe {
            let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null_mut(), bytes.as_ptr(), bytes.len() as isize, 0);
            let uti = CFStringCreateWithCString(std::ptr::null_mut(), c"public.png".as_ptr(), 0x0800_0100);
            let dest = CGImageDestinationCreateWithURL(url, uti, 1, std::ptr::null_mut());
            let ok = if dest.is_null() {
                false
            } else {
                CGImageDestinationAddImage(dest, self.0, std::ptr::null_mut());
                let ok = CGImageDestinationFinalize(dest) != 0;
                CFRelease(dest);
                ok
            };
            CFRelease(uti);
            CFRelease(url);
            ok
        }
    }
}

/// Kinds of mouse event [`post_mouse`] can send.
#[derive(Clone, Copy)]
pub enum MouseKind {
    Move,
    LeftDown,
    LeftUp,
}

/// Posts a real mouse event for `window` to this app's own event queue, so it travels
/// through AppKit and GPUI exactly like a click on the hardware would (smoke run).
/// `at` is in GPUI window coordinates.
pub fn post_mouse(window: &Window, kind: MouseKind, at: gpui::Point<gpui::Pixels>, clicks: isize) -> bool {
    use objc::runtime::{Object, NO};
    use objc::{class, msg_send, sel, sel_impl};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }

    let Some(number) = window_number(window) else { return false };
    let height: f32 = window.viewport_size().height.into();
    // AppKit window coordinates start at the bottom left.
    let location = NSPoint { x: f32::from(at.x) as f64, y: (height - f32::from(at.y)) as f64 };
    let event_type: usize = match kind {
        MouseKind::LeftDown => 1,
        MouseKind::LeftUp => 2,
        MouseKind::Move => 5,
    };
    // SAFETY: builds an autoreleased NSEvent and hands it to the shared NSApplication.
    unsafe {
        let event: *mut Object = msg_send![class!(NSEvent),
            mouseEventWithType: event_type
            location: location
            modifierFlags: 0usize
            timestamp: 0.0f64
            windowNumber: number as isize
            context: std::ptr::null_mut::<Object>()
            eventNumber: 0isize
            clickCount: clicks
            pressure: 1.0f32];
        if event.is_null() {
            return false;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, postEvent: event atStart: NO];
    }
    true
}
