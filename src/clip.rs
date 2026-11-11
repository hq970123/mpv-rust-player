//! Clip and GIF export.
//!
//! A separate headless libmpv instance runs in mpv's encoding mode (`o=`),
//! so playback in the window is not disturbed. Range is start/end in seconds;
//! subtitles are burned in by vo_lavc when a subtitle track is selected.

use crate::mpv::{Event, Mpv};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Mp4,
    Gif,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Mp4 => "mp4",
            Format::Gif => "gif",
        }
    }
}

/// Subtitles to burn in, as the main player has them selected.
#[derive(Clone, Debug)]
pub enum Subs {
    None,
    /// Embedded track id.
    Track(i64),
    /// External file loaded with sub-add.
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Job {
    pub src: String,
    pub start: f64,
    pub end: f64,
    pub format: Format,
    /// Max output width in px; 0 keeps the source size.
    pub width: u32,
    /// GIF frame rate.
    pub fps: u32,
    pub audio: bool,
    pub subs: Subs,
    pub out: PathBuf,
    /// Encoders the libmpv build offers (from `encoder-list`).
    pub encoders: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Running(f32),
    Done(PathBuf),
    Failed(String),
}

pub struct Exporter {
    pub state: Arc<Mutex<State>>,
    cancel: Arc<AtomicBool>,
}

impl Default for Exporter {
    fn default() -> Self {
        Self { state: Arc::new(Mutex::new(State::Idle)), cancel: Arc::new(AtomicBool::new(false)) }
    }
}

impl Exporter {
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    pub fn busy(&self) -> bool {
        matches!(self.state(), State::Running(_))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn reset(&self) {
        if !self.busy() {
            *self.state.lock().unwrap() = State::Idle;
        }
    }

    pub fn start(&mut self, job: Job, ctx: eframe::egui::Context) {
        if self.busy() {
            return;
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        *self.state.lock().unwrap() = State::Running(0.0);
        let (state, cancel) = (self.state.clone(), self.cancel.clone());
        let _ = std::thread::Builder::new().name("clip-export".into()).spawn(move || {
            let r = run(&job, &state, &cancel, &ctx);
            let end = match r {
                Ok(()) if cancel.load(Ordering::Relaxed) => {
                    let _ = std::fs::remove_file(&job.out);
                    State::Idle
                }
                Ok(()) => State::Done(job.out.clone()),
                Err(e) => {
                    let _ = std::fs::remove_file(&job.out);
                    State::Failed(e)
                }
            };
            *state.lock().unwrap() = end;
            ctx.request_repaint();
        });
    }
}

/// Parse mpv's `encoder-list` (JSON when read as a string) into codec names.
pub fn parse_encoders(json: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    v.as_array()
        .map(|a| a.iter().filter_map(|e| e.get("codec").and_then(|c| c.as_str()).map(str::to_string)).collect())
        .unwrap_or_default()
}

fn pick<'a>(have: &[String], prefs: &[&'a str]) -> Option<&'a str> {
    prefs.iter().copied().find(|p| have.iter().any(|h| h == p))
}

/// H.264 encoders in order of preference, with their options.
const H264: &[(&str, &str)] = &[
    ("libx264", "preset=veryfast,crf=20"),
    ("h264_videotoolbox", "b=8000000"),
    ("h264_nvenc", "b=8000000"),
    ("h264_qsv", "b=8000000"),
    ("h264_amf", "b=8000000"),
    ("h264_mf", "b=8000000"),
    ("libopenh264", "b=8000000"),
    ("mpeg4", "b=8000000"),
];
const AAC: &[&str] = &["aac", "aac_at", "libfdk_aac"];

/// A libavfilter graph as an mpv `vf` entry; %N% quoting keeps its commas,
/// brackets and quotes intact.
fn lavfi(graph: &str) -> String {
    format!("lavfi=graph={}", crate::mpv::quote_opt(graph))
}

/// The mpv options for a job (everything but the input file).
pub fn options(job: &Job) -> Result<Vec<(String, String)>, String> {
    let mut o: Vec<(String, String)> = vec![
        ("config".into(), "no".into()),
        ("load-scripts".into(), "no".into()),
        ("ytdl".into(), "no".into()),
        ("osd-level".into(), "0".into()),
        ("hwdec".into(), "no".into()),
        ("keep-open".into(), "no".into()),
        ("idle".into(), "yes".into()),
        ("start".into(), format!("{:.3}", job.start)),
        ("end".into(), format!("{:.3}", job.end)),
        ("o".into(), job.out.to_string_lossy().to_string()),
    ];
    let scale = if job.width > 0 { format!("scale='min({},iw)':-2:flags=lanczos,", job.width) } else { String::new() };
    match job.format {
        Format::Gif => {
            if !job.encoders.is_empty() && pick(&job.encoders, &["gif"]).is_none() {
                return Err("gif encoder missing".into());
            }
            let fps = job.fps.clamp(5, 30);
            o.push(("of".into(), "gif".into()));
            o.push(("ovc".into(), "gif".into()));
            o.push(("aid".into(), "no".into()));
            o.push((
                "vf".into(),
                lavfi(&format!(
                    "fps={fps},{scale}split[a][b];[a]palettegen=stats_mode=single[p];[b][p]paletteuse=new=1:dither=bayer:bayer_scale=4"
                )),
            ));
        }
        Format::Mp4 => {
            let (venc, vopts) = if job.encoders.is_empty() {
                H264[0]
            } else {
                *H264
                    .iter()
                    .find(|(c, _)| job.encoders.iter().any(|h| h == c))
                    .ok_or("no H.264 / MPEG-4 encoder in this libmpv build")?
            };
            o.push(("of".into(), "mp4".into()));
            o.push(("ofopts".into(), "movflags=+faststart".into()));
            o.push(("ovc".into(), venc.into()));
            o.push(("ovcopts".into(), vopts.into()));
            o.push(("vf".into(), lavfi(&format!("{scale}format=yuv420p"))));
            if job.audio {
                let aenc = if job.encoders.is_empty() { Some("aac") } else { pick(&job.encoders, AAC) };
                match aenc {
                    Some(a) => {
                        o.push(("oac".into(), a.into()));
                        o.push(("oacopts".into(), "b=192000".into()));
                    }
                    None => o.push(("aid".into(), "no".into())),
                }
            } else {
                o.push(("aid".into(), "no".into()));
            }
        }
    }
    match &job.subs {
        Subs::None => o.push(("sid".into(), "no".into())),
        Subs::Track(id) => o.push(("sid".into(), id.to_string())),
        Subs::File(_) => {}
    }
    Ok(o)
}

fn run(job: &Job, state: &Mutex<State>, cancel: &AtomicBool, ctx: &eframe::egui::Context) -> Result<(), String> {
    if job.end - job.start < 0.1 {
        return Err("empty range".into());
    }
    if let Some(dir) = job.out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let opts = options(job)?;
    let pairs: Vec<(&str, &str)> = opts.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mpv = Mpv::new(&pairs)?;
    if let Subs::File(f) = &job.subs {
        // Load the external subtitle before the first frame is encoded.
        let _ = mpv.set("pause", "yes");
        mpv.command(&["loadfile", &job.src])?;
        wait(&mpv, cancel, |e| matches!(e, Event::FileLoaded))?;
        let _ = mpv.command(&["sub-add", &f.to_string_lossy(), "select"]);
        let _ = mpv.set("pause", "no");
    } else {
        mpv.command(&["loadfile", &job.src])?;
    }
    let span = job.end - job.start;
    let mut last = -1.0f32;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = mpv.command(&["stop"]);
            return Ok(());
        }
        match mpv.wait_event(0.25) {
            Some(Event::EndFile { reason, error }) => {
                if reason == 4 || error < 0 {
                    return Err(format!("encode failed ({error})"));
                }
                break;
            }
            Some(Event::Shutdown) => break,
            _ => {}
        }
        if let Some(t) = mpv.get_f64("time-pos") {
            let f = ((t - job.start) / span).clamp(0.0, 1.0) as f32;
            if (f - last).abs() > 0.005 {
                last = f;
                *state.lock().unwrap() = State::Running(f);
                ctx.request_repaint();
            }
        }
    }
    // Finish the file: destroying the handle flushes and closes the muxer.
    drop(mpv);
    match std::fs::metadata(&job.out) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => Err("no output written".into()),
    }
}

fn wait(mpv: &Mpv, cancel: &AtomicBool, want: impl Fn(&Event) -> bool) -> Result<(), String> {
    for _ in 0..120 {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(e) = mpv.wait_event(0.25) {
            if want(&e) {
                return Ok(());
            }
            if let Event::EndFile { .. } = e {
                return Err("could not open the file".into());
            }
        }
    }
    Err(format!("timed out after {:?}", Duration::from_secs(30)))
}

/// `<stem>_<from>-<to>.<ext>` in `dir`, never overwriting an existing file.
pub fn out_path(dir: &Path, src: &str, start: f64, end: f64, format: Format) -> PathBuf {
    let stem = Path::new(src)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty() && !s.contains("://"))
        .unwrap_or_else(|| "clip".into());
    let stem: String = stem.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c }).take(80).collect();
    let tag = |t: f64| {
        let t = t.max(0.0) as u64;
        if t >= 3600 {
            format!("{}h{:02}m{:02}s", t / 3600, t / 60 % 60, t % 60)
        } else {
            format!("{:02}m{:02}s", t / 60, t % 60)
        }
    };
    let base = format!("{stem}_{}-{}", tag(start), tag(end));
    let mut p = dir.join(format!("{base}.{}", format.ext()));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({n}).{}", format.ext()));
        n += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(format: Format, encoders: &[&str]) -> Job {
        Job {
            src: "/v/a.mkv".into(),
            start: 10.0,
            end: 20.0,
            format,
            width: 720,
            fps: 15,
            audio: true,
            subs: Subs::None,
            out: "/tmp/x".into(),
            encoders: encoders.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn get<'a>(o: &'a [(String, String)], k: &str) -> Option<&'a str> {
        o.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn picks_encoders() {
        let o = options(&job(Format::Mp4, &["h264_videotoolbox", "aac_at", "gif"])).unwrap();
        assert_eq!(get(&o, "ovc"), Some("h264_videotoolbox"));
        assert_eq!(get(&o, "oac"), Some("aac_at"));
        assert_eq!(get(&o, "sid"), Some("no"));
        let o = options(&job(Format::Mp4, &["libx264", "h264_mf", "gif"])).unwrap();
        assert_eq!(get(&o, "ovc"), Some("libx264"));
        assert_eq!(get(&o, "aid"), Some("no"));
        assert!(options(&job(Format::Mp4, &["gif"])).is_err());
        let o = options(&job(Format::Gif, &["gif"])).unwrap();
        assert_eq!(get(&o, "of"), Some("gif"));
        assert!(get(&o, "vf").unwrap().starts_with("lavfi=graph=%"));
        assert!(get(&o, "vf").unwrap().contains("fps=15,scale='min(720,iw)':-2"));
    }

    #[test]
    fn parses_encoder_list() {
        let j = r#"[{"driver":"libx264","codec":"libx264","description":"x"},{"driver":"gif","codec":"gif"}]"#;
        assert_eq!(parse_encoders(j), vec!["libx264", "gif"]);
    }

    #[test]
    fn names_clips() {
        let d = std::env::temp_dir();
        let p = out_path(&d, "/v/My: Movie.mkv", 65.0, 3725.0, Format::Gif);
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "My_ Movie_01m05s-1h02m05s.gif");
    }
}
