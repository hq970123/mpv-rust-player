//! Keyboard dispatch through the remappable keymap, and the shortcut editor (F1).

use super::*;
use crate::keymap::{reserved, Action, Combo, Keymap};

pub(super) struct Keys {
    pub(super) map: Keymap,
    /// Waiting for the next key press to bind to this action.
    recording: Option<Action>,
    filter: String,
}

impl Keys {
    pub(super) fn new(s: &Settings) -> Self {
        Self { map: Keymap::from_overrides(&s.keys), recording: None, filter: String::new() }
    }
}

impl PlayerApp {
    /// Menu hint for an action ("Ctrl + O").
    pub(super) fn sc(&self, a: Action) -> String {
        self.kb.map.hint(a)
    }

    fn save_keys(&mut self) {
        self.s.keys = self.kb.map.to_overrides();
        self.s.save();
    }

    pub(super) fn keyboard_all(&mut self, ctx: &egui::Context) {
        if let Some(a) = self.kb.recording {
            self.capture_key(ctx, a);
            return;
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let mut fired = Vec::new();
        for (c, a) in self.kb.map.ordered() {
            let sc = egui::KeyboardShortcut::new(c.modifiers(), c.key);
            while ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                fired.push(a);
            }
        }
        for a in fired {
            self.run_action(a, ctx);
        }
        // Fixed keys.
        let k = |m: Modifiers, key: Key| ctx.input_mut(|i| i.consume_key(m, key));
        if k(Modifiers::NONE, Key::Escape) {
            if self.pip_restore.is_some() {
                self.toggle_pip(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
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
            if k(Modifiers::NONE, *d) && self.st.duration > 0.0 {
                self.seek_abs(self.st.duration * i as f64 / 10.0, false);
            }
        }
    }

    fn capture_key(&mut self, ctx: &egui::Context, a: Action) {
        let pressed = ctx.input_mut(|i| {
            let hit = i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            });
            if hit.is_some() {
                i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_)));
            }
            hit
        });
        let Some((key, mods)) = pressed else { return };
        self.kb.recording = None;
        if key == Key::Escape && mods.is_none() {
            return;
        }
        if reserved(key) && mods.is_none() {
            self.toast(self.t("Esc 和数字键 0–9 是固定按键", "Esc and 0–9 are fixed keys"));
            return;
        }
        let c = Combo::new(mods, key);
        if let Some(prev) = self.kb.map.bind(a, c) {
            let zh = self.s.ui_lang == UiLang::Zh;
            self.toast(format!("{} {} {}", c.display(), self.t("已从此项移除：", "was removed from"), prev.label(zh)));
        }
        self.save_keys();
    }

    pub(super) fn run_action(&mut self, a: Action, ctx: &egui::Context) {
        let casting = self.casting();
        match a {
            Action::PlayPause => self.toggle_pause(),
            Action::SeekBack => self.seek_rel(-5.0),
            Action::SeekFwd => self.seek_rel(5.0),
            Action::SeekBackSmall => self.seek_rel(-1.0),
            Action::SeekFwdSmall => self.seek_rel(1.0),
            Action::SeekBackBig => self.seek_rel(-30.0),
            Action::SeekFwdBig => self.seek_rel(30.0),
            Action::Restart => self.seek_abs(0.0, true),
            Action::FrameBack if !casting => self.cmd(&["frame-back-step"]),
            Action::FrameFwd if !casting => self.cmd(&["frame-step"]),
            Action::FrameBack | Action::FrameFwd => {}
            Action::VolUp => self.set_volume(self.st.volume + 5.0),
            Action::VolDown => self.set_volume(self.st.volume - 5.0),
            Action::Mute => self.cmd(&["cycle", "mute"]),
            Action::Slower => self.cmd(&["multiply", "speed", "0.9091"]),
            Action::Faster => self.cmd(&["multiply", "speed", "1.1"]),
            Action::SpeedReset => self.set("speed", "1"),
            Action::Prev => self.step(-1),
            Action::Next => self.step(1),
            Action::Fullscreen => self.toggle_fullscreen(ctx),
            Action::AbLoop => self.ab_loop(),
            Action::Playlist => self.x.show_playlist = !self.x.show_playlist,
            Action::OnTop => self.toggle_on_top(ctx),
            Action::Rotate => self.rotate(),
            Action::SubToggle => {
                let vis = self.mp_s("sub-visibility") != "no";
                self.set("sub-visibility", if vis { "no" } else { "yes" });
                self.toast(if vis {
                    self.t("字幕：隐藏", "Subtitles hidden")
                } else {
                    self.t("字幕：显示", "Subtitles shown")
                });
            }
            Action::SubDelayMinus | Action::SubDelayPlus => {
                let d = if a == Action::SubDelayMinus { "-0.1" } else { "0.1" };
                self.cmd(&["add", "sub-delay", d]);
                self.toast(format!("{} {:+.1}s", self.t("字幕延迟", "Subtitle delay"), self.mp_f("sub-delay")));
            }
            Action::AudioDelayMinus | Action::AudioDelayPlus => {
                let d = if a == Action::AudioDelayMinus { "-0.1" } else { "0.1" };
                self.cmd(&["add", "audio-delay", d]);
                self.toast(format!("{} {:+.1}s", self.t("音频延迟", "Audio delay"), self.mp_f("audio-delay")));
            }
            Action::Live => {
                if self.live_available() {
                    let on = !self.s.live_enabled;
                    self.set_live(on);
                }
            }
            Action::Snapshot => self.snapshot(self.s.shot_with_subs, false),
            Action::SnapshotAlt => self.snapshot(!self.s.shot_with_subs, false),
            Action::CopyFrame => self.snapshot(self.s.shot_with_subs, true),
            Action::ExportClip => self.open_export(false),
            Action::ExportGif => self.open_export(true),
            Action::Open => self.open_dialog(),
            Action::OpenFolder => self.open_folder_dialog(),
            Action::OpenUrl => self.x.show_url = true,
            Action::Info => self.x.show_info = !self.x.show_info,
            Action::Stats => self.show_stats = !self.show_stats,
            Action::Shortcuts => self.x.show_keys = !self.x.show_keys,
            Action::BookmarkAdd => self.add_bookmark(),
            Action::BookmarkPrev => self.jump_bookmark(-1),
            Action::BookmarkNext => self.jump_bookmark(1),
        }
    }

    /// The shortcut sheet, editable: click + to record a key, click a key to remove it.
    pub(super) fn keys_window(&mut self, ctx: &egui::Context) {
        if !self.x.show_keys {
            self.kb.recording = None;
            return;
        }
        let mut open = true;
        let zh = self.s.ui_lang == UiLang::Zh;
        let mut changed = false;
        egui::Window::new(self.t("快捷键", "Keyboard shortcuts"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([520.0, 560.0])
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let hint = self.t("搜索功能或按键", "Search action or key");
                    ui.add(egui::TextEdit::singleline(&mut self.kb.filter).hint_text(hint).desired_width(220.0));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.button(self.t("全部恢复默认", "Reset all")).clicked() {
                            self.kb.map = Keymap::from_overrides(&Default::default());
                            changed = true;
                        }
                    });
                });
                ui.label(
                    RichText::new(self.t(
                        "点 + 后按下新按键（Esc 取消）；点已有按键可删除。",
                        "Click + then press a key (Esc cancels); click a key to remove it.",
                    ))
                    .size(12.0)
                    .color(MUTED),
                );
                ui.separator();
                let filter = self.kb.filter.to_lowercase();
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    egui::Grid::new("keys-edit").num_columns(3).spacing([16.0, 6.0]).striped(true).show(ui, |ui| {
                        for a in Action::all() {
                            let combos: Vec<Combo> = self.kb.map.get(a).to_vec();
                            let text = combos.iter().map(Combo::display).collect::<Vec<_>>().join(" ").to_lowercase();
                            if !filter.is_empty()
                                && !a.label(true).to_lowercase().contains(&filter)
                                && !a.label(false).to_lowercase().contains(&filter)
                                && !text.contains(&filter)
                            {
                                continue;
                            }
                            ui.label(a.label(zh));
                            ui.horizontal_wrapped(|ui| {
                                for c in &combos {
                                    let b = egui::Button::new(RichText::new(c.display()).monospace().color(ACCENT));
                                    if ui.add(b).on_hover_text(self.t("点击删除", "Click to remove")).clicked() {
                                        self.kb.map.unbind(a, *c);
                                        changed = true;
                                    }
                                }
                                let rec = self.kb.recording == Some(a);
                                let label = if rec { self.t("请按键…", "Press a key…") } else { "+" };
                                if ui.add(egui::Button::new(label).selected(rec)).clicked() {
                                    self.kb.recording = if rec { None } else { Some(a) };
                                }
                            });
                            if combos != a.defaults() {
                                if ui.small_button(ic::UNDO).on_hover_text(self.t("恢复默认", "Reset")).clicked() {
                                    self.kb.map.reset(a);
                                    changed = true;
                                }
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(self.t(
                            "固定：Esc 退出全屏 / 画中画，0–9 跳到 0%–90%。鼠标：单击暂停，双击全屏，滚轮音量，Ctrl+滚轮缩放。",
                            "Fixed: Esc leaves full screen / PiP, 0–9 jump to 0–90%. Mouse: click pause, double-click full screen, wheel volume, Ctrl+wheel zoom.",
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                });
            });
        if changed {
            self.save_keys();
        }
        if !open {
            self.x.show_keys = false;
            self.kb.recording = None;
        }
    }
}
