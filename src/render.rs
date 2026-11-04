//! Draws mpv's video into the egui window through the libmpv OpenGL render API.
//!
//! mpv renders into an offscreen framebuffer the size of the video area, which
//! is then blitted into egui's framebuffer inside a paint callback. mpv and
//! egui share the same GL context, so frames never leave the GPU.

use crate::mpv::*;
use eframe::glow::{self, HasContext};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::{Arc, Mutex};

/// eframe only lends its GL loader during app creation, so mpv's render
/// context is created right then. Should mpv ask for a symbol later, the
/// platform's own GL loader answers instead.
struct GpaBox {
    borrowed: Option<*const (dyn Fn(&CStr) -> *const c_void + 'static)>,
}

unsafe extern "C" fn gpa_trampoline(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let b = &*(ctx as *const GpaBox);
    let name = CStr::from_ptr(name);
    if let Some(f) = b.borrowed {
        let p = (*f)(name);
        if !p.is_null() {
            return p as *mut c_void;
        }
    }
    platform_gpa(name) as *mut c_void
}

fn platform_gpa(name: &CStr) -> *const c_void {
    use std::sync::OnceLock;
    struct Libs(Vec<libloading::Library>);
    static LIBS: OnceLock<Libs> = OnceLock::new();
    let libs = LIBS.get_or_init(|| {
        let names: &[&str] = if cfg!(windows) {
            &["opengl32.dll"]
        } else if cfg!(target_os = "macos") {
            &["/System/Library/Frameworks/OpenGL.framework/OpenGL"]
        } else {
            &["libEGL.so.1", "libGL.so.1"]
        };
        Libs(names.iter().filter_map(|n| unsafe { libloading::Library::new(n) }.ok()).collect())
    });
    unsafe {
        for lib in &libs.0 {
            let loaders: [&[u8]; 3] = [b"wglGetProcAddress\0", b"eglGetProcAddress\0", b"glXGetProcAddressARB\0"];
            for loader in loaders {
                if let Ok(f) = lib.get::<unsafe extern "system" fn(*const c_char) -> *const c_void>(loader) {
                    let p = f(name.as_ptr());
                    // wglGetProcAddress returns small sentinels on failure
                    if !p.is_null() && (p as isize) > 3 && (p as isize) != -1 {
                        return p;
                    }
                }
            }
            if let Ok(sym) = lib.get::<*const c_void>(name.to_bytes_with_nul()) {
                return *sym;
            }
        }
    }
    std::ptr::null()
}

unsafe extern "C" fn on_update(ctx: *mut c_void) {
    let c = &*(ctx as *const eframe::egui::Context);
    c.request_repaint();
}

pub struct VideoRenderer {
    api: &'static Api,
    mpv: *mut MpvHandle,
    ctx: *mut MpvRenderContext,
    gpa: Box<GpaBox>,
    egui_ctx: Box<eframe::egui::Context>,
    fbo: Option<glow::Framebuffer>,
    tex: Option<glow::Texture>,
    size: (i32, i32),
    pub failed: Option<String>,
}

// Only ever touched from the GL thread (inside paint callbacks / on_exit).
unsafe impl Send for VideoRenderer {}

impl VideoRenderer {
    /// Must be called with the GL context current (eframe's creation callback).
    pub fn new(
        mpv: &Mpv,
        gpa: &dyn Fn(&CStr) -> *const c_void,
        egui_ctx: eframe::egui::Context,
    ) -> Arc<Mutex<Self>> {
        // SAFETY: the borrowed loader is only used inside ensure_context()
        // below and cleared before this function returns.
        let borrowed: *const (dyn Fn(&CStr) -> *const c_void + '_) = gpa;
        let borrowed: *const (dyn Fn(&CStr) -> *const c_void + 'static) = unsafe { std::mem::transmute(borrowed) };
        let mut r = Self {
            api: mpv.api(),
            mpv: mpv.raw(),
            ctx: std::ptr::null_mut(),
            gpa: Box::new(GpaBox { borrowed: Some(borrowed) }),
            egui_ctx: Box::new(egui_ctx),
            fbo: None,
            tex: None,
            size: (0, 0),
            failed: None,
        };
        r.ensure_context();
        r.gpa.borrowed = None;
        Arc::new(Mutex::new(r))
    }

    fn ensure_context(&mut self) -> bool {
        if !self.ctx.is_null() {
            return true;
        }
        if self.failed.is_some() {
            return false;
        }
        let api_type = CString::new("opengl").unwrap();
        let mut init = MpvOpenGlInitParams {
            get_proc_address: Some(gpa_trampoline),
            get_proc_address_ctx: &*self.gpa as *const GpaBox as *mut c_void,
        };
        let mut params = [
            MpvRenderParam { kind: RENDER_PARAM_API_TYPE, data: api_type.as_ptr() as *mut c_void },
            MpvRenderParam {
                kind: RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init as *mut _ as *mut c_void,
            },
            MpvRenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        let mut ctx = std::ptr::null_mut();
        let r = unsafe { (self.api.render_create)(&mut ctx, self.mpv, params.as_mut_ptr()) };
        if r < 0 || ctx.is_null() {
            self.failed = Some(format!("mpv_render_context_create failed ({r})"));
            return false;
        }
        self.ctx = ctx;
        unsafe {
            (self.api.render_set_update_callback)(
                ctx,
                Some(on_update),
                &*self.egui_ctx as *const eframe::egui::Context as *mut c_void,
            )
        };
        true
    }

    unsafe fn ensure_fbo(&mut self, gl: &glow::Context, w: i32, h: i32) {
        if self.size == (w, h) && self.fbo.is_some() {
            return;
        }
        if let Some(t) = self.tex.take() {
            gl.delete_texture(t);
        }
        if let Some(f) = self.fbo.take() {
            gl.delete_framebuffer(f);
        }
        let prev_tex = gl.get_parameter_i32(glow::TEXTURE_BINDING_2D);
        let prev_fb = gl.get_parameter_i32(glow::FRAMEBUFFER_BINDING);
        let tex = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        let fbo = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
        gl.bind_framebuffer(glow::FRAMEBUFFER, raw_fb(prev_fb));
        gl.bind_texture(glow::TEXTURE_2D, raw_tex(prev_tex));
        self.tex = Some(tex);
        self.fbo = Some(fbo);
        self.size = (w, h);
    }

    /// Render the current frame into `rect` (pixels, origin bottom-left) of the
    /// framebuffer that is bound when called.
    pub fn paint(&mut self, gl: &glow::Context, left: i32, bottom: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 || !self.ensure_context() {
            return;
        }
        unsafe {
            let target = gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING);
            self.ensure_fbo(gl, w, h);
            let fbo_id = self.fbo.map(|f| f.0.get() as c_int).unwrap_or(0);
            (self.api.render_update)(self.ctx);
            let mut fbo = MpvOpenGlFbo { fbo: fbo_id, w, h, internal_format: 0 };
            // Same orientation as rendering to the window framebuffer.
            let mut flip: c_int = 1;
            let mut params = [
                MpvRenderParam { kind: RENDER_PARAM_OPENGL_FBO, data: &mut fbo as *mut _ as *mut c_void },
                MpvRenderParam { kind: RENDER_PARAM_FLIP_Y, data: &mut flip as *mut _ as *mut c_void },
                MpvRenderParam { kind: RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
            ];
            (self.api.render_render)(self.ctx, params.as_mut_ptr());

            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, self.fbo);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, raw_fb(target));
            gl.blit_framebuffer(0, 0, w, h, left, bottom, left + w, bottom + h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, raw_fb(target));
        }
    }

    /// Free GL and mpv render resources. Must run with the GL context current,
    /// before the mpv core is destroyed.
    pub fn destroy(&mut self, gl: Option<&glow::Context>) {
        if !self.ctx.is_null() {
            unsafe { (self.api.render_free)(self.ctx) };
            self.ctx = std::ptr::null_mut();
        }
        if let Some(gl) = gl {
            unsafe {
                if let Some(t) = self.tex.take() {
                    gl.delete_texture(t);
                }
                if let Some(f) = self.fbo.take() {
                    gl.delete_framebuffer(f);
                }
            }
        }
    }
}

fn raw_fb(id: i32) -> Option<glow::Framebuffer> {
    std::num::NonZeroU32::new(id as u32).map(glow::NativeFramebuffer)
}
fn raw_tex(id: i32) -> Option<glow::Texture> {
    std::num::NonZeroU32::new(id as u32).map(glow::NativeTexture)
}
