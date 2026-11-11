//! macOS: files opened from Finder (double-click, "Open With", drag onto the
//! Dock icon) arrive as an `application:openURLs:` message to the
//! NSApplication delegate, not as argv. winit's delegate does not implement
//! it, and its class is only created inside `run_native`, so we add the method
//! to NSObject before the event loop starts: every delegate inherits it, and
//! AppKit checks `respondsToSelector:` when the event comes in.
//!
//! URLs land in a queue that `PlayerApp::update` drains each frame. Files
//! sent at launch arrive before the first frame and are picked up by it.

use std::ffi::{c_char, CStr};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use objc2::ffi;
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;

static QUEUE: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

/// Call once in `main`, before `eframe::run_native`.
pub fn install() {
    let Some(cls) = AnyClass::get(c"NSObject") else { return };
    let sel = sel!(application:openURLs:);
    // Never replace an implementation someone else already provides.
    if cls.responds_to(sel) {
        return;
    }
    unsafe {
        let imp: Imp = std::mem::transmute(
            open_urls as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject),
        );
        ffi::class_addMethod(cls as *const AnyClass as *mut AnyClass, sel, imp, c"v@:@@".as_ptr());
    }
}

/// Lets later events wake an idle window.
pub fn set_ctx(ctx: &eframe::egui::Context) {
    let _ = CTX.set(ctx.clone());
}

pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *QUEUE.lock().unwrap())
}

unsafe fn nsstring(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let p: *const c_char = msg_send![&*s, UTF8String];
    (!p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}

unsafe extern "C-unwind" fn open_urls(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut AnyObject) {
    if urls.is_null() {
        return;
    }
    let urls = &*urls;
    let n: usize = msg_send![urls, count];
    let mut got = Vec::new();
    for i in 0..n {
        let u: *mut AnyObject = msg_send![urls, objectAtIndex: i];
        if u.is_null() {
            continue;
        }
        let is_file: bool = msg_send![&*u, isFileURL];
        let s: *mut AnyObject = if is_file { msg_send![&*u, path] } else { msg_send![&*u, absoluteString] };
        if let Some(s) = nsstring(s) {
            got.push(PathBuf::from(s));
        }
    }
    if got.is_empty() {
        return;
    }
    QUEUE.lock().unwrap().extend(got);
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}
