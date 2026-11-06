//! Snapshot post-processing: burning live (speech) subtitles into the image
//! and copying the result to the clipboard. mpv writes the file first.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// One subtitle line to burn in: text, size as a fraction of image height, colour.
#[derive(Clone)]
pub struct Line {
    pub text: String,
    pub rel_size: f32,
    pub color: [u8; 3],
}

pub struct Job {
    pub path: PathBuf,
    pub lines: Vec<Line>,
    pub clipboard: bool,
    /// Delete the file afterwards (clipboard-only snapshot).
    pub temp: bool,
    pub jpeg_quality: u8,
}

pub fn run(job: Job) -> Result<(), String> {
    if job.lines.is_empty() && !job.clipboard {
        return Ok(());
    }
    let mut img = image::open(&job.path).map_err(|e| format!("open snapshot: {e}"))?.to_rgba8();
    if !job.lines.is_empty() {
        if let Some(font) = font() {
            burn(&mut img, font, &job.lines);
            save(&img, &job.path, job.jpeg_quality)?;
        }
    }
    if job.clipboard {
        copy_to_clipboard(&img)?;
    }
    if job.temp {
        let _ = std::fs::remove_file(&job.path);
    }
    Ok(())
}

fn save(img: &RgbaImage, path: &Path, quality: u8) -> Result<(), String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let r = if ext == "jpg" || ext == "jpeg" {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        std::fs::File::create(path).map_err(|e| e.to_string()).and_then(|f| {
            let mut w = std::io::BufWriter::new(f);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, quality)
                .encode_image(&rgb)
                .map_err(|e| e.to_string())
        })
    } else {
        img.save(path).map_err(|e| e.to_string())
    };
    r.map_err(|e| format!("save snapshot: {e}"))
}

fn copy_to_clipboard(img: &RgbaImage) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_image(arboard::ImageData {
        width: img.width() as usize,
        height: img.height() as usize,
        bytes: std::borrow::Cow::Borrowed(img.as_raw()),
    })
    .map_err(|e| e.to_string())
}

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        crate::icons::cjk_font_paths().iter().find_map(|(p, idx)| {
            let bytes = std::fs::read(p).ok()?;
            FontVec::try_from_vec_and_index(bytes, *idx).ok()
        })
    })
    .as_ref()
}

/// Draw `lines` centred near the bottom, last line lowest, with a dark outline.
pub fn burn(img: &mut RgbaImage, font: &FontVec, lines: &[Line]) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let max_w = w * 0.86;
    let mut bottom = h - h * 0.06;
    for line in lines.iter().rev() {
        let px = (h * line.rel_size).max(12.0);
        let scale = PxScale::from(px);
        let rows = wrap(font, scale, &line.text, max_w);
        let sf = font.as_scaled(scale);
        let row_h = sf.height() + sf.line_gap();
        for row in rows.iter().rev() {
            let top = bottom - row_h;
            draw_row(img, font, scale, row, w / 2.0, top, line.color);
            bottom = top;
        }
        bottom -= px * 0.15;
    }
}

fn text_width(font: &FontVec, scale: PxScale, s: &str) -> f32 {
    let sf = font.as_scaled(scale);
    let mut wsum = 0.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            wsum += sf.kern(p, id);
        }
        wsum += sf.h_advance(id);
        prev = Some(id);
    }
    wsum
}

fn wrap(font: &FontVec, scale: PxScale, text: &str, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut cur = String::new();
        for ch in para.chars() {
            let mut next = cur.clone();
            next.push(ch);
            if text_width(font, scale, &next) > max_w && !cur.is_empty() {
                // break at the last space when the line has words
                if let Some(i) = cur.rfind(' ').filter(|i| *i > 0) {
                    let rest = cur[i + 1..].to_string();
                    out.push(cur[..i].to_string());
                    cur = rest;
                    cur.push(ch);
                } else {
                    out.push(std::mem::take(&mut cur));
                    if ch != ' ' {
                        cur.push(ch);
                    }
                }
            } else {
                cur = next;
            }
        }
        if !cur.trim().is_empty() {
            out.push(cur);
        }
    }
    out
}

fn draw_row(img: &mut RgbaImage, font: &FontVec, scale: PxScale, s: &str, cx: f32, top: f32, color: [u8; 3]) {
    let sf = font.as_scaled(scale);
    let width = text_width(font, scale, s);
    let baseline = top + sf.ascent();
    let mut glyphs = Vec::new();
    let mut x = cx - width / 2.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            x += sf.kern(p, id);
        }
        glyphs.push(id.with_scale_and_position(scale, ab_glyph::point(x, baseline)));
        x += sf.h_advance(id);
        prev = Some(id);
    }
    let o = (scale.y / 14.0).max(1.5);
    let offsets = [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, o), (-o, o), (o, -o)];
    for (dx, dy) in offsets {
        for g in &glyphs {
            let mut g = g.clone();
            g.position.x += dx;
            g.position.y += dy;
            paint(img, font, g, [0, 0, 0], 0.9);
        }
    }
    for g in glyphs {
        paint(img, font, g, color, 1.0);
    }
}

fn paint(img: &mut RgbaImage, font: &FontVec, g: ab_glyph::Glyph, color: [u8; 3], alpha: f32) {
    let Some(og) = font.outline_glyph(g) else { return };
    let b = og.px_bounds();
    let (w, h) = (img.width() as i32, img.height() as i32);
    og.draw(|x, y, c| {
        let px = b.min.x as i32 + x as i32;
        let py = b.min.y as i32 + y as i32;
        if px < 0 || py < 0 || px >= w || py >= h {
            return;
        }
        let a = (c * alpha).clamp(0.0, 1.0);
        let Rgba([r, gg, bb, aa]) = *img.get_pixel(px as u32, py as u32);
        let mix = |d: u8, s: u8| (d as f32 * (1.0 - a) + s as f32 * a).round() as u8;
        img.put_pixel(px as u32, py as u32, Rgba([mix(r, color[0]), mix(gg, color[1]), mix(bb, color[2]), aa]));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burns_text_when_a_font_exists() {
        let Some(font) = font() else { return };
        let mut img = RgbaImage::from_pixel(640, 360, Rgba([40, 40, 40, 255]));
        burn(
            &mut img,
            font,
            &[
                Line { text: "我们今晚去哪儿？".into(), rel_size: 0.06, color: [255, 255, 255] },
                Line { text: "Where are we going tonight?".into(), rel_size: 0.045, color: [255, 226, 140] },
            ],
        );
        let changed = img.pixels().filter(|p| p.0 != [40, 40, 40, 255]).count();
        assert!(changed > 500);
        let _ = img.save(std::env::temp_dir().join("fp-burn-test.png"));
    }

    #[test]
    fn wraps_long_lines() {
        let Some(font) = font() else { return };
        let rows = wrap(font, PxScale::from(20.0), "one two three four five six seven eight nine ten", 120.0);
        assert!(rows.len() > 1);
        assert!(rows.iter().all(|r| !r.starts_with(' ')));
    }
}
