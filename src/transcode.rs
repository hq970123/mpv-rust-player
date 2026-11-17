//! On-the-fly transcoding for casting.
//!
//! When the TV is unlikely to decode the file (VP9, AV1, 10-bit H.264, DTS,
//! TrueHD, ...), a headless libmpv instance in encoding mode writes an
//! MPEG-TS stream (H.264 + AAC stereo, at most 1080p) to a temp file, and the
//! cast media server streams that file while it grows. Seeking restarts the
//! encoder at the new position. The encoder pauses when it runs far ahead of
//! what the TV has fetched, so disk use stays bounded by playback progress.

use crate::clip::{self, Subs};
use crate::mpv::{Event, Mpv};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Transcode only what the TV probably can't play.
    Auto,
    Always,
    Never,
}

/// What the main player knows about the open file.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    /// Lower-case file extension.
    pub ext: String,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    /// `video-params/pixelformat`, e.g. "yuv420p10".
    pub pixfmt: String,
}

const DIRECT_CONTAINERS: &[&str] = &["mp4", "m4v", "mov", "mkv", "ts", "m2ts", "mts", "mpg", "mpeg", "vob"];
const AUDIO_CONTAINERS: &[&str] = &["mp3", "m4a", "aac", "flac", "wav"];
const DIRECT_VIDEO: &[&str] = &["h264", "hevc", "mpeg2video"];
const DIRECT_AUDIO: &[&str] = &["aac", "mp3", "ac3", "eac3", "mp2"];
const DIRECT_AUDIO_ONLY: &[&str] = &["aac", "mp3", "flac", "pcm_s16le", "pcm_s24le", "alac"];

/// Which kind of receiver the stream is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Dlna,
    Chromecast,
}

const CC_CONTAINERS: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv"];
const CC_AUDIO_CONTAINERS: &[&str] = &["mp3", "m4a", "aac", "flac", "wav", "ogg", "oga", "opus", "webm", "mp4"];
const CC_VIDEO: &[&str] = &["h264", "vp8"];
const CC_AUDIO: &[&str] = &["aac", "mp3", "opus", "vorbis", "flac"];

/// `Some(reason)` when the file should be transcoded for `target`.
pub fn reason(p: &Probe, target: Target) -> Option<String> {
    match target {
        Target::Dlna => reason_dlna(p),
        Target::Chromecast => reason_cast(p),
    }
}

/// Every Chromecast generation: H.264 (8-bit 4:2:0) or VP8 with AAC/MP3/Opus/Vorbis/FLAC
/// in MP4/WebM/MKV. HEVC, VP9 and AV1 only play on newer models, so they are converted.
fn reason_cast(p: &Probe) -> Option<String> {
    let codec = |c: &Option<String>| c.as_deref().unwrap_or("").to_ascii_lowercase();
    let (v, a) = (codec(&p.vcodec), codec(&p.acodec));
    if v.is_empty() {
        if !CC_AUDIO_CONTAINERS.contains(&p.ext.as_str()) {
            return Some(format!("container .{}", p.ext));
        }
        if !a.is_empty() && !CC_AUDIO.contains(&a.as_str()) && !a.starts_with("pcm_") {
            return Some(a);
        }
        return None;
    }
    if !CC_CONTAINERS.contains(&p.ext.as_str()) {
        return Some(format!("container .{}", p.ext));
    }
    if !CC_VIDEO.contains(&v.as_str()) {
        return Some(v);
    }
    let deep =
        p.pixfmt.contains("10") || p.pixfmt.contains("12") || p.pixfmt.contains("444") || p.pixfmt.contains("422");
    if deep {
        return Some(format!("{v} {}", p.pixfmt));
    }
    if !a.is_empty() && !CC_AUDIO.contains(&a.as_str()) {
        return Some(a);
    }
    None
}

fn reason_dlna(p: &Probe) -> Option<String> {
    let codec = |c: &Option<String>| c.as_deref().unwrap_or("").to_ascii_lowercase();
    let (v, a) = (codec(&p.vcodec), codec(&p.acodec));
    if v.is_empty() {
        // Audio only.
        if !AUDIO_CONTAINERS.contains(&p.ext.as_str()) && !DIRECT_CONTAINERS.contains(&p.ext.as_str()) {
            return Some(format!("container .{}", p.ext));
        }
        if !a.is_empty() && !DIRECT_AUDIO_ONLY.contains(&a.as_str()) && !DIRECT_AUDIO.contains(&a.as_str()) {
            return Some(a);
        }
        return None;
    }
    if !DIRECT_CONTAINERS.contains(&p.ext.as_str()) {
        return Some(format!("container .{}", p.ext));
    }
    if !DIRECT_VIDEO.contains(&v.as_str()) {
        return Some(v);
    }
    let deep =
        p.pixfmt.contains("10") || p.pixfmt.contains("12") || p.pixfmt.contains("444") || p.pixfmt.contains("422");
    if v == "h264" && deep {
        return Some(format!("h264 {}", p.pixfmt));
    }
    if v == "mpeg2video" && !["ts", "m2ts", "mts", "mpg", "mpeg", "vob", "mkv"].contains(&p.ext.as_str()) {
        return Some(format!("mpeg2 in .{}", p.ext));
    }
    if !a.is_empty() && !DIRECT_AUDIO.contains(&a.as_str()) {
        return Some(a);
    }
    None
}

#[derive(Clone, Debug)]
pub struct Job {
    pub src: String,
    pub start: f64,
    /// Audio track to keep (main player's selection).
    pub aid: Option<i64>,
    pub subs: Subs,
    pub encoders: Vec<String>,
    pub out: PathBuf,
    /// Fragmented MP4 (for Chromecast) instead of MPEG-TS.
    pub fmp4: bool,
}

/// True when this libmpv build can encode the cast stream at all.
pub fn can_encode(encoders: &[String]) -> bool {
    encoders.is_empty() || clip::H264.iter().any(|(c, _)| encoders.iter().any(|h| h == c))
}

pub fn options(job: &Job) -> Result<Vec<(String, String)>, String> {
    let (venc, vopts) = if job.encoders.is_empty() {
        clip::H264[0]
    } else {
        *clip::H264
            .iter()
            .find(|(c, _)| job.encoders.iter().any(|h| h == c))
            .ok_or("no H.264 encoder in this libmpv build")?
    };
    // Low latency to first byte, regular keyframes for the TV's decoder.
    let vopts = if venc == "libx264" {
        "preset=veryfast,crf=21,g=50,keyint_min=25,bf=0,profile=high,level=4.1".to_string()
    } else if venc == "mpeg4" {
        format!("{vopts},g=50")
    } else {
        "b=10000000,maxrate=14000000,bufsize=20000000,g=50".to_string()
    };
    let mut o: Vec<(String, String)> = vec![
        ("config".into(), "no".into()),
        ("load-scripts".into(), "no".into()),
        ("ytdl".into(), "no".into()),
        ("osd-level".into(), "0".into()),
        ("hwdec".into(), "auto-copy-safe".into()),
        ("keep-open".into(), "no".into()),
        ("idle".into(), "yes".into()),
        ("start".into(), format!("{:.3}", job.start.max(0.0))),
        ("o".into(), job.out.to_string_lossy().to_string()),
        ("of".into(), if job.fmp4 { "mp4" } else { "mpegts" }.into()),
        ("ovc".into(), venc.into()),
        ("ovcopts".into(), vopts),
        (
            "vf".into(),
            clip::lavfi(
                "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2,format=yuv420p",
            ),
        ),
        ("audio-channels".into(), "stereo".into()),
    ];
    if job.fmp4 {
        // Playable while it is still being written.
        o.push(("ofopts".into(), "movflags=+frag_keyframe+empty_moov+default_base_moof".into()));
    }
    let aenc = if job.encoders.is_empty() { Some("aac") } else { clip::pick(&job.encoders, clip::AAC) };
    match aenc {
        Some(a) => {
            o.push(("oac".into(), a.into()));
            o.push(("oacopts".into(), "b=192000".into()));
            if let Some(id) = job.aid {
                o.push(("aid".into(), id.to_string()));
            }
        }
        None => o.push(("aid".into(), "no".into())),
    }
    match &job.subs {
        Subs::None | Subs::File(_) => o.push(("sid".into(), "no".into())),
        Subs::Track(id) => o.push(("sid".into(), id.to_string())),
    }
    Ok(o)
}

/// Folder for transcoded streams; emptied when the app starts.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("okplayer-cast")
}

pub fn clean_temp() {
    let _ = std::fs::remove_dir_all(temp_dir());
}

/// Bytes the encoder may run ahead of what the TV has fetched.
const AHEAD_PAUSE: u64 = 300 << 20;
const AHEAD_RESUME: u64 = 150 << 20;
/// Bytes written before the TV is told to start.
const READY_BYTES: u64 = 1 << 20;

pub struct Transcoder {
    pub out: PathBuf,
    /// Set once the encoder has finished and the file is complete.
    pub done: Arc<AtomicBool>,
    pub error: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

impl Transcoder {
    /// `sent`: highest byte offset the HTTP server has handed out.
    pub fn start(job: Job, sent: Arc<AtomicU64>) -> Self {
        let out = job.out.clone();
        let done = Arc::new(AtomicBool::new(false));
        let error: Arc<Mutex<Option<String>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (d, e, s) = (done.clone(), error.clone(), stop.clone());
        let spawned = std::thread::Builder::new().name("cast-transcode".into()).spawn(move || {
            if let Err(err) = run(&job, &sent, &s) {
                *e.lock().unwrap() = Some(err);
            }
            d.store(true, Ordering::SeqCst);
            if s.load(Ordering::SeqCst) {
                remove_later(job.out);
            }
        });
        if let Err(err) = spawned {
            *error.lock().unwrap() = Some(err.to_string());
            done.store(true, Ordering::SeqCst);
        }
        Self { out, done, error, stop }
    }

    /// Block until enough of the stream exists to hand to the TV.
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let t0 = Instant::now();
        loop {
            if let Some(e) = self.error.lock().unwrap().clone() {
                return Err(e);
            }
            let size = std::fs::metadata(&self.out).map(|m| m.len()).unwrap_or(0);
            if size >= READY_BYTES || (self.done.load(Ordering::SeqCst) && size > 0) {
                return Ok(());
            }
            if self.done.load(Ordering::SeqCst) {
                return Err("transcoder produced no output".into());
            }
            if t0.elapsed() > timeout {
                return Err("transcoding too slow to start".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Transcoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if self.done.load(Ordering::SeqCst) {
            remove_later(self.out.clone());
        }
    }
}

/// The HTTP server may still hold the file open (Windows can't delete it then).
fn remove_later(p: PathBuf) {
    std::thread::spawn(move || {
        for _ in 0..30 {
            if std::fs::remove_file(&p).is_ok() || !p.exists() {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn run(job: &Job, sent: &AtomicU64, stop: &AtomicBool) -> Result<(), String> {
    if let Some(dir) = job.out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let opts = options(job)?;
    let pairs: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mpv = Mpv::new(&pairs)?;
    if let Subs::File(f) = &job.subs {
        let _ = mpv.set("pause", "yes");
        mpv.command(&["loadfile", &job.src])?;
        let t0 = Instant::now();
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            match mpv.wait_event(0.25) {
                Some(Event::FileLoaded) => break,
                Some(Event::EndFile { .. }) => return Err("could not open the file".into()),
                _ if t0.elapsed() > Duration::from_secs(30) => return Err("timed out opening the file".into()),
                _ => {}
            }
        }
        let _ = mpv.command(&["sub-add", &f.to_string_lossy(), "select"]);
        let _ = mpv.set("pause", "no");
    } else {
        mpv.command(&["loadfile", &job.src])?;
    }
    let mut paused = false;
    loop {
        if stop.load(Ordering::SeqCst) {
            let _ = mpv.command(&["stop"]);
            return Ok(());
        }
        match mpv.wait_event(0.25) {
            Some(Event::EndFile { reason, error }) => {
                if reason == 4 || error < 0 {
                    return Err(format!("transcode failed ({error})"));
                }
                break;
            }
            Some(Event::Shutdown) => break,
            _ => {}
        }
        let size = std::fs::metadata(&job.out).map(|m| m.len()).unwrap_or(0);
        let ahead = size.saturating_sub(sent.load(Ordering::Relaxed));
        if !paused && ahead > AHEAD_PAUSE {
            paused = mpv.set("pause", "yes").is_ok();
        } else if paused && ahead < AHEAD_RESUME {
            paused = mpv.set("pause", "no").is_err();
        }
    }
    // Destroying the handle flushes and closes the muxer.
    drop(mpv);
    Ok(())
}

// ------------------------------------------------------------------ SRT shift

fn srt_time(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let mut t = 0.0;
    for p in s.split(':') {
        t = t * 60.0 + p.parse::<f64>().ok()?;
    }
    Some(t)
}

fn fmt_srt(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Move every cue `by` seconds earlier, dropping cues that end before 0 and
/// renumbering the rest. Used when a transcoded stream starts mid-file.
/// SRT to WebVTT (what Chromecast reads).
pub fn srt_to_vtt(srt: &str) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for line in srt.replace("\r\n", "\n").lines() {
        if line.contains("-->") {
            out.push_str(&line.replace(',', "."));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

pub fn shift_srt(srt: &str, by: f64) -> String {
    if by <= 0.0 {
        return srt.to_string();
    }
    let text = srt.replace("\r\n", "\n");
    let mut out = String::new();
    let mut n = 0;
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let Some((a, b)) = lines[ti].split_once("-->") else { continue };
        let (Some(a), Some(b)) = (srt_time(a), srt_time(b.split_whitespace().next().unwrap_or(""))) else { continue };
        let (a, b) = (a - by, b - by);
        if b <= 0.0 {
            continue;
        }
        n += 1;
        out.push_str(&format!("{n}\n{} --> {}\n", fmt_srt(a), fmt_srt(b)));
        for l in &lines[ti + 1..] {
            out.push_str(l);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(ext: &str, v: Option<&str>, a: Option<&str>, pix: &str) -> Probe {
        Probe { ext: ext.into(), vcodec: v.map(Into::into), acodec: a.map(Into::into), pixfmt: pix.into() }
    }

    #[test]
    fn decides() {
        assert_eq!(reason_dlna(&probe("mp4", Some("h264"), Some("aac"), "yuv420p")), None);
        assert_eq!(reason_dlna(&probe("mkv", Some("hevc"), Some("eac3"), "yuv420p10")), None);
        assert!(reason_dlna(&probe("mkv", Some("vp9"), Some("opus"), "yuv420p")).is_some());
        assert!(reason_dlna(&probe("mkv", Some("h264"), Some("dts"), "yuv420p")).is_some());
        assert!(reason_dlna(&probe("mkv", Some("h264"), Some("aac"), "yuv420p10")).is_some());
        assert!(reason_dlna(&probe("avi", Some("mpeg4"), Some("mp3"), "yuv420p")).is_some());
        assert!(reason_dlna(&probe("webm", Some("av1"), None, "yuv420p")).is_some());
        assert_eq!(reason_dlna(&probe("flac", None, Some("flac"), "")), None);
        assert!(reason_dlna(&probe("ogg", None, Some("vorbis"), "")).is_some());
    }

    #[test]
    fn decides_for_chromecast() {
        let cc = |e, v, a, x| reason(&probe(e, v, a, x), Target::Chromecast);
        assert_eq!(cc("mp4", Some("h264"), Some("aac"), "yuv420p"), None);
        assert_eq!(cc("mkv", Some("h264"), Some("opus"), "yuv420p"), None);
        assert_eq!(cc("webm", Some("vp8"), Some("vorbis"), "yuv420p"), None);
        assert!(cc("mkv", Some("hevc"), Some("aac"), "yuv420p").is_some());
        assert!(cc("webm", Some("vp9"), Some("opus"), "yuv420p").is_some());
        assert!(cc("mp4", Some("h264"), Some("ac3"), "yuv420p").is_some());
        assert!(cc("ts", Some("h264"), Some("aac"), "yuv420p").is_some());
        assert!(cc("mp4", Some("h264"), Some("aac"), "yuv420p10").is_some());
        assert_eq!(cc("ogg", None, Some("vorbis"), ""), None);
        assert!(cc("wma", None, Some("wmav2"), "").is_some());
    }

    #[test]
    fn builds_options() {
        let job = Job {
            src: "/v/a.mkv".into(),
            start: 61.5,
            aid: Some(2),
            subs: Subs::Track(3),
            encoders: vec!["h264_videotoolbox".into(), "aac_at".into()],
            out: "/tmp/x.ts".into(),
            fmp4: false,
        };
        let o = options(&job).unwrap();
        let get = |k: &str| o.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        assert_eq!(get("of").as_deref(), Some("mpegts"));
        assert_eq!(get("ovc").as_deref(), Some("h264_videotoolbox"));
        assert_eq!(get("oac").as_deref(), Some("aac_at"));
        assert_eq!(get("start").as_deref(), Some("61.500"));
        assert_eq!(get("aid").as_deref(), Some("2"));
        assert_eq!(get("sid").as_deref(), Some("3"));
        assert!(!can_encode(&["gif".into()]));
        let o = options(&Job { fmp4: true, ..job.clone() }).unwrap();
        assert!(o.iter().any(|(k, v)| k == "of" && v == "mp4"));
        assert!(o.iter().any(|(k, v)| k == "ofopts" && v.contains("empty_moov")));
        assert!(options(&Job { encoders: vec!["gif".into()], ..job }).is_err());
    }

    #[test]
    fn shifts_srt() {
        let srt = "1\r\n00:00:01,000 --> 00:00:02,000\r\ngone\r\n\r\n2\r\n00:01:00,500 --> 00:01:02,000\r\nhello\r\nworld\r\n\r\n";
        let s = shift_srt(srt, 30.0);
        assert_eq!(s, "1\n00:00:30,500 --> 00:00:32,000\nhello\nworld\n\n");
        assert_eq!(shift_srt(srt, 0.0), srt);
        assert_eq!(
            srt_to_vtt("1\n00:00:01,000 --> 00:00:02,500\nhi\n"),
            "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.500\nhi\n"
        );
    }
}
