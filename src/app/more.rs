//! Bookmarks, per-folder intro / outro skipping for series, and subtitle
//! styling (font, colours, outline, background box).

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Bookmark {
    pub(super) t: f64,
    pub(super) label: String,
}

/// Intro / outro lengths shared by every file in a folder (one season).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Skip {
    /// Seconds from the start at which the intro ends.
    pub(super) intro: Option<f64>,
    /// Seconds before the end at which the outro starts.
    pub(super) outro: Option<f64>,
    pub(super) enabled: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Marks {
    bookmarks: HashMap<String, Vec<Bookmark>>,
    skips: HashMap<String, Skip>,
}

impl Marks {
    fn path() -> PathBuf {
        settings::data_dir().join("marks.json")
    }
    fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn save(&self) {
        if let Ok(b) = serde_json::to_vec(self) {
            let _ = std::fs::write(Self::path(), b);
        }
    }
}

pub(super) struct More {
    marks: Marks,
    pub(super) show_style: bool,
    /// The intro check is still due for the file that just loaded.
    intro_pending: bool,
    /// The outro already fired for this file.
    outro_done: bool,
}

impl Default for More {
    fn default() -> Self {
        Self { marks: Marks::load(), show_style: false, intro_pending: false, outro_done: false }
    }
}

fn folder_key(p: &Path) -> Option<String> {
    if p.to_string_lossy().contains("://") {
        return None;
    }
    p.parent().map(|d| d.to_string_lossy().to_string())
}

/// Where to jump when a file starts at `start_at`, if the intro should be skipped.
pub(super) fn intro_target(skip: &Skip, start_at: f64, dur: f64) -> Option<f64> {
    if !skip.enabled {
        return None;
    }
    let end = skip.intro.filter(|e| *e > 1.0)?;
    (start_at < end - 1.0 && (dur <= 0.0 || end < dur - 5.0)).then_some(end)
}

/// True once `t` is inside the outro.
pub(super) fn in_outro(skip: &Skip, t: f64, dur: f64) -> bool {
    skip.enabled && dur > 0.0 && t > 5.0 && skip.outro.is_some_and(|o| o > 1.0 && o < dur - 5.0 && t >= dur - o)
}

/// Insert keeping the list sorted; a mark within a second of another replaces it.
pub(super) fn insert_bookmark(list: &mut Vec<Bookmark>, b: Bookmark) {
    list.retain(|x| (x.t - b.t).abs() >= 1.0);
    let i = list.iter().position(|x| x.t > b.t).unwrap_or(list.len());
    list.insert(i, b);
}

/// mpv colour string `#AARRGGBB` from straight RGBA.
pub(super) fn mpv_color(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c[3], c[0], c[1], c[2])
}

fn font_presets() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["Microsoft YaHei", "SimHei", "KaiTi", "Segoe UI", "Arial"]
    } else if cfg!(target_os = "macos") {
        &["PingFang SC", "Hiragino Sans GB", "Songti SC", "Helvetica Neue", "Arial"]
    } else {
        &["Noto Sans CJK SC", "WenQuanYi Micro Hei", "DejaVu Sans"]
    }
}

const BOOKMARK_COLOR: Color32 = Color32::from_rgb(110, 210, 255);

impl PlayerApp {
    fn cur_key(&self) -> Option<String> {
        self.current.as_ref().map(|p| p.to_string_lossy().to_string())
    }

    fn cur_folder(&self) -> Option<String> {
        self.current.as_deref().and_then(folder_key)
    }

    fn cur_skip(&self) -> Skip {
        self.cur_folder().and_then(|k| self.m.marks.skips.get(&k).copied()).unwrap_or_default()
    }

    fn edit_skip(&mut self, f: impl FnOnce(&mut Skip)) {
        let Some(k) = self.cur_folder() else { return };
        let s = self.m.marks.skips.entry(k.clone()).or_default();
        f(s);
        if s.intro.is_none() && s.outro.is_none() {
            self.m.marks.skips.remove(&k);
        }
        self.m.marks.save();
    }

    // ------------------------------------------------------------ lifecycle

    pub(super) fn on_loaded_more(&mut self) {
        self.m.intro_pending = true;
        self.m.outro_done = false;
    }

    pub(super) fn tick_more(&mut self) {
        if self.casting() || self.st.idle || self.st.duration <= 0.0 {
            return;
        }
        let skip = self.cur_skip();
        if self.m.intro_pending {
            self.m.intro_pending = false;
            // A resume seek may still be in flight; count it as where we start.
            let resumed = if self.s.resume {
                self.cur_key().and_then(|k| self.x.history.positions.get(&k).copied()).filter(|p| *p > 5.0)
            } else {
                None
            };
            let start_at = self.mp_f("time-pos").max(resumed.unwrap_or(0.0));
            if let Some(t) = intro_target(&skip, start_at, self.st.duration) {
                self.seek_abs(t, true);
                self.toast(self.t("已跳过片头 · 按 Home 回到开头", "Intro skipped · Home to go back"));
            }
        }
        if !self.m.outro_done && !self.st.paused && in_outro(&skip, self.st.time, self.st.duration) {
            self.m.outro_done = true;
            let n = self.playlist.len();
            let has_next = match self.s.loop_mode {
                crate::settings::LoopMode::One => false,
                crate::settings::LoopMode::All | crate::settings::LoopMode::Shuffle => n > 1,
                crate::settings::LoopMode::Off => self.index + 1 < n,
            };
            if has_next {
                self.toast(self.t("已跳过片尾，播放下一集", "Outro skipped, next episode"));
                self.on_eof();
            }
        }
    }

    // ------------------------------------------------------------ bookmarks

    fn bookmarks(&self) -> Vec<Bookmark> {
        self.cur_key().and_then(|k| self.m.marks.bookmarks.get(&k).cloned()).unwrap_or_default()
    }

    pub(super) fn add_bookmark(&mut self) {
        let Some(k) = self.cur_key() else { return };
        if self.st.idle {
            return;
        }
        let t = self.st.time;
        let label = self.chapter_at(t).map(|c| c.to_string()).unwrap_or_default();
        insert_bookmark(self.m.marks.bookmarks.entry(k).or_default(), Bookmark { t, label });
        self.m.marks.save();
        self.toast(format!("{} {}", self.t("已添加书签", "Bookmark added at"), fmt_time(t)));
    }

    fn remove_bookmark(&mut self, t: f64) {
        let Some(k) = self.cur_key() else { return };
        if let Some(list) = self.m.marks.bookmarks.get_mut(&k) {
            list.retain(|b| b.t != t);
            if list.is_empty() {
                self.m.marks.bookmarks.remove(&k);
            }
        }
        self.m.marks.save();
    }

    /// Jump to the next (`dir` > 0) or previous bookmark.
    pub(super) fn jump_bookmark(&mut self, dir: i32) {
        let t = self.st.time;
        let list = self.bookmarks();
        let target =
            if dir > 0 { list.iter().find(|b| b.t > t + 1.0) } else { list.iter().rev().find(|b| b.t < t - 2.0) };
        match target {
            Some(b) => {
                let bt = b.t;
                self.seek_abs(bt, true);
                self.toast(format!("{} {}", self.t("书签", "Bookmark"), fmt_time(bt)));
            }
            None => self.toast(self.t("没有更多书签", "No more bookmarks")),
        }
    }

    fn bookmark_items(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(260.0);
        if ui
            .add(
                egui::Button::new(menu_label(ic::BOOKMARK, self.t("在当前位置添加书签", "Add bookmark here")))
                    .shortcut_text(self.sc(crate::keymap::Action::BookmarkAdd)),
            )
            .clicked()
        {
            self.add_bookmark();
        }
        let list = self.bookmarks();
        if list.is_empty() {
            ui.label(RichText::new(self.t("这个视频还没有书签", "No bookmarks in this video")).size(12.5).color(MUTED));
            return;
        }
        ui.separator();
        let mut remove = None;
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for b in &list {
                ui.horizontal(|ui| {
                    let text =
                        if b.label.is_empty() { fmt_time(b.t) } else { format!("{}   {}", fmt_time(b.t), b.label) };
                    if ui.selectable_label(false, text).clicked() {
                        ui.close_menu();
                        self.seek_abs(b.t, true);
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(ic::X).on_hover_text(self.t("删除", "Remove")).clicked() {
                            remove = Some(b.t);
                        }
                    });
                });
            }
        });
        if let Some(t) = remove {
            self.remove_bookmark(t);
        }
        ui.label(
            RichText::new(
                self.t("可在快捷键里设置上一个 / 下一个书签", "Previous / next bookmark keys are in Shortcuts"),
            )
            .size(11.5)
            .color(MUTED),
        );
    }

    /// Bookmark dots above the seek bar.
    pub(super) fn bookmark_marks(&self, p: &egui::Painter, track: Rect, dur: f64) {
        if dur <= 0.0 {
            return;
        }
        for b in self.bookmarks() {
            let x = track.left() + track.width() * (b.t / dur).clamp(0.0, 1.0) as f32;
            p.circle_filled(pos2(x, track.top() - 5.0), 2.6, BOOKMARK_COLOR);
        }
    }

    // ------------------------------------------------------------ intro / outro

    fn skip_items(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(300.0);
        if self.cur_folder().is_none() {
            ui.label(RichText::new(self.t("仅支持本地文件", "Local files only")).color(MUTED));
            return;
        }
        ui.label(
            RichText::new(self.t("对同一文件夹里的所有剧集生效", "Applies to every episode in this folder"))
                .size(12.0)
                .color(MUTED),
        );
        let mut s = self.cur_skip();
        let before = s;
        ui.checkbox(&mut s.enabled, self.t("自动跳过片头片尾", "Skip intro and outro automatically"));
        let (t, dur) = (self.st.time, self.st.duration);
        ui.horizontal(|ui| {
            ui.label(self.t("片头长度", "Intro length"));
            let mut v = s.intro.unwrap_or(0.0);
            if ui
                .add(egui::DragValue::new(&mut v).range(0.0..=1800.0).speed(1.0).custom_formatter(|v, _| fmt_time(v)))
                .changed()
            {
                s.intro = (v > 0.0).then_some(v);
            }
            if ui.small_button(self.t("设为当前位置", "Set to now")).clicked() {
                s.intro = (t > 1.0).then_some(t);
                s.enabled = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label(self.t("片尾长度", "Outro length"));
            let mut v = s.outro.unwrap_or(0.0);
            if ui
                .add(egui::DragValue::new(&mut v).range(0.0..=1800.0).speed(1.0).custom_formatter(|v, _| fmt_time(v)))
                .changed()
            {
                s.outro = (v > 0.0).then_some(v);
            }
            if ui.small_button(self.t("从当前位置开始", "Starts now")).clicked() && dur > t {
                s.outro = Some(dur - t);
                s.enabled = true;
            }
        });
        if ui.button(menu_label(ic::TRASH, self.t("清除", "Clear"))).clicked() {
            s = Skip::default();
        }
        if s != before {
            self.edit_skip(|x| *x = s);
            if s.enabled && s.intro != before.intro {
                self.m.outro_done = false;
            }
        }
    }

    /// Entries added to the gear / right-click menu.
    pub(super) fn more_menu(&mut self, ui: &mut egui::Ui) {
        let n = self.bookmarks().len();
        let label = if n > 0 {
            format!("{} ({n})", self.t("书签", "Bookmarks"))
        } else {
            self.t("书签", "Bookmarks").to_string()
        };
        ui.menu_button(menu_label(ic::BOOKMARK, &label), |ui| self.bookmark_items(ui));
        let on = self.cur_skip().enabled;
        let label = if on {
            format!("{}  ●", self.t("跳过片头片尾", "Skip intro / outro"))
        } else {
            self.t("跳过片头片尾", "Skip intro / outro").to_string()
        };
        ui.menu_button(menu_label(ic::SKIP_FORWARD, &label), |ui| self.skip_items(ui));
    }

    // ------------------------------------------------------------ subtitle style

    fn set_first(&self, keys: &[&str], v: &str) {
        if let Some(m) = &self.mpv {
            for k in keys {
                if m.set(k, v).is_ok() {
                    return;
                }
            }
        }
    }

    /// Push the saved subtitle style to mpv (text subtitles; ASS too when forced).
    pub(super) fn apply_sub_style(&self) {
        let s = &self.s;
        self.set("sub-font", if s.sub_font.is_empty() { "sans-serif" } else { &s.sub_font });
        self.set("sub-color", &mpv_color(s.sub_color));
        self.set_first(&["sub-outline-color", "sub-border-color"], &mpv_color(s.sub_outline_color));
        self.set_first(&["sub-outline-size", "sub-border-size"], &format!("{:.1}", s.sub_outline));
        self.set("sub-shadow-offset", &format!("{:.1}", s.sub_shadow));
        self.set("sub-bold", if s.sub_bold { "yes" } else { "no" });
        if s.sub_box {
            self.set("sub-border-style", "background-box");
            if self.mp_s("sub-border-style") != "background-box" {
                self.set("sub-border-style", "opaque-box");
            }
            self.set("sub-back-color", &mpv_color(s.sub_box_color));
        } else {
            self.set("sub-border-style", "outline-and-shadow");
            self.set("sub-back-color", &mpv_color([0, 0, 0, if s.sub_shadow > 0.0 { 160 } else { 0 }]));
        }
        self.set("sub-ass-override", if s.sub_force_style { "force" } else { "scale" });
    }

    fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [u8; 4]) -> bool {
        ui.horizontal(|ui| {
            ui.label(label);
            let mut col = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
            let changed =
                egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::OnlyBlend)
                    .changed();
            if changed {
                *c = col.to_srgba_unmultiplied();
            }
            changed
        })
        .inner
    }

    pub(super) fn more_windows(&mut self, ctx: &egui::Context) {
        if !self.m.show_style {
            return;
        }
        let mut open = true;
        let mut changed = false;
        egui::Window::new(self.t("字幕样式", "Subtitle style"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .anchor(Align2::RIGHT_CENTER, [-24.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(self.t("字体", "Font"));
                    let shown = if self.s.sub_font.is_empty() {
                        self.t("默认", "Default").to_string()
                    } else {
                        self.s.sub_font.clone()
                    };
                    egui::ComboBox::from_id_salt("sub-font").selected_text(shown).width(180.0).show_ui(ui, |ui| {
                        if ui.selectable_label(self.s.sub_font.is_empty(), self.t("默认", "Default")).clicked() {
                            self.s.sub_font.clear();
                            changed = true;
                        }
                        for f in font_presets() {
                            if ui.selectable_label(self.s.sub_font == *f, *f).clicked() {
                                self.s.sub_font = f.to_string();
                                changed = true;
                            }
                        }
                    });
                });
                let hint = self.t("或输入已安装的字体名", "or type an installed font name");
                let r = ui.add(egui::TextEdit::singleline(&mut self.s.sub_font).hint_text(hint).desired_width(300.0));
                changed |= r.changed();
                ui.separator();
                changed |= Self::color_row(ui, self.t("文字颜色", "Text colour"), &mut self.s.sub_color);
                changed |= Self::color_row(ui, self.t("描边颜色", "Outline colour"), &mut self.s.sub_outline_color);
                let label = self.t("描边粗细", "Outline");
                changed |=
                    ui.add(egui::Slider::new(&mut self.s.sub_outline, 0.0..=8.0).step_by(0.5).text(label)).changed();
                let label = self.t("阴影", "Shadow");
                changed |=
                    ui.add(egui::Slider::new(&mut self.s.sub_shadow, 0.0..=6.0).step_by(0.5).text(label)).changed();
                let label = self.t("粗体", "Bold");
                changed |= ui.checkbox(&mut self.s.sub_bold, label).changed();
                let label = self.t("背景框", "Background box");
                changed |= ui.checkbox(&mut self.s.sub_box, label).changed();
                if self.s.sub_box {
                    changed |= Self::color_row(ui, self.t("背景颜色", "Box colour"), &mut self.s.sub_box_color);
                }
                let label = self.t(
                    "同样作用于 ASS 特效字幕（会覆盖原有样式）",
                    "Also restyle ASS subtitles (overrides their styling)",
                );
                changed |= ui.checkbox(&mut self.s.sub_force_style, label).changed();
                ui.separator();
                if ui.button(self.t("恢复默认", "Reset")).clicked() {
                    let d = crate::settings::Settings::default();
                    self.s.sub_font = d.sub_font;
                    self.s.sub_color = d.sub_color;
                    self.s.sub_outline_color = d.sub_outline_color;
                    self.s.sub_outline = d.sub_outline;
                    self.s.sub_shadow = d.sub_shadow;
                    self.s.sub_bold = d.sub_bold;
                    self.s.sub_box = d.sub_box;
                    self.s.sub_box_color = d.sub_box_color;
                    self.s.sub_force_style = d.sub_force_style;
                    changed = true;
                }
            });
        if changed {
            self.s.save();
            self.apply_sub_style();
        }
        if !open {
            self.m.show_style = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intro_and_outro() {
        let s = Skip { intro: Some(90.0), outro: Some(120.0), enabled: true };
        assert_eq!(intro_target(&s, 0.0, 1440.0), Some(90.0));
        assert_eq!(intro_target(&s, 600.0, 1440.0), None);
        assert_eq!(intro_target(&Skip { enabled: false, ..s }, 0.0, 1440.0), None);
        assert_eq!(intro_target(&s, 0.0, 60.0), None);
        assert!(!in_outro(&s, 1300.0, 1440.0));
        assert!(in_outro(&s, 1320.0, 1440.0));
        assert!(!in_outro(&Skip { outro: None, ..s }, 1430.0, 1440.0));
    }

    #[test]
    fn bookmarks_sorted() {
        let mut v = Vec::new();
        for t in [30.0, 10.0, 20.0, 10.4] {
            insert_bookmark(&mut v, Bookmark { t, label: String::new() });
        }
        let ts: Vec<f64> = v.iter().map(|b| b.t).collect();
        assert_eq!(ts, vec![10.4, 20.0, 30.0]);
        assert_eq!(mpv_color([255, 128, 0, 160]), "#A0FF8000");
        assert_eq!(folder_key(Path::new("http://x/a.mp4")), None);
    }
}
