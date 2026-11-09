//! Seek-bar thumbnail previews.
//!
//! A separate, silent libmpv instance opens the same file and renders single
//! frames on the CPU through the render API's software backend ("sw"). It
//! seeks to keyframes only, so a preview costs one keyframe decode. Requests
//! are coalesced: only the newest hovered position is worked on.

use crate::mpv::*;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_int, c_void, CString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const THUMB_W: i32 = 208;
const CACHE_MAX: usize = 240;

#[derive(Default)]
struct Req {
    path: Option<PathBuf>,
    /// bucket index wanted next
    want: Option<u32>,
    step: f64,
    quit: bool,
}

struct Shared {
    req: Mutex<Req>,
    cv: Condvar,
    done: Mutex<Vec<(PathBuf, u32, ColorImage)>>,
}

pub struct Thumbs {
    sh: Arc<Shared>,
    path: Option<PathBuf>,
    step: f64,
    cache: HashMap<u32, TextureHandle>,
    order: VecDeque<u32>,
    pending: Option<u32>,
    last_shown: Option<u32>,
    started: bool,
    ctx: egui::Context,
}

impl Thumbs {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            sh: Arc::new(Shared { req: Mutex::new(Req::default()), cv: Condvar::new(), done: Mutex::new(Vec::new()) }),
            path: None,
            step: 2.0,
            cache: HashMap::new(),
            order: VecDeque::new(),
            pending: None,
            last_shown: None,
            started: false,
            ctx,
        }
    }

    /// Bucket size: about 400 previews across the file, at least 2 s apart.
    fn step_for(duration: f64) -> f64 {
        (duration / 400.0).max(2.0)
    }

    /// Point the worker at a new file (or none). Clears the cache.
    pub fn set_file(&mut self, path: Option<&Path>, duration: f64) {
        let path = path.filter(|p| p.is_file()).map(|p| p.to_path_buf());
        let step = Self::step_for(duration);
        if path == self.path && (step - self.step).abs() < 1e-6 {
            return;
        }
        self.path = path.clone();
        self.step = step;
        self.cache.clear();
        self.order.clear();
        self.pending = None;
        self.last_shown = None;
        let mut r = self.sh.req.lock().unwrap();
        r.path = path;
        r.want = None;
        r.step = step;
        drop(r);
        self.sh.cv.notify_all();
    }

    fn ensure_worker(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let sh = self.sh.clone();
        let ctx = self.ctx.clone();
        std::thread::Builder::new().name("thumbs".into()).spawn(move || worker(sh, ctx)).ok();
    }

    /// Texture for time `t`, requesting it if missing. Falls back to the last
    /// preview shown so the box does not flicker while a new frame decodes.
    pub fn get(&mut self, t: f64) -> Option<TextureHandle> {
        self.path.as_ref()?;
        self.ensure_worker();
        self.collect();
        let b = (t / self.step).floor().max(0.0) as u32;
        if let Some(tex) = self.cache.get(&b) {
            self.last_shown = Some(b);
            return Some(tex.clone());
        }
        if self.pending != Some(b) {
            self.pending = Some(b);
            self.sh.req.lock().unwrap().want = Some(b);
            self.sh.cv.notify_all();
        }
        self.last_shown.and_then(|l| self.cache.get(&l).cloned())
    }

    fn collect(&mut self) {
        let done: Vec<_> = std::mem::take(&mut *self.sh.done.lock().unwrap());
        for (path, b, img) in done {
            if Some(&path) != self.path.as_ref() {
                continue;
            }
            let tex = self.ctx.load_texture(format!("thumb-{b}"), img, TextureOptions::LINEAR);
            self.cache.insert(b, tex);
            self.order.push_back(b);
            if self.pending == Some(b) {
                self.pending = None;
            }
            while self.order.len() > CACHE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.cache.remove(&old);
                }
            }
        }
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.sh.req.lock().unwrap().quit = true;
        self.sh.cv.notify_all();
    }
}

struct Engine {
    mpv: Mpv,
    rctx: *mut MpvRenderContext,
    path: Option<PathBuf>,
    loaded: bool,
}

impl Engine {
    fn new() -> Result<Self, String> {
        let mpv = Mpv::new(&[
            ("vo", "libmpv"),
            ("ao", "null"),
            ("aid", "no"),
            ("sid", "no"),
            ("pause", "yes"),
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("hwdec", "no"),
            ("hr-seek", "no"),
            ("vd-lavc-skiploopfilter", "all"),
            ("vd-lavc-fast", "yes"),
            ("vd-lavc-threads", "2"),
            ("cache", "no"),
            ("demuxer-readahead-secs", "0"),
            ("sub-auto", "no"),
            ("audio-file-auto", "no"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
            ("config", "no"),
            ("terminal", "no"),
            ("input-default-bindings", "no"),
            ("sw-fast", "yes"),
        ])?;
        let api = mpv.api();
        let api_type = CString::new("sw").unwrap();
        let mut params = [
            MpvRenderParam { kind: RENDER_PARAM_API_TYPE, data: api_type.as_ptr() as *mut c_void },
            MpvRenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        let mut rctx: *mut MpvRenderContext = std::ptr::null_mut();
        let r = unsafe { (api.render_create)(&mut rctx, mpv.raw(), params.as_mut_ptr()) };
        if r < 0 || rctx.is_null() {
            return Err(format!("sw render context: {r}"));
        }
        Ok(Self { mpv, rctx, path: None, loaded: false })
    }

    /// Wait for one of `want` events, up to `timeout`.
    fn wait_for(&self, timeout: Duration, want: impl Fn(&Event) -> bool) -> bool {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if let Some(ev) = self.mpv.wait_event(0.05) {
                if want(&ev) {
                    return true;
                }
                if matches!(ev, Event::EndFile { .. }) {
                    return false;
                }
            }
        }
        false
    }

    fn open(&mut self, path: &Path) -> bool {
        if self.path.as_deref() == Some(path) && self.loaded {
            return true;
        }
        self.path = Some(path.to_path_buf());
        self.loaded = false;
        if self.mpv.command(&["loadfile", &path.to_string_lossy()]).is_err() {
            return false;
        }
        self.loaded = self.wait_for(Duration::from_secs(8), |e| matches!(e, Event::FileLoaded));
        if self.loaded {
            // let the first frame settle; a timeout here is harmless
            self.wait_for(Duration::from_secs(3), |e| matches!(e, Event::PlaybackRestart));
        }
        self.loaded
    }

    fn grab(&mut self, t: f64) -> Option<ColorImage> {
        let api = self.mpv.api();
        self.mpv.command(&["seek", &format!("{t:.3}"), "absolute+keyframes"]).ok()?;
        if !self.wait_for(Duration::from_secs(4), |e| matches!(e, Event::PlaybackRestart)) {
            return None;
        }
        // wait until the render context has the new frame
        let end = Instant::now() + Duration::from_millis(600);
        loop {
            let flags = unsafe { (api.render_update)(self.rctx) };
            if flags & RENDER_UPDATE_FRAME != 0 {
                break;
            }
            if Instant::now() > end {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let (dw, dh) = (self.mpv.get_f64("dwidth").unwrap_or(16.0), self.mpv.get_f64("dheight").unwrap_or(9.0));
        if dw <= 0.0 || dh <= 0.0 {
            return None;
        }
        let w = THUMB_W;
        let h = ((w as f64 * dh / dw).round() as i32).clamp(32, 400) & !1;
        let mut size: [c_int; 2] = [w, h];
        let fmt = CString::new("rgb0").unwrap();
        let mut stride: usize = (w as usize) * 4;
        let mut buf = vec![0u8; stride * h as usize];
        let mut params = [
            MpvRenderParam { kind: RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr() as *mut c_void },
            MpvRenderParam { kind: RENDER_PARAM_SW_FORMAT, data: fmt.as_ptr() as *mut c_void },
            MpvRenderParam { kind: RENDER_PARAM_SW_STRIDE, data: &mut stride as *mut usize as *mut c_void },
            MpvRenderParam { kind: RENDER_PARAM_SW_POINTER, data: buf.as_mut_ptr() as *mut c_void },
            MpvRenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        let r = unsafe { (api.render_render)(self.rctx, params.as_mut_ptr()) };
        if r < 0 {
            return None;
        }
        for px in buf.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Some(ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &buf))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let api = self.mpv.api();
        unsafe { (api.render_free)(self.rctx) };
    }
}

fn worker(sh: Arc<Shared>, ctx: egui::Context) {
    let mut engine: Option<Engine> = None;
    let mut broken = false;
    loop {
        let (path, b, step) = {
            let mut r = sh.req.lock().unwrap();
            while !r.quit && (r.want.is_none() || r.path.is_none()) {
                r = sh.cv.wait(r).unwrap();
            }
            if r.quit {
                return;
            }
            let b = r.want.take().unwrap();
            (r.path.clone().unwrap(), b, r.step)
        };
        if broken {
            continue;
        }
        if engine.is_none() {
            match Engine::new() {
                Ok(e) => engine = Some(e),
                Err(e) => {
                    eprintln!("thumbnails disabled: {e}");
                    broken = true;
                    continue;
                }
            }
        }
        let e = engine.as_mut().unwrap();
        if !e.open(&path) {
            continue;
        }
        let t = (b as f64 + 0.5) * step;
        if let Some(img) = e.grab(t) {
            sh.done.lock().unwrap().push((path, b, img));
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_scales_with_duration() {
        assert_eq!(Thumbs::step_for(60.0), 2.0);
        assert!((Thumbs::step_for(7200.0) - 18.0).abs() < 1e-9);
    }
}
