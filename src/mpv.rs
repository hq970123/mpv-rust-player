//! Minimal libmpv binding, loaded at runtime.
//!
//! libmpv is loaded dynamically (libloading) instead of linked, so the app
//! builds without mpv import libraries and can show a clear message when the
//! library is missing. Only the small, stable part of the client API and the
//! OpenGL render API that the player uses is declared here.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::OnceLock;

#[repr(C)]
pub struct MpvHandle {
    _p: [u8; 0],
}
#[repr(C)]
pub struct MpvRenderContext {
    _p: [u8; 0],
}

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct MpvEventEndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
    playlist_insert_num_entries: c_int,
}

#[repr(C)]
pub struct MpvRenderParam {
    pub kind: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvOpenGlInitParams {
    pub get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
pub struct MpvOpenGlFbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

pub const RENDER_PARAM_INVALID: c_int = 0;
pub const RENDER_PARAM_API_TYPE: c_int = 1;
pub const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const RENDER_PARAM_FLIP_Y: c_int = 4;

const FORMAT_DOUBLE: c_int = 5;

const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_SEEK: c_int = 20;
const EVENT_PLAYBACK_RESTART: c_int = 21;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    StartFile,
    FileLoaded,
    /// reason: 0 eof, 2 stop, 3 quit, 4 error, 5 redirect
    EndFile { reason: i32, error: i32 },
    Seek,
    PlaybackRestart,
    Shutdown,
    Other(i32),
}

type FnCreate = unsafe extern "C" fn() -> *mut MpvHandle;
type FnInitialize = unsafe extern "C" fn(*mut MpvHandle) -> c_int;
type FnTerminateDestroy = unsafe extern "C" fn(*mut MpvHandle);
type FnSetString = unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type FnGetPropertyString = unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> *mut c_char;
type FnGetProperty = unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnCommand = unsafe extern "C" fn(*mut MpvHandle, *mut *const c_char) -> c_int;
type FnWaitEvent = unsafe extern "C" fn(*mut MpvHandle, f64) -> *mut MpvEvent;
type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
type FnClientApiVersion = unsafe extern "C" fn() -> std::ffi::c_ulong;
type FnRenderCreate =
    unsafe extern "C" fn(*mut *mut MpvRenderContext, *mut MpvHandle, *mut MpvRenderParam) -> c_int;
type FnRenderSetUpdateCallback =
    unsafe extern "C" fn(*mut MpvRenderContext, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void);
type FnRenderUpdate = unsafe extern "C" fn(*mut MpvRenderContext) -> u64;
type FnRenderRender = unsafe extern "C" fn(*mut MpvRenderContext, *mut MpvRenderParam) -> c_int;
type FnRenderFree = unsafe extern "C" fn(*mut MpvRenderContext);

pub struct Api {
    _lib: libloading::Library,
    pub path: String,
    create: FnCreate,
    initialize: FnInitialize,
    terminate_destroy: FnTerminateDestroy,
    set_option_string: FnSetString,
    set_property_string: FnSetString,
    get_property_string: FnGetPropertyString,
    get_property: FnGetProperty,
    free: FnFree,
    command: FnCommand,
    wait_event: FnWaitEvent,
    error_string: FnErrorString,
    client_api_version: FnClientApiVersion,
    pub render_create: FnRenderCreate,
    pub render_set_update_callback: FnRenderSetUpdateCallback,
    pub render_update: FnRenderUpdate,
    pub render_render: FnRenderRender,
    pub render_free: FnRenderFree,
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

fn candidates() -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default();
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("MPV_LIBRARY_PATH") {
        v.push(PathBuf::from(p));
    }
    #[cfg(windows)]
    {
        for n in ["libmpv-2.dll", "mpv-2.dll", "mpv-1.dll"] {
            v.push(exe_dir.join(n));
        }
        for n in ["libmpv-2.dll", "mpv-2.dll"] {
            v.push(PathBuf::from(n));
        }
    }
    #[cfg(target_os = "macos")]
    {
        v.push(exe_dir.join("../Frameworks/libmpv.2.dylib"));
        v.push(exe_dir.join("libmpv.2.dylib"));
        v.push(PathBuf::from("/opt/homebrew/lib/libmpv.2.dylib"));
        v.push(PathBuf::from("/usr/local/lib/libmpv.2.dylib"));
        v.push(PathBuf::from("libmpv.2.dylib"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        v.push(exe_dir.join("libmpv.so.2"));
        v.push(PathBuf::from("libmpv.so.2"));
        v.push(PathBuf::from("libmpv.so"));
    }
    v
}

pub fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(|e| e.clone())
}

fn load() -> Result<Api, String> {
    let mut errors = Vec::new();
    for path in candidates() {
        // A bare file name lets the OS search its default library paths.
        let is_bare = path.components().count() == 1;
        if !is_bare && !path.exists() {
            continue;
        }
        match unsafe { libloading::Library::new(&path) } {
            Ok(lib) => match unsafe { bind(lib, path.display().to_string()) } {
                Ok(api) => return Ok(api),
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            },
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    Err(format!("libmpv not found. {}", errors.join("; ")))
}

unsafe fn bind(lib: libloading::Library, path: String) -> Result<Api, String> {
    macro_rules! sym {
        ($name:literal) => {
            *lib.get(concat!($name, "\0").as_bytes())
                .map_err(|e| format!("missing symbol {}: {e}", $name))?
        };
    }
    Ok(Api {
        create: sym!("mpv_create"),
        initialize: sym!("mpv_initialize"),
        terminate_destroy: sym!("mpv_terminate_destroy"),
        set_option_string: sym!("mpv_set_option_string"),
        set_property_string: sym!("mpv_set_property_string"),
        get_property_string: sym!("mpv_get_property_string"),
        get_property: sym!("mpv_get_property"),
        free: sym!("mpv_free"),
        command: sym!("mpv_command"),
        wait_event: sym!("mpv_wait_event"),
        error_string: sym!("mpv_error_string"),
        client_api_version: sym!("mpv_client_api_version"),
        render_create: sym!("mpv_render_context_create"),
        render_set_update_callback: sym!("mpv_render_context_set_update_callback"),
        render_update: sym!("mpv_render_context_update"),
        render_render: sym!("mpv_render_context_render"),
        render_free: sym!("mpv_render_context_free"),
        path,
        _lib: lib,
    })
}

/// One mpv core instance.
pub struct Mpv {
    api: &'static Api,
    h: *mut MpvHandle,
}

// libmpv's client API is thread-safe.
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

impl Mpv {
    /// Create and initialize an instance with the given options (set before init).
    pub fn new(options: &[(&str, &str)]) -> Result<Self, String> {
        let api = api()?;
        let h = unsafe { (api.create)() };
        if h.is_null() {
            return Err("mpv_create failed (is LC_NUMERIC set to C?)".into());
        }
        let mpv = Mpv { api, h };
        for (k, v) in options {
            // Unknown options (older libmpv) are not fatal.
            let _ = mpv.set_option(k, v);
        }
        let r = unsafe { (api.initialize)(h) };
        if r < 0 {
            return Err(format!("mpv_initialize: {}", mpv.err(r)));
        }
        Ok(mpv)
    }

    pub fn raw(&self) -> *mut MpvHandle {
        self.h
    }

    pub fn api(&self) -> &'static Api {
        self.api
    }

    pub fn client_api_version(&self) -> (u32, u32) {
        let v = unsafe { (self.api.client_api_version)() } as u64;
        ((v >> 16) as u32, (v & 0xffff) as u32)
    }

    fn err(&self, code: c_int) -> String {
        unsafe { CStr::from_ptr((self.api.error_string)(code)) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        let r = unsafe { (self.api.set_option_string)(self.h, n.as_ptr(), v.as_ptr()) };
        if r < 0 {
            Err(format!("{name}={value}: {}", self.err(r)))
        } else {
            Ok(())
        }
    }

    pub fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        let r = unsafe { (self.api.set_property_string)(self.h, n.as_ptr(), v.as_ptr()) };
        if r < 0 {
            Err(format!("{name}={value}: {}", self.err(r)))
        } else {
            Ok(())
        }
    }

    pub fn get(&self, name: &str) -> Option<String> {
        let n = cstr(name);
        let p = unsafe { (self.api.get_property_string)(self.h, n.as_ptr()) };
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { (self.api.free)(p as *mut c_void) };
        Some(s)
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let n = cstr(name);
        let mut out = 0f64;
        let r = unsafe {
            (self.api.get_property)(self.h, n.as_ptr(), FORMAT_DOUBLE, &mut out as *mut f64 as *mut c_void)
        };
        (r >= 0).then_some(out)
    }

    pub fn get_flag(&self, name: &str) -> bool {
        self.get(name).as_deref() == Some("yes")
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let r = unsafe { (self.api.command)(self.h, ptrs.as_mut_ptr()) };
        if r < 0 {
            Err(format!("{}: {}", args.join(" "), self.err(r)))
        } else {
            Ok(())
        }
    }

    /// Non-blocking: returns the next queued event, if any.
    pub fn poll_event(&self) -> Option<Event> {
        let ev = unsafe { &*(self.api.wait_event)(self.h, 0.0) };
        Some(match ev.event_id {
            EVENT_NONE => return None,
            EVENT_SHUTDOWN => Event::Shutdown,
            EVENT_START_FILE => Event::StartFile,
            EVENT_FILE_LOADED => Event::FileLoaded,
            EVENT_END_FILE => {
                let (reason, error) = if ev.data.is_null() {
                    (0, 0)
                } else {
                    let d = unsafe { &*(ev.data as *const MpvEventEndFile) };
                    (d.reason, d.error)
                };
                Event::EndFile { reason, error }
            }
            EVENT_SEEK => Event::Seek,
            EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
            other => Event::Other(other),
        })
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe { (self.api.terminate_destroy)(self.h) };
    }
}

/// Escape a value for mpv's `key=value,...` option lists (loadfile options).
#[allow(dead_code)]
pub fn quote_opt(v: &str) -> String {
    // mpv accepts %N%value where N is the byte length.
    format!("%{}%{}", v.len(), v)
}
