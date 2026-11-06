//! Lucide icon font (lucide.dev, ISC licence) plus system CJK fallback fonts.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

pub const PLAY: &str = "\u{e13c}";
pub const PAUSE: &str = "\u{e12e}";
pub const SKIP_BACK: &str = "\u{e15f}";
pub const SKIP_FORWARD: &str = "\u{e160}";
pub const VOLUME: &str = "\u{e1ab}";
pub const VOLUME_LOW: &str = "\u{e1aa}";
pub const VOLUME_X: &str = "\u{e1ac}";
pub const MAXIMIZE: &str = "\u{e112}";
pub const MINIMIZE: &str = "\u{e11a}";
pub const CAPTIONS: &str = "\u{e3a4}";
pub const LANGUAGES: &str = "\u{e0fe}";
pub const SETTINGS: &str = "\u{e154}";
pub const FOLDER_OPEN: &str = "\u{e247}";
pub const DOWNLOAD: &str = "\u{e0b2}";
pub const TRASH: &str = "\u{e18e}";
pub const AUDIO_LINES: &str = "\u{e55a}";
pub const GAUGE: &str = "\u{e1bf}";
pub const X: &str = "\u{e1b2}";
pub const CHECK: &str = "\u{e06c}";
pub const FILM: &str = "\u{e0d0}";
pub const CPU: &str = "\u{e0a9}";
pub const SAVE: &str = "\u{e14d}";
pub const CAMERA: &str = "\u{e064}";
pub const CHART: &str = "\u{e068}";
pub const MONITOR: &str = "\u{e11d}";
pub const PIP: &str = "\u{e3af}";
pub const WIN_MIN: &str = "\u{e11c}";
pub const WIN_MAX: &str = "\u{e167}";
pub const WIN_RESTORE: &str = "\u{e09e}";
pub const CAST: &str = "\u{e066}";
pub const COPY: &str = "\u{e225}";
pub const REFRESH: &str = "\u{e145}";
pub const TV: &str = "\u{e195}";
pub const STOP: &str = "\u{e083}";
pub const FOLDER: &str = "\u{e0d7}";

pub fn cjk_font_paths() -> &'static [(&'static str, u32)] {
    if cfg!(windows) {
        &[
            ("C:\\Windows\\Fonts\\msyh.ttc", 0),
            ("C:\\Windows\\Fonts\\msyh.ttf", 0),
            ("C:\\Windows\\Fonts\\simhei.ttf", 0),
            ("C:\\Windows\\Fonts\\YuGothM.ttc", 0),
            ("C:\\Windows\\Fonts\\malgun.ttf", 0),
            ("C:\\Windows\\Fonts\\seguisym.ttf", 0),
        ]
    } else if cfg!(target_os = "macos") {
        &[
            ("/System/Library/Fonts/PingFang.ttc", 0),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
            ("/System/Library/Fonts/STHeiti Medium.ttc", 0),
            ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
            ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
        ]
    } else {
        &[
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
            ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
        ]
    }
}

pub fn family() -> FontFamily {
    FontFamily::Name("icons".into())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("lucide".into(), std::sync::Arc::new(FontData::from_static(include_bytes!("../assets/lucide.ttf"))));
    fonts.families.insert(family(), vec!["lucide".into()]);

    // CJK (and Korean) glyphs from the OS so the UI and subtitles can show them.
    let candidates = cjk_font_paths();
    for (i, (path, index)) in candidates.iter().enumerate() {
        if let Ok(bytes) = std::fs::read(path) {
            let name = format!("sys{i}");
            let mut fd = FontData::from_owned(bytes);
            fd.index = *index;
            fonts.font_data.insert(name.clone(), std::sync::Arc::new(fd));
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push(name.clone());
            }
        }
    }
    // Lucide right after the primary font, so its private-use glyphs win over
    // egui's bundled emoji-icon font and icons can be mixed into menu labels.
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = fonts.families.entry(fam).or_default();
        let at = 1.min(list.len());
        list.insert(at, "lucide".into());
    }
    ctx.set_fonts(fonts);
}
