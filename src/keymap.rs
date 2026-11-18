//! Remappable keyboard shortcuts: the action table, default bindings and the
//! user's overrides (stored in settings as action id -> list of combos).

use eframe::egui::{Key, Modifiers};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    PlayPause,
    SeekBack,
    SeekFwd,
    SeekBackSmall,
    SeekFwdSmall,
    SeekBackBig,
    SeekFwdBig,
    Restart,
    FrameBack,
    FrameFwd,
    VolUp,
    VolDown,
    Mute,
    Slower,
    Faster,
    SpeedReset,
    Prev,
    Next,
    Fullscreen,
    AbLoop,
    Playlist,
    OnTop,
    Rotate,
    SubToggle,
    SubDelayMinus,
    SubDelayPlus,
    AudioDelayMinus,
    AudioDelayPlus,
    Live,
    Snapshot,
    SnapshotAlt,
    CopyFrame,
    ExportClip,
    ExportGif,
    Open,
    OpenFolder,
    OpenUrl,
    Info,
    Stats,
    Shortcuts,
    BookmarkAdd,
    BookmarkPrev,
    BookmarkNext,
}

use Action::*;

/// (action, id, Chinese label, English label, default combos)
const TABLE: &[(Action, &str, &str, &str, &[&str])] = &[
    (PlayPause, "play_pause", "播放 / 暂停", "Play / pause", &["Space"]),
    (SeekBack, "seek_back", "后退 5 秒", "Back 5 s", &["ArrowLeft"]),
    (SeekFwd, "seek_fwd", "前进 5 秒", "Forward 5 s", &["ArrowRight"]),
    (SeekBackSmall, "seek_back_small", "后退 1 秒", "Back 1 s", &["Shift+ArrowLeft"]),
    (SeekFwdSmall, "seek_fwd_small", "前进 1 秒", "Forward 1 s", &["Shift+ArrowRight"]),
    (SeekBackBig, "seek_back_big", "后退 30 秒", "Back 30 s", &["Ctrl+ArrowLeft"]),
    (SeekFwdBig, "seek_fwd_big", "前进 30 秒", "Forward 30 s", &["Ctrl+ArrowRight"]),
    (Restart, "restart", "从头播放", "Restart", &["Home"]),
    (FrameBack, "frame_back", "上一帧", "Frame back", &["Comma"]),
    (FrameFwd, "frame_fwd", "下一帧", "Frame forward", &["Period"]),
    (VolUp, "vol_up", "音量 +", "Volume up", &["ArrowUp"]),
    (VolDown, "vol_down", "音量 −", "Volume down", &["ArrowDown"]),
    (Mute, "mute", "静音", "Mute", &["M"]),
    (Slower, "slower", "减速", "Slower", &["OpenBracket"]),
    (Faster, "faster", "加速", "Faster", &["CloseBracket"]),
    (SpeedReset, "speed_reset", "恢复 1.0x", "Normal speed", &["Backspace"]),
    (Prev, "prev", "上一个文件", "Previous file", &["PageUp"]),
    (Next, "next", "下一个文件", "Next file", &["PageDown"]),
    (Fullscreen, "fullscreen", "全屏", "Full screen", &["F", "Enter"]),
    (AbLoop, "ab_loop", "A-B 循环", "A-B loop", &["L"]),
    (Playlist, "playlist", "播放列表", "Playlist", &["P"]),
    (OnTop, "on_top", "窗口置顶", "Always on top", &["T"]),
    (Rotate, "rotate", "旋转画面", "Rotate", &["R"]),
    (SubToggle, "sub_toggle", "显示 / 隐藏字幕", "Toggle subtitles", &["V"]),
    (SubDelayMinus, "sub_delay_minus", "字幕延迟 −0.1 秒", "Subtitle delay −0.1 s", &["Z"]),
    (SubDelayPlus, "sub_delay_plus", "字幕延迟 +0.1 秒", "Subtitle delay +0.1 s", &["X"]),
    (AudioDelayMinus, "audio_delay_minus", "音频延迟 −0.1 秒", "Audio delay −0.1 s", &["Ctrl+Minus"]),
    (AudioDelayPlus, "audio_delay_plus", "音频延迟 +0.1 秒", "Audio delay +0.1 s", &["Ctrl+Equals", "Ctrl+Plus"]),
    (Live, "live", "实时识别翻译字幕", "Live subtitles", &["C"]),
    (Snapshot, "snapshot", "截图", "Snapshot", &["S"]),
    (SnapshotAlt, "snapshot_alt", "截图（另一种字幕模式）", "Snapshot (other subtitle mode)", &["Shift+S"]),
    (CopyFrame, "copy_frame", "复制当前画面", "Copy frame", &["Ctrl+C"]),
    (ExportClip, "export_clip", "导出片段", "Export clip", &["Ctrl+E"]),
    (ExportGif, "export_gif", "导出 GIF", "Export GIF", &["Ctrl+Shift+E"]),
    (Open, "open", "打开文件", "Open file", &["Ctrl+O"]),
    (OpenFolder, "open_folder", "打开文件夹", "Open folder", &["Ctrl+Shift+O"]),
    (OpenUrl, "open_url", "打开网络地址", "Open URL", &["Ctrl+U"]),
    (Info, "info", "媒体信息", "Media info", &["Tab"]),
    (Stats, "stats", "播放统计", "Playback stats", &["Ctrl+I"]),
    (Shortcuts, "shortcuts", "快捷键", "Shortcuts", &["F1"]),
    (BookmarkAdd, "bookmark_add", "添加书签", "Add bookmark", &["B"]),
    (BookmarkPrev, "bookmark_prev", "上一个书签", "Previous bookmark", &["Shift+B"]),
    (BookmarkNext, "bookmark_next", "下一个书签", "Next bookmark", &["Ctrl+B"]),
];

fn row(a: Action) -> &'static (Action, &'static str, &'static str, &'static str, &'static [&'static str]) {
    TABLE.iter().find(|r| r.0 == a).expect("action in table")
}

impl Action {
    pub fn all() -> impl Iterator<Item = Action> {
        TABLE.iter().map(|r| r.0)
    }
    pub fn id(self) -> &'static str {
        row(self).1
    }
    pub fn label(self, zh: bool) -> &'static str {
        if zh {
            row(self).2
        } else {
            row(self).3
        }
    }
    pub fn defaults(self) -> Vec<Combo> {
        row(self).4.iter().filter_map(|s| Combo::parse(s)).collect()
    }
    fn from_id(id: &str) -> Option<Action> {
        TABLE.iter().find(|r| r.1 == id).map(|r| r.0)
    }
}

/// A key plus modifiers. `cmd` is Ctrl on Windows / Linux and ⌘ on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Combo {
    pub fn new(m: Modifiers, key: Key) -> Self {
        Self { cmd: m.command, shift: m.shift, alt: m.alt, key }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let mut c = Combo { cmd: false, shift: false, alt: false, key: Key::Space };
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = parts.split_last()?;
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "cmd" | "command" => c.cmd = true,
                "shift" => c.shift = true,
                "alt" | "option" => c.alt = true,
                _ => return None,
            }
        }
        // "Ctrl++" splits into an empty last part
        c.key = if key.is_empty() { Key::Plus } else { Key::from_name(key)? };
        Some(c)
    }

    /// Stable form for settings.json.
    pub fn store(&self) -> String {
        let mut s = String::new();
        if self.cmd {
            s.push_str("Ctrl+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s.push_str(self.key.name());
        s
    }

    /// Short human form, e.g. "Ctrl + ←" or "⌘ + ←".
    pub fn display(&self) -> String {
        let mac = cfg!(target_os = "macos");
        let mut parts: Vec<&str> = Vec::new();
        if self.cmd {
            parts.push(if mac { "⌘" } else { "Ctrl" });
        }
        if self.alt {
            parts.push(if mac { "⌥" } else { "Alt" });
        }
        if self.shift {
            parts.push(if mac { "⇧" } else { "Shift" });
        }
        let key = self.key.symbol_or_name();
        let mut out = parts.join(" + ");
        if !out.is_empty() {
            out.push_str(" + ");
        }
        out.push_str(key);
        out
    }

    pub fn modifiers(&self) -> Modifiers {
        let mut m = Modifiers::NONE;
        if self.cmd {
            m = m | Modifiers::COMMAND;
        }
        if self.shift {
            m = m | Modifiers::SHIFT;
        }
        if self.alt {
            m = m | Modifiers::ALT;
        }
        m
    }

    fn weight(&self) -> u8 {
        self.cmd as u8 + self.shift as u8 + self.alt as u8
    }
}

/// Keys with a fixed meaning that can't be bound: Esc and the 0-9 jumps.
pub fn reserved(k: Key) -> bool {
    matches!(
        k,
        Key::Escape
            | Key::Num0
            | Key::Num1
            | Key::Num2
            | Key::Num3
            | Key::Num4
            | Key::Num5
            | Key::Num6
            | Key::Num7
            | Key::Num8
            | Key::Num9
    )
}

#[derive(Clone, Debug)]
pub struct Keymap {
    binds: Vec<(Action, Vec<Combo>)>,
}

impl Keymap {
    pub fn from_overrides(o: &BTreeMap<String, Vec<String>>) -> Self {
        let mut binds: Vec<(Action, Vec<Combo>)> = Action::all().map(|a| (a, a.defaults())).collect();
        for (id, list) in o {
            if let Some(a) = Action::from_id(id) {
                let combos: Vec<Combo> = list.iter().filter_map(|s| Combo::parse(s)).collect();
                if let Some(b) = binds.iter_mut().find(|b| b.0 == a) {
                    b.1 = combos;
                }
            }
        }
        // A combo belongs to one action; the user's override wins over a default.
        let mut seen: Vec<Combo> = Vec::new();
        let order: Vec<usize> = (0..binds.len())
            .filter(|i| o.contains_key(binds[*i].0.id()))
            .chain((0..binds.len()).filter(|i| !o.contains_key(binds[*i].0.id())))
            .collect();
        for i in order {
            binds[i].1.retain(|c| {
                let dup = seen.contains(c);
                seen.push(*c);
                !dup
            });
        }
        Self { binds }
    }

    pub fn to_overrides(&self) -> BTreeMap<String, Vec<String>> {
        self.binds
            .iter()
            .filter(|(a, c)| *c != a.defaults())
            .map(|(a, c)| (a.id().to_string(), c.iter().map(Combo::store).collect()))
            .collect()
    }

    pub fn get(&self, a: Action) -> &[Combo] {
        self.binds.iter().find(|b| b.0 == a).map(|b| b.1.as_slice()).unwrap_or(&[])
    }

    pub fn owner(&self, c: &Combo) -> Option<Action> {
        self.binds.iter().find(|b| b.1.contains(c)).map(|b| b.0)
    }

    /// Bind `c` to `a`, taking it away from any other action (returned).
    pub fn bind(&mut self, a: Action, c: Combo) -> Option<Action> {
        let prev = self.owner(&c).filter(|o| *o != a);
        for b in &mut self.binds {
            b.1.retain(|x| *x != c);
        }
        if let Some(b) = self.binds.iter_mut().find(|b| b.0 == a) {
            b.1.push(c);
        }
        prev
    }

    pub fn unbind(&mut self, a: Action, c: Combo) {
        if let Some(b) = self.binds.iter_mut().find(|b| b.0 == a) {
            b.1.retain(|x| *x != c);
        }
    }

    pub fn reset(&mut self, a: Action) {
        let d = a.defaults();
        for c in &d {
            for b in &mut self.binds {
                b.1.retain(|x| x != c);
            }
        }
        if let Some(b) = self.binds.iter_mut().find(|b| b.0 == a) {
            b.1 = d;
        }
    }

    /// Every binding, most modifiers first, so Shift+B is checked before B.
    pub fn ordered(&self) -> Vec<(Combo, Action)> {
        let mut v: Vec<(Combo, Action)> = self.binds.iter().flat_map(|(a, cs)| cs.iter().map(|c| (*c, *a))).collect();
        v.sort_by_key(|(c, _)| std::cmp::Reverse(c.weight()));
        v
    }

    /// First binding as text, for menu hints.
    pub fn hint(&self, a: Action) -> String {
        self.get(a).first().map(Combo::display).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_are_unique() {
        let mut all = Vec::new();
        for a in Action::all() {
            let d = a.defaults();
            assert!(!d.is_empty(), "{a:?}");
            for c in d {
                assert!(!all.contains(&c), "{a:?} {c:?}");
                all.push(c);
            }
        }
    }

    #[test]
    fn combo_round_trip() {
        for s in ["Ctrl+Shift+E", "ArrowLeft", "Alt+F", "Comma", "Ctrl+Plus"] {
            let c = Combo::parse(s).unwrap();
            assert_eq!(Combo::parse(&c.store()), Some(c), "{s}");
        }
        assert_eq!(Combo::parse("Ctrl++").unwrap().key, Key::Plus);
        assert!(Combo::parse("Hyper+A").is_none());
    }

    #[test]
    fn overrides_take_keys_away() {
        let mut o = BTreeMap::new();
        o.insert("mute".to_string(), vec!["Space".to_string()]);
        let km = Keymap::from_overrides(&o);
        let space = Combo::parse("Space").unwrap();
        assert_eq!(km.owner(&space), Some(Action::Mute));
        assert!(km.get(Action::PlayPause).is_empty());
        let back = km.to_overrides();
        assert_eq!(back.get("mute").unwrap(), &vec!["Space".to_string()]);
        let mut km = km;
        assert_eq!(km.bind(Action::PlayPause, space), Some(Action::Mute));
        km.reset(Action::Mute);
        assert_eq!(km.get(Action::Mute), &[Combo::parse("M").unwrap()]);
        assert!(km.ordered()[0].0.weight() >= km.ordered().last().unwrap().0.weight());
    }
}
