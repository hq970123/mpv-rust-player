use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const APP_DIR: &str = "OKPlayer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiLang {
    Zh,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    /// Bilibili Index-Translate, OpenAI-compatible, no key.
    Bilibili,
    /// Any OpenAI-compatible endpoint (OpenAI, DeepSeek, Ollama, ...).
    OpenAiCompatible,
    /// Whisper's built-in translation, offline, English target only.
    WhisperToEnglish,
    /// Transcribe only.
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubMode {
    Original,
    Translated,
    Bilingual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopMode {
    Off,
    One,
    All,
    Shuffle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ui_lang: UiLang,
    /// mpv `hwdec` value: "auto-safe", "no", "auto-copy", "d3d11va", "nvdec", "videotoolbox", ...
    pub hwdec: String,
    pub volume: f64,
    pub quality: u32,
    pub model: String,
    pub source_lang: String,
    pub target_lang: String,
    pub provider: Provider,
    pub openai_base_url: String,
    pub openai_api_key: String,
    pub openai_model: String,
    pub sub_mode: SubMode,
    pub sub_font_size: f32,
    /// Download models through hf-mirror.com (faster in mainland China).
    pub use_mirror: bool,
    pub live_enabled: bool,
    /// Snapshot file format: "png", "jpg" or "webp".
    pub shot_format: String,
    pub shot_jpeg_quality: u8,
    /// Custom snapshot folder; empty = Pictures/OK Player.
    pub shot_dir: String,
    /// Default for S: include subtitles (file + live) in the snapshot.
    pub shot_with_subs: bool,
    /// Copy every snapshot to the clipboard as well.
    pub shot_copy: bool,
    /// Send live / loaded subtitles along when casting.
    pub cast_subs: bool,
    pub loop_mode: LoopMode,
    /// Resume files where they were left.
    pub resume: bool,
    /// Audio EQ preset: off / bass / treble / vocal / night.
    pub audio_eq: String,
    pub audio_norm: bool,
    /// Open files in the running window instead of a new one.
    pub single_instance: bool,
    pub seek_thumbs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_lang: UiLang::Zh,
            hwdec: "auto-safe".into(),
            volume: 100.0,
            quality: 0,
            model: "base".into(),
            source_lang: "auto".into(),
            target_lang: "zh".into(),
            provider: Provider::Bilibili,
            openai_base_url: "https://api.openai.com/v1".into(),
            openai_api_key: String::new(),
            openai_model: "gpt-4o-mini".into(),
            sub_mode: SubMode::Bilingual,
            sub_font_size: 28.0,
            use_mirror: false,
            live_enabled: false,
            shot_format: "png".into(),
            shot_jpeg_quality: 92,
            shot_dir: String::new(),
            shot_with_subs: false,
            shot_copy: false,
            cast_subs: true,
            loop_mode: LoopMode::Off,
            resume: true,
            audio_eq: "off".into(),
            audio_norm: false,
            single_instance: true,
            seek_thumbs: true,
        }
    }
}

pub fn data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    let d = base.join(APP_DIR);
    // Renamed from "FilmoraPlayer": carry over settings, history and models.
    let old = base.join("FilmoraPlayer");
    if !d.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &d);
    }
    let _ = std::fs::create_dir_all(&d);
    d
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(settings_path(), b);
        }
    }
}

/// Hardware decoding choices offered in settings for this platform.
pub fn hwdec_choices() -> Vec<(&'static str, &'static str)> {
    #[allow(unused_mut)]
    let mut v = vec![("auto-safe", "Auto"), ("no", "Off (software)"), ("auto-copy", "Auto (copy-back)")];
    #[cfg(windows)]
    v.extend([
        ("d3d11va", "D3D11VA"),
        ("d3d11va-copy", "D3D11VA (copy)"),
        ("nvdec", "NVDEC"),
        ("nvdec-copy", "NVDEC (copy)"),
        ("dxva2-copy", "DXVA2 (copy)"),
    ]);
    #[cfg(target_os = "macos")]
    v.extend([("videotoolbox", "VideoToolbox"), ("videotoolbox-copy", "VideoToolbox (copy)")]);
    v
}

/// Speech languages Whisper can be told to expect.
pub const SOURCE_LANGS: &[(&str, &str, &str)] = &[
    ("auto", "自动检测", "Auto detect"),
    ("en", "英语", "English"),
    ("zh", "中文", "Chinese"),
    ("ja", "日语", "Japanese"),
    ("ko", "韩语", "Korean"),
    ("fr", "法语", "French"),
    ("de", "德语", "German"),
    ("es", "西班牙语", "Spanish"),
    ("ru", "俄语", "Russian"),
    ("pt", "葡萄牙语", "Portuguese"),
    ("it", "意大利语", "Italian"),
    ("ar", "阿拉伯语", "Arabic"),
    ("hi", "印地语", "Hindi"),
    ("th", "泰语", "Thai"),
    ("vi", "越南语", "Vietnamese"),
    ("id", "印尼语", "Indonesian"),
];

/// Subtitle translation targets: (code, zh label, en label, name used in the prompt).
pub const TARGET_LANGS: &[(&str, &str, &str, &str)] = &[
    ("zh", "简体中文", "Chinese (Simplified)", "Simplified Chinese"),
    ("zh-Hant", "繁體中文", "Chinese (Traditional)", "Traditional Chinese"),
    ("en", "英语", "English", "English"),
    ("ja", "日语", "Japanese", "Japanese"),
    ("ko", "韩语", "Korean", "Korean"),
    ("fr", "法语", "French", "French"),
    ("de", "德语", "German", "German"),
    ("es", "西班牙语", "Spanish", "Spanish"),
    ("ru", "俄语", "Russian", "Russian"),
    ("pt", "葡萄牙语", "Portuguese", "Portuguese"),
    ("it", "意大利语", "Italian", "Italian"),
    ("ar", "阿拉伯语", "Arabic", "Arabic"),
    ("th", "泰语", "Thai", "Thai"),
    ("vi", "越南语", "Vietnamese", "Vietnamese"),
    ("id", "印尼语", "Indonesian", "Indonesian"),
];

pub fn target_prompt_name(code: &str) -> &'static str {
    TARGET_LANGS.iter().find(|t| t.0 == code).map(|t| t.3).unwrap_or("Simplified Chinese")
}
