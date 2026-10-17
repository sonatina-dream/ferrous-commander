//! Dragging files out of the window to other apps: a browser upload, Mail, the Desktop.
//!
//! Inside the window GPUI's own drag is used, so dropping on panels, folders and the Disk
//! Map collector works as before. When the pointer leaves the window while dragging files,
//! the drag is handed to macOS as a regular file drag, the way Finder starts one. It offers
//! only the copy operation, so dropping on Finder or the Desktop never moves the original.

// objc 0.2 macros test a `cargo-clippy` feature this crate does not declare.
#![allow(unexpected_cfgs)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};

use gpui::{px, App, DispatchPhase, Global, MouseButton, MouseMoveEvent, MouseUpEvent, WeakEntity, Window};

use crate::workspace::Workspace;

/// The macOS window number the last native drag started from.
static DRAG_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// The files of a drag that started in a panel and may leave the window.
#[derive(Default)]
struct Armed(Option<Vec<PathBuf>>);

impl Global for Armed {}

/// A panel drag is under way and would be handed to macOS on leaving the window.
pub fn is_armed(cx: &App) -> bool {
    cx.try_global::<Armed>().is_some_and(|a| a.0.is_some())
}

/// Called when a panel drag starts.
pub fn arm(paths: Vec<PathBuf>, cx: &mut App) {
    cx.set_global(Armed(Some(paths)));
}

/// Watches mouse moves for the main window; registered on every frame while painting it.
/// macOS keeps sending a dragging mouse's moves to the window it started in, even outside.
pub fn listen(workspace: WeakEntity<Workspace>, window: &mut Window) {
    // Released inside the window: the drag stayed in the app.
    window.on_mouse_event(|_: &MouseUpEvent, phase, _window, cx| {
        if phase == DispatchPhase::Capture && is_armed(cx) {
            cx.global_mut::<Armed>().0 = None;
        }
    });
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture || !cx.try_global::<Armed>().is_some_and(|a| a.0.is_some()) {
            return;
        }
        if !cx.has_active_drag() || ev.pressed_button != Some(MouseButton::Left) {
            // The drag ended inside the window.
            cx.global_mut::<Armed>().0 = None;
            return;
        }
        let size = window.viewport_size();
        let p = ev.position;
        if p.x >= px(0.) && p.y >= px(0.) && p.x < size.width && p.y < size.height {
            return;
        }
        let Some(paths) = cx.global_mut::<Armed>().0.take() else { return };
        if begin_native_drag(window, &paths) {
            // macOS owns the drag now and will never report the button going up here.
            cx.stop_active_drag(window);
            let _ = workspace.update(cx, |ws, cx| ws.drag_handed_off(cx));
        }
    });
}

/// Starts a macOS file drag of `paths` from the mouse event being handled. Must run while
/// AppKit is delivering a left-mouse-dragged event to this window.
#[cfg(target_os = "macos")]
pub fn begin_native_drag(window: &Window, paths: &[PathBuf]) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

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

    // NSEventTypeLeftMouseDown, NSEventTypeLeftMouseDragged.
    const LEFT_DOWN: usize = 1;
    const LEFT_DRAGGED: usize = 6;
    const ICON: f64 = 48.0;

    if paths.is_empty() {
        return false;
    }
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return false };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return false };
    let view = h.ns_view.as_ptr() as *mut Object;
    let Some(source) = drag_source() else { return false };

    // SAFETY: AppKit calls on the main thread, during the mouse event being dispatched;
    // the dragging items are released once the array holds them.
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let event: *mut Object = msg_send![app, currentEvent];
        if event.is_null() {
            return false;
        }
        let kind: usize = msg_send![event, type];
        if kind != LEFT_DRAGGED && kind != LEFT_DOWN {
            return false;
        }
        let in_window: NSPoint = msg_send![event, locationInWindow];
        let at: NSPoint = msg_send![view, convertPoint: in_window fromView: std::ptr::null_mut::<Object>()];
        let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let items: *mut Object = msg_send![class!(NSMutableArray), array];
        for (i, path) in paths.iter().enumerate() {
            let Ok(c) = CString::new(path.as_os_str().as_bytes()) else { continue };
            let s: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()];
            let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: s];
            let item: *mut Object = msg_send![class!(NSDraggingItem), alloc];
            let item: *mut Object = msg_send![item, initWithPasteboardWriter: url];
            if item.is_null() {
                continue;
            }
            // Finder-style: each file's icon under the pointer, fanned out a little.
            let icon: *mut Object = msg_send![workspace, iconForFile: s];
            let _: () = msg_send![icon, setSize: NSSize { width: ICON, height: ICON }];
            let shift = i.min(4) as f64 * 6.0;
            let frame = NSRect { origin: NSPoint { x: at.x - ICON / 2.0 + shift, y: at.y - ICON / 2.0 - shift }, size: NSSize { width: ICON, height: ICON } };
            let _: () = msg_send![item, setDraggingFrame: frame contents: icon];
            let _: () = msg_send![items, addObject: item];
            let _: () = msg_send![item, release];
        }
        let count: usize = msg_send![items, count];
        if count == 0 {
            return false;
        }
        let ns_window: *mut Object = msg_send![view, window];
        let number: isize = msg_send![ns_window, windowNumber];
        DRAG_WINDOW.store(number, Ordering::Relaxed);
        let session: *mut Object = msg_send![view, beginDraggingSessionWithItems: items event: event source: source];
        !session.is_null()
    }
}

#[cfg(not(target_os = "macos"))]
pub fn begin_native_drag(_window: &Window, _paths: &[PathBuf]) -> bool {
    false
}

/// The object macOS asks which operations the drag allows: copy only, inside or outside
/// this app. Created once and kept for the life of the process.
#[cfg(target_os = "macos")]
fn drag_source() -> Option<*mut objc::runtime::Object> {
    use std::sync::OnceLock;

    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel, BOOL, NO, YES};
    use objc::{class, msg_send, sel, sel_impl, Encode, Encoding};

    // NSDragOperationCopy, NSEventTypeLeftMouseUp.
    const COPY: usize = 1;
    const LEFT_UP: usize = 2;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: f64,
        y: f64,
    }
    // SAFETY: matches the layout and type encoding of NSPoint (CGPoint) on 64-bit macOS.
    unsafe impl Encode for NSPoint {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("{CGPoint=dd}") }
        }
    }

    extern "C" fn operations(_: &Object, _: Sel, _session: *mut Object, _context: isize) -> usize {
        COPY
    }
    extern "C" fn ignore_modifiers(_: &Object, _: Sel, _session: *mut Object) -> BOOL {
        YES
    }
    /// The drag ended (dropped or cancelled). The window that started it never saw the
    /// button go up; GPUI keeps replaying the last drag move until it does, so post one.
    extern "C" fn ended(_: &Object, _: Sel, _session: *mut Object, _at: NSPoint, _operation: usize) {
        let number = DRAG_WINDOW.load(Ordering::Relaxed);
        // SAFETY: an autoreleased NSEvent handed to the shared NSApplication's queue.
        unsafe {
            let event: *mut Object = msg_send![class!(NSEvent),
                mouseEventWithType: LEFT_UP
                location: NSPoint { x: -10_000.0, y: -10_000.0 }
                modifierFlags: 0usize
                timestamp: 0.0f64
                windowNumber: number
                context: std::ptr::null_mut::<Object>()
                eventNumber: 0isize
                clickCount: 1isize
                pressure: 0.0f32];
            if !event.is_null() {
                let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
                let _: () = msg_send![app, postEvent: event atStart: NO];
            }
        }
    }

    static SOURCE: OnceLock<Option<usize>> = OnceLock::new();
    let source = SOURCE.get_or_init(|| {
        let mut decl = ClassDecl::new("FCFileDragSource", class!(NSObject))?;
        // SAFETY: the signatures match NSDraggingSource's methods.
        unsafe {
            decl.add_method(sel!(draggingSession:sourceOperationMaskForDraggingContext:), operations as extern "C" fn(&Object, Sel, *mut Object, isize) -> usize);
            decl.add_method(sel!(ignoreModifierKeysForDraggingSession:), ignore_modifiers as extern "C" fn(&Object, Sel, *mut Object) -> BOOL);
            decl.add_method(sel!(draggingSession:endedAtPoint:operation:), ended as extern "C" fn(&Object, Sel, *mut Object, NSPoint, usize));
        }
        let class = decl.register();
        // SAFETY: a plain NSObject subclass instance, intentionally never released.
        let object: *mut Object = unsafe { msg_send![class, new] };
        (!object.is_null()).then_some(object as usize)
    });
    source.map(|p| p as *mut Object)
}
