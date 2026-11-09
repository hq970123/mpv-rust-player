mod extra;

use crate::cast::{CastState, Caster, Device};
use crate::icons as ic;
use crate::live::{self, Live, Status};
use crate::models::{self, Downloads};
use crate::mpv::{Event, Mpv};
use crate::render::VideoRenderer;
use crate::settings::{self, Provider, Settings, SubMode, UiLang};
use crate::shots;
use crate::APP_NAME;
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Frame, Id, Key, Margin, Modifiers, Pos2, Rect,
    RichText, Sense, Stroke, Vec2,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(124, 108, 242);
const BG: Color32 = Color32::from_rgb(14, 14, 20);
const TITLE_BG: Color32 = Color32::from_rgb(24, 24, 32);
const PANEL: Color32 = Color32::from_rgb(30, 30, 40);
const TEXT: Color32 = Color32::from_rgb(236, 236, 244);
const MUTED: Color32 = Color32::from_rgb(150, 150, 168);

pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "asf", "flv", "f4v", "ts", "m2ts", "mts", "mpg", "mpeg", "vob",
    "3gp", "3g2", "ogv", "rm", "rmvb", "divx", "dv", "mxf", "y4m", "hevc", "h264", "265", "264", "av1", "ivf", "nut",
    "iso", "mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "wma", "ape", "alac", "dts", "ac3", "mka", "zip", "rar",
    "7z", "m3u", "m3u8", "pls",
];
pub const SUB_EXTS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "sup", "idx", "smi", "lrc", "ttml"];

pub fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(20, 20, 28);
    v.faint_bg_color = Color32::from_rgb(36, 36, 48);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(52, 52, 68));
    v.selection.bg_fill = ACCENT.linear_multiply(0.55);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.bg_fill = Color32::from_rgb(52, 52, 70);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 52, 70);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT.linear_multiply(0.7);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(8);
    }
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.button_padding = vec2(10.0, 6.0);
        s.spacing.menu_margin = Margin::same(8);
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.interact_size.y = 28.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.5));
    });
}

#[derive(Default, Clone)]
struct PlayState {
    time: f64,
    duration: f64,
    paused: bool,
    volume: f64,
    muted: bool,
    speed: f64,
    idle: bool,
    eof: bool,
    height: f64,
    aid: String,
    sid: String,
}

#[derive(Clone)]
struct Track {
    id: i64,
    kind: String,
    title: String,
    lang: String,
    codec: String,
    selected: bool,
    external: bool,
    external_file: String,
}

impl Track {
    fn label(&self) -> String {
        let mut parts = vec![format!("#{}", self.id)];
        if !self.title.is_empty() {
            parts.push(self.title.clone());
        }
        if !self.lang.is_empty() {
            parts.push(format!("[{}]", self.lang));
        }
        if !self.codec.is_empty() {
            parts.push(self.codec.clone());
        }
        if self.external {
            parts.push("ext".into());
        }
        parts.join("  ")
    }
}

#[derive(PartialEq, Clone, Copy)]
enum PrefTab {
    General,
    Translate,
    Models,
}

pub struct PlayerApp {
    mpv: Option<Arc<Mpv>>,
    renderer: Option<Arc<Mutex<VideoRenderer>>>,
    init_error: Option<String>,
    s: Settings,
    st: PlayState,
    tracks: Vec<Track>,
    tracks_polled: Instant,
    playlist: Vec<PathBuf>,
    index: usize,
    current: Option<PathBuf>,
    last_motion: Instant,
    last_pointer: Option<Pos2>,
    seek_preview: Option<f64>,
    last_drag_seek: Instant,
    show_stats: bool,
    show_prefs: bool,
    pref_tab: PrefTab,
    toast: Option<(String, Instant)>,
    live: Live,
    downloads: Downloads,
    pip_restore: Option<Vec2>,
    live_restart_at: Option<(Instant, bool)>,
    seek_pending: bool,
    eof_handled: bool,
    test_result: Arc<Mutex<Option<String>>>,
    controls_hovered: bool,
    caster: Caster,
    /// Device we were casting to, so we notice when the session ends.
    cast_device: Option<Device>,
    shot_msg: Arc<Mutex<Option<String>>>,
    x: extra::Extra,
    thumbs: crate::thumbs::Thumbs,
    inbox: Option<std::sync::mpsc::Receiver<Option<PathBuf>>>,
}

impl PlayerApp {
    pub fn new(cc: &eframe::CreationContext<'_>, file: Option<String>) -> Self {
        let s = Settings::load();
        let live = Live::new(live::Config::from_settings(&s), cc.egui_ctx.clone());
        let mut app = Self {
            mpv: None,
            renderer: None,
            init_error: None,
            st: PlayState { idle: true, speed: 1.0, volume: s.volume, ..Default::default() },
            s,
            tracks: Vec::new(),
            tracks_polled: Instant::now(),
            playlist: Vec::new(),
            index: 0,
            current: None,
            last_motion: Instant::now(),
            last_pointer: None,
            seek_preview: None,
            last_drag_seek: Instant::now(),
            show_stats: false,
            show_prefs: false,
            pref_tab: PrefTab::General,
            toast: None,
            live,
            downloads: Arc::new(Mutex::new(Vec::new())),
            pip_restore: None,
            live_restart_at: None,
            seek_pending: false,
            eof_handled: false,
            test_result: Arc::new(Mutex::new(None)),
            controls_hovered: false,
            caster: Caster::new(cc.egui_ctx.clone()),
            cast_device: None,
            shot_msg: Arc::new(Mutex::new(None)),
            x: extra::Extra::default(),
            thumbs: crate::thumbs::Thumbs::new(cc.egui_ctx.clone()),
            inbox: None,
        };
        if app.s.single_instance {
            app.inbox = crate::single::listen(cc.egui_ctx.clone());
        }
        let volume = format!("{}", app.s.volume);
        let shots = shots_dir(&app.s).to_string_lossy().to_string();
        let jpeg_q = app.s.shot_jpeg_quality.to_string();
        let opts: Vec<(&str, &str)> = vec![
            ("vo", "libmpv"),
            ("config", "no"),
            ("terminal", "no"),
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("osd-level", "0"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("hwdec", &app.s.hwdec),
            ("volume", &volume),
            ("volume-max", "150"),
            ("sub-auto", "fuzzy"),
            ("audio-file-auto", "fuzzy"),
            ("screenshot-directory", &shots),
            ("screenshot-format", "png"),
            ("screenshot-jpeg-quality", &jpeg_q),
            ("hr-seek", "default"),
            ("ytdl", "no"),
        ];
        match Mpv::new(&opts) {
            Ok(m) => match cc.get_proc_address {
                Some(gpa) => {
                    app.renderer = Some(VideoRenderer::new(&m, gpa, cc.egui_ctx.clone()));
                    app.mpv = Some(Arc::new(m));
                }
                None => app.init_error = Some("OpenGL get_proc_address unavailable".into()),
            },
            Err(e) => app.init_error = Some(e),
        }
        app.init_extra();
        if let Some(f) = file {
            let p = PathBuf::from(&f);
            if p.is_dir() {
                app.open_many(vec![p]);
            } else if f.contains("://") {
                app.playlist = vec![p.clone()];
                app.load(p);
            } else {
                app.open(p);
            }
        }
        app
    }

    fn t(&self, zh: &'static str, en: &'static str) -> &'static str {
        match self.s.ui_lang {
            UiLang::Zh => zh,
            UiLang::En => en,
        }
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn cmd(&self, args: &[&str]) {
        if let Some(m) = &self.mpv {
            let _ = m.command(args);
        }
    }

    fn set(&self, k: &str, v: &str) {
        if let Some(m) = &self.mpv {
            let _ = m.set(k, v);
        }
    }

    // ------------------------------------------------------------ files

    fn open(&mut self, path: PathBuf) {
        self.playlist = sibling_media(&path);
        self.index = self.playlist.iter().position(|p| p == &path).unwrap_or(0);
        if self.playlist.is_empty() {
            self.playlist.push(path.clone());
        }
        self.load(path);
    }

    fn load(&mut self, path: PathBuf) {
        if self.s.resume {
            self.save_position();
        }
        if self.casting() {
            self.stop_cast(false);
        }
        self.cmd(&["loadfile", &path.to_string_lossy()]);
        self.set("pause", "no");
        self.current = Some(path);
        self.eof_handled = false;
        self.live.shared.cues.lock().unwrap().clear();
    }

    fn step(&mut self, delta: i64) {
        if self.playlist.is_empty() {
            return;
        }
        match self.next_index(delta) {
            Some(i) => self.play_index(i),
            None => self.toast(self.t("没有更多文件了", "No more files")),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter(self.t("媒体文件", "Media"), VIDEO_EXTS)
            .add_filter(self.t("所有文件", "All files"), &["*"])
            .pick_file()
        {
            self.open(p);
        }
    }

    fn add_sub_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter(self.t("字幕", "Subtitles"), SUB_EXTS).pick_file() {
            self.cmd(&["sub-add", &p.to_string_lossy(), "select"]);
        }
    }

    fn handle_drop(&mut self, files: Vec<PathBuf>) {
        let mut media = Vec::new();
        for f in files {
            let ext = ext_of(&f);
            if SUB_EXTS.contains(&ext.as_str()) {
                if self.current.is_some() {
                    self.cmd(&["sub-add", &f.to_string_lossy(), "select"]);
                    self.toast(self.t("已加载字幕", "Subtitle loaded"));
                }
            } else {
                media.push(f);
            }
        }
        if !media.is_empty() {
            self.open_many(media);
        }
    }

    // ------------------------------------------------------------ state

    fn pump(&mut self) {
        let Some(m) = self.mpv.clone() else { return };
        while let Some(ev) = m.poll_event() {
            match ev {
                Event::FileLoaded => {
                    self.refresh_tracks();
                    self.on_loaded_extra();
                    let dur = m.get_f64("duration").unwrap_or(0.0);
                    self.thumbs.set_file(self.current.as_deref(), dur);
                    self.schedule_live(true, 0);
                    self.apply_quality();
                }
                Event::Seek => self.seek_pending = true,
                Event::PlaybackRestart if self.seek_pending => {
                    self.seek_pending = false;
                    self.schedule_live(false, 250);
                }
                Event::EndFile { reason: 4, error } => {
                    self.toast(format!("{} ({error})", self.t("无法播放此文件", "Cannot play this file")));
                }
                _ => {}
            }
        }
        let st = &mut self.st;
        st.time = m.get_f64("time-pos").unwrap_or(0.0);
        st.duration = m.get_f64("duration").unwrap_or(0.0);
        st.paused = m.get_flag("pause");
        st.volume = m.get_f64("volume").unwrap_or(st.volume);
        st.muted = m.get_flag("mute");
        st.speed = m.get_f64("speed").unwrap_or(1.0);
        st.idle = m.get_flag("idle-active");
        st.eof = m.get_flag("eof-reached");
        st.height = m.get_f64("height").unwrap_or(0.0);
        let aid = m.get("aid").unwrap_or_default();
        if aid != st.aid && !st.aid.is_empty() && !st.idle {
            st.aid = aid;
            self.schedule_live(false, 100);
        } else {
            self.st.aid = aid;
        }
        self.st.sid = m.get("sid").unwrap_or_default();
        self.pump_cast();
        self.live.shared.set_player_time(self.st.time);
        let shot_msg = self.shot_msg.lock().unwrap().take();
        if let Some(msg) = shot_msg {
            self.toast(msg);
        }

        if self.tracks_polled.elapsed() > Duration::from_secs(2) {
            self.refresh_tracks();
        }
        if self.st.eof && !self.eof_handled && !self.st.idle {
            self.eof_handled = true;
            self.on_eof();
        }
        self.tick_extra();
        if let Some((at, new_media)) = self.live_restart_at {
            if Instant::now() >= at {
                self.live_restart_at = None;
                self.restart_live(new_media);
            }
        }
    }

    fn refresh_tracks(&mut self) {
        self.tracks_polled = Instant::now();
        let Some(m) = &self.mpv else { return };
        let json = m.get("track-list").unwrap_or_default();
        let list: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap_or_default();
        self.tracks = list
            .iter()
            .map(|t| Track {
                id: t["id"].as_i64().unwrap_or(0),
                kind: t["type"].as_str().unwrap_or("").into(),
                title: t["title"].as_str().unwrap_or("").into(),
                lang: t["lang"].as_str().unwrap_or("").into(),
                codec: t["codec"].as_str().unwrap_or("").into(),
                selected: t["selected"].as_bool().unwrap_or(false),
                external: t["external"].as_bool().unwrap_or(false),
                external_file: t["external-filename"].as_str().unwrap_or("").into(),
            })
            .collect();
    }

    fn live_available(&self) -> bool {
        models::is_installed(&self.s.model)
    }

    fn schedule_live(&mut self, new_media: bool, delay_ms: u64) {
        if self.s.live_enabled {
            let keep_new = new_media || self.live_restart_at.map(|(_, n)| n).unwrap_or(false);
            self.live_restart_at = Some((Instant::now() + Duration::from_millis(delay_ms), keep_new));
        }
    }

    fn restart_live(&mut self, new_media: bool) {
        if !self.s.live_enabled || !self.live_available() {
            self.live.stop();
            return;
        }
        let Some(path) = self.current.clone() else {
            return;
        };
        let aid = (!self.st.aid.is_empty() && self.st.aid != "no").then(|| self.st.aid.clone());
        self.live.configure(live::Config::from_settings(&self.s));
        self.live.start(&path.to_string_lossy(), self.st.time, aid, new_media);
    }

    fn settings_changed(&mut self, restart: bool) {
        self.s.save();
        self.live.configure(live::Config::from_settings(&self.s));
        if restart && self.s.live_enabled {
            self.schedule_live(false, 300);
        }
    }

    fn set_live(&mut self, on: bool) {
        self.s.live_enabled = on;
        self.s.save();
        if on {
            self.restart_live(false);
        } else {
            self.live.stop();
        }
    }

    fn apply_quality(&self) {
        self.cmd(&["vf", "remove", "@quality"]);
        if self.s.quality > 0 {
            let f = format!("@quality:lavfi=[scale=-2:'min(ih,{})']", self.s.quality);
            self.cmd(&["vf", "add", &f]);
        }
    }

    fn seek_abs(&mut self, t: f64, exact: bool) {
        if self.casting() {
            let t = t.clamp(0.0, self.st.duration.max(t));
            self.caster.seek(t);
            self.st.time = t;
            self.schedule_live(false, 400);
            return;
        }
        let flag = if exact { "absolute+exact" } else { "absolute+keyframes" };
        self.cmd(&["seek", &format!("{t:.3}"), flag]);
    }

    fn seek_rel(&mut self, d: f64) {
        if self.casting() {
            let t = (self.st.time + d).max(0.0);
            self.seek_abs(t, true);
        } else {
            self.cmd(&["seek", &format!("{d}")]);
        }
    }

    fn toggle_pause(&mut self) {
        if self.casting() {
            if self.caster.status.state() == CastState::Paused {
                self.caster.play();
            } else {
                self.caster.pause();
            }
        } else {
            self.cmd(&["cycle", "pause"]);
        }
    }

    fn set_volume(&mut self, v: f64) {
        let v = v.clamp(0.0, if self.casting() { 100.0 } else { 150.0 });
        if self.casting() {
            self.caster.set_volume(v);
        } else {
            self.set("volume", &format!("{v:.0}"));
            self.set("mute", "no");
        }
        self.st.volume = v;
    }

    // ------------------------------------------------------------ casting

    fn casting(&self) -> bool {
        self.cast_device.is_some()
    }

    fn start_cast(&mut self, device: Device) {
        let Some(path) = self.current.clone() else {
            self.toast(self.t("请先打开一个视频", "Open a video first"));
            return;
        };
        let sub = if self.s.cast_subs { self.cast_subtitle() } else { None };
        let pos = self.st.time;
        self.set("pause", "yes");
        self.caster.start(device.clone(), path, pos, sub);
        self.toast(format!("{} {}", self.t("正在连接", "Connecting to"), device.name));
        self.cast_device = Some(device);
    }

    fn stop_cast(&mut self, resume: bool) {
        if self.cast_device.take().is_none() {
            return;
        }
        let pos = self.caster.status.pos();
        self.caster.stop();
        self.seek_abs(pos, true);
        if resume {
            self.set("pause", "no");
        }
        self.schedule_live(false, 300);
    }

    /// Subtitle to send along: live speech subtitles if any, else a loaded .srt file.
    fn cast_subtitle(&self) -> Option<Arc<dyn Fn() -> String + Send + Sync>> {
        if self.s.live_enabled && self.live_available() {
            let shared = self.live.shared.clone();
            let (translated, bilingual) = match self.s.sub_mode {
                SubMode::Original => (false, false),
                SubMode::Translated => (true, false),
                SubMode::Bilingual => (true, true),
            };
            return Some(Arc::new(move || shared.to_srt(translated, bilingual)));
        }
        let file = self
            .tracks
            .iter()
            .find(|t| t.kind == "sub" && t.selected && t.external_file.to_ascii_lowercase().ends_with(".srt"))
            .map(|t| t.external_file.clone())?;
        Some(Arc::new(move || {
            std::fs::read(&file).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        }))
    }

    fn pump_cast(&mut self) {
        let Some(dev) = self.cast_device.clone() else {
            return;
        };
        let st = self.caster.status.state();
        match st {
            CastState::Error(e) => {
                self.cast_device = None;
                self.toast(format!("{} {}: {e}", self.t("投屏失败", "Cast failed"), dev.name));
                return;
            }
            CastState::Stopped => {
                self.cast_device = None;
                let pos = self.caster.status.pos();
                self.seek_abs(pos, true);
                self.toast(format!("{} {}", self.t("投屏已结束：", "Casting ended:"), dev.name));
                return;
            }
            _ => {}
        }
        // Local playback stays paused; the UI mirrors the TV.
        if !self.st.paused {
            self.set("pause", "yes");
        }
        let d = self.caster.status.duration();
        self.st.time = self.caster.status.pos();
        if d > 0.0 {
            self.st.duration = d;
        }
        self.st.paused = matches!(st, CastState::Paused | CastState::Connecting);
        self.st.volume = f64::from_bits(self.caster.status.volume.load(Ordering::Relaxed));
        self.st.muted = false;
        self.st.eof = false;
    }

    // ------------------------------------------------------------ snapshots

    /// `with_subs`: include subtitles (mpv's and the live ones).
    /// `clipboard_only`: copy to the clipboard without keeping a file.
    fn snapshot(&mut self, with_subs: bool, clipboard_only: bool) {
        if self.current.is_none() || self.st.idle {
            return;
        }
        let name = self
            .current
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "snapshot".into());
        let stamp = fmt_time(self.st.time).replace(':', "-");
        let (file, ext) = if clipboard_only {
            (std::env::temp_dir().join(format!("okplayer-clip-{}.png", std::process::id())), "png".to_string())
        } else {
            let ext = match self.s.shot_format.as_str() {
                "jpg" | "webp" => self.s.shot_format.clone(),
                _ => "png".into(),
            };
            let dir = shots_dir(&self.s);
            let _ = std::fs::create_dir_all(&dir);
            let mut f = dir.join(format!("{name}-{stamp}.{ext}"));
            let mut n = 2;
            while f.exists() {
                f = dir.join(format!("{name}-{stamp}-{n}.{ext}"));
                n += 1;
            }
            (f, ext)
        };
        let _ = ext;
        let flag = if with_subs { "subtitles" } else { "video" };
        if let Some(m) = &self.mpv {
            if let Err(e) = m.command(&["screenshot-to-file", &file.to_string_lossy(), flag]) {
                self.toast(format!("{} {e}", self.t("截图失败：", "Snapshot failed:")));
                return;
            }
        }
        let lines = if with_subs { self.live_lines() } else { Vec::new() };
        let clipboard = clipboard_only || self.s.shot_copy;
        let ok_msg = if clipboard_only {
            self.t("已复制当前画面到剪贴板", "Frame copied to clipboard").to_string()
        } else if clipboard {
            format!("{} {}（{}）", self.t("已截图：", "Saved:"), file.display(), self.t("已复制", "copied"))
        } else {
            format!("{} {}", self.t("已截图：", "Saved:"), file.display())
        };
        if lines.is_empty() && !clipboard {
            self.toast(ok_msg);
            return;
        }
        let job =
            shots::Job { path: file, lines, clipboard, temp: clipboard_only, jpeg_quality: self.s.shot_jpeg_quality };
        let out = self.shot_msg.clone();
        let fail = self.t("截图处理失败：", "Snapshot failed:").to_string();
        std::thread::spawn(move || {
            let msg = match shots::run(job) {
                Ok(()) => ok_msg,
                Err(e) => format!("{fail} {e}"),
            };
            *out.lock().unwrap() = Some(msg);
        });
    }

    /// The live subtitle on screen right now, as lines to burn into a snapshot.
    fn live_lines(&self) -> Vec<shots::Line> {
        if !self.s.live_enabled {
            return Vec::new();
        }
        let Some(cue) = self.live.shared.cue_at(self.st.time) else {
            return Vec::new();
        };
        let big = self.s.sub_font_size / 720.0;
        let small = big * 0.72;
        let white = [255, 255, 255];
        let yellow = [255, 226, 140];
        let line = |t: &str, r: f32, c: [u8; 3]| shots::Line { text: t.to_string(), rel_size: r, color: c };
        match (self.s.sub_mode, &cue.trans) {
            (SubMode::Original, _) | (_, None) => vec![line(&cue.text, big, white)],
            (SubMode::Translated, Some(t)) => vec![line(t, big, white)],
            (SubMode::Bilingual, Some(t)) => {
                vec![line(t, big, white), line(&cue.text, small, yellow)]
            }
        }
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
    }

    fn toggle_pip(&mut self, ctx: &egui::Context) {
        if let Some(size) = self.pip_restore.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        } else {
            let size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size())).unwrap_or(vec2(1280.0, 760.0));
            self.pip_restore = Some(size);
            if ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(480.0, 270.0)));
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let k = |m: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(m, key));
        let none = Modifiers::NONE;
        let ctrl = Modifiers::COMMAND;
        let space = k(none, Key::Space);
        let left = k(none, Key::ArrowLeft);
        let right = k(none, Key::ArrowRight);
        let up = k(none, Key::ArrowUp);
        let down = k(none, Key::ArrowDown);
        let f = k(none, Key::F) || k(none, Key::Enter);
        let esc = k(none, Key::Escape);
        let m = k(none, Key::M);
        let shift_s = k(Modifiers::SHIFT, Key::S);
        let s = k(none, Key::S);
        let copy = k(ctrl, Key::C);
        let stats = k(ctrl, Key::I);
        let open = k(ctrl, Key::O);
        let slower = k(none, Key::OpenBracket);
        let faster = k(none, Key::CloseBracket);
        let prev = k(none, Key::PageUp);
        let next = k(none, Key::PageDown);
        let c = k(none, Key::C);
        if space {
            self.toggle_pause();
        }
        if left {
            self.seek_rel(-5.0);
        }
        if right {
            self.seek_rel(5.0);
        }
        if up {
            self.set_volume(self.st.volume + 5.0);
        }
        if down {
            self.set_volume(self.st.volume - 5.0);
        }
        if f {
            self.toggle_fullscreen(ctx);
        }
        if esc {
            if self.pip_restore.is_some() {
                self.toggle_pip(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
        }
        if m {
            self.cmd(&["cycle", "mute"]);
        }
        if s {
            self.snapshot(self.s.shot_with_subs, false);
        }
        if shift_s {
            self.snapshot(!self.s.shot_with_subs, false);
        }
        if copy {
            self.snapshot(self.s.shot_with_subs, true);
        }
        if stats {
            self.show_stats = !self.show_stats;
        }
        if open {
            self.open_dialog();
        }
        if slower {
            self.cmd(&["multiply", "speed", "0.9091"]);
        }
        if faster {
            self.cmd(&["multiply", "speed", "1.1"]);
        }
        if prev {
            self.step(-1);
        }
        if next {
            self.step(1);
        }
        if c && self.live_available() {
            let on = !self.s.live_enabled;
            self.set_live(on);
        }
    }
}

impl eframe::App for PlayerApp {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump();
        let inbox: Vec<Option<PathBuf>> = self.inbox.as_ref().map(|r| r.try_iter().collect()).unwrap_or_default();
        for msg in inbox {
            if let Some(p) = msg {
                if p.to_string_lossy().contains("://") {
                    self.playlist = vec![p.clone()];
                    self.index = 0;
                    self.load(p);
                } else {
                    self.open_many(vec![p]);
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.keyboard_extra(ctx);
        self.keyboard(ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if !dropped.is_empty() {
            self.handle_drop(dropped);
        }

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        if pointer != self.last_pointer || ctx.input(|i| i.pointer.any_down()) {
            self.last_pointer = pointer;
            self.last_motion = Instant::now();
        }

        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let pip = self.pip_restore.is_some();
        if !fullscreen && !pip {
            self.title_bar(ctx);
        }

        if !pip {
            self.playlist_panel(ctx);
        }
        egui::CentralPanel::default().frame(Frame::NONE.fill(Color32::BLACK)).show(ctx, |ui| {
            let rect = ui.max_rect();
            self.video_area(ui, rect, fullscreen);
        });

        if self.show_prefs {
            self.prefs_window(ctx);
        }
        self.extra_windows(ctx);

        #[cfg(not(target_os = "macos"))]
        if !fullscreen {
            resize_edges(ctx);
        }

        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if self.s.resume {
            self.save_position();
        }
        self.live.stop();
        if self.casting() {
            self.caster.stop();
            std::thread::sleep(Duration::from_millis(300));
        }
        if let Some(r) = &self.renderer {
            r.lock().unwrap().destroy(gl);
        }
        self.s.volume = self.st.volume;
        self.s.save();
    }
}

// ---------------------------------------------------------------- UI pieces

fn icon(s: &str, size: f32) -> RichText {
    RichText::new(s).family(ic::family()).size(size).color(TEXT)
}

fn icon_button(ui: &mut egui::Ui, s: &str, size: f32, tip: &str) -> egui::Response {
    let r = ui.add(egui::Button::new(icon(s, size)).frame(false).min_size(vec2(size + 14.0, size + 14.0)));
    if r.hovered() {
        ui.painter().rect_filled(r.rect, CornerRadius::same(8), Color32::from_white_alpha(18));
    }
    r.on_hover_text(tip)
}

fn menu_label(icon_s: &str, text: &str) -> String {
    format!("{icon_s}   {text}")
}

impl PlayerApp {
    fn title_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("title")
            .exact_height(42.0)
            .frame(Frame::NONE.fill(TITLE_BG).inner_margin(Margin::symmetric(12, 0)))
            .show_separator_line(false)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                let resp = ui.interact(bar, Id::new("title-drag"), Sense::click_and_drag());
                if resp.double_clicked() {
                    let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                } else if resp.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.horizontal_centered(|ui| {
                    if cfg!(target_os = "macos") {
                        ui.add_space(66.0);
                    }
                    let (logo, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                    ui.painter().rect_filled(logo, CornerRadius::same(6), ACCENT);
                    ui.painter().text(
                        logo.center() + vec2(1.0, 0.0),
                        Align2::CENTER_CENTER,
                        ic::PLAY,
                        FontId::new(13.0, ic::family()),
                        Color32::WHITE,
                    );
                    ui.label(RichText::new(APP_NAME).strong().size(15.0).color(TEXT));
                    ui.add_space(8.0);
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, 18.0), Sense::hover());
                    ui.painter().rect_filled(sep, CornerRadius::ZERO, Color32::from_white_alpha(40));
                    ui.add_space(8.0);
                    let name = self
                        .current
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    ui.add(egui::Label::new(RichText::new(name).size(14.5).color(TEXT)).truncate());

                    #[cfg(not(target_os = "macos"))]
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, ic::X, 16.0, self.t("关闭", "Close")).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                        let (glyph, tip) = if max {
                            (ic::WIN_RESTORE, self.t("还原", "Restore"))
                        } else {
                            (ic::WIN_MAX, self.t("最大化", "Maximize"))
                        };
                        if icon_button(ui, glyph, 14.0, tip).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                        }
                        if icon_button(ui, ic::WIN_MIN, 16.0, self.t("最小化", "Minimize")).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                    });
                });
            });
    }

    fn video_area(&mut self, ui: &mut egui::Ui, rect: Rect, fullscreen: bool) {
        let ctx = ui.ctx().clone();
        if let Some(err) = self.init_error.clone() {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("{}\n\n{err}", self.t("无法加载 libmpv", "Could not load libmpv")),
                FontId::proportional(16.0),
                TEXT,
            );
            return;
        }

        // video
        if let Some(r) = self.renderer.clone() {
            if !self.st.idle {
                let cb = egui::PaintCallback {
                    rect,
                    callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                        let vp = info.viewport_in_pixels();
                        if let Ok(mut r) = r.lock() {
                            r.paint(painter.gl(), vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
                        }
                    })),
                };
                ui.painter().add(cb);
            }
        }

        // background interaction (click = pause, double click = fullscreen, wheel = volume)
        let bg = ui.interact(rect, Id::new("video-bg"), Sense::click());
        if !self.st.idle {
            if bg.clicked() {
                self.toggle_pause();
            }
            if bg.double_clicked() {
                self.toggle_fullscreen(&ctx);
            }
            if bg.hovered() {
                let (dy, zoom) = ctx.input(|i| (i.raw_scroll_delta.y, i.modifiers.command));
                if dy.abs() > 0.5 {
                    if zoom {
                        self.zoom_by(if dy > 0.0 { 0.1 } else { -0.1 });
                    } else {
                        self.set_volume(self.st.volume + if dy > 0.0 { 2.0 } else { -2.0 });
                    }
                }
            }
        }
        bg.context_menu(|ui| self.settings_menu(ui, &ctx));

        if self.st.idle {
            self.empty_state(ui, rect);
        }
        if self.casting() {
            self.cast_banner(ui, rect);
        }

        let active = self.last_motion.elapsed() < Duration::from_millis(2500)
            || self.st.paused
            || self.st.idle
            || self.controls_hovered;
        if fullscreen && !active {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }

        let controls_h = if active && !self.st.idle { 96.0 } else { 0.0 };
        self.subtitles(ui, rect, controls_h);
        if self.show_stats {
            self.stats_overlay(ui, rect);
        }
        self.live_badge(ui, rect);
        if active && !self.st.idle {
            self.controls(ui, rect);
        } else {
            self.controls_hovered = false;
        }
        self.toast_overlay(ui, rect, controls_h);
    }

    fn empty_state(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, BG);
        let c = rect.center();
        painter.text(c - vec2(0.0, 70.0), Align2::CENTER_CENTER, ic::FILM, FontId::new(56.0, ic::family()), ACCENT);
        painter.text(
            c + vec2(0.0, 52.0),
            Align2::CENTER_CENTER,
            self.t("或将视频 / 字幕文件拖放到这里", "or drop a video / subtitle file here"),
            FontId::proportional(14.0),
            MUTED,
        );
        painter.text(
            c + vec2(0.0, 78.0),
            Align2::CENTER_CENTER,
            "MKV · MP4 · MOV · AVI · WebM · HEVC · AV1 · VP9 · TS · FLV · Blu-ray/DVD · ...",
            FontId::proportional(12.0),
            MUTED.linear_multiply(0.7),
        );
        let btn = Rect::from_center_size(c + vec2(0.0, 8.0), vec2(180.0, 42.0));
        let r = ui.put(
            btn,
            egui::Button::new(
                RichText::new(menu_label(ic::FOLDER_OPEN, self.t("打开视频", "Open video")))
                    .size(15.0)
                    .color(Color32::WHITE),
            )
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(10)),
        );
        if r.clicked() {
            self.open_dialog();
        }
        self.recent_on_empty(ui, c + vec2(0.0, 118.0));
    }

    fn controls(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let h = 96.0;
        let area = Rect::from_min_max(pos2(rect.left(), rect.bottom() - h - 30.0), rect.max);
        // soft gradient
        let steps = 12;
        for i in 0..steps {
            let y0 = area.top() + area.height() * i as f32 / steps as f32;
            let y1 = area.top() + area.height() * (i + 1) as f32 / steps as f32;
            let a = (i as f32 / steps as f32).powf(1.4) * 200.0;
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(area.left(), y0), pos2(area.right(), y1)),
                CornerRadius::ZERO,
                Color32::from_black_alpha(a as u8),
            );
        }
        let inner = Rect::from_min_max(
            pos2(rect.left() + 28.0, rect.bottom() - h),
            pos2(rect.right() - 28.0, rect.bottom() - 8.0),
        );
        let resp = ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inner), |ui| {
            ui.style_mut().visuals.button_frame = false;
            ui.vertical(|ui| {
                self.seek_bar(ui, inner.width());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (g, tip) = if self.st.paused {
                        (ic::PLAY, self.t("播放 (空格)", "Play (Space)"))
                    } else {
                        (ic::PAUSE, self.t("暂停 (空格)", "Pause (Space)"))
                    };
                    if icon_button(ui, g, 24.0, tip).clicked() {
                        self.toggle_pause();
                    }
                    ui.add_space(8.0);
                    if icon_button(ui, ic::SKIP_BACK, 20.0, self.t("上一个", "Previous")).clicked() {
                        self.step(-1);
                    }
                    if icon_button(ui, ic::SKIP_FORWARD, 20.0, self.t("下一个", "Next")).clicked() {
                        self.step(1);
                    }
                    ui.add_space(8.0);
                    let vg = if self.st.muted || self.st.volume <= 0.0 {
                        ic::VOLUME_X
                    } else if self.st.volume < 50.0 {
                        ic::VOLUME_LOW
                    } else {
                        ic::VOLUME
                    };
                    if icon_button(ui, vg, 20.0, self.t("静音 (M)", "Mute (M)")).clicked() {
                        self.cmd(&["cycle", "mute"]);
                    }
                    self.volume_bar(ui);
                    ui.add_space(14.0);
                    let shown = self.seek_preview.unwrap_or(self.st.time);
                    ui.label(
                        RichText::new(format!("{} / {}", fmt_time(shown), fmt_time(self.st.duration)))
                            .size(14.5)
                            .color(TEXT),
                    );
                    if (self.st.speed - 1.0).abs() > 0.01 {
                        ui.label(RichText::new(format!("{:.2}x", self.st.speed)).size(13.0).color(ACCENT));
                    }

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                        let (g, tip) = if fs {
                            (ic::MINIMIZE, self.t("退出全屏 (F)", "Exit full screen (F)"))
                        } else {
                            (ic::MAXIMIZE, self.t("全屏 (F)", "Full screen (F)"))
                        };
                        if icon_button(ui, g, 20.0, tip).clicked() {
                            self.toggle_fullscreen(&ctx);
                        }
                        if icon_button(ui, ic::PIP, 20.0, self.t("画中画", "Picture in picture")).clicked() {
                            self.toggle_pip(&ctx);
                        }
                        ui.menu_button(icon(ic::SETTINGS, 21.0), |ui| self.settings_menu(ui, &ctx))
                            .response
                            .on_hover_text(self.t("设置", "Settings"));
                        let cast_color = if self.casting() { ACCENT } else { TEXT };
                        let cast_btn =
                            ui.menu_button(icon(ic::CAST, 21.0).color(cast_color), |ui| self.cast_menu(ui, &ctx));
                        if cast_btn.response.clicked()
                            && !self.caster.discovery.searching.load(Ordering::Relaxed)
                            && self.caster.discovery.devices.lock().unwrap().is_empty()
                        {
                            self.caster.discovery.search(ctx.clone());
                        }
                        cast_btn.response.on_hover_text(self.t("投屏到电视", "Cast to TV"));
                        let cc_color = if self.s.live_enabled { ACCENT } else { TEXT };
                        ui.menu_button(icon(ic::CAPTIONS, 21.0).color(cc_color), |ui| self.cc_menu(ui))
                            .response
                            .on_hover_text(self.t("字幕 / 实时翻译", "Subtitles / live translation"));
                        let pl_color = if self.x.show_playlist { ACCENT } else { TEXT };
                        let r = ui.add(egui::Button::new(icon(ic::LIST, 21.0).color(pl_color)).frame(false));
                        if r.on_hover_text(self.t("播放列表 (P)", "Playlist (P)")).clicked() {
                            self.x.show_playlist = !self.x.show_playlist;
                        }
                        let loop_color = if self.s.loop_mode == settings::LoopMode::Off { TEXT } else { ACCENT };
                        let r = ui.add(egui::Button::new(icon(self.loop_icon(), 20.0).color(loop_color)).frame(false));
                        if r.on_hover_text(self.t("播放模式", "Play mode")).clicked() {
                            self.cycle_loop_mode();
                        }
                    });
                });
            });
        });
        self.controls_hovered = ui.rect_contains_pointer(area) || resp.response.hovered();
    }

    fn seek_bar(&mut self, ui: &mut egui::Ui, width: f32) {
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
        let hovered = resp.hovered() || resp.dragged();
        let thick = if hovered { 6.0 } else { 4.0 };
        let track = Rect::from_center_size(rect.center(), vec2(rect.width(), thick));
        let p = ui.painter();
        p.rect_filled(track, CornerRadius::same(3), Color32::from_white_alpha(60));
        let dur = self.st.duration.max(0.001);
        // live-subtitle look-ahead, drawn faintly
        if self.s.live_enabled && self.live.running.load(Ordering::Relaxed) {
            let ahead = f64::from_bits(self.live.shared.ahead_secs.load(Ordering::Relaxed));
            let a = ((self.st.time + ahead) / dur).clamp(0.0, 1.0) as f32;
            let mut r = track;
            r.set_right(track.left() + track.width() * a);
            p.rect_filled(r, CornerRadius::same(3), Color32::from_white_alpha(35));
        }
        self.seek_marks(p, track, self.st.duration);
        let shown = self.seek_preview.unwrap_or(self.st.time);
        let frac = (shown / dur).clamp(0.0, 1.0) as f32;
        let x = track.left() + track.width() * frac;
        let mut played = track;
        played.set_right(x);
        p.rect_filled(played, CornerRadius::same(3), ACCENT);
        p.circle_filled(pos2(x, track.center().y), if hovered { 8.0 } else { 6.5 }, Color32::WHITE);

        if let Some(pos) = resp.hover_pos() {
            let t = (((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) as f64) * self.st.duration;
            let label = match self.chapter_at(t) {
                Some(c) => format!("{}  ·  {c}", fmt_time(t)),
                None => fmt_time(t),
            };
            let galley = ui.painter().layout_no_wrap(label, FontId::proportional(12.5), Color32::WHITE);
            let r = Rect::from_center_size(pos2(pos.x, rect.top() - 16.0), galley.size() + vec2(12.0, 6.0));
            if self.s.seek_thumbs && !self.casting() {
                if let Some(tex) = self.thumbs.get(t) {
                    let sz = tex.size_vec2();
                    let w = 176.0f32;
                    let h = w * sz.y / sz.x.max(1.0);
                    let screen = ui.ctx().screen_rect();
                    let cx = pos.x.clamp(screen.left() + w / 2.0 + 6.0, screen.right() - w / 2.0 - 6.0);
                    let img = Rect::from_center_size(pos2(cx, r.top() - 6.0 - h / 2.0), vec2(w, h));
                    let p = ui.painter();
                    p.rect_filled(img.expand(2.0), CornerRadius::same(6), Color32::from_black_alpha(220));
                    p.image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
            }
            ui.painter().rect_filled(r, CornerRadius::same(5), Color32::from_black_alpha(200));
            ui.painter().galley(r.min + vec2(6.0, 3.0), galley, Color32::WHITE);
        }
        if (resp.dragged() || resp.clicked()) && self.st.duration > 0.0 {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = (((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) as f64) * self.st.duration;
                self.seek_preview = Some(t);
                if resp.clicked() {
                    self.seek_abs(t, true);
                    self.seek_preview = None;
                } else if self.last_drag_seek.elapsed() > Duration::from_millis(80) {
                    self.last_drag_seek = Instant::now();
                    self.seek_abs(t, false);
                }
            }
        }
        if resp.drag_stopped() {
            if let Some(t) = self.seek_preview.take() {
                self.seek_abs(t, true);
            }
        }
    }

    fn volume_bar(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(vec2(100.0, 20.0), Sense::click_and_drag());
        let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
        let p = ui.painter();
        p.rect_filled(track, CornerRadius::same(2), Color32::from_white_alpha(60));
        let frac = (self.st.volume / 100.0).clamp(0.0, 1.0) as f32;
        let x = track.left() + track.width() * frac;
        let mut f = track;
        f.set_right(x);
        p.rect_filled(f, CornerRadius::same(2), ACCENT);
        p.circle_filled(pos2(x, track.center().y), 6.0, Color32::WHITE);
        if resp.dragged() || resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let v = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) * 100.0;
                self.set_volume(v as f64);
            }
        }
        resp.on_hover_text(format!("{:.0}%", self.st.volume));
    }

    fn speed_items(&mut self, ui: &mut egui::Ui) {
        for sp in [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0] {
            let on = (self.st.speed - sp).abs() < 0.01;
            let label = if sp == 1.0 { format!("1.0x  {}", self.t("正常", "Normal")) } else { format!("{sp}x") };
            if ui.selectable_label(on, label).clicked() {
                self.set("speed", &sp.to_string());
                ui.close_menu();
            }
        }
    }

    fn track_items(&mut self, ui: &mut egui::Ui, kind: &str) {
        let prop = if kind == "sub" { "sid" } else { "aid" };
        let cur = if kind == "sub" { self.st.sid.clone() } else { self.st.aid.clone() };
        if kind == "sub" && ui.selectable_label(cur == "no", self.t("关闭", "Off")).clicked() {
            self.set(prop, "no");
            ui.close_menu();
        }
        let list: Vec<Track> = self.tracks.iter().filter(|t| t.kind == kind).cloned().collect();
        if list.is_empty() {
            ui.label(RichText::new(self.t("（无）", "(none)")).color(MUTED));
        }
        for t in list {
            if ui.selectable_label(t.selected, t.label()).clicked() {
                self.set(prop, &t.id.to_string());
                ui.close_menu();
            }
        }
        if kind == "sub" {
            ui.separator();
            if ui.button(menu_label(ic::FOLDER_OPEN, self.t("加载字幕文件…", "Load subtitle file…"))).clicked()
            {
                ui.close_menu();
                self.add_sub_dialog();
            }
        }
    }

    fn quality_items(&mut self, ui: &mut egui::Ui) {
        let h = self.st.height as u32;
        let mut opts = vec![(0u32, self.t("自动（原始画质，推荐）", "Auto (original, recommended)").to_string())];
        for (q, name) in
            [(2160, "4K (2160p)"), (1440, "2K (1440p)"), (1080, "1080p"), (720, "720p"), (480, "480p"), (360, "360p")]
        {
            if h == 0 || q < h || q == self.s.quality {
                opts.push((q, name.to_string()));
            }
        }
        for (q, name) in opts {
            let on = self.s.quality == q;
            let text = if on { format!("{}  {name}", ic::CHECK) } else { format!("      {name}") };
            if ui.selectable_label(on, text).clicked() {
                self.s.quality = q;
                self.s.save();
                self.apply_quality();
                ui.close_menu();
            }
        }
        ui.label(
            RichText::new(self.t("降低清晰度可减轻低配设备负载", "Lower quality eases the load on slow machines"))
                .size(12.0)
                .color(MUTED),
        );
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_min_width(230.0);
        ui.menu_button(menu_label(ic::GAUGE, self.t("倍速", "Speed")), |ui| self.speed_items(ui));
        ui.menu_button(menu_label(ic::CAPTIONS, self.t("字幕", "Subtitles")), |ui| self.subtitle_menu(ui));
        ui.menu_button(menu_label(ic::VOLUME, self.t("音频", "Audio")), |ui| self.audio_menu(ui));
        ui.menu_button(menu_label(ic::MONITOR, self.t("清晰度", "Quality")), |ui| self.quality_items(ui));
        self.extra_menu(ui, ctx);
        ui.separator();
        ui.menu_button(menu_label(ic::CAMERA, self.t("截图", "Snapshot")), |ui| self.snapshot_items(ui, ctx));
        if ui.button(menu_label(ic::CAST, self.t("投屏…", "Cast…"))).clicked() {
            ui.close_menu();
            if !self.caster.discovery.searching.load(Ordering::Relaxed) {
                self.caster.discovery.search(ctx.clone());
            }
            self.toast(self.t("点击控制栏的投屏按钮选择设备", "Pick a device from the cast button in the control bar"));
        }
        let stats_sc = ctx.format_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::I));
        if ui
            .add(egui::Button::new(menu_label(ic::CHART, self.t("播放统计", "Playback Stats"))).shortcut_text(stats_sc))
            .clicked()
        {
            ui.close_menu();
            self.show_stats = !self.show_stats;
        }
        self.info_menu_items(ui);
        ui.separator();
        self.open_menu_items(ui, ctx);
        if ui.button(menu_label(ic::SETTINGS, self.t("偏好设置…", "Preferences…"))).clicked() {
            ui.close_menu();
            self.show_prefs = true;
        }
    }

    fn snapshot_items(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_min_width(240.0);
        let (s_main, s_alt) = if self.s.shot_with_subs {
            (self.t("截图（含字幕）", "Snapshot (with subtitles)"), self.t("截图（纯画面）", "Snapshot (video only)"))
        } else {
            (self.t("截图（纯画面）", "Snapshot (video only)"), self.t("截图（含字幕）", "Snapshot (with subtitles)"))
        };
        if ui.add(egui::Button::new(menu_label(ic::CAMERA, s_main)).shortcut_text("S")).clicked() {
            ui.close_menu();
            self.snapshot(self.s.shot_with_subs, false);
        }
        if ui.add(egui::Button::new(menu_label(ic::CAPTIONS, s_alt)).shortcut_text("Shift+S")).clicked() {
            ui.close_menu();
            self.snapshot(!self.s.shot_with_subs, false);
        }
        let copy_sc = ctx.format_shortcut(&egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::C));
        if ui
            .add(egui::Button::new(menu_label(ic::COPY, self.t("复制当前画面", "Copy frame"))).shortcut_text(copy_sc))
            .clicked()
        {
            ui.close_menu();
            self.snapshot(self.s.shot_with_subs, true);
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.t("格式", "Format")).color(MUTED));
            for f in ["png", "jpg", "webp"] {
                if ui.selectable_label(self.s.shot_format == f, f.to_uppercase()).clicked() {
                    self.s.shot_format = f.into();
                    self.s.save();
                }
            }
        });
        if {
            let label = self.t("同时复制到剪贴板", "Also copy to clipboard");
            ui.checkbox(&mut self.s.shot_copy, label)
        }
        .changed()
        {
            self.s.save();
        }
        if ui.button(menu_label(ic::FOLDER, self.t("打开截图文件夹", "Open snapshot folder"))).clicked() {
            ui.close_menu();
            let d = shots_dir(&self.s);
            let _ = std::fs::create_dir_all(&d);
            open_path(&d);
        }
    }

    fn cast_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.set_min_width(280.0);
        if let Some(dev) = self.cast_device.clone() {
            let state = match self.caster.status.state() {
                CastState::Connecting => self.t("连接中…", "Connecting…"),
                CastState::Buffering => self.t("缓冲中…", "Buffering…"),
                CastState::Playing => self.t("播放中", "Playing"),
                CastState::Paused => self.t("已暂停", "Paused"),
                _ => "",
            };
            ui.label(RichText::new(format!("{}  {}", ic::TV, dev.name)).strong());
            ui.label(RichText::new(state).size(12.5).color(MUTED));
            ui.separator();
            if ui.button(menu_label(ic::STOP, self.t("停止投屏，在本机继续", "Stop casting, resume here"))).clicked()
            {
                ui.close_menu();
                self.stop_cast(true);
            }
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.t("投屏到（DLNA）", "Cast to (DLNA)")).color(MUTED).size(12.5));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let searching = self.caster.discovery.searching.load(Ordering::Relaxed);
                if searching {
                    ui.add(egui::Spinner::new().size(14.0));
                } else if ui.small_button(menu_label(ic::REFRESH, self.t("刷新", "Refresh"))).clicked() {
                    self.caster.discovery.search(ctx.clone());
                }
            });
        });
        let devices = self.caster.discovery.devices.lock().unwrap().clone();
        if devices.is_empty() {
            let msg = if self.caster.discovery.searching.load(Ordering::Relaxed) {
                self.t("正在搜索局域网中的电视和盒子…", "Searching the network for TVs and boxes…")
            } else {
                self.t("没有找到设备。请确认电视已开机、和电脑在同一 Wi-Fi，并允许本应用通过防火墙。", "No devices found. Make sure the TV is on, on the same Wi-Fi, and this app is allowed through the firewall.")
            };
            ui.add(egui::Label::new(RichText::new(msg).size(13.0).color(MUTED)).wrap());
        }
        for d in devices {
            let label = if d.model.is_empty() || d.model == d.name {
                format!("{}   {}", ic::TV, d.name)
            } else {
                format!("{}   {}  ·  {}", ic::TV, d.name, d.model)
            };
            if ui.button(label).clicked() {
                ui.close_menu();
                self.start_cast(d);
            }
        }
        ui.separator();
        if {
            let label = self.t("同时发送字幕（实时字幕或 SRT）", "Send subtitles too (live or SRT)");
            ui.checkbox(&mut self.s.cast_subs, label)
        }
        .changed()
        {
            self.s.save();
        }
        ui.label(
            RichText::new(
                self.t("电视自己解码，不支持的格式需先转码", "The TV decodes the file; unsupported formats won't play"),
            )
            .size(12.0)
            .color(MUTED),
        );
    }

    fn cast_banner(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(dev) = self.cast_device.clone() else {
            return;
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(170));
        let c = rect.center() - vec2(0.0, 30.0);
        p.text(c - vec2(0.0, 40.0), Align2::CENTER_CENTER, ic::CAST, FontId::new(54.0, ic::family()), ACCENT);
        p.text(
            c + vec2(0.0, 20.0),
            Align2::CENTER_CENTER,
            format!("{} {}", self.t("正在投屏到", "Casting to"), dev.name),
            FontId::proportional(20.0),
            TEXT,
        );
        let state = match self.caster.status.state() {
            CastState::Connecting => self.t("连接中…", "Connecting…"),
            CastState::Buffering => self.t("电视正在缓冲…", "TV is buffering…"),
            CastState::Paused => self.t("已暂停", "Paused"),
            _ => self.t("用下方控制栏操控电视", "Use the controls below to drive the TV"),
        };
        p.text(c + vec2(0.0, 50.0), Align2::CENTER_CENTER, state, FontId::proportional(14.0), MUTED);
        let btn = Rect::from_center_size(c + vec2(0.0, 95.0), vec2(200.0, 38.0));
        let r = ui.put(
            btn,
            egui::Button::new(
                RichText::new(menu_label(ic::STOP, self.t("停止投屏", "Stop casting"))).color(Color32::WHITE),
            )
            .fill(Color32::from_white_alpha(28))
            .corner_radius(CornerRadius::same(10)),
        );
        if r.clicked() {
            self.stop_cast(true);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }

    fn cc_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(260.0);
        ui.label(RichText::new(self.t("字幕轨道", "Subtitle track")).color(MUTED).size(12.5));
        self.track_items(ui, "sub");
        ui.separator();
        ui.label(RichText::new(self.t("实时识别 + 翻译字幕", "Live speech subtitles")).color(MUTED).size(12.5));
        if !self.live_available() {
            ui.label(
                RichText::new(self.t("需要先下载语音识别模型", "Download a speech model first"))
                    .size(13.0)
                    .color(MUTED),
            );
            if ui.button(menu_label(ic::DOWNLOAD, self.t("下载模型…", "Download models…"))).clicked() {
                ui.close_menu();
                self.show_prefs = true;
                self.pref_tab = PrefTab::Models;
            }
            return;
        }
        let mut on = self.s.live_enabled;
        if ui.checkbox(&mut on, self.t("开启实时字幕 (C)", "Live subtitles (C)")).changed() {
            self.set_live(on);
        }
        ui.horizontal(|ui| {
            for (mode, zh, en) in [
                (SubMode::Original, "原文", "Original"),
                (SubMode::Translated, "译文", "Translation"),
                (SubMode::Bilingual, "双语", "Bilingual"),
            ] {
                if ui.selectable_label(self.s.sub_mode == mode, self.t(zh, en)).clicked() {
                    self.s.sub_mode = mode;
                    self.s.save();
                }
            }
        });
        let zh = self.s.ui_lang == UiLang::Zh;
        let target_name = settings::TARGET_LANGS
            .iter()
            .find(|t| t.0 == self.s.target_lang)
            .map(|t| if zh { t.1 } else { t.2 })
            .unwrap_or("?");
        ui.menu_button(format!("{}  {}: {target_name}", ic::LANGUAGES, self.t("翻译为", "Translate to")), |ui| {
            for (code, z, e, _) in settings::TARGET_LANGS {
                if ui.selectable_label(self.s.target_lang == *code, if zh { *z } else { *e }).clicked() {
                    self.s.target_lang = code.to_string();
                    self.settings_changed(false);
                    // re-translate future lines only; existing lines keep their text
                    ui.close_menu();
                }
            }
        });
        let src_name = settings::SOURCE_LANGS
            .iter()
            .find(|t| t.0 == self.s.source_lang)
            .map(|t| if zh { t.1 } else { t.2 })
            .unwrap_or("?");
        ui.menu_button(
            format!("{}  {}: {src_name}", ic::AUDIO_LINES, self.t("语音语言", "Spoken language")),
            |ui| {
                for (code, z, e) in settings::SOURCE_LANGS {
                    if ui.selectable_label(self.s.source_lang == *code, if zh { *z } else { *e }).clicked() {
                        self.s.source_lang = code.to_string();
                        self.settings_changed(true);
                        ui.close_menu();
                    }
                }
            },
        );
        if ui.button(menu_label(ic::SAVE, self.t("导出字幕 SRT…", "Export SRT…"))).clicked() {
            ui.close_menu();
            let srt = self.live.shared.to_srt(true, self.s.sub_mode == SubMode::Bilingual);
            let default = self
                .current
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| format!("{}.srt", s.to_string_lossy()))
                .unwrap_or_else(|| "subtitles.srt".into());
            if let Some(p) = rfd::FileDialog::new().set_file_name(default).add_filter("SRT", &["srt"]).save_file() {
                match std::fs::write(&p, srt) {
                    Ok(_) => self.toast(self.t("字幕已导出", "Subtitles exported")),
                    Err(e) => self.toast(e.to_string()),
                }
            }
        }
        if ui.button(menu_label(ic::SETTINGS, self.t("翻译与模型设置…", "Translation & models…"))).clicked()
        {
            ui.close_menu();
            self.show_prefs = true;
            self.pref_tab = PrefTab::Translate;
        }
    }

    fn subtitles(&mut self, ui: &mut egui::Ui, rect: Rect, controls_h: f32) {
        if !self.s.live_enabled {
            return;
        }
        let Some(cue) = self.live.shared.cue_at(self.st.time) else {
            return;
        };
        let scale = (rect.height() / 720.0).clamp(0.6, 2.2);
        let big = self.s.sub_font_size * scale;
        let small = big * 0.72;
        let mut lines: Vec<(String, f32, Color32)> = Vec::new();
        let trans = cue.trans.clone();
        match self.s.sub_mode {
            SubMode::Original => lines.push((cue.text.clone(), big, Color32::WHITE)),
            SubMode::Translated => lines.push((trans.unwrap_or_else(|| cue.text.clone()), big, Color32::WHITE)),
            SubMode::Bilingual => {
                if let Some(t) = trans {
                    lines.push((t, big, Color32::WHITE));
                    lines.push((cue.text.clone(), small, Color32::from_rgb(255, 226, 140)));
                } else {
                    lines.push((cue.text.clone(), big, Color32::WHITE));
                }
            }
        }
        let wrap = rect.width() * 0.86;
        let mut bottom = rect.bottom() - controls_h - 28.0 * scale;
        let painter = ui.painter();
        for (text, size, color) in lines.into_iter().rev() {
            let mut job = egui::text::LayoutJob::simple(text, FontId::proportional(size), color, wrap);
            job.halign = Align::Center;
            let galley = painter.ctx().fonts(|f| f.layout_job(job));
            let top = bottom - galley.size().y;
            let pos = pos2(rect.center().x, top);
            let o = (size / 14.0).max(1.5);
            for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, o), (-o, o), (o, -o)] {
                painter.galley_with_override_text_color(
                    pos + vec2(dx, dy),
                    galley.clone(),
                    Color32::from_black_alpha(230),
                );
            }
            painter.galley(pos, galley, color);
            bottom = top - 4.0;
        }
    }

    fn live_badge(&mut self, ui: &mut egui::Ui, rect: Rect) {
        if !self.s.live_enabled || self.st.idle {
            return;
        }
        let status = self.live.shared.status.lock().unwrap().clone();
        let (txt, color) = match status {
            Status::Idle => return,
            Status::LoadingModel => (format!("{} {}", self.t("加载模型", "Loading model"), self.s.model), MUTED),
            Status::Listening => (format!("{} · {}", self.t("实时字幕", "Live subtitles"), self.s.model), ACCENT),
            Status::Error(e) => {
                (format!("{}: {e}", self.t("实时字幕出错", "Live subtitles error")), Color32::from_rgb(255, 120, 120))
            }
        };
        let galley = ui.painter().layout_no_wrap(format!("● {txt}"), FontId::proportional(12.5), color);
        let r = Rect::from_min_size(
            pos2(rect.right() - galley.size().x - 30.0, rect.top() + 14.0),
            galley.size() + vec2(16.0, 8.0),
        );
        ui.painter().rect_filled(r, CornerRadius::same(8), Color32::from_black_alpha(150));
        ui.painter().galley(r.min + vec2(8.0, 4.0), galley, color);
    }

    fn stats_overlay(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(m) = self.mpv.clone() else { return };
        let g = |k: &str| m.get(k).unwrap_or_else(|| "-".into());
        let mut lines = vec![
            format!("File       {}", g("filename")),
            format!("Format     {}", g("file-format")),
            format!(
                "Video      {}  {}x{}  {}",
                g("video-codec"),
                g("video-params/w"),
                g("video-params/h"),
                g("video-params/pixelformat")
            ),
            format!(
                "Color      {} / {} / {}",
                g("video-params/colormatrix"),
                g("video-params/primaries"),
                g("video-params/gamma")
            ),
            format!("FPS        {} (display {})", g("container-fps"), g("estimated-vf-fps")),
            format!("HW decode  {}", g("hwdec-current")),
            format!(
                "Bitrate    video {}  audio {}",
                fmt_bitrate(&g("video-bitrate")),
                fmt_bitrate(&g("audio-bitrate"))
            ),
            format!(
                "Audio      {}  {} Hz  {} ch",
                g("audio-codec-name"),
                g("audio-params/samplerate"),
                g("audio-params/channel-count")
            ),
            format!("Dropped    vo {}  decoder {}", g("frame-drop-count"), g("decoder-frame-drop-count")),
            format!("Cache      {} s", g("demuxer-cache-duration")),
            format!("mpv        {}", g("mpv-version")),
            format!("FFmpeg     {}", g("ffmpeg-version")),
        ];
        if self.s.live_enabled {
            let ahead = f64::from_bits(self.live.shared.ahead_secs.load(Ordering::Relaxed));
            let n = self.live.shared.cues.lock().unwrap().len();
            lines.push(format!("Live ASR   model {}  lead {:.1}s  cues {n}", self.s.model, ahead));
        }
        let text = lines.join("\n");
        let galley = ui.painter().layout(text, FontId::monospace(12.0), TEXT, rect.width() * 0.6);
        let r = Rect::from_min_size(rect.min + vec2(16.0, 16.0), galley.size() + vec2(20.0, 16.0));
        ui.painter().rect_filled(r, CornerRadius::same(10), Color32::from_black_alpha(185));
        ui.painter().galley(r.min + vec2(10.0, 8.0), galley, TEXT);
    }

    fn toast_overlay(&mut self, ui: &mut egui::Ui, rect: Rect, controls_h: f32) {
        let Some((msg, at)) = self.toast.clone() else {
            return;
        };
        if at.elapsed() > Duration::from_millis(2600) {
            self.toast = None;
            return;
        }
        let galley = ui.painter().layout(msg, FontId::proportional(13.5), Color32::WHITE, rect.width() * 0.8);
        let r = Rect::from_center_size(
            pos2(rect.center().x, rect.bottom() - controls_h - 120.0),
            galley.size() + vec2(24.0, 14.0),
        );
        ui.painter().rect_filled(r, CornerRadius::same(10), Color32::from_black_alpha(200));
        ui.painter().galley(r.min + vec2(12.0, 7.0), galley, Color32::WHITE);
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }

    // ------------------------------------------------------------ preferences

    fn prefs_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new(self.t("偏好设置", "Preferences"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([600.0, 480.0])
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (tab, zh, en, g) in [
                        (PrefTab::General, "通用", "General", ic::SETTINGS),
                        (PrefTab::Translate, "字幕翻译", "Translation", ic::LANGUAGES),
                        (PrefTab::Models, "语音模型", "Speech models", ic::CPU),
                    ] {
                        if ui.selectable_label(self.pref_tab == tab, menu_label(g, self.t(zh, en))).clicked() {
                            self.pref_tab = tab;
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.pref_tab {
                    PrefTab::General => self.prefs_general(ui),
                    PrefTab::Translate => self.prefs_translate(ui),
                    PrefTab::Models => self.prefs_models(ui),
                });
            });
        if !open {
            self.show_prefs = false;
        }
    }

    fn prefs_general(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("general").num_columns(2).spacing([16.0, 12.0]).show(ui, |ui| {
            ui.label(self.t("界面语言", "Language"));
            ui.horizontal(|ui| {
                if ui.selectable_label(self.s.ui_lang == UiLang::Zh, "简体中文").clicked() {
                    self.s.ui_lang = UiLang::Zh;
                    self.s.save();
                }
                if ui.selectable_label(self.s.ui_lang == UiLang::En, "English").clicked() {
                    self.s.ui_lang = UiLang::En;
                    self.s.save();
                }
            });
            ui.end_row();

            ui.label(self.t("硬件解码", "Hardware decoding"));
            let mut hw = self.s.hwdec.clone();
            let choices = settings::hwdec_choices();
            let cur_label = choices.iter().find(|c| c.0 == hw).map(|c| c.1).unwrap_or(hw.as_str()).to_string();
            egui::ComboBox::from_id_salt("hwdec").selected_text(cur_label).width(220.0).show_ui(ui, |ui| {
                for (v, l) in &choices {
                    ui.selectable_value(&mut hw, v.to_string(), *l);
                }
            });
            if hw != self.s.hwdec {
                self.s.hwdec = hw;
                self.set("hwdec", &self.s.hwdec.clone());
                self.s.save();
            }
            ui.end_row();

            ui.label("");
            let cur = self.mpv.as_ref().and_then(|m| m.get("hwdec-current")).unwrap_or_else(|| "-".into());
            ui.label(RichText::new(format!("{} {cur}", self.t("当前生效：", "In use:"))).size(12.5).color(MUTED));
            ui.end_row();

            ui.label(self.t("截图目录", "Snapshots"));
            ui.horizontal(|ui| {
                let d = shots_dir(&self.s);
                if ui.link(d.display().to_string()).clicked() {
                    let _ = std::fs::create_dir_all(&d);
                    open_path(&d);
                }
                if ui.small_button(self.t("更改…", "Change…")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().set_directory(&d).pick_folder() {
                        self.s.shot_dir = p.to_string_lossy().to_string();
                        self.s.save();
                        self.set("screenshot-directory", &self.s.shot_dir.clone());
                    }
                }
                if !self.s.shot_dir.is_empty() && ui.small_button(self.t("恢复默认", "Default")).clicked() {
                    self.s.shot_dir.clear();
                    self.s.save();
                }
            });
            ui.end_row();

            ui.label(self.t("截图格式", "Snapshot format"));
            ui.horizontal(|ui| {
                for f in ["png", "jpg", "webp"] {
                    if ui.selectable_label(self.s.shot_format == f, f.to_uppercase()).clicked() {
                        self.s.shot_format = f.into();
                        self.s.save();
                    }
                }
                if self.s.shot_format == "jpg" {
                    let mut q = self.s.shot_jpeg_quality as u32;
                    if ui.add(egui::Slider::new(&mut q, 50..=100).text(self.t("质量", "quality"))).changed() {
                        self.s.shot_jpeg_quality = q as u8;
                        self.s.save();
                        self.set("screenshot-jpeg-quality", &q.to_string());
                    }
                }
            });
            ui.end_row();

            ui.label("");
            ui.vertical(|ui| {
                if {
                    let label =
                        self.t("S 键截图包含字幕（Shift+S 相反）", "S includes subtitles (Shift+S does the opposite)");
                    ui.checkbox(&mut self.s.shot_with_subs, label)
                }
                .changed()
                {
                    self.s.save();
                }
                if {
                    let label = self.t("截图同时复制到剪贴板", "Also copy snapshots to the clipboard");
                    ui.checkbox(&mut self.s.shot_copy, label)
                }
                .changed()
                {
                    self.s.save();
                }
            });
            ui.end_row();

            ui.label(self.t("播放", "Playback"));
            {
                let label = self.t("进度条悬停显示缩略图", "Thumbnail preview on the seek bar");
                if ui.checkbox(&mut self.s.seek_thumbs, label).changed() {
                    self.s.save();
                }
            }
            ui.end_row();
            ui.label("");
            {
                let label = self.t(
                    "只开一个窗口（双击文件在当前窗口播放，重启后生效）",
                    "Single window (open files in the running player; after restart)",
                );
                if ui.checkbox(&mut self.s.single_instance, label).changed() {
                    self.s.save();
                }
            }
            ui.end_row();
            ui.label("");
            {
                let label = self.t("记住播放进度，下次从上次位置继续", "Resume files where you left off");
                if ui.checkbox(&mut self.s.resume, label).changed() {
                    self.s.save();
                }
            }
            ui.end_row();

            ui.label(self.t("投屏", "Casting"));
            if {
                let label = self
                    .t("投屏时发送字幕（实时字幕或已加载的 SRT）", "Send subtitles when casting (live or loaded SRT)");
                ui.checkbox(&mut self.s.cast_subs, label)
            }
            .changed()
            {
                self.s.save();
            }
            ui.end_row();

            ui.label("libmpv");
            let info = match &self.mpv {
                Some(m) => {
                    let (a, b) = m.client_api_version();
                    format!("{}  (API {a}.{b})\n{}", m.get("mpv-version").unwrap_or_default(), m.api().path)
                }
                None => self.init_error.clone().unwrap_or_default(),
            };
            ui.label(RichText::new(info).size(12.5).color(MUTED));
            ui.end_row();
        });
        ui.add_space(10.0);
        ui.label(
            RichText::new(self.t(
                "快捷键：空格 播放/暂停 · ←/→ 快退/快进 5 秒 · ↑/↓ 音量 · F 全屏 · M 静音 · S 截图 · Shift+S 截图（另一种字幕模式）· Ctrl+C 复制画面 · [ ] 调速 · PgUp/PgDn 上/下一个 · C 实时字幕 · Ctrl+I 统计 · Ctrl+O 打开",
                "Keys: Space play/pause · ←/→ seek 5 s · ↑/↓ volume · F full screen · M mute · S snapshot · Shift+S snapshot (other subtitle mode) · Ctrl+C copy frame · [ ] speed · PgUp/PgDn prev/next · C live subtitles · Ctrl+I stats · Ctrl+O open",
            ))
            .size(12.5)
            .color(MUTED),
        );
    }

    fn prefs_translate(&mut self, ui: &mut egui::Ui) {
        let zh = self.s.ui_lang == UiLang::Zh;
        let before = (self.s.provider, self.s.source_lang.clone());
        ui.label(RichText::new(self.t("翻译来源", "Translation source")).strong());
        for (p, z, e) in [
            (
                Provider::Bilibili,
                "Bilibili Index-Translate（免费，无需 Key，默认）",
                "Bilibili Index-Translate (free, no key, default)",
            ),
            (
                Provider::OpenAiCompatible,
                "OpenAI 兼容接口（OpenAI / DeepSeek / Ollama 等）",
                "OpenAI-compatible API (OpenAI / DeepSeek / Ollama …)",
            ),
            (
                Provider::WhisperToEnglish,
                "Whisper 内置翻译（离线，仅翻译成英文）",
                "Whisper built-in (offline, English only)",
            ),
            (Provider::Off, "不翻译，只显示识别原文", "No translation, transcript only"),
        ] {
            if ui.radio(self.s.provider == p, if zh { z } else { e }).clicked() {
                self.s.provider = p;
            }
        }
        if self.s.provider == Provider::OpenAiCompatible {
            egui::Grid::new("oa").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Base URL");
                ui.add(egui::TextEdit::singleline(&mut self.s.openai_base_url).desired_width(320.0));
                ui.end_row();
                ui.label("API Key");
                ui.add(egui::TextEdit::singleline(&mut self.s.openai_api_key).password(true).desired_width(320.0));
                ui.end_row();
                ui.label(self.t("模型", "Model"));
                ui.add(egui::TextEdit::singleline(&mut self.s.openai_model).desired_width(320.0));
                ui.end_row();
            });
            ui.label(
                RichText::new(
                    "DeepSeek: https://api.deepseek.com/v1 · deepseek-chat    Ollama: http://127.0.0.1:11434/v1",
                )
                .size(12.0)
                .color(MUTED),
            );
        }
        if matches!(self.s.provider, Provider::Bilibili | Provider::OpenAiCompatible) {
            ui.horizontal(|ui| {
                if ui.button(self.t("测试翻译", "Test")).clicked() {
                    let cfg = live::Config::from_settings(&self.s);
                    let out = self.test_result.clone();
                    *out.lock().unwrap() = Some("…".into());
                    let ctx = ui.ctx().clone();
                    std::thread::spawn(move || {
                        let client = reqwest::blocking::Client::new();
                        let r = live::translate(&client, &cfg, "Where are we going tonight?");
                        *out.lock().unwrap() = Some(match r {
                            Ok(t) => format!("✓ {t}"),
                            Err(e) => format!("✗ {e}"),
                        });
                        ctx.request_repaint();
                    });
                }
                if let Some(r) = self.test_result.lock().unwrap().clone() {
                    ui.label(r);
                }
            });
        }
        ui.add_space(10.0);
        egui::Grid::new("langs").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            ui.label(self.t("语音语言", "Spoken language"));
            let mut src = self.s.source_lang.clone();
            let src_label =
                settings::SOURCE_LANGS.iter().find(|t| t.0 == src).map(|t| if zh { t.1 } else { t.2 }).unwrap_or("?");
            egui::ComboBox::from_id_salt("src").selected_text(src_label).show_ui(ui, |ui| {
                for (c, z, e) in settings::SOURCE_LANGS {
                    ui.selectable_value(&mut src, c.to_string(), if zh { *z } else { *e });
                }
            });
            self.s.source_lang = src;
            ui.end_row();

            ui.label(self.t("翻译为", "Translate to"));
            let mut tgt = self.s.target_lang.clone();
            let tgt_label =
                settings::TARGET_LANGS.iter().find(|t| t.0 == tgt).map(|t| if zh { t.1 } else { t.2 }).unwrap_or("?");
            egui::ComboBox::from_id_salt("tgt").selected_text(tgt_label).show_ui(ui, |ui| {
                for (c, z, e, _) in settings::TARGET_LANGS {
                    ui.selectable_value(&mut tgt, c.to_string(), if zh { *z } else { *e });
                }
            });
            self.s.target_lang = tgt;
            ui.end_row();

            ui.label(self.t("字幕显示", "Show"));
            ui.horizontal(|ui| {
                for (mode, z, e) in [
                    (SubMode::Original, "原文", "Original"),
                    (SubMode::Translated, "译文", "Translation"),
                    (SubMode::Bilingual, "双语", "Bilingual"),
                ] {
                    if ui.selectable_label(self.s.sub_mode == mode, if zh { z } else { e }).clicked() {
                        self.s.sub_mode = mode;
                    }
                }
            });
            ui.end_row();

            ui.label(self.t("字号", "Font size"));
            ui.add(egui::Slider::new(&mut self.s.sub_font_size, 16.0..=56.0));
            ui.end_row();
        });
        let restart = before != (self.s.provider, self.s.source_lang.clone());
        if ui.ctx().input(|i| i.pointer.any_released() || i.key_released(Key::Enter)) || restart {
            self.settings_changed(restart);
        }
    }

    fn prefs_models(&mut self, ui: &mut egui::Ui) {
        let zh = self.s.ui_lang == UiLang::Zh;
        ui.label(
            RichText::new(self.t(
                "实时字幕使用 whisper.cpp 在本机离线识别语音。下载一个模型后即可在 CC 菜单开启。",
                "Live subtitles run whisper.cpp offline on this computer. Download a model, then turn it on from the CC menu.",
            ))
            .color(MUTED),
        );
        let mirror_label = self.t("使用 hf-mirror.com 镜像下载（国内更快）", "Download via hf-mirror.com");
        if ui.checkbox(&mut self.s.use_mirror, mirror_label).changed() {
            self.s.save();
        }
        ui.add_space(8.0);
        let downloads = self.downloads.lock().unwrap().clone();
        for m in models::MODELS {
            Frame::NONE
                .fill(Color32::from_rgb(38, 38, 52))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("{}   {} MB", m.id, m.size_mb)).strong());
                            ui.label(RichText::new(if zh { m.note_zh } else { m.note_en }).size(12.5).color(MUTED));
                        });
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            let dl = downloads.iter().find(|(id, _)| id == m.id).map(|(_, d)| d.clone());
                            let running = dl.as_ref().map(|d| !d.finished.load(Ordering::Relaxed)).unwrap_or(false);
                            if models::is_installed(m.id) {
                                if icon_button(ui, ic::TRASH, 16.0, self.t("删除", "Delete")).clicked() {
                                    models::delete(m.id);
                                    if self.s.model == m.id && self.s.live_enabled {
                                        self.set_live(false);
                                    }
                                }
                                if self.s.model == m.id {
                                    ui.label(
                                        RichText::new(format!("{} {}", ic::CHECK, self.t("使用中", "In use")))
                                            .color(ACCENT),
                                    );
                                } else if ui.button(self.t("设为当前", "Use")).clicked() {
                                    self.s.model = m.id.to_string();
                                    self.settings_changed(true);
                                }
                            } else if running {
                                let d = dl.unwrap();
                                if icon_button(ui, ic::X, 16.0, self.t("取消", "Cancel")).clicked() {
                                    d.cancel.store(true, Ordering::Relaxed);
                                }
                                let done = d.done.load(Ordering::Relaxed) as f32;
                                let total = d.total.load(Ordering::Relaxed).max(1) as f32;
                                ui.add(
                                    egui::ProgressBar::new(done / total)
                                        .desired_width(180.0)
                                        .fill(ACCENT)
                                        .text(format!("{:.0} / {:.0} MB", done / 1e6, total / 1e6)),
                                );
                            } else {
                                if ui.button(menu_label(ic::DOWNLOAD, self.t("下载", "Download"))).clicked() {
                                    models::start_download(&self.downloads, m.id, self.s.use_mirror, ui.ctx().clone());
                                    if !self.live_available() {
                                        self.s.model = m.id.to_string();
                                        self.s.save();
                                    }
                                }
                                if let Some(err) = dl.as_ref().and_then(|d| d.error.lock().unwrap().clone()) {
                                    ui.label(RichText::new(err).size(12.0).color(Color32::from_rgb(255, 120, 120)));
                                }
                            }
                        });
                    });
                });
            ui.add_space(6.0);
        }
        // a finished download makes the model selectable; refresh live config
        if downloads.iter().any(|(_, d)| d.finished.load(Ordering::Relaxed)) {
            self.live.configure(live::Config::from_settings(&self.s));
        }
    }
}

// ---------------------------------------------------------------- helpers

#[cfg(not(target_os = "macos"))]
fn resize_edges(ctx: &egui::Context) {
    use egui::{CursorIcon, ResizeDirection as D};
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }
    let Some(p) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let r = ctx.screen_rect();
    let m = 6.0;
    let (l, rt, t, b) = (p.x < r.left() + m, p.x > r.right() - m, p.y < r.top() + m, p.y > r.bottom() - m);
    let dir = match (l, rt, t, b) {
        (true, _, true, _) => Some((D::NorthWest, CursorIcon::ResizeNorthWest)),
        (_, true, true, _) => Some((D::NorthEast, CursorIcon::ResizeNorthEast)),
        (true, _, _, true) => Some((D::SouthWest, CursorIcon::ResizeSouthWest)),
        (_, true, _, true) => Some((D::SouthEast, CursorIcon::ResizeSouthEast)),
        (true, ..) => Some((D::West, CursorIcon::ResizeWest)),
        (_, true, ..) => Some((D::East, CursorIcon::ResizeEast)),
        (_, _, true, _) => Some((D::North, CursorIcon::ResizeNorth)),
        (_, _, _, true) => Some((D::South, CursorIcon::ResizeSouth)),
        _ => None,
    };
    if let Some((d, cur)) = dir {
        ctx.set_cursor_icon(cur);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(d));
        }
    }
}

pub fn fmt_time(t: f64) -> String {
    let s = t.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn fmt_bitrate(v: &str) -> String {
    v.parse::<f64>().map(|b| format!("{:.0} kbps", b / 1000.0)).unwrap_or_else(|_| "-".into())
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn sibling_media(path: &Path) -> Vec<PathBuf> {
    let Some(dir) = path.parent() else {
        return vec![];
    };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && VIDEO_EXTS.contains(&ext_of(p).as_str()))
        .collect();
    v.sort_by_key(|p| natural_key(&p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()));
    v
}

/// Sort "ep2" before "ep10".
fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut out = Vec::new();
    let mut num = String::new();
    let mut txt = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            if !txt.is_empty() {
                out.push((1, 0, std::mem::take(&mut txt)));
            }
            num.push(ch);
        } else {
            if !num.is_empty() {
                out.push((0, num.parse().unwrap_or(0), String::new()));
                num.clear();
            }
            txt.push(ch);
        }
    }
    if !num.is_empty() {
        out.push((0, num.parse().unwrap_or(0), String::new()));
    }
    if !txt.is_empty() {
        out.push((1, 0, txt));
    }
    out
}

fn shots_dir(s: &Settings) -> PathBuf {
    if !s.shot_dir.is_empty() {
        return PathBuf::from(&s.shot_dir);
    }
    dirs::picture_dir().unwrap_or_else(|| settings::data_dir()).join(APP_NAME)
}

fn open_path(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}
