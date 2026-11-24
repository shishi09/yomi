use image::imageops::FilterType;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrLine {
    pub text: String,
    pub confidence: f32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub white_ratio: f64,
    pub dark_ratio: f64,
    pub bubble_region_id: i32,
    pub region_x: Option<f64>,
    pub region_y: Option<f64>,
    pub region_w: Option<f64>,
    pub region_h: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<OcrLine>,
}

#[derive(Debug, Clone)]
pub struct RawOcrLine {
    pub text: String,
    pub confidence: f32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bubble {
    pub id: usize,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub line_count: usize,
    pub min_conf: f64,
    pub style: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
}

pub struct GrayBuffer {
    pub pixels: Vec<u8>,
    pub w: usize,
    pub h: usize,
}

#[derive(Debug, Clone, Copy)]
struct RegionInfo {
    id: i32,
    rx: f64,
    ry: f64,
    rw: f64,
    rh: f64,
}

pub fn build_grayscale_buffer(image_bytes: &[u8], max_dim: u32) -> Option<(u32, u32, GrayBuffer)> {
    let dyn_img = image::load_from_memory(image_bytes).ok()?;
    let orig_w = dyn_img.width();
    let orig_h = dyn_img.height();
    if orig_w == 0 || orig_h == 0 {
        return None;
    }
    let max_orig = orig_w.max(orig_h);
    let resized = if max_orig > max_dim {
        let scale = max_dim as f64 / max_orig as f64;
        let nw = (orig_w as f64 * scale).round().max(1.0) as u32;
        let nh = (orig_h as f64 * scale).round().max(1.0) as u32;
        dyn_img.resize_exact(nw, nh, FilterType::Triangle)
    } else {
        dyn_img
    };
    let gray = resized.to_luma8();
    let w = gray.width() as usize;
    let h = gray.height() as usize;
    Some((
        orig_w,
        orig_h,
        GrayBuffer {
            pixels: gray.into_raw(),
            w,
            h,
        },
    ))
}

pub fn sample_background_stats(
    buf: &GrayBuffer,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> (f64, f64) {
    let bw = buf.w;
    let bh = buf.h;
    if bw == 0 || bh == 0 {
        return (1.0, 0.0);
    }
    let pad_x = (h * 0.35).clamp(0.004, 0.014);
    let pad_y = (h * 0.30).clamp(0.003, 0.012);

    let ox1 = (((x - pad_x) * bw as f64) as isize).max(0) as usize;
    let oy1 = (((y - pad_y) * bh as f64) as isize).max(0) as usize;
    let ox2 = ((((x + w + pad_x) * bw as f64) as usize).min(bw.saturating_sub(1))).max(ox1);
    let oy2 = ((((y + h + pad_y) * bh as f64) as usize).min(bh.saturating_sub(1))).max(oy1);

    let ix1 = ((x * bw as f64) as isize).max(0) as usize;
    let iy1 = ((y * bh as f64) as isize).max(0) as usize;
    let ix2 = (((x + w) * bw as f64) as usize).min(bw.saturating_sub(1));
    let iy2 = (((y + h) * bh as f64) as usize).min(bh.saturating_sub(1));

    let mut total = 0usize;
    let mut white_count = 0usize;
    let mut dark_count = 0usize;

    let step_x = ((ox2 - ox1) / 24).max(1);
    let step_y = ((oy2 - oy1) / 16).max(1);

    let mut py = oy1;
    while py <= oy2 {
        let mut px = ox1;
        while px <= ox2 {
            let inside_inner = px >= ix1 && px <= ix2 && py >= iy1 && py <= iy2;
            if !inside_inner {
                let luma = buf.pixels[py * bw + px];
                total += 1;
                if luma >= 218 {
                    white_count += 1;
                } else if luma <= 55 {
                    dark_count += 1;
                }
            }
            px += step_x;
        }
        py += step_y;
    }

    if total == 0 {
        (1.0, 0.0)
    } else {
        (
            white_count as f64 / total as f64,
            dark_count as f64 / total as f64,
        )
    }
}

struct LineWithStats {
    raw: RawOcrLine,
    white_ratio: f64,
    dark_ratio: f64,
}

fn detect_closed_bubble_regions(
    buf: &GrayBuffer,
    lines: &[LineWithStats],
) -> Vec<Option<RegionInfo>> {
    if lines.is_empty() || buf.w < 3 || buf.h < 3 {
        return vec![None; lines.len()];
    }

    let bw = buf.w;
    let bh = buf.h;
    let total_pixels = bw * bh;

    let mut passable: Vec<bool> = buf.pixels.iter().map(|&p| p >= 198).collect();

    for ln in lines {
        if ln.white_ratio < 0.50 || (ln.raw.h >= 0.045 && ln.white_ratio < 0.68) {
            continue;
        }
        let shrink_x = (ln.raw.w * 0.08).max(0.002);
        let shrink_y = (ln.raw.h * 0.14).max(0.002);
        let x1 = (((ln.raw.x + shrink_x) * bw as f64) as usize).max(1);
        let y1 = (((ln.raw.y + shrink_y) * bh as f64) as usize).max(1);
        let x2 = (((ln.raw.x + ln.raw.w - shrink_x) * bw as f64) as usize).min(bw - 2);
        let y2 = (((ln.raw.y + ln.raw.h - shrink_y) * bh as f64) as usize).min(bh - 2);
        if x2 <= x1 || y2 <= y1 {
            continue;
        }
        for py in y1..=y2 {
            let row_off = py * bw;
            for px in x1..=x2 {
                passable[row_off + px] = true;
            }
        }
    }

    let mut region_map = vec![0i32; total_pixels];
    let mut region_infos: BTreeMap<i32, RegionInfo> = BTreeMap::new();
    let mut next_region_id = 1i32;

    let max_closed_pixels = (total_pixels as f64 * 0.17) as usize;
    let max_closed_w = (bw as f64 * 0.52) as usize;
    let max_closed_h = (bh as f64 * 0.44) as usize;

    let find_seed = |ln: &RawOcrLine| -> Option<usize> {
        let cx = ((ln.x + ln.w * 0.5) * bw as f64).clamp(0.0, (bw - 1) as f64) as usize;
        let cy = ((ln.y + ln.h * 0.5) * bh as f64).clamp(0.0, (bh - 1) as f64) as usize;
        if passable[cy * bw + cx] {
            return Some(cy * bw + cx);
        }
        let x1 = ((ln.x * bw as f64) as usize).min(bw - 1);
        let y1 = ((ln.y * bh as f64) as usize).min(bh - 1);
        let x2 = (((ln.x + ln.w) * bw as f64) as usize).min(bw - 1);
        let y2 = (((ln.y + ln.h) * bh as f64) as usize).min(bh - 1);
        let step_x = ((x2.saturating_sub(x1)) / 8).max(1);
        let step_y = ((y2.saturating_sub(y1)) / 4).max(1);
        let mut py = y1;
        while py <= y2 {
            let mut px = x1;
            while px <= x2 {
                let idx = py * bw + px;
                if passable[idx] {
                    return Some(idx);
                }
                px += step_x;
            }
            py += step_y;
        }
        None
    };

    let mut flood_fill = |seed: usize| -> Option<RegionInfo> {
        if region_map[seed] != 0 {
            let existing = region_map[seed];
            return if existing > 0 {
                region_infos.get(&existing).copied()
            } else {
                None
            };
        }
        let mut queue = VecDeque::new();
        let mut visited = Vec::new();
        queue.push_back(seed);
        visited.push(seed);
        region_map[seed] = -1;

        let mut min_x = seed % bw;
        let mut max_x = min_x;
        let mut min_y = seed / bw;
        let mut max_y = min_y;
        let mut touches_border = false;
        let mut overflowed = false;

        while let Some(cur) = queue.pop_front() {
            let px = cur % bw;
            let py = cur / bw;
            min_x = min_x.min(px);
            max_x = max_x.max(px);
            min_y = min_y.min(py);
            max_y = max_y.max(py);

            if px == 0 || py == 0 || px == bw - 1 || py == bh - 1 {
                touches_border = true;
            }
            if visited.len() > max_closed_pixels
                || (max_x - min_x) > max_closed_w
                || (max_y - min_y) > max_closed_h
            {
                overflowed = true;
                break;
            }

            let neighbors = [
                if px > 0 { Some(cur - 1) } else { None },
                if px + 1 < bw { Some(cur + 1) } else { None },
                if py > 0 { Some(cur - bw) } else { None },
                if py + 1 < bh { Some(cur + bw) } else { None },
            ];
            for nb_opt in neighbors.into_iter().flatten() {
                if passable[nb_opt] && region_map[nb_opt] == 0 {
                    region_map[nb_opt] = -1;
                    queue.push_back(nb_opt);
                    visited.push(nb_opt);
                }
            }
        }

        if overflowed || touches_border || visited.len() < 40 {
            for idx in visited {
                region_map[idx] = -2;
            }
            return None;
        }

        let rid = next_region_id;
        next_region_id += 1;

        let span_h = max_y - min_y + 1;
        let span_w = max_x - min_x + 1;
        let mut row_counts = vec![0usize; span_h];
        let mut col_counts = vec![0usize; span_w];

        for &idx in &visited {
            region_map[idx] = rid;
            let px = idx % bw;
            let py = idx / bw;
            row_counts[py - min_y] += 1;
            col_counts[px - min_x] += 1;
        }

        let max_row = row_counts.iter().copied().max().unwrap_or(1);
        let max_col = col_counts.iter().copied().max().unwrap_or(1);
        let row_thresh = ((max_row as f64 * 0.30) as usize).max(2);
        let col_thresh = ((max_col as f64 * 0.30) as usize).max(2);

        let trim_min_y = row_counts
            .iter()
            .position(|&c| c >= row_thresh)
            .map(|r| min_y + r)
            .unwrap_or(min_y);
        let trim_max_y = row_counts
            .iter()
            .rposition(|&c| c >= row_thresh)
            .map(|r| min_y + r)
            .unwrap_or(max_y);
        let trim_min_x = col_counts
            .iter()
            .position(|&c| c >= col_thresh)
            .map(|c| min_x + c)
            .unwrap_or(min_x);
        let trim_max_x = col_counts
            .iter()
            .rposition(|&c| c >= col_thresh)
            .map(|c| min_x + c)
            .unwrap_or(max_x);

        let inset_x = (trim_max_x.saturating_sub(trim_min_x)) as f64 * 0.05;
        let inset_y = (trim_max_y.saturating_sub(trim_min_y)) as f64 * 0.05;
        let rx = ((trim_min_x as f64 + inset_x) / bw as f64).max(0.0);
        let ry = ((trim_min_y as f64 + inset_y) / bh as f64).max(0.0);
        let rw = (((trim_max_x.saturating_sub(trim_min_x)) as f64 - 2.0 * inset_x) / bw as f64)
            .max(0.01);
        let rh = (((trim_max_y.saturating_sub(trim_min_y)) as f64 - 2.0 * inset_y) / bh as f64)
            .max(0.01);

        let info = RegionInfo { id: rid, rx, ry, rw, rh };
        region_infos.insert(rid, info);
        Some(info)
    };

    let mut out = Vec::with_capacity(lines.len());
    for ln in lines {
        if ln.white_ratio < 0.50 || (ln.raw.h >= 0.045 && ln.white_ratio < 0.68) {
            out.push(None);
            continue;
        }
        let Some(seed) = find_seed(&ln.raw) else {
            out.push(None);
            continue;
        };
        out.push(flood_fill(seed));
    }
    out
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn round5(v: f64) -> f64 {
    (v * 100000.0).round() / 100000.0
}

pub fn enrich_raw_lines(image_bytes: &[u8], raw_lines: Vec<RawOcrLine>) -> Result<OcrResult, String> {
    let Some((orig_w, orig_h, gray_buf)) = build_grayscale_buffer(image_bytes, 900) else {
        return Err("Impossible de décoder l'image pour l'analyse des bulles.".to_string());
    };

    let mut with_stats = Vec::with_capacity(raw_lines.len());
    for raw in raw_lines {
        let (wr, dr) = sample_background_stats(&gray_buf, raw.x, raw.y, raw.w, raw.h);
        with_stats.push(LineWithStats {
            raw,
            white_ratio: round3(wr),
            dark_ratio: round3(dr),
        });
    }

    let regions = detect_closed_bubble_regions(&gray_buf, &with_stats);
    let mut lines = Vec::with_capacity(with_stats.len());
    for (idx, item) in with_stats.into_iter().enumerate() {
        let reg = regions[idx];
        lines.push(OcrLine {
            text: item.raw.text,
            confidence: item.raw.confidence,
            x: item.raw.x,
            y: item.raw.y,
            w: item.raw.w,
            h: item.raw.h,
            white_ratio: item.white_ratio,
            dark_ratio: item.dark_ratio,
            bubble_region_id: reg.map(|r| r.id).unwrap_or(-1),
            region_x: reg.map(|r| round5(r.rx)),
            region_y: reg.map(|r| round5(r.ry)),
            region_w: reg.map(|r| round5(r.rw)),
            region_h: reg.map(|r| round5(r.rh)),
        });
    }

    Ok(OcrResult {
        width: orig_w,
        height: orig_h,
        lines,
    })
}

pub fn is_punctuation_only(text: &str) -> bool {
    let cleaned = text.trim();
    if cleaned.is_empty() {
        return true;
    }
    cleaned.chars().all(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '!' | '?'
                    | '.'
                    | ','
                    | '…'
                    | '-'
                    | '~'
                    | '•'
                    | '·'
                    | ':'
                    | ';'
                    | '('
                    | ')'
                    | '"'
                    | '\''
                    | '「'
                    | '」'
                    | '『'
                    | '』'
                    | '！'
                    | '？'
                    | '。'
                    | '、'
                    | '・'
                    | '*'
            )
    })
}

pub fn normalize_vision_mode(raw: Option<&str>) -> String {
    match raw.unwrap_or("auto").trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" => "on".to_string(),
        "off" | "false" | "0" => "off".to_string(),
        _ => "auto".to_string(),
    }
}

pub fn should_use_vision(
    vision_mode: &str,
    model: &str,
    bubbles: &[Bubble],
    cfg: &crate::config::AppConfig,
) -> bool {
    if !crate::config::model_supports_vision(model, cfg) {
        return false;
    }
    if vision_mode == "off" {
        return false;
    }
    let translatable: Vec<&Bubble> = bubbles
        .iter()
        .filter(|b| !is_punctuation_only(&b.text))
        .collect();
    if translatable.is_empty() {
        return false;
    }
    if vision_mode == "on" {
        return true;
    }
    if translatable.len() >= 3 {
        return true;
    }
    if translatable.iter().any(|b| b.style == "floating") {
        return true;
    }
    if translatable.iter().any(|b| b.min_conf < 0.85) {
        return true;
    }
    false
}

pub fn is_noise_line(line: &OcrLine, source_lang: &str) -> bool {
    let text = line.text.trim();
    if text.is_empty() {
        return true;
    }
    let conf = line.confidence as f64;
    if conf < 0.22 {
        return true;
    }
    let in_closed = line.bubble_region_id > 0;
    let char_count = text.chars().count();
    if char_count == 1 {
        let ch = text.chars().next().unwrap();
        let is_jp = ('\u{3040}'..='\u{30ff}').contains(&ch)
            || ('\u{4e00}'..='\u{9fff}').contains(&ch);
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '!' | '?' | '…') || is_jp) {
            return true;
        }
    }
    if source_lang == "en" {
        if char_count == 1 {
            let ch = text.chars().next().unwrap();
            if ch.is_alphabetic() && !matches!(ch, 'I' | 'A') {
                return true;
            }
        }
        if !text.chars().any(|c| c.is_ascii_alphanumeric() || c == '!' || c == '?') {
            return true;
        }
        if !in_closed
            && !text.chars().any(|c| c.is_ascii_alphabetic())
            && !is_punctuation_only(text)
        {
            return true;
        }
        let has_noise_sym = text
            .chars()
            .any(|c| matches!(c, '÷' | '±' | '×' | '=' | '^' | '|' | '\\' | '<' | '>' | '~' | '`'));
        let alpha_cnt = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
        if char_count <= 6 && has_noise_sym && alpha_cnt <= 2 {
            return true;
        }
        if !in_closed && conf <= 0.35 && line.white_ratio < 0.65 && line.dark_ratio < 0.55 {
            return true;
        }
        if line.h > 0.10 && (line.h / line.w.max(1e-5)) > 2.4 {
            return true;
        }
    }
    if char_count <= 3 && text.chars().all(|c| c.is_ascii_digit()) && line.y > 0.92 && line.h < 0.03 {
        return true;
    }
    false
}

pub fn join_bubble_lines(lines: &[&OcrLine]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let avg_h: f64 = lines.iter().map(|l| l.h).sum::<f64>() / lines.len() as f64;
    let mut sorted: Vec<&OcrLine> = lines.to_vec();
    sorted.sort_by(|a, b| {
        a.y.partial_cmp(&b.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut rows: Vec<Vec<&OcrLine>> = Vec::new();
    for ln in sorted {
        let cy = ln.y + ln.h / 2.0;
        let mut placed = false;
        for row in &mut rows {
            let row_cy =
                row.iter().map(|r| r.y + r.h / 2.0).sum::<f64>() / row.len() as f64;
            if (cy - row_cy).abs() <= avg_h * 0.5 {
                row.push(ln);
                placed = true;
                break;
            }
        }
        if !placed {
            rows.push(vec![ln]);
        }
    }

    let mut ordered_texts = Vec::with_capacity(rows.len());
    for mut row in rows {
        row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let joined = row
            .into_iter()
            .map(|l| l.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        ordered_texts.push(joined);
    }

    let mut result = ordered_texts.remove(0);
    for nxt in ordered_texts {
        if result.ends_with('-')
            && !result.ends_with("--")
            && nxt.chars().next().is_some_and(|c| c.is_alphabetic())
        {
            result.pop();
            result.push_str(&nxt);
        } else {
            result.push(' ');
            result.push_str(&nxt);
        }
    }
    let re_ws = Regex::new(r"\s+").unwrap();
    re_ws.replace_all(&result, " ").trim().to_string()
}

pub fn classify_bubble_style(lines: &[&OcrLine], _bw: f64, bh: f64) -> String {
    if lines.is_empty() {
        return "bubble_light".to_string();
    }
    let avg_white = lines.iter().map(|l| l.white_ratio).sum::<f64>() / lines.len() as f64;
    let avg_dark = lines.iter().map(|l| l.dark_ratio).sum::<f64>() / lines.len() as f64;
    let max_white = lines.iter().map(|l| l.white_ratio).fold(0.0_f64, f64::max);

    if avg_dark >= 0.56 && avg_white < 0.25 {
        return "bubble_dark".to_string();
    }
    if lines.len() == 1 && bh >= 0.045 && avg_white < 0.68 {
        return "floating".to_string();
    }
    if avg_white >= 0.54 || (lines.len() >= 2 && max_white >= 0.72 && avg_white >= 0.48) {
        return "bubble_light".to_string();
    }
    "floating".to_string()
}

pub fn resolve_bubble_overlaps(bubbles: &mut [Bubble], gap: f64) {
    let n = bubbles.len();
    if n < 2 {
        return;
    }
    let min_size = 0.022;

    for _ in 0..3 {
        let mut changed = false;
        for i in 0..n {
            for j in (i + 1)..n {
                let (ax1, ay1, aw, ah) = (bubbles[i].x, bubbles[i].y, bubbles[i].w, bubbles[i].h);
                let (bx1, by1, bw, bh) = (bubbles[j].x, bubbles[j].y, bubbles[j].w, bubbles[j].h);
                let (ax2, ay2) = (ax1 + aw, ay1 + ah);
                let (bx2, by2) = (bx1 + bw, by1 + bh);

                if !(ax1 < bx2 && bx1 < ax2 && ay1 < by2 && by1 < ay2) {
                    continue;
                }
                let ov_x = ax2.min(bx2) - ax1.max(bx1);
                let ov_y = ay2.min(by2) - ay1.max(by1);
                let norm_x = ov_x / aw.min(bw).max(1e-5);
                let norm_y = ov_y / ah.min(bh).max(1e-5);

                if norm_x <= norm_y {
                    let (li, ri) = if (ax1 + ax2) <= (bx1 + bx2) { (i, j) } else { (j, i) };
                    let lx1 = bubbles[li].x;
                    let lx2 = lx1 + bubbles[li].w;
                    let rx1 = bubbles[ri].x;
                    let rx2 = rx1 + bubbles[ri].w;
                    let mid = (lx2 + rx1) * 0.5;
                    let new_lw = (mid - gap * 0.5 - lx1).max(min_size);
                    if lx1 + new_lw + gap * 0.5 > rx2 - min_size {
                        let split = mid.min(rx2 - min_size * 0.5).max(lx1 + min_size * 0.5);
                        bubbles[li].w = (split - lx1).max(0.01);
                        bubbles[ri].x = split;
                        bubbles[ri].w = (rx2 - split).max(0.01);
                    } else {
                        bubbles[li].w = new_lw;
                        let new_rx = lx1 + new_lw + gap;
                        bubbles[ri].x = new_rx;
                        bubbles[ri].w = (rx2 - new_rx).max(min_size);
                    }
                } else {
                    let (ti, bi) = if (ay1 + ay2) <= (by1 + by2) { (i, j) } else { (j, i) };
                    let ty1 = bubbles[ti].y;
                    let ty2 = ty1 + bubbles[ti].h;
                    let by1_c = bubbles[bi].y;
                    let by2_c = by1_c + bubbles[bi].h;
                    let mid = (ty2 + by1_c) * 0.5;
                    let new_th = (mid - gap * 0.5 - ty1).max(min_size);
                    if ty1 + new_th + gap * 0.5 > by2_c - min_size {
                        let split = mid.min(by2_c - min_size * 0.5).max(ty1 + min_size * 0.5);
                        bubbles[ti].h = (split - ty1).max(0.01);
                        bubbles[bi].y = split;
                        bubbles[bi].h = (by2_c - split).max(0.01);
                    } else {
                        bubbles[ti].h = new_th;
                        let new_by = ty1 + new_th + gap;
                        bubbles[bi].y = new_by;
                        bubbles[bi].h = (by2_c - new_by).max(min_size);
                    }
                }
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    for b in bubbles.iter_mut() {
        b.x = round5(b.x.clamp(0.0, 0.99));
        b.y = round5(b.y.clamp(0.0, 0.99));
        b.w = round5(b.w.clamp(0.01, 1.0 - b.x));
        b.h = round5(b.h.clamp(0.01, 1.0 - b.y));
    }
}

pub fn group_lines_into_bubbles(ocr_lines: &[OcrLine], source_lang: &str) -> Vec<Bubble> {
    let clean: Vec<&OcrLine> = ocr_lines
        .iter()
        .filter(|l| !is_noise_line(l, source_lang))
        .collect();
    let n = clean.len();
    if n == 0 {
        return Vec::new();
    }

    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    fn union(parent: &mut [usize], i: usize, j: usize) {
        let ri = find(parent, i);
        let rj = find(parent, j);
        if ri != rj {
            parent[ri] = rj;
        }
    }

    for i in 0..n {
        let a = clean[i];
        let (ax1, ay1) = (a.x, a.y);
        let (ax2, ay2) = (ax1 + a.w, ay1 + a.h);
        let a_rid = a.bubble_region_id;
        for j in (i + 1)..n {
            let b = clean[j];
            let (bx1, by1) = (b.x, b.y);
            let (bx2, by2) = (bx1 + b.w, by1 + b.h);
            let b_rid = b.bubble_region_id;

            let lh = a.h.min(b.h);
            let max_h = a.h.max(b.h);
            if lh <= 0.0 {
                continue;
            }
            let size_ratio = max_h / lh;

            if a_rid > 0 && b_rid > 0 && a_rid != b_rid {
                continue;
            }
            let a_dark = a.dark_ratio >= 0.58 && a.white_ratio < 0.25;
            let b_dark = b.dark_ratio >= 0.58 && b.white_ratio < 0.25;
            if a_dark != b_dark {
                continue;
            }

            let v_gap = (ay1.max(by1) - ay2.min(by2)).max(0.0);
            let h_gap = (ax1.max(bx1) - ax2.min(bx2)).max(0.0);
            let h_overlap = ax2.min(bx2) - ax1.max(bx1);
            let v_overlap = ay2.min(by2) - ay1.max(by1);
            let min_w = a.w.min(b.w);
            let avg_h = (a.h + b.h) * 0.5;

            let (vertically_stacked, same_row) = if a_rid > 0 && a_rid == b_rid {
                (
                    size_ratio <= 2.8
                        && v_gap <= avg_h * 2.0
                        && v_overlap <= lh * 0.45
                        && h_overlap >= min_w * 0.12,
                    size_ratio <= 1.7 && v_overlap >= lh * 0.52 && h_gap <= lh * 0.26,
                )
            } else {
                (
                    size_ratio <= 2.6
                        && v_gap <= avg_h * 1.35
                        && v_overlap <= lh * 0.42
                        && h_overlap >= min_w * 0.28,
                    size_ratio <= 1.6 && v_overlap >= lh * 0.58 && h_gap <= lh * 0.28,
                )
            };

            if vertically_stacked || same_row {
                union(&mut parent, i, j);
            }
        }
    }

    let mut clusters: BTreeMap<usize, Vec<&OcrLine>> = BTreeMap::new();
    for (idx, line) in clean.iter().enumerate() {
        let root = find(&mut parent, idx);
        clusters.entry(root).or_default().push(*line);
    }

    struct Provisional<'a> {
        lines: Vec<&'a OcrLine>,
        text: String,
        bx: f64,
        by: f64,
        bw: f64,
        bh: f64,
        style: String,
        unique_rid: i32,
    }

    let mut provisional = Vec::new();
    let mut region_cluster_counts: BTreeMap<i32, usize> = BTreeMap::new();

    for lines in clusters.into_values() {
        let x1 = lines.iter().map(|l| l.x).fold(f64::INFINITY, f64::min);
        let y1 = lines.iter().map(|l| l.y).fold(f64::INFINITY, f64::min);
        let x2 = lines.iter().map(|l| l.x + l.w).fold(0.0_f64, f64::max);
        let y2 = lines.iter().map(|l| l.y + l.h).fold(0.0_f64, f64::max);

        let pad_x = 0.006;
        let pad_y = 0.005;
        let bx = (x1 - pad_x).max(0.0);
        let by = (y1 - pad_y).max(0.0);
        let bw = ((x2 - x1) + 2.0 * pad_x).min(1.0 - bx);
        let bh = ((y2 - y1) + 2.0 * pad_y).min(1.0 - by);

        let text = join_bubble_lines(&lines);
        if text.is_empty() {
            continue;
        }
        let style = classify_bubble_style(&lines, bw, bh);
        let rids: HashSet<i32> = lines.iter().map(|l| l.bubble_region_id).collect();
        let unique_rid = if rids.len() == 1 {
            let rid = *rids.iter().next().unwrap();
            if rid > 0 { rid } else { -1 }
        } else {
            -1
        };
        if unique_rid > 0 {
            *region_cluster_counts.entry(unique_rid).or_insert(0) += 1;
        }
        provisional.push(Provisional {
            lines,
            text,
            bx,
            by,
            bw,
            bh,
            style,
            unique_rid,
        });
    }

    let raw_boxes: Vec<(f64, f64, f64, f64)> = provisional
        .iter()
        .map(|p| (p.bx, p.by, p.bx + p.bw, p.by + p.bh))
        .collect();

    let mut bubbles = Vec::with_capacity(provisional.len());
    for (idx, item) in provisional.iter().enumerate() {
        let (mut bx, mut by, mut bw, mut bh) = (item.bx, item.by, item.bw, item.bh);
        let rid = item.unique_rid;

        if rid > 0
            && region_cluster_counts.get(&rid).copied().unwrap_or(0) == 1
            && item.style == "bubble_light"
        {
            let r = item.lines[0];
            if let (Some(rx), Some(ry), Some(rw), Some(rh)) =
                (r.region_x, r.region_y, r.region_w, r.region_h)
            {
                let (tx1, ty1, tx2, ty2) = (bx, by, bx + bw, by + bh);
                let text_w = (tx2 - tx1).max(0.01);
                let text_h = (ty2 - ty1).max(0.01);

                let mut ux1 = tx1.min(rx);
                let mut uy1 = ty1.min(ry);
                let mut ux2 = tx2.max(rx + rw);
                let mut uy2 = ty2.max(ry + rh);

                let max_w = text_w * 1.55;
                let max_h = text_h * 1.45;
                if ux2 - ux1 > max_w {
                    let extra_w = (max_w - text_w) * 0.5;
                    ux1 = ux1.max(tx1 - extra_w);
                    ux2 = ux2.min(tx2 + extra_w);
                }
                if uy2 - uy1 > max_h {
                    let extra_h = (max_h - text_h) * 0.5;
                    uy1 = uy1.max(ty1 - extra_h);
                    uy2 = uy2.min(ty2 + extra_h);
                }

                for (j, &(ox1, oy1, ox2, oy2)) in raw_boxes.iter().enumerate() {
                    if j == idx {
                        continue;
                    }
                    if uy2.min(oy2) > uy1.max(oy1) {
                        if ox1 >= tx2 - 0.005 {
                            ux2 = ux2.min(ox1 - 0.004);
                        }
                        if ox2 <= tx1 + 0.005 {
                            ux1 = ux1.max(ox2 + 0.004);
                        }
                    }
                    if ux2.min(ox2) > ux1.max(ox1) {
                        if oy1 >= ty2 - 0.005 {
                            uy2 = uy2.min(oy1 - 0.004);
                        }
                        if oy2 <= ty1 + 0.005 {
                            uy1 = uy1.max(oy2 + 0.004);
                        }
                    }
                }

                ux1 = ux1.min(tx1);
                ux2 = ux2.max(tx2);
                uy1 = uy1.min(ty1);
                uy2 = uy2.max(ty2);

                bx = ux1.max(0.0);
                by = uy1.max(0.0);
                bw = (ux2 - ux1).min(1.0 - bx);
                bh = (uy2 - uy1).min(1.0 - by);
            }
        }

        let min_conf = item
            .lines
            .iter()
            .map(|l| l.confidence as f64)
            .fold(f64::INFINITY, f64::min);

        bubbles.push(Bubble {
            id: 0,
            text: item.text.clone(),
            x: round5(bx),
            y: round5(by),
            w: round5(bw),
            h: round5(bh),
            line_count: item.lines.len(),
            min_conf: round3(min_conf),
            style: item.style.clone(),
            fr: None,
            who: None,
        });
    }

    resolve_bubble_overlaps(&mut bubbles, 0.004);

    bubbles.sort_by(|a, b| {
        let ba = (a.y / 0.22) as i64;
        let bb = (b.y / 0.22) as i64;
        ba.cmp(&bb).then_with(|| {
            let ca = -(a.x + a.w / 2.0);
            let cb = -(b.x + b.w / 2.0);
            ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    for (idx, b) in bubbles.iter_mut().enumerate() {
        b.id = idx;
    }
    bubbles
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Luma};
    use std::io::Cursor;

    #[test]
    fn pure_rust_flood_fill_detects_closed_white_bubble() {
        // Dark manga background with a crisp white enclosed speech bubble rectangle
        let mut img: ImageBuffer<Luma<u8>, Vec<u8>> = ImageBuffer::from_pixel(300, 300, Luma([20]));
        for y in 60..140 {
            for x in 80..220 {
                let border = x < 84 || x > 215 || y < 64 || y > 135;
                img.put_pixel(x, y, Luma([if border { 10 } else { 250 }]));
            }
        }
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png).unwrap();

        let raw = vec![RawOcrLine {
            text: "HELLO KOMI!".to_string(),
            confidence: 0.98,
            x: 100.0 / 300.0,
            y: 90.0 / 300.0,
            w: 100.0 / 300.0,
            h: 20.0 / 300.0,
        }];
        let res = enrich_raw_lines(&bytes, raw).unwrap();
        assert_eq!(res.lines.len(), 1);
        assert!(res.lines[0].bubble_region_id > 0);
        assert!(res.lines[0].white_ratio >= 0.8);

        let bubbles = group_lines_into_bubbles(&res.lines, "en");
        assert_eq!(bubbles.len(), 1);
        assert_eq!(bubbles[0].style, "bubble_light");
    }

    #[test]
    fn bubble_grouping_and_overlap_resolution() {
        let mut bubbles = vec![
            Bubble {
                id: 0, text: "A".into(), x: 0.1, y: 0.1, w: 0.3, h: 0.2,
                line_count: 1, min_conf: 0.9, style: "bubble_light".into(), fr: None, who: None,
            },
            Bubble {
                id: 1, text: "B".into(), x: 0.35, y: 0.1, w: 0.3, h: 0.2,
                line_count: 1, min_conf: 0.9, style: "bubble_light".into(), fr: None, who: None,
            },
        ];
        resolve_bubble_overlaps(&mut bubbles, 0.004);
        assert!(bubbles[0].x + bubbles[0].w <= bubbles[1].x + 1e-5);
    }
}
