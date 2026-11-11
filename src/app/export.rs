//! Export dialog for clips (MP4) and GIFs. The encoding itself is in
//! `crate::clip`; this is the window, the range picking and the hand-off.

use super::*;
use crate::clip::{self, Exporter, Format, Job, State, Subs};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SubPick {
    None,
    Track,
    Live,
}

pub(super) struct ExportUi {
    pub(super) open: bool,
    start: f64,
    end: f64,
    format: Format,
    width: u32,
    fps: u32,
    audio: bool,
    subs: SubPick,
    encoders: Option<Vec<String>>,
    pub(super) job: Exporter,
    /// Toast once when a job finishes while the window is closed.
    notified: bool,
}

impl Default for ExportUi {
    fn default() -> Self {
        Self {
            open: false,
            start: 0.0,
            end: 0.0,
            format: Format::Mp4,
            width: 0,
            fps: 15,
            audio: true,
            subs: SubPick::None,
            encoders: None,
            job: Exporter::default(),
            notified: true,
        }
    }
}

const WIDTHS: &[(u32, &str, &str)] = &[
    (0, "原始尺寸", "Original"),
    (1920, "1920 宽", "1920 wide"),
    (1280, "1280 宽", "1280 wide"),
    (854, "854 宽", "854 wide"),
    (640, "640 宽", "640 wide"),
    (480, "480 宽", "480 wide"),
    (320, "320 宽", "320 wide"),
];

pub(super) fn clips_dir(s: &Settings) -> PathBuf {
    if !s.clip_dir.is_empty() {
        return PathBuf::from(&s.clip_dir);
    }
    dirs::video_dir().or_else(dirs::picture_dir).unwrap_or_else(settings::data_dir).join(APP_NAME)
}

impl PlayerApp {
    fn sub_track_selected(&self) -> bool {
        self.tracks.iter().any(|t| t.kind == "sub" && t.selected)
    }

    fn live_cues(&self) -> usize {
        self.live.shared.cues.lock().unwrap().len()
    }

    /// Open the dialog with a sensible range: the A-B loop if set, else from
    /// the current position.
    pub(super) fn open_export(&mut self, gif: bool) {
        if self.current.is_none() {
            self.toast(self.t("先打开一个视频", "Open a video first"));
            return;
        }
        if self.casting() {
            self.toast(self.t("投屏时不能导出", "Not available while casting"));
            return;
        }
        let dur = self.st.duration.max(0.0);
        let pos = self.st.time.clamp(0.0, dur);
        let (start, end) = match self.ab_points() {
            (Some(a), Some(b)) if b > a => (a, b),
            _ => (pos, (pos + if gif { 5.0 } else { 30.0 }).min(dur.max(pos))),
        };
        if self.x_export_busy() {
            self.ex.open = true;
            return;
        }
        self.ex.job.reset();
        self.ex.start = start;
        self.ex.end = end;
        if gif {
            self.ex.format = Format::Gif;
            if self.ex.width == 0 {
                self.ex.width = 480;
            }
        }
        self.ex.subs = if self.s.live_enabled && self.live_cues() > 0 {
            SubPick::Live
        } else if self.sub_track_selected() && self.mp_s("sub-visibility") != "no" {
            SubPick::Track
        } else {
            SubPick::None
        };
        if self.ex.encoders.is_none() {
            let list = self.mpv.as_ref().and_then(|m| m.get("encoder-list")).map(|j| clip::parse_encoders(&j));
            self.ex.encoders = Some(list.unwrap_or_default());
        }
        self.ex.open = true;
    }

    fn x_export_busy(&self) -> bool {
        self.ex.job.busy()
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        let Some(src) = self.current.clone() else { return };
        let src = src.to_string_lossy().to_string();
        let subs = match self.ex.subs {
            SubPick::None => Subs::None,
            SubPick::Track => match self.tracks.iter().find(|t| t.kind == "sub" && t.selected) {
                Some(t) if t.external && !t.external_file.is_empty() => Subs::File(PathBuf::from(&t.external_file)),
                Some(t) => Subs::Track(t.id),
                None => Subs::None,
            },
            SubPick::Live => {
                let (translated, bilingual) = match self.s.sub_mode {
                    SubMode::Original => (false, false),
                    SubMode::Translated => (true, false),
                    SubMode::Bilingual => (true, true),
                };
                let srt = self.live.shared.to_srt(translated, bilingual);
                let p = std::env::temp_dir().join(format!("okplayer-live-{}.srt", std::process::id()));
                match std::fs::write(&p, srt) {
                    Ok(()) => Subs::File(p),
                    Err(_) => Subs::None,
                }
            }
        };
        let out = clip::out_path(&clips_dir(&self.s), &src, self.ex.start, self.ex.end, self.ex.format);
        let job = Job {
            src,
            start: self.ex.start,
            end: self.ex.end,
            format: self.ex.format,
            width: self.ex.width,
            fps: self.ex.fps,
            audio: self.ex.audio,
            subs,
            out,
            encoders: self.ex.encoders.clone().unwrap_or_default(),
        };
        self.ex.notified = false;
        self.ex.job.start(job, ctx.clone());
    }

    /// Called every frame: the window, and a toast when a background job ends.
    pub(super) fn export_frame(&mut self, ctx: &egui::Context) {
        let state = self.ex.job.state();
        if !self.ex.notified && !matches!(state, State::Running(_)) {
            self.ex.notified = true;
            if !self.ex.open {
                match &state {
                    State::Done(p) => {
                        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        self.toast(format!("{} {name}", self.t("已导出", "Exported")));
                    }
                    State::Failed(e) => self.toast(format!("{} {e}", self.t("导出失败：", "Export failed:"))),
                    _ => {}
                }
            }
        }
        if !self.ex.open {
            return;
        }
        let mut open = true;
        let mut go = false;
        let dur = self.st.duration.max(0.0);
        let pos = self.st.time;
        let zh = self.s.ui_lang == UiLang::Zh;
        let tr = move |a: &'static str, b: &'static str| if zh { a } else { b };
        let title = self.t("导出片段 / GIF", "Export clip / GIF");
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let busy = matches!(state, State::Running(_));
                ui.add_enabled_ui(!busy, |ui| {
                    egui::Grid::new("export").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                        ui.label(tr("格式", "Format"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.ex.format, Format::Mp4, "MP4");
                            ui.radio_value(&mut self.ex.format, Format::Gif, "GIF");
                        });
                        ui.end_row();

                        for (is_start, label) in [(true, tr("起点", "Start")), (false, tr("终点", "End"))] {
                            ui.label(label);
                            ui.horizontal(|ui| {
                                let v = if is_start { &mut self.ex.start } else { &mut self.ex.end };
                                ui.add(
                                    egui::DragValue::new(v)
                                        .range(0.0..=dur.max(0.1))
                                        .speed(0.1)
                                        .custom_formatter(|n, _| {
                                            format!("{}.{}", fmt_time(n), ((n.fract() * 10.0) as u32).min(9))
                                        })
                                        .custom_parser(parse_time),
                                );
                                if ui.small_button(tr("设为当前", "Use current")).clicked() {
                                    *v = pos.clamp(0.0, dur);
                                }
                            });
                            ui.end_row();
                        }

                        ui.label(tr("尺寸", "Size"));
                        let cur = WIDTHS.iter().find(|w| w.0 == self.ex.width).copied().unwrap_or(WIDTHS[0]);
                        egui::ComboBox::from_id_salt("export-width")
                            .selected_text(if zh { cur.1 } else { cur.2 })
                            .show_ui(ui, |ui| {
                                for w in WIDTHS {
                                    ui.selectable_value(&mut self.ex.width, w.0, if zh { w.1 } else { w.2 });
                                }
                            });
                        ui.end_row();

                        if self.ex.format == Format::Gif {
                            ui.label(tr("帧率", "Frame rate"));
                            ui.horizontal(|ui| {
                                for f in [10, 15, 20, 24] {
                                    ui.selectable_value(&mut self.ex.fps, f, format!("{f}"));
                                }
                            });
                            ui.end_row();
                        } else {
                            ui.label(tr("声音", "Audio"));
                            ui.checkbox(&mut self.ex.audio, tr("包含声音", "Include audio"));
                            ui.end_row();
                        }

                        ui.label(tr("字幕", "Subtitles"));
                        ui.vertical(|ui| {
                            ui.radio_value(&mut self.ex.subs, SubPick::None, tr("不加字幕", "None"));
                            if self.sub_track_selected() {
                                ui.radio_value(
                                    &mut self.ex.subs,
                                    SubPick::Track,
                                    tr("烧录当前字幕轨", "Burn in current track"),
                                );
                            }
                            if self.live_cues() > 0 {
                                ui.radio_value(
                                    &mut self.ex.subs,
                                    SubPick::Live,
                                    tr("烧录实时识别字幕", "Burn in live subtitles"),
                                );
                            }
                        });
                        ui.end_row();

                        ui.label(tr("保存到", "Save to"));
                        ui.horizontal(|ui| {
                            let dir = clips_dir(&self.s);
                            ui.label(RichText::new(dir.to_string_lossy()).color(MUTED).size(12.0));
                            if ui.small_button(tr("更改…", "Change…")).clicked() {
                                if let Some(p) = rfd::FileDialog::new().set_directory(&dir).pick_folder() {
                                    self.s.clip_dir = p.to_string_lossy().to_string();
                                    self.s.save();
                                }
                            }
                        });
                        ui.end_row();
                    });
                });

                if self.ex.end < self.ex.start {
                    std::mem::swap(&mut self.ex.start, &mut self.ex.end);
                }
                let len = self.ex.end - self.ex.start;
                let mut note = format!("{} {}", tr("时长", "Length"), fmt_dur(len));
                if self.ex.format == Format::Gif && len > 20.0 {
                    note.push_str(tr("，GIF 超过 20 秒文件会很大", " — GIFs over 20 s get large"));
                }
                if self.ex.subs == SubPick::Live {
                    note.push_str(tr("；实时字幕只含已识别的部分", "; live subtitles cover only what was recognised"));
                }
                ui.label(RichText::new(note).color(MUTED).size(12.5));
                ui.add_space(6.0);

                match &state {
                    State::Running(f) => {
                        ui.add(egui::ProgressBar::new(*f).show_percentage().animate(true));
                        if ui.button(tr("取消", "Cancel")).clicked() {
                            self.ex.job.cancel();
                        }
                    }
                    State::Done(p) => {
                        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        ui.label(RichText::new(format!("✓ {name}")).color(ACCENT));
                        ui.horizontal(|ui| {
                            if ui.button(tr("打开所在文件夹", "Show in folder")).clicked() {
                                reveal(p);
                            }
                            if ui.button(tr("再导出一段", "Export another")).clicked() {
                                self.ex.job.reset();
                            }
                        });
                    }
                    State::Failed(e) => {
                        ui.label(
                            RichText::new(format!("{} {e}", tr("导出失败：", "Export failed:")))
                                .color(Color32::from_rgb(240, 110, 110)),
                        );
                        if ui.button(tr("重试", "Retry")).clicked() {
                            go = true;
                        }
                    }
                    State::Idle => {
                        let ok = len >= 0.1;
                        if ui.add_enabled(ok, egui::Button::new(menu_label(ic::FILM, tr("导出", "Export")))).clicked()
                        {
                            go = true;
                        }
                    }
                }
            });
        if go {
            self.start_export(ctx);
        }
        // Closing the window keeps a running export going; a toast reports it.
        self.ex.open = open;
    }
}

fn fmt_dur(t: f64) -> String {
    if t < 60.0 {
        format!("{t:.1}s")
    } else {
        fmt_time(t)
    }
}

/// "1:02:03.4", "2:03", "75.5" → seconds.
fn parse_time(s: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in s.trim().split(':') {
        total = total * 60.0 + part.trim().parse::<f64>().ok()?;
    }
    Some(total)
}

fn reveal(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(d) = p.parent() {
        let _ = std::process::Command::new("xdg-open").arg(d).spawn();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_times() {
        assert_eq!(super::parse_time("1:02:03.5"), Some(3723.5));
        assert_eq!(super::parse_time("2:03"), Some(123.0));
        assert_eq!(super::parse_time("75.5"), Some(75.5));
        assert_eq!(super::parse_time("x"), None);
    }
}
