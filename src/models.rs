//! Whisper model catalogue and resumable downloads.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct ModelInfo {
    pub id: &'static str,
    pub file: &'static str,
    pub size_mb: u32,
    pub note_zh: &'static str,
    pub note_en: &'static str,
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo { id: "tiny", file: "ggml-tiny.bin", size_mb: 75, note_zh: "最快，准确度一般", note_en: "Fastest, rough" },
    ModelInfo { id: "base", file: "ggml-base.bin", size_mb: 142, note_zh: "推荐：速度与准确度平衡", note_en: "Recommended balance" },
    ModelInfo { id: "small", file: "ggml-small.bin", size_mb: 466, note_zh: "更准，需较快 CPU", note_en: "More accurate, needs a fast CPU" },
    ModelInfo { id: "medium", file: "ggml-medium.bin", size_mb: 1533, note_zh: "高准确度，较慢", note_en: "High accuracy, slow" },
    ModelInfo {
        id: "large-v3-turbo-q5",
        file: "ggml-large-v3-turbo-q5_0.bin",
        size_mb: 547,
        note_zh: "最高准确度（量化版），建议 GPU / Apple 芯片",
        note_en: "Best accuracy (quantized), GPU / Apple silicon",
    },
];

pub fn info(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn models_dir() -> PathBuf {
    let d = crate::settings::data_dir().join("models");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn model_path(id: &str) -> Option<PathBuf> {
    info(id).map(|m| models_dir().join(m.file))
}

pub fn is_installed(id: &str) -> bool {
    model_path(id).map(|p| p.is_file()).unwrap_or(false)
}

#[allow(dead_code)]
pub fn installed() -> Vec<&'static str> {
    MODELS.iter().filter(|m| is_installed(m.id)).map(|m| m.id).collect()
}

pub fn delete(id: &str) {
    if let Some(p) = model_path(id) {
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(p.with_extension("part"));
    }
}

#[derive(Default)]
pub struct Download {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub finished: AtomicBool,
    pub error: Mutex<Option<String>>,
}

pub type Downloads = Arc<Mutex<Vec<(String, Arc<Download>)>>>;

pub fn start_download(downloads: &Downloads, id: &str, mirror: bool, ctx: eframe::egui::Context) {
    let Some(m) = info(id) else { return };
    let mut list = downloads.lock().unwrap();
    if list.iter().any(|(i, d)| i == id && !d.finished.load(Ordering::Relaxed)) {
        return;
    }
    list.retain(|(i, _)| i != id);
    let dl = Arc::new(Download::default());
    list.push((id.to_string(), dl.clone()));
    let host = if mirror { "https://hf-mirror.com" } else { "https://huggingface.co" };
    let url = format!("{host}/ggerganov/whisper.cpp/resolve/main/{}", m.file);
    let dest = models_dir().join(m.file);
    std::thread::spawn(move || {
        if let Err(e) = run_download(&url, &dest, &dl, &ctx) {
            *dl.error.lock().unwrap() = Some(e);
        }
        dl.finished.store(true, Ordering::Relaxed);
        ctx.request_repaint();
    });
}

fn run_download(url: &str, dest: &PathBuf, dl: &Download, ctx: &eframe::egui::Context) -> Result<(), String> {
    let part = dest.with_extension("part");
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let client = reqwest::blocking::Client::builder()
        .user_agent("FilmoraPlayer/0.1")
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(None)
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let mut resp = req.send().map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    if status.as_u16() != 206 {
        have = 0; // server ignored the range: start over
    }
    let total = have + resp.content_length().unwrap_or(0);
    dl.total.store(total, Ordering::Relaxed);
    dl.done.store(have, Ordering::Relaxed);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(have > 0)
        .truncate(have == 0)
        .open(&part)
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut last_paint = std::time::Instant::now();
    loop {
        if dl.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        have += n as u64;
        dl.done.store(have, Ordering::Relaxed);
        if last_paint.elapsed().as_millis() > 200 {
            ctx.request_repaint();
            last_paint = std::time::Instant::now();
        }
    }
    f.flush().map_err(|e| e.to_string())?;
    drop(f);
    if total > 0 && have != total {
        return Err(format!("incomplete download ({have}/{total})"));
    }
    // whisper.cpp ggml files start with the "lmgg" magic (0x67676d6c LE).
    let mut magic = [0u8; 4];
    std::fs::File::open(&part).and_then(|mut f| f.read_exact(&mut magic)).map_err(|e| e.to_string())?;
    if u32::from_le_bytes(magic) != 0x6767_6d6c {
        let _ = std::fs::remove_file(&part);
        return Err("downloaded file is not a ggml model".into());
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
    Ok(())
}
