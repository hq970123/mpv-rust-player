//! Live speech recognition and translated subtitles.
//!
//! Pipeline (all off the UI thread):
//!   audio tap  — a second, headless mpv core decodes the same file's audio
//!                ahead of playback into 16 kHz mono s16 PCM (ao=pcm),
//!   VAD        — energy-based segmentation into utterances (<= 10 s),
//!   ASR        — whisper.cpp (whisper-rs) on each utterance,
//!   translate  — OpenAI-compatible chat API (Bilibili Index-Translate by
//!                default, no key) on a small worker pool,
//!   display    — cues with absolute media timestamps, drawn by the UI on top
//!                of the video in original / translated / bilingual mode.

use crate::mpv::Mpv;
use crate::settings::{Provider, Settings};
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SAMPLE_RATE: usize = 16_000;
const BYTES_PER_SEC: f64 = (SAMPLE_RATE * 2) as f64;
/// How far the audio tap may run ahead of playback.
const MAX_LEAD_SECS: f64 = 40.0;

#[derive(Debug, Clone)]
pub struct Cue {
    pub id: u64,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub trans: Option<String>,
}

#[derive(Default)]
pub struct Shared {
    pub cues: Mutex<Vec<Cue>>,
    pub status: Mutex<Status>,
    player_time: AtomicU64,
    pub ahead_secs: AtomicU64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    LoadingModel,
    Listening,
    Error(String),
}

impl Shared {
    pub fn set_player_time(&self, t: f64) {
        self.player_time.store(t.to_bits(), Ordering::Relaxed);
    }
    pub fn player_time(&self) -> f64 {
        f64::from_bits(self.player_time.load(Ordering::Relaxed))
    }
    fn set_status(&self, s: Status) {
        *self.status.lock().unwrap() = s;
    }

    /// The cue to show at time `t`, if any.
    pub fn cue_at(&self, t: f64) -> Option<Cue> {
        let cues = self.cues.lock().unwrap();
        cues.iter().rev().find(|c| c.start <= t && t < c.end).cloned()
    }

    pub fn to_srt(&self, translated: bool, bilingual: bool) -> String {
        let cues = self.cues.lock().unwrap();
        let mut out = String::new();
        for (i, c) in cues.iter().enumerate() {
            let body = match (bilingual, translated, &c.trans) {
                (true, _, Some(t)) => format!("{}\n{}", c.text, t),
                (false, true, Some(t)) => t.clone(),
                _ => c.text.clone(),
            };
            out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, srt_time(c.start), srt_time(c.end), body));
        }
        out
    }
}

fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

#[derive(Clone)]
pub struct Config {
    pub model_path: Option<PathBuf>,
    pub source_lang: String,
    pub target_lang: String,
    pub provider: Provider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Config {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            model_path: crate::models::model_path(&s.model).filter(|p| p.is_file()),
            source_lang: s.source_lang.clone(),
            target_lang: s.target_lang.clone(),
            provider: s.provider,
            base_url: s.openai_base_url.clone(),
            api_key: s.openai_api_key.clone(),
            model: s.openai_model.clone(),
        }
    }
}

enum Cmd {
    Start { media: String, pos: f64, aid: Option<String>, new_media: bool },
    Stop,
    Config(Config),
}

pub struct Live {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
    pub running: Arc<AtomicBool>,
}

impl Live {
    pub fn new(cfg: Config, ctx: eframe::egui::Context) -> Self {
        let (tx, rx) = channel();
        let shared = Arc::new(Shared::default());
        let running = Arc::new(AtomicBool::new(false));
        let (s, r) = (shared.clone(), running.clone());
        std::thread::Builder::new()
            .name("live-asr".into())
            .spawn(move || Worker::new(cfg, s, r, ctx).run(rx))
            .expect("spawn live worker");
        Self { tx, shared, running }
    }
    /// Start (or restart after a seek / track change) at `pos` seconds.
    pub fn start(&self, media: &str, pos: f64, aid: Option<String>, new_media: bool) {
        let _ = self.tx.send(Cmd::Start { media: media.to_string(), pos, aid, new_media });
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    pub fn configure(&self, cfg: Config) {
        let _ = self.tx.send(Cmd::Config(cfg));
    }
}

// ---------------------------------------------------------------- audio tap

struct Tap {
    mpv: Mpv,
    file: PathBuf,
    reader: Option<std::fs::File>,
    offset: u64,
    start: f64,
    odd: Option<u8>,
    paused: bool,
}

impl Tap {
    fn new(media: &str, pos: f64, aid: Option<&str>) -> Result<Self, String> {
        static N: AtomicU64 = AtomicU64::new(0);
        let file = std::env::temp_dir().join(format!(
            "ok-player-{}-{}.pcm",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&file);
        let file_s = file.to_string_lossy().to_string();
        let start_s = format!("{pos:.3}");
        let opts: Vec<(&str, &str)> = vec![
            ("config", "no"),
            ("terminal", "no"),
            ("msg-level", "all=no"),
            ("idle", "yes"),
            ("keep-open", "yes"),
            ("vid", "no"),
            ("sid", "no"),
            ("audio-display", "no"),
            ("load-scripts", "no"),
            ("ytdl", "no"),
            ("ao", "pcm"),
            ("ao-pcm-file", &file_s),
            ("ao-pcm-waveheader", "no"),
            ("audio-samplerate", "16000"),
            ("audio-channels", "mono"),
            ("audio-format", "s16"),
            ("volume", "100"),
            ("start", &start_s),
            ("aid", aid.unwrap_or("auto")),
        ];
        let mpv = Mpv::new(&opts)?;
        mpv.command(&["loadfile", media])?;
        Ok(Self { mpv, file, reader: None, offset: 0, start: pos, odd: None, paused: false })
    }

    /// Media time of the next sample to be read.
    fn time(&self) -> f64 {
        self.start + self.offset as f64 / BYTES_PER_SEC
    }

    fn read(&mut self) -> Vec<f32> {
        if self.reader.is_none() {
            self.reader = std::fs::File::open(&self.file).ok();
        }
        let Some(r) = self.reader.as_mut() else { return Vec::new() };
        let mut bytes = Vec::new();
        if r.seek(SeekFrom::Start(self.offset)).is_err() || r.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        self.offset += bytes.len() as u64;
        if let Some(b) = self.odd.take() {
            bytes.insert(0, b);
        }
        if bytes.len() % 2 == 1 {
            self.odd = bytes.pop();
        }
        bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect()
    }

    fn set_paused(&mut self, p: bool) {
        if self.paused != p && self.mpv.set("pause", if p { "yes" } else { "no" }).is_ok() {
            self.paused = p;
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        let _ = self.mpv.command(&["stop"]);
        self.reader = None;
        let _ = std::fs::remove_file(&self.file);
    }
}

// ---------------------------------------------------------------- VAD

const FRAME: usize = 480; // 30 ms
const MAX_SEG: usize = SAMPLE_RATE * 10;
const MIN_SEG: usize = SAMPLE_RATE * 2 / 5;
const END_SILENCE: usize = SAMPLE_RATE / 2;
const PRE_ROLL: usize = SAMPLE_RATE / 5;

struct Vad {
    frame: Vec<f32>,
    frame_t: f64,
    seg: Vec<f32>,
    seg_t: f64,
    in_speech: bool,
    silence: usize,
    noise: f32,
    pre: VecDeque<f32>,
}

impl Vad {
    fn new() -> Self {
        Self {
            frame: Vec::with_capacity(FRAME),
            frame_t: 0.0,
            seg: Vec::new(),
            seg_t: 0.0,
            in_speech: false,
            silence: 0,
            noise: 0.005,
            pre: VecDeque::new(),
        }
    }

    fn reset(&mut self) {
        let noise = self.noise;
        *self = Self::new();
        self.noise = noise;
    }

    /// Feed samples starting at media time `t`; returns finished utterances.
    fn push(&mut self, samples: &[f32], mut t: f64) -> Vec<(f64, Vec<f32>)> {
        let mut out = Vec::new();
        if self.frame.is_empty() {
            self.frame_t = t;
        }
        for &s in samples {
            self.frame.push(s);
            t += 1.0 / SAMPLE_RATE as f64;
            if self.frame.len() == FRAME {
                let frame = std::mem::take(&mut self.frame);
                self.on_frame(&frame, &mut out);
                self.frame = frame;
                self.frame.clear();
                self.frame_t = t;
            }
        }
        out
    }

    fn on_frame(&mut self, f: &[f32], out: &mut Vec<(f64, Vec<f32>)>) {
        let rms = (f.iter().map(|x| x * x).sum::<f32>() / f.len() as f32).sqrt();
        let threshold = (self.noise * 3.0).max(0.008);
        let speech = rms > threshold;
        if !speech {
            // slow-moving noise floor estimate
            self.noise = self.noise * 0.95 + rms * 0.05;
        }
        if self.in_speech {
            self.seg.extend_from_slice(f);
            if speech {
                self.silence = 0;
            } else {
                self.silence += f.len();
            }
            if self.silence >= END_SILENCE || self.seg.len() >= MAX_SEG {
                let seg = std::mem::take(&mut self.seg);
                if seg.len() - self.silence.min(seg.len()) >= MIN_SEG {
                    out.push((self.seg_t, seg));
                }
                self.in_speech = false;
                self.silence = 0;
                if speech {
                    // hard cut in the middle of speech: continue straight on
                    self.in_speech = true;
                    self.seg_t = self.frame_t + FRAME as f64 / SAMPLE_RATE as f64;
                }
            }
        } else if speech {
            self.in_speech = true;
            self.silence = 0;
            self.seg = self.pre.drain(..).collect();
            self.seg_t = self.frame_t - self.seg.len() as f64 / SAMPLE_RATE as f64;
            self.seg.extend_from_slice(f);
        } else {
            self.pre.extend(f.iter().copied());
            while self.pre.len() > PRE_ROLL {
                self.pre.pop_front();
            }
        }
    }
}

// ---------------------------------------------------------------- worker

struct TransJob {
    id: u64,
    text: String,
    cfg: Config,
}

struct Worker {
    cfg: Config,
    shared: Arc<Shared>,
    running: Arc<AtomicBool>,
    ctx: eframe::egui::Context,
    whisper: Option<(PathBuf, whisper_rs::WhisperContext)>,
    tap: Option<Tap>,
    vad: Vad,
    next_id: u64,
    trans_tx: Sender<TransJob>,
    media: String,
}

impl Worker {
    fn new(cfg: Config, shared: Arc<Shared>, running: Arc<AtomicBool>, ctx: eframe::egui::Context) -> Self {
        let (trans_tx, trans_rx) = channel::<TransJob>();
        let trans_rx = Arc::new(Mutex::new(trans_rx));
        for i in 0..3 {
            let (rx, sh, cx) = (trans_rx.clone(), shared.clone(), ctx.clone());
            std::thread::Builder::new()
                .name(format!("translate-{i}"))
                .spawn(move || translator(rx, sh, cx))
                .expect("spawn translator");
        }
        Self {
            cfg,
            shared,
            running,
            ctx,
            whisper: None,
            tap: None,
            vad: Vad::new(),
            next_id: 1,
            trans_tx,
            media: String::new(),
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            match rx.recv_timeout(Duration::from_millis(if self.tap.is_some() { 40 } else { 500 })) {
                Ok(Cmd::Start { media, pos, aid, new_media }) => {
                    if new_media || media != self.media {
                        self.shared.cues.lock().unwrap().clear();
                    }
                    self.media = media.clone();
                    self.tap = None;
                    self.vad.reset();
                    if !self.ensure_model() {
                        continue;
                    }
                    match Tap::new(&media, pos, aid.as_deref()) {
                        Ok(t) => {
                            self.tap = Some(t);
                            self.running.store(true, Ordering::Relaxed);
                            self.shared.set_status(Status::Listening);
                        }
                        Err(e) => self.shared.set_status(Status::Error(e)),
                    }
                }
                Ok(Cmd::Stop) => {
                    self.tap = None;
                    self.running.store(false, Ordering::Relaxed);
                    self.shared.set_status(Status::Idle);
                }
                Ok(Cmd::Config(c)) => self.cfg = c,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.tick();
        }
    }

    fn ensure_model(&mut self) -> bool {
        let Some(path) = self.cfg.model_path.clone() else {
            self.shared.set_status(Status::Error("no speech model downloaded".into()));
            return false;
        };
        if self.whisper.as_ref().map(|(p, _)| p == &path).unwrap_or(false) {
            return true;
        }
        self.shared.set_status(Status::LoadingModel);
        self.ctx.request_repaint();
        self.whisper = None;
        let params = whisper_rs::WhisperContextParameters::default();
        match whisper_rs::WhisperContext::new_with_params(&path.to_string_lossy(), params) {
            Ok(c) => {
                self.whisper = Some((path, c));
                true
            }
            Err(e) => {
                self.shared.set_status(Status::Error(format!("model load failed: {e}")));
                false
            }
        }
    }

    fn tick(&mut self) {
        let Some(tap) = self.tap.as_mut() else { return };
        let now = self.shared.player_time();
        let t0 = tap.time();
        let mut samples = tap.read();
        let ahead = tap.time() - now;
        self.shared.ahead_secs.store(ahead.max(0.0).to_bits(), Ordering::Relaxed);
        tap.set_paused(ahead > MAX_LEAD_SECS);
        if samples.is_empty() {
            return;
        }
        // Recognition fell behind playback: drop audio that is already past.
        let mut start = t0;
        let behind = now - 1.0 - t0;
        if behind > 0.0 {
            let skip = ((behind * SAMPLE_RATE as f64) as usize).min(samples.len());
            samples.drain(..skip);
            start += skip as f64 / SAMPLE_RATE as f64;
            self.vad.reset();
        }
        let segments = self.vad.push(&samples, start);
        for (seg_t, seg) in segments {
            let seg_end = seg_t + seg.len() as f64 / SAMPLE_RATE as f64;
            if seg_end < self.shared.player_time() - 1.5 {
                continue; // stale
            }
            self.transcribe(seg_t, &seg);
        }
    }

    fn transcribe(&mut self, seg_t: f64, pcm: &[f32]) {
        use whisper_rs::{FullParams, SamplingStrategy};
        let Some((_, ctx)) = self.whisper.as_ref() else { return };
        let Ok(mut state) = ctx.create_state() else { return };
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
        p.set_n_threads(threads as i32);
        p.set_language(Some(self.cfg.source_lang.as_str()));
        p.set_translate(self.cfg.provider == Provider::WhisperToEnglish);
        p.set_no_context(true);
        p.set_suppress_blank(true);
        p.set_no_speech_thold(0.6);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        // Pad very short clips to 1 s; whisper dislikes tiny inputs.
        let mut audio = pcm.to_vec();
        if audio.len() < SAMPLE_RATE {
            audio.resize(SAMPLE_RATE, 0.0);
        }
        if state.full(p, &audio).is_err() {
            return;
        }
        let n = state.full_n_segments().unwrap_or(0);
        let seg_len = pcm.len() as f64 / SAMPLE_RATE as f64;
        let mut new_cues = Vec::new();
        for i in 0..n {
            let Ok(text) = state.full_get_segment_text_lossy(i) else { continue };
            let text = text.trim().to_string();
            if is_noise(&text) {
                continue;
            }
            let a = state.full_get_segment_t0(i).unwrap_or(0) as f64 / 100.0;
            let b = state.full_get_segment_t1(i).unwrap_or(0) as f64 / 100.0;
            let start = seg_t + a.min(seg_len);
            let end = (seg_t + b.min(seg_len)).max(start + 1.2);
            let id = self.next_id;
            self.next_id += 1;
            new_cues.push(Cue { id, start, end, text, trans: None });
        }
        if new_cues.is_empty() {
            return;
        }
        {
            let mut cues = self.shared.cues.lock().unwrap();
            for c in &new_cues {
                // trim the previous cue so lines don't overlap
                if let Some(prev) = cues.iter_mut().rev().find(|p| p.start < c.start) {
                    if prev.end > c.start {
                        prev.end = c.start;
                    }
                }
                let pos = cues.partition_point(|p| p.start <= c.start);
                cues.insert(pos, c.clone());
            }
            if cues.len() > 5000 {
                let excess = cues.len() - 5000;
                cues.drain(..excess);
            }
        }
        if matches!(self.cfg.provider, Provider::Bilibili | Provider::OpenAiCompatible) {
            for c in new_cues {
                let _ = self.trans_tx.send(TransJob { id: c.id, text: c.text, cfg: self.cfg.clone() });
            }
        }
        self.ctx.request_repaint();
    }
}

fn is_noise(t: &str) -> bool {
    if t.chars().all(|c| !c.is_alphanumeric()) {
        return true;
    }
    let l = t.to_lowercase();
    const JUNK: &[&str] = &[
        "[music]", "(music)", "[音乐]", "[blank_audio]", "(applause)", "thank you for watching",
        "thanks for watching", "amara.org", "字幕由", "请不吝点赞", "订阅", "subtitles by",
    ];
    JUNK.iter().any(|j| l.contains(j))
}

// ---------------------------------------------------------------- translation

fn translator(rx: Arc<Mutex<Receiver<TransJob>>>, shared: Arc<Shared>, ctx: eframe::egui::Context) {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("http client");
    loop {
        let job = match rx.lock().unwrap().recv() {
            Ok(j) => j,
            Err(_) => return,
        };
        let result = translate(&client, &job.cfg, &job.text);
        let trans = match result {
            Ok(t) => t,
            Err(e) => format!("⚠ {e}"),
        };
        let mut cues = shared.cues.lock().unwrap();
        if let Some(c) = cues.iter_mut().find(|c| c.id == job.id) {
            c.trans = Some(trans);
        }
        drop(cues);
        ctx.request_repaint();
    }
}

pub fn translate(client: &reqwest::blocking::Client, cfg: &Config, text: &str) -> Result<String, String> {
    let (base, model, key, bilibili) = match cfg.provider {
        Provider::Bilibili => (
            "https://index-translate.bilibili.com/v1".to_string(),
            "Index-Translate-35B-A3B".to_string(),
            String::new(),
            true,
        ),
        _ => (cfg.base_url.trim_end_matches('/').to_string(), cfg.model.clone(), cfg.api_key.clone(), false),
    };
    let lang = crate::settings::target_prompt_name(&cfg.target_lang);
    let mut body = serde_json::json!({
        "model": model,
        "temperature": 0.2,
        "messages": [
            {"role": "system", "content": format!(
                "You translate video subtitles. Translate the user's subtitle line into {lang}. \
                 Output only the translation, with no quotes, notes or explanations.")},
            {"role": "user", "content": text}
        ]
    });
    if bilibili {
        body["enable_thinking"] = serde_json::json!(false);
    }
    let mut req = client.post(format!("{base}/chat/completions")).json(&body);
    req = req.header("User-Agent", if bilibili { "Index-Translate-Client/1.0" } else { "OKPlayer/0.1" });
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let v: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let msg = v["error"]["message"].as_str().unwrap_or("request failed");
        return Err(format!("HTTP {status}: {msg}"));
    }
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| "empty response".into())
}
