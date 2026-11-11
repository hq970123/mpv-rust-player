//! Everyday player features: playlist panel and play modes, resume and recent
//! files, chapters, A-B loop, frame stepping, picture / audio / subtitle
//! adjustments, open folder / URL, window options, media info, shortcut sheet
//! and sleep timer.

use super::*;
use crate::settings::LoopMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const HISTORY_MAX_POSITIONS: usize = 400;
const HISTORY_MAX_RECENT: usize = 20;

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct History {
    /// file path -> last position in seconds
    positions: HashMap<String, f64>,
    /// most recent first
    pub(super) recent: Vec<String>,
}

impl History {
    fn path() -> PathBuf {
        settings::data_dir().join("history.json")
    }
    pub(super) fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn save(&self) {
        if let Ok(b) = serde_json::to_vec(self) {
            let _ = std::fs::write(Self::path(), b);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Sleep {
    At(Instant),
    EndOfFile,
}

pub(super) struct Extra {
    pub(super) show_playlist: bool,
    pub(super) show_info: bool,
    pub(super) show_keys: bool,
    pub(super) show_url: bool,
    url_input: String,
    pub(super) history: History,
    last_hist_save: Instant,
    pub(super) chapters: Vec<(f64, String)>,
    pub(super) sleep: Option<Sleep>,
    pub(super) on_top: bool,
    aspect: &'static str,
    hflip: bool,
    vflip: bool,
    rng: u64,
}

impl Default for Extra {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        Self {
            show_playlist: false,
            show_info: false,
            show_keys: false,
            show_url: false,
            url_input: String::new(),
            history: History::load(),
            last_hist_save: Instant::now(),
            chapters: Vec::new(),
            sleep: None,
            on_top: false,
            aspect: "auto",
            hflip: false,
            vflip: false,
            rng: seed,
        }
    }
}

impl Extra {
    fn rand(&mut self, n: usize) -> usize {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x % n.max(1) as u64) as usize
    }
}

fn is_url(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.contains("://")
}

fn display_name(p: &Path) -> String {
    if is_url(p) {
        return p.to_string_lossy().to_string();
    }
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string())
}

fn media_in_dir(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && VIDEO_EXTS.contains(&ext_of(p).as_str())).collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|p| natural_key(&p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
    v
}

impl PlayerApp {
    pub(super) fn mp_f(&self, k: &str) -> f64 {
        self.mpv.as_ref().and_then(|m| m.get_f64(k)).unwrap_or(0.0)
    }
    pub(super) fn mp_s(&self, k: &str) -> String {
        self.mpv.as_ref().and_then(|m| m.get(k)).unwrap_or_default()
    }

    /// Apply persisted audio filters and play mode once mpv is up.
    pub(super) fn init_extra(&mut self) {
        self.apply_audio_filters();
        self.apply_loop_mode();
    }

    // ------------------------------------------------------------ open

    pub(super) fn open_many(&mut self, files: Vec<PathBuf>) {
        let files: Vec<PathBuf> =
            files.into_iter().flat_map(|f| if f.is_dir() { media_in_dir(&f) } else { vec![f] }).collect();
        if files.is_empty() {
            self.toast(self.t("这里没有可播放的文件", "No playable files here"));
            return;
        }
        if files.len() == 1 {
            let f = files.into_iter().next().unwrap();
            self.open(f);
            return;
        }
        self.playlist = files;
        self.index = 0;
        let p = self.playlist[0].clone();
        self.load(p);
    }

    pub(super) fn open_folder_dialog(&mut self) {
        if let Some(d) = rfd::FileDialog::new().pick_folder() {
            self.open_many(vec![d]);
        }
    }

    fn add_files_dialog(&mut self) {
        if let Some(files) = rfd::FileDialog::new().add_filter(self.t("媒体文件", "Media"), VIDEO_EXTS).pick_files()
        {
            let start_now = self.current.is_none();
            self.playlist.extend(files);
            if start_now && !self.playlist.is_empty() {
                self.index = 0;
                let p = self.playlist[0].clone();
                self.load(p);
            }
        }
    }

    fn open_url(&mut self, url: String) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        let p = PathBuf::from(url);
        self.playlist = vec![p.clone()];
        self.index = 0;
        self.load(p);
    }

    pub(super) fn play_index(&mut self, i: usize) {
        if i < self.playlist.len() {
            self.index = i;
            let p = self.playlist[i].clone();
            self.load(p);
        }
    }

    /// Next index for `delta` under the current play mode, or None at the ends.
    pub(super) fn next_index(&mut self, delta: i64) -> Option<usize> {
        let n = self.playlist.len();
        if n == 0 {
            return None;
        }
        match self.s.loop_mode {
            LoopMode::Shuffle if n > 1 => {
                let mut i = self.x.rand(n);
                if i == self.index {
                    i = (i + 1) % n;
                }
                Some(i)
            }
            LoopMode::All => Some(((self.index as i64 + delta).rem_euclid(n as i64)) as usize),
            _ => {
                let i = self.index as i64 + delta;
                (i >= 0 && i < n as i64).then_some(i as usize)
            }
        }
    }

    /// End of file reached (keep-open): advance according to the play mode.
    pub(super) fn on_eof(&mut self) {
        if let Some(p) = self.current.clone() {
            self.x.history.positions.remove(&p.to_string_lossy().to_string());
            self.x.history.save();
        }
        if self.x.sleep == Some(Sleep::EndOfFile) {
            self.x.sleep = None;
            self.toast(self.t("定时关闭：本集已播完", "Sleep timer: finished this file"));
            return;
        }
        if let Some(i) = self.next_index(1) {
            self.play_index(i);
        }
    }

    pub(super) fn apply_loop_mode(&self) {
        self.set("loop-file", if self.s.loop_mode == LoopMode::One { "inf" } else { "no" });
    }

    fn set_loop_mode(&mut self, m: LoopMode) {
        self.s.loop_mode = m;
        self.s.save();
        self.apply_loop_mode();
        self.toast(format!("{} {}", self.t("播放模式：", "Play mode:"), self.loop_label(m)));
    }

    fn loop_label(&self, m: LoopMode) -> &'static str {
        match m {
            LoopMode::Off => self.t("顺序播放", "Play in order"),
            LoopMode::One => self.t("单个循环", "Repeat one"),
            LoopMode::All => self.t("列表循环", "Repeat all"),
            LoopMode::Shuffle => self.t("随机播放", "Shuffle"),
        }
    }

    pub(super) fn loop_icon(&self) -> &'static str {
        match self.s.loop_mode {
            LoopMode::Off => ic::LIST_ORDERED,
            LoopMode::One => ic::REPEAT_ONE,
            LoopMode::All => ic::REPEAT,
            LoopMode::Shuffle => ic::SHUFFLE,
        }
    }

    pub(super) fn cycle_loop_mode(&mut self) {
        let next = match self.s.loop_mode {
            LoopMode::Off => LoopMode::All,
            LoopMode::All => LoopMode::One,
            LoopMode::One => LoopMode::Shuffle,
            LoopMode::Shuffle => LoopMode::Off,
        };
        self.set_loop_mode(next);
    }

    // ------------------------------------------------------------ resume / recent

    /// Called when a new file finished loading.
    pub(super) fn on_loaded_extra(&mut self) {
        self.refresh_chapters();
        let Some(p) = self.current.clone() else {
            return;
        };
        let key = p.to_string_lossy().to_string();
        let h = &mut self.x.history;
        h.recent.retain(|r| r != &key);
        h.recent.insert(0, key.clone());
        h.recent.truncate(HISTORY_MAX_RECENT);
        h.save();
        if !self.s.resume {
            return;
        }
        if let Some(&pos) = self.x.history.positions.get(&key) {
            let dur = self.mp_f("duration");
            if pos > 5.0 && (dur <= 0.0 || pos < dur - 15.0) {
                self.seek_abs(pos, true);
                self.toast(format!(
                    "{} {} · {}",
                    self.t("从上次位置继续：", "Resumed at"),
                    fmt_time(pos),
                    self.t("按 Home 从头播放", "Home to restart")
                ));
            }
        }
    }

    pub(super) fn save_position(&mut self) {
        let Some(p) = self.current.clone() else {
            return;
        };
        if self.st.idle || self.casting() && self.st.time <= 0.0 {
            return;
        }
        let key = p.to_string_lossy().to_string();
        let (t, d) = (self.st.time, self.st.duration);
        if t > 5.0 && (d <= 0.0 || t < d - 15.0) {
            self.x.history.positions.insert(key, t);
        } else {
            self.x.history.positions.remove(&key);
        }
        if self.x.history.positions.len() > HISTORY_MAX_POSITIONS {
            let drop: Vec<String> = self.x.history.positions.keys().take(50).cloned().collect();
            for k in drop {
                self.x.history.positions.remove(&k);
            }
        }
        self.x.history.save();
    }

    /// Periodic work: position autosave and the sleep timer.
    pub(super) fn tick_extra(&mut self) {
        if self.x.last_hist_save.elapsed() > Duration::from_secs(5) {
            self.x.last_hist_save = Instant::now();
            if !self.st.paused && self.s.resume {
                self.save_position();
            }
        }
        if let Some(Sleep::At(at)) = self.x.sleep {
            if Instant::now() >= at {
                self.x.sleep = None;
                if self.casting() {
                    self.caster.pause();
                } else {
                    self.set("pause", "yes");
                }
                self.toast(self.t("定时关闭：已暂停播放", "Sleep timer: paused"));
            }
        }
    }

    fn recent_items(&mut self, ui: &mut egui::Ui) {
        let recent = self.x.history.recent.clone();
        if recent.is_empty() {
            ui.label(RichText::new(self.t("（暂无）", "(none yet)")).color(MUTED));
        }
        for r in recent.iter().take(12) {
            let p = PathBuf::from(r);
            if ui.button(display_name(&p)).on_hover_text(r).clicked() {
                ui.close_menu();
                if is_url(&p) {
                    self.open_url(r.clone());
                } else if p.exists() {
                    self.open(p);
                } else {
                    self.toast(self.t("文件已不存在", "File no longer exists"));
                }
            }
        }
        if !recent.is_empty() {
            ui.separator();
            if ui.button(menu_label(ic::TRASH, self.t("清空记录", "Clear history"))).clicked() {
                ui.close_menu();
                self.x.history = History::default();
                self.x.history.save();
            }
        }
    }

    /// Recent files under the empty-state button.
    pub(super) fn recent_on_empty(&mut self, ui: &mut egui::Ui, top_center: Pos2) {
        let recent: Vec<String> = self.x.history.recent.iter().take(5).cloned().collect();
        if recent.is_empty() {
            return;
        }
        ui.painter().text(
            top_center,
            Align2::CENTER_CENTER,
            self.t("最近播放", "Recent"),
            FontId::proportional(12.5),
            MUTED,
        );
        for (i, r) in recent.iter().enumerate() {
            let p = PathBuf::from(r);
            let rect = Rect::from_center_size(top_center + vec2(0.0, 26.0 + i as f32 * 28.0), vec2(420.0, 24.0));
            let pos = self.x.history.positions.get(r).copied();
            let mut label = display_name(&p);
            if let Some(t) = pos {
                label = format!("{label}   ·  {}", fmt_time(t));
            }
            let resp = ui.put(
                rect,
                egui::Button::new(RichText::new(format!("{}  {label}", ic::HISTORY)).size(13.0).color(TEXT))
                    .frame(false),
            );
            if resp.on_hover_text(r).clicked() {
                if is_url(&p) {
                    self.open_url(r.clone());
                } else if p.exists() {
                    self.open(p);
                } else {
                    self.toast(self.t("文件已不存在", "File no longer exists"));
                }
            }
        }
    }

    // ------------------------------------------------------------ chapters / A-B

    pub(super) fn refresh_chapters(&mut self) {
        let json = self.mp_s("chapter-list");
        let list: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap_or_default();
        self.x.chapters = list
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let title = c["title"].as_str().unwrap_or("").to_string();
                let title = if title.is_empty() { format!("{} {}", self.t("章节", "Chapter"), i + 1) } else { title };
                (c["time"].as_f64().unwrap_or(0.0), title)
            })
            .collect();
    }

    fn chapter_items(&mut self, ui: &mut egui::Ui) {
        let cur = self.mp_s("chapter").parse::<i64>().unwrap_or(-1);
        let chapters = self.x.chapters.clone();
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for (i, (t, title)) in chapters.iter().enumerate() {
                if ui.selectable_label(cur == i as i64, format!("{}   {title}", fmt_time(*t))).clicked() {
                    ui.close_menu();
                    self.seek_abs(*t, true);
                }
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(self.t("上一章", "Previous")).clicked() {
                self.cmd(&["add", "chapter", "-1"]);
            }
            if ui.button(self.t("下一章", "Next")).clicked() {
                self.cmd(&["add", "chapter", "1"]);
            }
        });
    }

    /// Chapter title at `t`, for the seek-bar tooltip.
    pub(super) fn chapter_at(&self, t: f64) -> Option<&str> {
        self.x.chapters.iter().rev().find(|c| c.0 <= t).map(|c| c.1.as_str())
    }

    pub(super) fn ab_points(&self) -> (Option<f64>, Option<f64>) {
        let a = self.mp_s("ab-loop-a").parse::<f64>().ok();
        let b = self.mp_s("ab-loop-b").parse::<f64>().ok();
        (a, b)
    }

    pub(super) fn ab_loop(&mut self) {
        if self.casting() {
            return;
        }
        self.cmd(&["ab-loop"]);
        let msg = match self.ab_points() {
            (Some(a), None) => format!("{} {}", self.t("A-B 循环：起点", "A-B loop: A ="), fmt_time(a)),
            (Some(a), Some(b)) => format!("{} {} → {}", self.t("A-B 循环：", "A-B loop:"), fmt_time(a), fmt_time(b)),
            _ => self.t("A-B 循环已取消", "A-B loop cleared").to_string(),
        };
        self.toast(msg);
    }

    /// Chapter ticks and the A-B range on the seek bar.
    pub(super) fn seek_marks(&self, p: &egui::Painter, track: Rect, dur: f64) {
        if dur <= 0.0 {
            return;
        }
        let x_of = |t: f64| track.left() + track.width() * (t / dur).clamp(0.0, 1.0) as f32;
        let (a, b) = self.ab_points();
        if let Some(a) = a {
            let b = b.unwrap_or(a);
            let r = Rect::from_min_max(
                pos2(x_of(a), track.top() - 2.0),
                pos2(x_of(b).max(x_of(a) + 2.0), track.bottom() + 2.0),
            );
            p.rect_filled(r, CornerRadius::same(2), Color32::from_rgb(255, 196, 80).linear_multiply(0.6));
        }
        for (t, _) in &self.x.chapters {
            if *t <= 0.5 {
                continue;
            }
            let x = x_of(*t);
            p.rect_filled(
                Rect::from_center_size(pos2(x, track.center().y), vec2(2.0, track.height() + 2.0)),
                CornerRadius::ZERO,
                Color32::from_black_alpha(160),
            );
        }
    }

    // ------------------------------------------------------------ picture

    fn video_items(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(290.0);
        for (k, zh, en) in [
            ("brightness", "亮度", "Brightness"),
            ("contrast", "对比度", "Contrast"),
            ("saturation", "饱和度", "Saturation"),
            ("gamma", "伽马", "Gamma"),
            ("hue", "色调", "Hue"),
        ] {
            let mut v = self.mp_f(k);
            let label = self.t(zh, en);
            if ui.add(egui::Slider::new(&mut v, -100.0..=100.0).step_by(1.0).text(label)).changed() {
                self.set(k, &format!("{v:.0}"));
            }
        }
        if ui.button(menu_label(ic::UNDO, self.t("重置画面", "Reset picture"))).clicked() {
            for k in ["brightness", "contrast", "saturation", "gamma", "hue"] {
                self.set(k, "0");
            }
        }
        ui.separator();
        ui.label(RichText::new(self.t("画面比例", "Aspect ratio")).color(MUTED).size(12.5));
        ui.horizontal_wrapped(|ui| {
            let opts: [(&'static str, &str); 7] = [
                ("auto", self.t("原始", "Original")),
                ("16:9", "16:9"),
                ("4:3", "4:3"),
                ("2.35:1", "21:9"),
                ("1:1", "1:1"),
                ("fill", self.t("铺满裁剪", "Fill (crop)")),
                ("stretch", self.t("拉伸", "Stretch")),
            ];
            for (v, l) in opts {
                if ui.selectable_label(self.x.aspect == v, l).clicked() {
                    self.x.aspect = v;
                    self.apply_aspect();
                }
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(menu_label(ic::ROTATE_CW, self.t("旋转 90°", "Rotate 90°"))).on_hover_text("R").clicked() {
                self.rotate();
            }
            if ui.selectable_label(self.x.hflip, menu_label(ic::FLIP_H, self.t("水平翻转", "Flip H"))).clicked() {
                self.x.hflip = !self.x.hflip;
                self.apply_flip();
            }
            if ui.selectable_label(self.x.vflip, menu_label(ic::FLIP_V, self.t("垂直翻转", "Flip V"))).clicked() {
                self.x.vflip = !self.x.vflip;
                self.apply_flip();
            }
        });
        let mut z = self.mp_f("video-zoom");
        let label = self.t("缩放（Ctrl+滚轮）", "Zoom (Ctrl+wheel)");
        if ui.add(egui::Slider::new(&mut z, -1.0..=2.0).step_by(0.05).text(label)).changed() {
            self.set("video-zoom", &format!("{z:.2}"));
        }
        let mut deint = self.mp_s("deinterlace") == "yes";
        let label = self.t("去隔行扫描", "Deinterlace");
        if ui.checkbox(&mut deint, label).changed() {
            self.set("deinterlace", if deint { "yes" } else { "no" });
        }
    }

    fn apply_aspect(&self) {
        let a = self.x.aspect;
        let ratio = if matches!(a, "auto" | "fill" | "stretch") { "-1" } else { a };
        self.set("video-aspect-override", ratio);
        self.set("panscan", if a == "fill" { "1.0" } else { "0.0" });
        self.set("keepaspect", if a == "stretch" { "no" } else { "yes" });
    }

    pub(super) fn rotate(&mut self) {
        let r = (self.mp_f("video-rotate") as i64 + 90) % 360;
        self.set("video-rotate", &r.to_string());
        self.toast(format!("{} {r}°", self.t("旋转", "Rotation")));
    }

    fn apply_flip(&self) {
        self.cmd(&["vf", "remove", "@hflip"]);
        self.cmd(&["vf", "remove", "@vflip"]);
        if self.x.hflip {
            self.cmd(&["vf", "add", "@hflip:hflip"]);
        }
        if self.x.vflip {
            self.cmd(&["vf", "add", "@vflip:vflip"]);
        }
    }

    pub(super) fn zoom_by(&mut self, d: f64) {
        let z = (self.mp_f("video-zoom") + d).clamp(-1.0, 2.0);
        self.set("video-zoom", &format!("{z:.2}"));
    }

    // ------------------------------------------------------------ audio

    fn audio_items(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(280.0);
        self.track_items(ui, "audio");
        ui.separator();
        self.delay_row(ui, "audio-delay", self.t("音频延迟", "Audio delay"), "Ctrl+ -/+");
        ui.separator();
        ui.label(RichText::new(self.t("均衡器", "Equalizer")).color(MUTED).size(12.5));
        ui.horizontal_wrapped(|ui| {
            let opts: [(&str, &str); 5] = [
                ("off", self.t("关闭", "Off")),
                ("bass", self.t("低音增强", "Bass boost")),
                ("treble", self.t("高音增强", "Treble boost")),
                ("vocal", self.t("人声", "Vocal")),
                ("night", self.t("夜间（压缩动态）", "Night")),
            ];
            for (v, l) in opts {
                if ui.selectable_label(self.s.audio_eq == v, l).clicked() {
                    self.s.audio_eq = v.into();
                    self.s.save();
                    self.apply_audio_filters();
                }
            }
        });
        let label = self.t("音量均衡（响度自动调整）", "Normalize loudness");
        if ui.checkbox(&mut self.s.audio_norm, label).changed() {
            self.s.save();
            self.apply_audio_filters();
        }
        let mut stereo = self.mp_s("audio-channels") == "stereo";
        let label = self.t("强制立体声输出", "Force stereo output");
        if ui.checkbox(&mut stereo, label).changed() {
            self.set("audio-channels", if stereo { "stereo" } else { "auto-safe" });
        }
        ui.separator();
        ui.menu_button(menu_label(ic::VOLUME, self.t("输出设备", "Output device")), |ui| {
            let cur = self.mp_s("audio-device");
            let list: Vec<serde_json::Value> =
                serde_json::from_str(&self.mp_s("audio-device-list")).unwrap_or_default();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for d in list {
                    let name = d["name"].as_str().unwrap_or("").to_string();
                    let desc = d["description"].as_str().unwrap_or(&name).to_string();
                    let desc = if name == "auto" {
                        self.t("自动（系统默认）", "Auto (system default)").to_string()
                    } else {
                        desc
                    };
                    if ui.selectable_label(cur == name, desc).clicked() {
                        self.set("audio-device", &name);
                        ui.close_menu();
                    }
                }
            });
        });
    }

    pub(super) fn apply_audio_filters(&self) {
        self.cmd(&["af", "remove", "@eq"]);
        self.cmd(&["af", "remove", "@norm"]);
        let eq = match self.s.audio_eq.as_str() {
            "bass" => Some("bass=g=7:f=110"),
            "treble" => Some("treble=g=6:f=4000"),
            "vocal" => Some("equalizer=f=250:t=q:w=1:g=-3,equalizer=f=2500:t=q:w=1.2:g=4"),
            "night" => Some("acompressor=threshold=0.08:ratio=6:attack=10:release=200:makeup=3"),
            _ => None,
        };
        if let Some(f) = eq {
            self.cmd(&["af", "add", &format!("@eq:lavfi=[{f}]")]);
        }
        if self.s.audio_norm {
            self.cmd(&["af", "add", "@norm:lavfi=[dynaudnorm=f=250:g=15:p=0.9]"]);
        }
    }

    fn delay_row(&mut self, ui: &mut egui::Ui, prop: &str, label: &str, keys: &str) {
        let d = self.mp_f(prop);
        ui.horizontal(|ui| {
            ui.label(label);
            if ui.small_button("−0.1s").clicked() {
                self.cmd(&["add", prop, "-0.1"]);
            }
            ui.label(RichText::new(format!("{:+.1}s", d)).color(if d.abs() > 0.01 { ACCENT } else { TEXT }));
            if ui.small_button("+0.1s").clicked() {
                self.cmd(&["add", prop, "0.1"]);
            }
            if ui.small_button(self.t("重置", "Reset")).clicked() {
                self.set(prop, "0");
            }
            ui.label(RichText::new(keys).size(11.5).color(MUTED));
        });
    }

    // ------------------------------------------------------------ subtitles

    fn subtitle_items(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(300.0);
        self.track_items(ui, "sub");
        ui.separator();
        self.delay_row(ui, "sub-delay", self.t("字幕延迟", "Subtitle delay"), "Z / X");
        let mut scale = self.mp_f("sub-scale");
        let label = self.t("字幕大小", "Size");
        if ui.add(egui::Slider::new(&mut scale, 0.4..=3.0).step_by(0.05).text(label)).changed() {
            self.set("sub-scale", &format!("{scale:.2}"));
        }
        let mut pos = self.mp_f("sub-pos");
        let label = self.t("字幕位置（越大越靠下）", "Position (higher = lower)");
        if ui.add(egui::Slider::new(&mut pos, 0.0..=110.0).step_by(1.0).text(label)).changed() {
            self.set("sub-pos", &format!("{pos:.0}"));
        }
        let mut vis = self.mp_s("sub-visibility") != "no";
        let label = self.t("显示字幕 (V)", "Show subtitles (V)");
        if ui.checkbox(&mut vis, label).changed() {
            self.set("sub-visibility", if vis { "yes" } else { "no" });
        }
        let list: Vec<Track> = self.tracks.iter().filter(|t| t.kind == "sub").cloned().collect();
        if list.len() > 1 {
            ui.menu_button(
                menu_label(ic::CAPTIONS, self.t("第二字幕（双字幕）", "Secondary subtitle")),
                |ui| {
                    let cur = self.mp_s("secondary-sid");
                    if ui.selectable_label(cur == "no" || cur.is_empty(), self.t("关闭", "Off")).clicked() {
                        self.set("secondary-sid", "no");
                        ui.close_menu();
                    }
                    for t in list {
                        if ui.selectable_label(cur == t.id.to_string(), t.label()).clicked() {
                            self.set("secondary-sid", &t.id.to_string());
                            ui.close_menu();
                        }
                    }
                },
            );
        }
        if self.s.live_enabled {
            ui.separator();
            let label = self.t("实时字幕字号", "Live subtitle size");
            if ui.add(egui::Slider::new(&mut self.s.sub_font_size, 16.0..=56.0).step_by(1.0).text(label)).changed() {
                self.s.save();
            }
        }
    }

    // ------------------------------------------------------------ window / sleep

    pub(super) fn toggle_on_top(&mut self, ctx: &egui::Context) {
        self.x.on_top = !self.x.on_top;
        let lvl = if self.x.on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(lvl));
        self.toast(if self.x.on_top {
            self.t("窗口置顶：开", "Always on top: on")
        } else {
            self.t("窗口置顶：关", "Always on top: off")
        });
    }

    fn window_items(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let label = menu_label(ic::PIN, self.t("窗口置顶", "Always on top"));
        if ui.add(egui::Button::new(label).selected(self.x.on_top).shortcut_text("T")).clicked() {
            ui.close_menu();
            self.toggle_on_top(ctx);
        }
        ui.separator();
        let (w, h) = (self.mp_f("dwidth"), self.mp_f("dheight"));
        for (f, l) in [(0.5, "50%"), (1.0, "100%"), (1.5, "150%"), (2.0, "200%")] {
            if ui
                .add_enabled(w > 0.0, egui::Button::new(format!("{}  {l}", self.t("视频原始尺寸", "Video size"))))
                .clicked()
            {
                ui.close_menu();
                let ppp = ctx.pixels_per_point();
                let title = if cfg!(target_os = "macos") { 42.0 } else { 42.0 };
                let size = vec2((w * f) as f32 / ppp, (h * f) as f32 / ppp + title);
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size.max(vec2(480.0, 270.0))));
            }
        }
    }

    fn sleep_items(&mut self, ui: &mut egui::Ui) {
        let left = match self.x.sleep {
            Some(Sleep::At(at)) => Some(at.saturating_duration_since(Instant::now()).as_secs()),
            _ => None,
        };
        if ui.selectable_label(self.x.sleep.is_none(), self.t("关闭", "Off")).clicked() {
            self.x.sleep = None;
            ui.close_menu();
        }
        for m in [15u64, 30, 45, 60, 90, 120] {
            if ui.button(format!("{m} {}", self.t("分钟后暂停", "min, then pause"))).clicked() {
                self.x.sleep = Some(Sleep::At(Instant::now() + Duration::from_secs(m * 60)));
                ui.close_menu();
                self.toast(format!("{} {m} {}", self.t("将在", "Pausing in"), self.t("分钟后暂停", "min")));
            }
        }
        if ui
            .selectable_label(self.x.sleep == Some(Sleep::EndOfFile), self.t("播完当前后停止", "Stop after this file"))
            .clicked()
        {
            self.x.sleep = Some(Sleep::EndOfFile);
            ui.close_menu();
        }
        if let Some(s) = left {
            ui.label(
                RichText::new(format!("{} {}", self.t("剩余", "Remaining"), fmt_time(s as f64)))
                    .color(ACCENT)
                    .size(12.5),
            );
        }
    }

    fn play_mode_items(&mut self, ui: &mut egui::Ui) {
        for m in [LoopMode::Off, LoopMode::All, LoopMode::One, LoopMode::Shuffle] {
            let g = match m {
                LoopMode::Off => ic::LIST_ORDERED,
                LoopMode::One => ic::REPEAT_ONE,
                LoopMode::All => ic::REPEAT,
                LoopMode::Shuffle => ic::SHUFFLE,
            };
            if ui.selectable_label(self.s.loop_mode == m, menu_label(g, self.loop_label(m))).clicked() {
                ui.close_menu();
                self.set_loop_mode(m);
            }
        }
        ui.separator();
        if ui
            .add(egui::Button::new(menu_label(ic::REPEAT, self.t("A-B 循环", "A-B loop"))).shortcut_text("L"))
            .clicked()
        {
            ui.close_menu();
            self.ab_loop();
        }
    }

    /// Extra entries for the gear / right-click menu.
    pub(super) fn extra_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.menu_button(menu_label(ic::SUN, self.t("画面", "Picture")), |ui| self.video_items(ui));
        if !self.x.chapters.is_empty() {
            ui.menu_button(menu_label(ic::BOOKMARK, self.t("章节", "Chapters")), |ui| self.chapter_items(ui));
        }
        ui.menu_button(menu_label(self.loop_icon(), self.t("播放模式", "Play mode")), |ui| {
            self.play_mode_items(ui)
        });
        let sleep_label = if self.x.sleep.is_some() {
            format!("{}  ●", self.t("定时关闭", "Sleep timer"))
        } else {
            self.t("定时关闭", "Sleep timer").to_string()
        };
        ui.menu_button(menu_label(ic::TIMER, &sleep_label), |ui| self.sleep_items(ui));
        ui.menu_button(menu_label(ic::PIN, self.t("窗口", "Window")), |ui| self.window_items(ui, ctx));
    }

    pub(super) fn subtitle_menu(&mut self, ui: &mut egui::Ui) {
        self.subtitle_items(ui);
    }

    pub(super) fn audio_menu(&mut self, ui: &mut egui::Ui) {
        self.audio_items(ui);
    }

    pub(super) fn open_menu_items(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let sc = |m: Modifiers, k: Key| ctx.format_shortcut(&egui::KeyboardShortcut::new(m, k));
        if ui
            .add(
                egui::Button::new(menu_label(ic::FOLDER_OPEN, self.t("打开文件…", "Open file…")))
                    .shortcut_text(sc(Modifiers::COMMAND, Key::O)),
            )
            .clicked()
        {
            ui.close_menu();
            self.open_dialog();
        }
        if ui
            .add(
                egui::Button::new(menu_label(ic::FOLDER_PLUS, self.t("打开文件夹…", "Open folder…")))
                    .shortcut_text(sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::O)),
            )
            .clicked()
        {
            ui.close_menu();
            self.open_folder_dialog();
        }
        if ui
            .add(
                egui::Button::new(menu_label(ic::LINK, self.t("打开网络地址…", "Open URL…")))
                    .shortcut_text(sc(Modifiers::COMMAND, Key::U)),
            )
            .clicked()
        {
            ui.close_menu();
            self.x.show_url = true;
        }
        ui.menu_button(menu_label(ic::HISTORY, self.t("最近播放", "Recent")), |ui| self.recent_items(ui));
    }

    pub(super) fn info_menu_items(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(egui::Button::new(menu_label(ic::INFO, self.t("媒体信息", "Media info"))).shortcut_text("Tab"))
            .clicked()
        {
            ui.close_menu();
            self.x.show_info = true;
        }
        if ui
            .add(
                egui::Button::new(menu_label(ic::KEYBOARD, self.t("快捷键", "Keyboard shortcuts"))).shortcut_text("F1"),
            )
            .clicked()
        {
            ui.close_menu();
            self.x.show_keys = true;
        }
    }

    // ------------------------------------------------------------ keyboard

    /// Runs before the base key handler so modified keys win.
    pub(super) fn keyboard_extra(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let k = |m: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(m, key));
        let none = Modifiers::NONE;
        let ctrl = Modifiers::COMMAND;
        let shift = Modifiers::SHIFT;
        if k(ctrl | shift, Key::O) {
            self.open_folder_dialog();
        }
        if k(ctrl, Key::U) {
            self.x.show_url = true;
        }
        if k(shift, Key::ArrowLeft) {
            self.seek_rel(-1.0);
        }
        if k(shift, Key::ArrowRight) {
            self.seek_rel(1.0);
        }
        if k(ctrl, Key::ArrowLeft) {
            self.seek_rel(-30.0);
        }
        if k(ctrl, Key::ArrowRight) {
            self.seek_rel(30.0);
        }
        if k(ctrl, Key::Minus) {
            self.cmd(&["add", "audio-delay", "-0.1"]);
            self.toast(format!("{} {:+.1}s", self.t("音频延迟", "Audio delay"), self.mp_f("audio-delay")));
        }
        if k(ctrl, Key::Equals) || k(ctrl, Key::Plus) {
            self.cmd(&["add", "audio-delay", "0.1"]);
            self.toast(format!("{} {:+.1}s", self.t("音频延迟", "Audio delay"), self.mp_f("audio-delay")));
        }
        if k(none, Key::Period) && !self.casting() {
            self.cmd(&["frame-step"]);
        }
        if k(none, Key::Comma) && !self.casting() {
            self.cmd(&["frame-back-step"]);
        }
        if k(ctrl | shift, Key::E) {
            self.open_export(true);
        }
        if k(ctrl, Key::E) {
            self.open_export(false);
        }
        if k(none, Key::L) {
            self.ab_loop();
        }
        if k(none, Key::P) {
            self.x.show_playlist = !self.x.show_playlist;
        }
        if k(none, Key::R) {
            self.rotate();
        }
        if k(none, Key::T) {
            self.toggle_on_top(ctx);
        }
        if k(none, Key::V) {
            let vis = self.mp_s("sub-visibility") != "no";
            self.set("sub-visibility", if vis { "no" } else { "yes" });
            self.toast(if vis {
                self.t("字幕：隐藏", "Subtitles hidden")
            } else {
                self.t("字幕：显示", "Subtitles shown")
            });
        }
        if k(none, Key::Z) {
            self.cmd(&["add", "sub-delay", "-0.1"]);
            self.toast(format!("{} {:+.1}s", self.t("字幕延迟", "Subtitle delay"), self.mp_f("sub-delay")));
        }
        if k(none, Key::X) {
            self.cmd(&["add", "sub-delay", "0.1"]);
            self.toast(format!("{} {:+.1}s", self.t("字幕延迟", "Subtitle delay"), self.mp_f("sub-delay")));
        }
        if k(none, Key::Home) {
            self.seek_abs(0.0, true);
        }
        if k(none, Key::Backspace) {
            self.set("speed", "1");
        }
        if k(none, Key::Tab) {
            self.x.show_info = !self.x.show_info;
        }
        if k(none, Key::F1) {
            self.x.show_keys = !self.x.show_keys;
        }
        let digits = [
            Key::Num0,
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (i, d) in digits.iter().enumerate() {
            if k(none, *d) && self.st.duration > 0.0 {
                self.seek_abs(self.st.duration * i as f64 / 10.0, false);
            }
        }
    }

    // ------------------------------------------------------------ panels / windows

    pub(super) fn playlist_panel(&mut self, ctx: &egui::Context) {
        if !self.x.show_playlist {
            return;
        }
        egui::SidePanel::right("playlist")
            .resizable(true)
            .default_width(300.0)
            .min_width(220.0)
            .frame(Frame::NONE.fill(PANEL).inner_margin(Margin::same(10)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}  ({})", self.t("播放列表", "Playlist"), self.playlist.len()))
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, ic::X, 15.0, self.t("关闭 (P)", "Close (P)")).clicked() {
                            self.x.show_playlist = false;
                        }
                        if icon_button(ui, ic::TRASH, 15.0, self.t("清空列表", "Clear")).clicked() {
                            let keep = self.current.clone();
                            self.playlist = keep.into_iter().collect();
                            self.index = 0;
                        }
                        if icon_button(ui, ic::FOLDER_PLUS, 15.0, self.t("添加文件夹", "Add folder")).clicked() {
                            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                let start_now = self.current.is_none();
                                self.playlist.extend(media_in_dir(&d));
                                if start_now && !self.playlist.is_empty() {
                                    self.play_index(0);
                                }
                            }
                        }
                        if icon_button(ui, ic::FILE_PLUS, 15.0, self.t("添加文件", "Add files")).clicked() {
                            self.add_files_dialog();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    for m in [LoopMode::Off, LoopMode::All, LoopMode::One, LoopMode::Shuffle] {
                        let g = match m {
                            LoopMode::Off => ic::LIST_ORDERED,
                            LoopMode::One => ic::REPEAT_ONE,
                            LoopMode::All => ic::REPEAT,
                            LoopMode::Shuffle => ic::SHUFFLE,
                        };
                        let on = self.s.loop_mode == m;
                        let txt = icon(g, 15.0).color(if on { ACCENT } else { MUTED });
                        if ui.add(egui::Button::new(txt).frame(on)).on_hover_text(self.loop_label(m)).clicked() {
                            self.set_loop_mode(m);
                        }
                    }
                });
                ui.separator();
                let mut action: Option<(usize, &'static str)> = None;
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (i, p) in self.playlist.iter().enumerate() {
                        let cur = i == self.index && self.current.is_some();
                        let name = display_name(p);
                        let text = RichText::new(format!("{:>3}  {name}", i + 1)).size(13.5).color(if cur {
                            ACCENT
                        } else {
                            TEXT
                        });
                        let r = ui
                            .add(egui::Button::new(text).frame(false).wrap_mode(egui::TextWrapMode::Truncate))
                            .on_hover_text(p.to_string_lossy());
                        if r.double_clicked() || (r.clicked() && !cur) {
                            action = Some((i, "play"));
                        }
                        r.context_menu(|ui| {
                            if ui.button(self.t("播放", "Play")).clicked() {
                                action = Some((i, "play"));
                                ui.close_menu();
                            }
                            if ui.button(self.t("上移", "Move up")).clicked() {
                                action = Some((i, "up"));
                                ui.close_menu();
                            }
                            if ui.button(self.t("下移", "Move down")).clicked() {
                                action = Some((i, "down"));
                                ui.close_menu();
                            }
                            if ui.button(self.t("从列表移除", "Remove")).clicked() {
                                action = Some((i, "remove"));
                                ui.close_menu();
                            }
                            if !is_url(p) && ui.button(self.t("打开所在文件夹", "Show in folder")).clicked() {
                                if let Some(d) = p.parent() {
                                    open_path(d);
                                }
                                ui.close_menu();
                            }
                        });
                    }
                    if self.playlist.is_empty() {
                        ui.label(
                            RichText::new(self.t("拖入文件或点击右上角添加", "Drop files here or use the add buttons"))
                                .color(MUTED),
                        );
                    }
                });
                if let Some((i, what)) = action {
                    match what {
                        "play" => self.play_index(i),
                        "up" if i > 0 => {
                            self.playlist.swap(i, i - 1);
                            if self.index == i {
                                self.index -= 1;
                            } else if self.index == i - 1 {
                                self.index += 1;
                            }
                        }
                        "down" if i + 1 < self.playlist.len() => {
                            self.playlist.swap(i, i + 1);
                            if self.index == i {
                                self.index += 1;
                            } else if self.index == i + 1 {
                                self.index -= 1;
                            }
                        }
                        "remove" => {
                            self.playlist.remove(i);
                            if i < self.index {
                                self.index -= 1;
                            }
                            self.index = self.index.min(self.playlist.len().saturating_sub(1));
                        }
                        _ => {}
                    }
                }
            });
    }

    pub(super) fn extra_windows(&mut self, ctx: &egui::Context) {
        if self.x.show_url {
            let mut open = true;
            let mut go = false;
            egui::Window::new(self.t("打开网络地址", "Open URL"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(self.t(
                            "支持 http(s)、HLS (m3u8)、RTMP、RTSP、SMB 等地址",
                            "http(s), HLS (m3u8), RTMP, RTSP, SMB, ...",
                        ))
                        .color(MUTED)
                        .size(12.5),
                    );
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.x.url_input)
                            .desired_width(460.0)
                            .hint_text("https://example.com/video.m3u8"),
                    );
                    r.request_focus();
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        go = true;
                    }
                    ui.horizontal(|ui| {
                        if ui.button(self.t("播放", "Play")).clicked() {
                            go = true;
                        }
                        if ui.button(self.t("从剪贴板粘贴", "Paste")).clicked() {
                            if let Ok(t) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                                self.x.url_input = t.trim().to_string();
                            }
                        }
                    });
                });
            if go {
                let u = self.x.url_input.clone();
                self.open_url(u);
                open = false;
            }
            self.x.show_url = open;
        }

        if self.x.show_info {
            let mut open = true;
            let text = self.media_info_text();
            egui::Window::new(self.t("媒体信息", "Media info"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size([560.0, 420.0])
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    if ui.button(menu_label(ic::COPY, self.t("复制全部", "Copy all"))).clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(&text).monospace().size(12.5)).selectable(true));
                    });
                });
            self.x.show_info = open;
        }

        if self.x.show_keys {
            let mut open = true;
            let rows: Vec<(&str, &str)> = vec![
                ("Space / 单击", self.t("播放 / 暂停", "Play / pause")),
                ("← / →", self.t("后退 / 前进 5 秒", "Seek 5 s")),
                ("Shift + ← / →", self.t("后退 / 前进 1 秒", "Seek 1 s")),
                ("Ctrl + ← / →", self.t("后退 / 前进 30 秒", "Seek 30 s")),
                ("0 – 9", self.t("跳到 0% – 90%", "Jump to 0–90%")),
                ("Home", self.t("从头播放", "Restart")),
                (", / .", self.t("上一帧 / 下一帧", "Frame back / forward")),
                ("↑ / ↓ / 滚轮", self.t("音量", "Volume")),
                ("M", self.t("静音", "Mute")),
                ("[ / ] / Backspace", self.t("减速 / 加速 / 恢复 1.0x", "Slower / faster / reset")),
                ("PgUp / PgDn", self.t("上一个 / 下一个", "Previous / next")),
                ("L", self.t("A-B 循环", "A-B loop")),
                ("P", self.t("播放列表", "Playlist")),
                ("F / Enter / 双击", self.t("全屏", "Full screen")),
                ("T", self.t("窗口置顶", "Always on top")),
                ("R", self.t("旋转画面", "Rotate")),
                ("Ctrl + 滚轮", self.t("缩放画面", "Zoom")),
                ("V", self.t("显示 / 隐藏字幕", "Toggle subtitles")),
                ("Z / X", self.t("字幕延迟 −/+ 0.1 秒", "Subtitle delay ∓0.1 s")),
                ("Ctrl + − / +", self.t("音频延迟 −/+ 0.1 秒", "Audio delay ∓0.1 s")),
                ("C", self.t("实时识别翻译字幕", "Live subtitles")),
                ("S / Shift+S", self.t("截图 / 另一种字幕模式截图", "Snapshot / other subtitle mode")),
                ("Ctrl + C", self.t("复制当前画面", "Copy frame")),
                (
                    "Ctrl + E / Ctrl+Shift+E",
                    self.t("导出片段 / GIF（默认用 A-B 区间）", "Export clip / GIF (uses A-B range)"),
                ),
                ("Ctrl + O", self.t("打开文件", "Open file")),
                ("Ctrl + Shift + O", self.t("打开文件夹", "Open folder")),
                ("Ctrl + U", self.t("打开网络地址", "Open URL")),
                ("Tab", self.t("媒体信息", "Media info")),
                ("Ctrl + I", self.t("播放统计", "Playback stats")),
                ("F1", self.t("快捷键", "Shortcuts")),
                ("Esc", self.t("退出全屏 / 画中画", "Leave full screen / PiP")),
            ];
            egui::Window::new(self.t("快捷键", "Keyboard shortcuts"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size([460.0, 520.0])
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        egui::Grid::new("keys").num_columns(2).spacing([24.0, 8.0]).striped(true).show(ui, |ui| {
                            for (k, v) in rows {
                                ui.label(RichText::new(k).monospace().color(ACCENT));
                                ui.label(v);
                                ui.end_row();
                            }
                        });
                    });
                });
            self.x.show_keys = open;
        }
    }

    fn media_info_text(&self) -> String {
        let Some(m) = &self.mpv else {
            return String::new();
        };
        if self.st.idle {
            return self.t("没有正在播放的文件", "Nothing is playing").to_string();
        }
        let g = |k: &str| m.get(k).filter(|v| !v.is_empty()).unwrap_or_else(|| "-".into());
        let path = m.get("path").unwrap_or_default();
        let size = std::fs::metadata(&path)
            .map(|md| format!("{:.2} MB", md.len() as f64 / 1_048_576.0))
            .unwrap_or_else(|_| g("file-size"));
        let mut out = vec![
            format!("{}: {}", self.t("文件", "File"), g("filename")),
            format!("{}: {path}", self.t("路径", "Path")),
            format!("{}: {size}", self.t("大小", "Size")),
            format!("{}: {}", self.t("容器", "Container"), g("file-format")),
            format!("{}: {}", self.t("时长", "Duration"), fmt_time(self.st.duration)),
            format!("{}: {}", self.t("标题", "Title"), g("media-title")),
            String::new(),
        ];
        let list: Vec<serde_json::Value> =
            serde_json::from_str(&m.get("track-list").unwrap_or_default()).unwrap_or_default();
        for t in list {
            let kind = t["type"].as_str().unwrap_or("");
            let head = match kind {
                "video" => self.t("视频", "Video"),
                "audio" => self.t("音频", "Audio"),
                "sub" => self.t("字幕", "Subtitle"),
                _ => kind,
            };
            let mut parts = vec![format!("#{}", t["id"].as_i64().unwrap_or(0))];
            let codec = t["codec-desc"].as_str().or(t["codec"].as_str()).unwrap_or("");
            if !codec.is_empty() {
                parts.push(codec.to_string());
            }
            match kind {
                "video" => {
                    if let (Some(w), Some(h)) = (t["demux-w"].as_i64(), t["demux-h"].as_i64()) {
                        parts.push(format!("{w}x{h}"));
                    }
                    if let Some(f) = t["demux-fps"].as_f64() {
                        parts.push(format!("{f:.3} fps"));
                    }
                }
                "audio" => {
                    if let Some(sr) = t["demux-samplerate"].as_i64() {
                        parts.push(format!("{sr} Hz"));
                    }
                    if let Some(ch) = t["demux-channel-count"].as_i64() {
                        parts.push(format!("{ch} ch"));
                    }
                }
                _ => {}
            }
            if let Some(br) = t["demux-bitrate"].as_i64() {
                parts.push(format!("{} kb/s", br / 1000));
            }
            for k in ["lang", "title"] {
                if let Some(v) = t[k].as_str().filter(|v| !v.is_empty()) {
                    parts.push(v.to_string());
                }
            }
            if t["default"].as_bool() == Some(true) {
                parts.push("default".into());
            }
            if t["external"].as_bool() == Some(true) {
                parts.push("external".into());
            }
            out.push(format!("{head:<6} {}", parts.join("  ·  ")));
        }
        out.push(String::new());
        out.push(format!("{}: {}", self.t("硬件解码", "HW decode"), g("hwdec-current")));
        out.push(format!(
            "{}: {}",
            self.t("色彩", "Color"),
            format!(
                "{} / {} / {}",
                g("video-params/colormatrix"),
                g("video-params/primaries"),
                g("video-params/gamma")
            )
        ));
        if !self.x.chapters.is_empty() {
            out.push(String::new());
            out.push(format!("{} ({})", self.t("章节", "Chapters"), self.x.chapters.len()));
            for (t, title) in &self.x.chapters {
                out.push(format!("  {}  {title}", fmt_time(*t)));
            }
        }
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(is_url(Path::new("https://a.b/c.m3u8")));
        assert!(!is_url(Path::new("/home/x/movie.mkv")));
        assert_eq!(display_name(Path::new("/home/x/movie.mkv")), "movie.mkv");
    }

    #[test]
    fn rng_in_range() {
        let mut x = Extra::default();
        for _ in 0..100 {
            assert!(x.rand(7) < 7);
        }
    }
}
