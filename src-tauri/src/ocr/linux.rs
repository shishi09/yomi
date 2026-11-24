use super::{image_suffix, OcrEngine};
use crate::image_proc::{enrich_raw_lines, OcrResult, RawOcrLine};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

pub struct TesseractOcr;

impl OcrEngine for TesseractOcr {
    fn name(&self) -> &'static str {
        "Tesseract OCR"
    }

    fn is_ready(&self, _resource_dir: Option<&PathBuf>) -> bool {
        Command::new("tesseract")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn recognize(
        &self,
        image_bytes: &[u8],
        source_lang: &str,
        _resource_dir: Option<&PathBuf>,
    ) -> Result<OcrResult, String> {
        let dyn_img = image::load_from_memory(image_bytes)
            .map_err(|e| format!("Image illisible : {}", e))?;
        let img_w = dyn_img.width().max(1) as f64;
        let img_h = dyn_img.height().max(1) as f64;

        let suffix = image_suffix(image_bytes);
        let mut tmp = tempfile::Builder::new()
            .prefix("yomi_tess_")
            .suffix(suffix)
            .tempfile()
            .map_err(|e| format!("Temp OCR error: {}", e))?;
        tmp.write_all(image_bytes)
            .map_err(|e| format!("Write OCR error: {}", e))?;
        tmp.flush().map_err(|e| format!("Flush OCR error: {}", e))?;

        let primary_lang = match source_lang {
            "ja" => "jpn+eng",
            "auto" => "eng+jpn",
            _ => "eng",
        };

        let mut output = Command::new("tesseract")
            .arg(tmp.path())
            .arg("stdout")
            .args(["--psm", "11", "-l", primary_lang, "tsv"])
            .output()
            .map_err(|e| {
                format!(
                    "Tesseract introuvable ({}). Installe 'tesseract-ocr' sur Linux.",
                    e
                )
            })?;

        if !output.status.success() && primary_lang != "eng" {
            output = Command::new("tesseract")
                .arg(tmp.path())
                .arg("stdout")
                .args(["--psm", "11", "-l", "eng", "tsv"])
                .output()
                .map_err(|e| format!("Erreur Tesseract : {}", e))?;
        }

        if !output.status.success() {
            let err_txt = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if err_txt.is_empty() {
                "Erreur Tesseract OCR".to_string()
            } else {
                err_txt
            });
        }

        let tsv = String::from_utf8_lossy(&output.stdout);
        struct LineGroup {
            words: Vec<String>,
            min_x: f64,
            min_y: f64,
            max_x: f64,
            max_y: f64,
            conf_sum: f64,
            conf_cnt: usize,
        }
        let mut groups: BTreeMap<(i32, i32, i32, i32), LineGroup> = BTreeMap::new();

        for (line_idx, row) in tsv.lines().enumerate() {
            if line_idx == 0 {
                continue;
            }
            let cols: Vec<&str> = row.split('\t').collect();
            if cols.len() < 12 {
                continue;
            }
            let level: i32 = cols[0].parse().unwrap_or(0);
            if level != 5 {
                continue;
            }
            let text = cols[11].trim();
            if text.is_empty() {
                continue;
            }
            let page_num: i32 = cols[1].parse().unwrap_or(0);
            let block_num: i32 = cols[2].parse().unwrap_or(0);
            let par_num: i32 = cols[3].parse().unwrap_or(0);
            let line_num: i32 = cols[4].parse().unwrap_or(0);
            let left: f64 = cols[6].parse().unwrap_or(0.0);
            let top: f64 = cols[7].parse().unwrap_or(0.0);
            let width: f64 = cols[8].parse().unwrap_or(0.0);
            let height: f64 = cols[9].parse().unwrap_or(0.0);
            let conf: f64 = cols[10].parse().unwrap_or(-1.0);
            if width <= 0.0 || height <= 0.0 {
                continue;
            }

            let key = (page_num, block_num, par_num, line_num);
            let entry = groups.entry(key).or_insert_with(|| LineGroup {
                words: Vec::new(),
                min_x: left,
                min_y: top,
                max_x: left + width,
                max_y: top + height,
                conf_sum: 0.0,
                conf_cnt: 0,
            });
            entry.words.push(text.to_string());
            entry.min_x = entry.min_x.min(left);
            entry.min_y = entry.min_y.min(top);
            entry.max_x = entry.max_x.max(left + width);
            entry.max_y = entry.max_y.max(top + height);
            if conf >= 0.0 {
                entry.conf_sum += (conf / 100.0).clamp(0.0, 1.0);
                entry.conf_cnt += 1;
            }
        }

        let raw_lines = groups
            .into_values()
            .filter_map(|g| {
                let text = g.words.join(" ").trim().to_string();
                if text.is_empty() {
                    return None;
                }
                let avg_conf = if g.conf_cnt > 0 {
                    (g.conf_sum / g.conf_cnt as f64) as f32
                } else {
                    0.85
                };
                Some(RawOcrLine {
                    text,
                    confidence: avg_conf,
                    x: (g.min_x / img_w).clamp(0.0, 1.0),
                    y: (g.min_y / img_h).clamp(0.0, 1.0),
                    w: ((g.max_x - g.min_x) / img_w).clamp(0.0, 1.0),
                    h: ((g.max_y - g.min_y) / img_h).clamp(0.0, 1.0),
                })
            })
            .collect();

        enrich_raw_lines(image_bytes, raw_lines)
    }
}
