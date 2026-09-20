//! PaddleOCR PP-OCRv5 (mobile detection + Latin recognition) over `ort`
//! (ADR-0006). Detection is DB post-processing on the probability map;
//! recognition is a CTC decode over the character dictionary in `rec.yml`.

use std::sync::Mutex;

use image::RgbImage;
use image::imageops;
use ndarray::{Axis, Ix3, Ix4};
use ort::session::Session;
use ort::value::TensorRef;

use super::OcrEngine;
use crate::embed::e5::{build_session, init_onnxruntime};
use crate::error::{Error, Result};

const ENGINE_ID: &str = "paddle-ppocrv5-latin";
/// Detector input long side. Longer images are shrunk to this; shorter ones
/// keep their size (only padded up to a multiple of 32).
const DET_LONG_SIDE: f32 = 960.0;
const DET_THRESH: f32 = 0.3;
const DET_BOX_THRESH: f32 = 0.6;
const DET_UNCLIP_RATIO: f32 = 1.5;
const REC_HEIGHT: u32 = 48;
/// Widest recognizer input; a longer line is squeezed rather than refused.
const REC_MAX_WIDTH: u32 = 3200;

/// Real OCR engine. Both sessions sit behind mutexes because extraction
/// workers share one engine (`OcrEngine: Sync`).
pub struct PaddleOcr {
    det: Mutex<Session>,
    rec: Mutex<Session>,
    /// CTC classes: index 0 is the blank, then the dictionary, then a space.
    classes: Vec<String>,
}

impl PaddleOcr {
    /// Loads `det.onnx`, `rec.onnx` and `rec.yml` from
    /// `embed::manager::model_dir("ocr")`.
    pub fn load() -> Result<Self> {
        init_onnxruntime()?;
        let dir = crate::embed::manager::model_dir("ocr");
        let yml_path = dir.join("rec.yml");
        let yml = std::fs::read_to_string(&yml_path).map_err(|source| Error::Io {
            path: yml_path,
            source,
        })?;
        let mut classes = vec![String::new()];
        classes.extend(parse_dict(&yml));
        classes.push(" ".to_string());
        Ok(Self {
            det: Mutex::new(build_session(&dir.join("det.onnx"), 2)?),
            rec: Mutex::new(build_session(&dir.join("rec.onnx"), 2)?),
            classes,
        })
    }

    /// Text lines as rotated rectangles in the coordinates of `img`.
    fn detect(&self, img: &RgbImage) -> Result<Vec<TextLine>> {
        let (w, h) = img.dimensions();
        let scale = (DET_LONG_SIDE / w.max(h) as f32).min(1.0);
        let round32 = |v: u32| (((v as f32 * scale / 32.0).round() as u32).max(1)) * 32;
        let (nw, nh) = (round32(w), round32(h));
        let small = bilinear(img, nw, nh);

        // Channels are fed B, G, R with ImageNet mean/std: that is what the
        // PaddleOCR reference does (it decodes with OpenCV).
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        let plane = (nw * nh) as usize;
        let mut data = vec![0f32; 3 * plane];
        for (i, px) in small.pixels().enumerate() {
            for c in 0..3 {
                data[c * plane + i] = (px[2 - c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }

        let input = TensorRef::from_array_view(([1usize, 3, nh as usize, nw as usize], &*data))
            .map_err(|e| Error::Model(format!("building detector input: {e}")))?;
        let mut session = self
            .det
            .lock()
            .map_err(|_| Error::Model("OCR detector lock poisoned".to_string()))?;
        let outputs = session
            .run(ort::inputs! { "x" => input })
            .map_err(|e| Error::Model(format!("running detector: {e}")))?;
        let map = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| Error::Model(format!("extracting detector output: {e}")))?
            .into_dimensionality::<Ix4>()
            .map_err(|e| Error::Model(format!("unexpected detector output shape: {e}")))?;
        let prob: Vec<f32> = map.iter().copied().collect();

        let (sx, sy) = (w as f32 / nw as f32, h as f32 / nh as f32);
        Ok(boxes_from_map(&prob, nw as usize, nh as usize)
            .into_iter()
            .map(|b| b.to_line(sx, sy))
            .collect())
    }

    /// Recognizes one cropped text line.
    fn recognize_line(&self, line: &RgbImage) -> Result<String> {
        let (w, h) = line.dimensions();
        // Tall crops are vertical text; the recognizer only reads horizontal
        // lines. Same 1.5 ratio the reference uses.
        let line = if h * 2 >= w * 3 {
            imageops::rotate270(line)
        } else {
            line.clone()
        };
        let (w, h) = line.dimensions();
        let nw = (REC_HEIGHT * w).div_ceil(h).clamp(16, REC_MAX_WIDTH);
        let small = bilinear(&line, nw, REC_HEIGHT);

        let plane = (nw * REC_HEIGHT) as usize;
        let mut data = vec![0f32; 3 * plane];
        for (i, px) in small.pixels().enumerate() {
            for c in 0..3 {
                data[c * plane + i] = (px[2 - c] as f32 / 255.0 - 0.5) / 0.5;
            }
        }
        let input =
            TensorRef::from_array_view(([1usize, 3, REC_HEIGHT as usize, nw as usize], &*data))
                .map_err(|e| Error::Model(format!("building recognizer input: {e}")))?;
        let mut session = self
            .rec
            .lock()
            .map_err(|_| Error::Model("OCR recognizer lock poisoned".to_string()))?;
        let outputs = session
            .run(ort::inputs! { "x" => input })
            .map_err(|e| Error::Model(format!("running recognizer: {e}")))?;
        let logits = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| Error::Model(format!("extracting recognizer output: {e}")))?
            .into_dimensionality::<Ix3>()
            .map_err(|e| Error::Model(format!("unexpected recognizer output shape: {e}")))?;

        let ids: Vec<usize> = logits
            .index_axis(Axis(0), 0)
            .outer_iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .map_or(0, |(i, _)| i)
            })
            .collect();
        Ok(ctc_decode(ids.into_iter(), &self.classes))
    }
}

impl OcrEngine for PaddleOcr {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn recognize(&self, image: &RgbImage) -> Result<String> {
        let mut lines = self.detect(image)?;
        lines.sort_by(|a, b| a.cy.total_cmp(&b.cy));
        let mut out = Vec::new();
        for row in group_lines(lines) {
            let mut words = Vec::new();
            for line in row {
                let text = self.recognize_line(&crop(image, &line))?;
                if !text.trim().is_empty() {
                    words.push(text);
                }
            }
            if !words.is_empty() {
                out.push(words.join(" "));
            }
        }
        Ok(out.join("\n"))
    }
}

/// A detected text line: a rectangle rotated by `ux, uy` (unit vector along
/// the text, pointing right), in map coordinates.
struct MapBox {
    cx: f32,
    cy: f32,
    /// Extent along `u` and along its perpendicular (pointing down).
    w: f32,
    h: f32,
    ux: f32,
    uy: f32,
}

/// A text line in image coordinates: top-left corner plus the edge vectors
/// along the text (`du`) and down the line (`dv`).
struct TextLine {
    origin: [f32; 2],
    du: [f32; 2],
    dv: [f32; 2],
    /// Vertical centre and unrotated height, for grouping into rows.
    cy: f32,
    height: f32,
}

impl MapBox {
    fn to_line(&self, sx: f32, sy: f32) -> TextLine {
        let (vx, vy) = (-self.uy, self.ux);
        let du = [self.ux * self.w * sx, self.uy * self.w * sy];
        let dv = [vx * self.h * sx, vy * self.h * sy];
        let origin = [
            self.cx * sx - (du[0] + dv[0]) / 2.0,
            self.cy * sy - (du[1] + dv[1]) / 2.0,
        ];
        TextLine {
            origin,
            du,
            dv,
            cy: self.cy * sy,
            height: dv[0].hypot(dv[1]),
        }
    }
}

/// DB post-processing: threshold the probability map, take 8-connected
/// components, drop weak or tiny ones, fit a rotated rectangle to each (its
/// principal axis, kept within 45 degrees of horizontal) and grow it by the
/// reference's `area * unclip / perimeter` (the map holds a *shrunk* text
/// kernel).
///
/// ponytail: the rectangle comes from a PCA fit, not the reference's
/// minimum-area rectangle; the two agree for elongated blobs like text
/// lines. Curved or perspective-skewed text is cropped as a straight strip.
fn boxes_from_map(prob: &[f32], w: usize, h: usize) -> Vec<MapBox> {
    let mut seen = vec![false; w * h];
    let mut boxes = Vec::new();
    let mut stack = Vec::new();
    let mut pixels: Vec<(f32, f32)> = Vec::new();
    for start in 0..w * h {
        if seen[start] || prob[start] <= DET_THRESH {
            continue;
        }
        pixels.clear();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            pixels.push((x as f32, y as f32));
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let n = ny * w + nx;
                    if !seen[n] && prob[n] > DET_THRESH {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        let n = pixels.len() as f32;
        let (mx, my) = pixels
            .iter()
            .fold((0.0, 0.0), |a, p| (a.0 + p.0 / n, a.1 + p.1 / n));
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for &(x, y) in &pixels {
            let (dx, dy) = (x - mx, y - my);
            sxx += dx * dx;
            syy += dy * dy;
            sxy += dx * dy;
        }
        let mut theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
        // Text runs along the long axis; if that is closer to vertical, use
        // the other axis so `u` stays the near-horizontal one (the crop is
        // then tall, and gets rotated like the reference does).
        if theta > std::f32::consts::FRAC_PI_4 {
            theta -= std::f32::consts::FRAC_PI_2;
        } else if theta < -std::f32::consts::FRAC_PI_4 {
            theta += std::f32::consts::FRAC_PI_2;
        }
        let (ux, uy) = (theta.cos(), theta.sin());
        let (vx, vy) = (-uy, ux);
        let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, y) in &pixels {
            let (u, v) = (x * ux + y * uy, x * vx + y * vy);
            (u0, u1, v0, v1) = (u0.min(u), u1.max(u), v0.min(v), v1.max(v));
        }
        let (bw, bh) = (u1 - u0 + 1.0, v1 - v0 + 1.0);
        if bw.min(bh) < 3.0 {
            continue;
        }
        // Mean probability inside the rotated rectangle, as the reference
        // scores it: a speck grown out of an illustration is mostly empty.
        let (mut sum, mut cells) = (0f32, 0f32);
        let (px0, px1) = pixels.iter().fold((w, 0), |a, p| {
            (a.0.min(p.0 as usize), a.1.max(p.0 as usize))
        });
        let (py0, py1) = pixels.iter().fold((h, 0), |a, p| {
            (a.0.min(p.1 as usize), a.1.max(p.1 as usize))
        });
        for y in py0..=py1 {
            for x in px0..=px1 {
                let (u, v) = (x as f32 * ux + y as f32 * uy, x as f32 * vx + y as f32 * vy);
                if (u0..=u1).contains(&u) && (v0..=v1).contains(&v) {
                    sum += prob[y * w + x];
                    cells += 1.0;
                }
            }
        }
        if sum / cells.max(1.0) < DET_BOX_THRESH {
            continue;
        }
        let d = bw * bh * DET_UNCLIP_RATIO / (2.0 * (bw + bh));
        let (ww, hh) = (bw + 2.0 * d, bh + 2.0 * d);
        if ww.min(hh) < 5.0 {
            continue;
        }
        let (cu, cv) = ((u0 + u1) / 2.0 + 0.5, (v0 + v1) / 2.0 + 0.5);
        boxes.push(MapBox {
            cx: cu * ux + cv * vx,
            cy: cu * uy + cv * vy,
            w: ww,
            h: hh,
            ux,
            uy,
        });
    }
    boxes
}

/// Straightens a text line out of `img`: samples the rotated rectangle onto
/// an upright `|du| x |dv|` bitmap, clamping at the image border.
fn crop(img: &RgbImage, line: &TextLine) -> RgbImage {
    let (w, h) = img.dimensions();
    let cw = line.du[0].hypot(line.du[1]).round().max(1.0) as u32;
    let ch = line.dv[0].hypot(line.dv[1]).round().max(1.0) as u32;
    RgbImage::from_fn(cw, ch, |x, y| {
        let (a, b) = ((x as f32 + 0.5) / cw as f32, (y as f32 + 0.5) / ch as f32);
        let sx = line.origin[0] + a * line.du[0] + b * line.dv[0];
        let sy = line.origin[1] + a * line.du[1] + b * line.dv[1];
        *img.get_pixel(
            (sx.floor().max(0.0) as u32).min(w - 1),
            (sy.floor().max(0.0) as u32).min(h - 1),
        )
    })
}

/// Plain 2x2-tap bilinear resize with pixel-centre sampling, no
/// anti-aliasing: what OpenCV's `INTER_LINEAR` does, and so what both models
/// were trained against.
fn bilinear(img: &RgbImage, nw: u32, nh: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let (sx, sy) = (w as f32 / nw as f32, h as f32 / nh as f32);
    RgbImage::from_fn(nw, nh, |x, y| {
        let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (x0, y0) = ((fx as u32).min(w - 1), (fy as u32).min(h - 1));
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let mix = |c: usize| {
            let top = img[(x0, y0)][c] as f32 * (1.0 - tx) + img[(x1, y0)][c] as f32 * tx;
            let bottom = img[(x0, y1)][c] as f32 * (1.0 - tx) + img[(x1, y1)][c] as f32 * tx;
            (top * (1.0 - ty) + bottom * ty).round() as u8
        };
        image::Rgb([mix(0), mix(1), mix(2)])
    })
}

/// Groups lines (already sorted by vertical centre) into reading-order rows:
/// a line joins the current row when its centre is within half the row's
/// first line's height of that line's centre. Each row is ordered left to
/// right.
fn group_lines(lines: Vec<TextLine>) -> Vec<Vec<TextLine>> {
    let mut rows: Vec<Vec<TextLine>> = Vec::new();
    for line in lines {
        match rows.last_mut() {
            Some(row) if (line.cy - row[0].cy).abs() <= row[0].height / 2.0 => row.push(line),
            _ => rows.push(vec![line]),
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.origin[0].total_cmp(&b.origin[0]));
    }
    rows
}

/// CTC greedy decode: collapse repeats, then drop blanks (class 0).
fn ctc_decode(ids: impl Iterator<Item = usize>, classes: &[String]) -> String {
    let mut out = String::new();
    let mut prev = 0;
    for id in ids {
        if id != prev
            && id != 0
            && let Some(c) = classes.get(id)
        {
            out.push_str(c);
        }
        prev = id;
    }
    out
}

/// Reads the `PostProcess.character_dict` list out of `rec.yml`. Entries are
/// one per line as `  - x`, plain, `'single-quoted'` (`''` is a quote) or
/// `"double-quoted"`. Hand-parsed rather than pulling in a YAML crate for one
/// flat list.
fn parse_dict(yml: &str) -> Vec<String> {
    yml.lines()
        .skip_while(|l| l.trim() != "character_dict:")
        .skip(1)
        .map_while(|l| l.trim_start().strip_prefix("- "))
        .map(|v| {
            if let Some(v) = v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
                v.replace("''", "'")
            } else if let Some(v) = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                v.replace("\\\"", "\"").replace("\\\\", "\\")
            } else {
                v.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_parses_quoting_styles() {
        let yml = "A:\n  b: 1\nPostProcess:\n  character_dict:\n  - '0'\n  - a\n  - ''''\n  - ' '\n  - \"\\\\\"\n  - 'á'\nnext: 1\n";
        assert_eq!(parse_dict(yml), ["0", "a", "'", " ", "\\", "á"]);
    }

    #[test]
    fn ctc_collapses_repeats_and_drops_blanks() {
        let classes: Vec<String> = ["", "a", "b"].map(String::from).to_vec();
        // a a _ a b b  ->  "aab": the blank separates the two real 'a's.
        assert_eq!(ctc_decode([1, 1, 0, 1, 2, 2].into_iter(), &classes), "aab");
    }

    #[test]
    fn detects_a_tilted_bar_and_ignores_specks() {
        let (w, h) = (200, 100);
        let mut prob = vec![0f32; w * h];
        // A 150 x 6 bar tilted about 5 degrees (slope 0.09), then a speck.
        for x in 20..170 {
            let top = 40 + ((x - 20) as f32 * 0.09) as usize;
            for y in top..top + 6 {
                prob[y * w + x] = 0.9;
            }
        }
        prob[2 * w + 2] = 0.9;
        let boxes = boxes_from_map(&prob, w, h);
        assert_eq!(boxes.len(), 1);
        let b = &boxes[0];
        // Long side runs along the text, at roughly the tilt angle, and is
        // not inflated by the tilt the way an axis-aligned box would be.
        assert!(b.w > 150.0 && b.h < 30.0, "w={} h={}", b.w, b.h);
        assert!((b.uy.atan2(b.ux) - 0.09).abs() < 0.03);
    }

    #[test]
    fn lines_group_by_row_and_read_left_to_right() {
        let line = |x: f32, cy: f32| TextLine {
            origin: [x, cy - 5.0],
            du: [40.0, 0.0],
            dv: [0.0, 10.0],
            cy,
            height: 10.0,
        };
        let rows = group_lines(vec![line(50.0, 5.0), line(0.0, 6.0), line(0.0, 35.0)]);
        let xs: Vec<Vec<f32>> = rows
            .iter()
            .map(|r| r.iter().map(|l| l.origin[0]).collect())
            .collect();
        assert_eq!(xs, vec![vec![0.0, 50.0], vec![0.0]]);
    }

    /// Needs `just models` (ocr slot) and the vendored ONNX Runtime.
    #[test]
    #[ignore = "requires `just models` and `cargo xtask fetch-onnxruntime`"]
    fn reads_spanish_screenshot_with_accents() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/corpus/images/screenshot_es.png");
        let img = image::open(path).expect("fixture").to_rgb8();
        let text = PaddleOcr::load().expect("load").recognize(&img).unwrap();
        for want in [
            "¿Qué son las letras con tilde?",
            "español",
            "sílaba",
            "pingüino",
        ] {
            assert!(text.contains(want), "missing {want:?} in:\n{text}");
        }
    }
}
