use super::OcrEngine;
use crate::image_proc::{enrich_raw_lines, OcrResult, RawOcrLine};
use std::path::PathBuf;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine as WinOcrEngine;
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

pub struct WindowsMediaOcr;

fn create_win_ocr_engine(source_lang: &str) -> Result<WinOcrEngine, String> {
    let preferred_tags: &[&str] = match source_lang {
        "ja" => &["ja-JP", "en-US"],
        _ => &["en-US", "ja-JP"],
    };
    for tag in preferred_tags {
        if let Ok(lang) = Language::CreateLanguage(&HSTRING::from(*tag)) {
            if let Ok(engine) = WinOcrEngine::TryCreateFromLanguage(&lang) {
                return Ok(engine);
            }
        }
    }
    WinOcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("Windows.Media.Ocr indisponible : {}", e))
}

fn recognize_win_sync(image_bytes: &[u8], source_lang: &str) -> Result<OcrResult, String> {
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|e| format!("InMemoryRandomAccessStream error: {}", e))?;
    {
        let writer = DataWriter::CreateDataWriter(&stream)
            .map_err(|e| format!("DataWriter error: {}", e))?;
        writer
            .WriteBytes(image_bytes)
            .map_err(|e| format!("WriteBytes error: {}", e))?;
        writer
            .StoreAsync()
            .map_err(|e| format!("StoreAsync error: {}", e))?
            .get()
            .map_err(|e| format!("StoreAsync wait error: {}", e))?;
        let _ = writer.DetachStream();
    }
    stream.Seek(0).map_err(|e| format!("Seek error: {}", e))?;

    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| format!("BitmapDecoder error: {}", e))?
        .get()
        .map_err(|e| format!("BitmapDecoder wait error: {}", e))?;
    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| format!("GetSoftwareBitmapAsync error: {}", e))?
        .get()
        .map_err(|e| format!("GetSoftwareBitmapAsync wait error: {}", e))?;

    let w = bitmap.PixelWidth().unwrap_or(1).max(1) as f64;
    let h = bitmap.PixelHeight().unwrap_or(1).max(1) as f64;

    let engine = create_win_ocr_engine(source_lang)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("RecognizeAsync error: {}", e))?
        .get()
        .map_err(|e| format!("RecognizeAsync wait error: {}", e))?;

    let mut raw_lines = Vec::new();
    let lines = result
        .Lines()
        .map_err(|e| format!("OCR Lines error: {}", e))?;
    for line in lines {
        let text = line
            .Text()
            .map(|s| s.to_string_lossy().trim().to_string())
            .unwrap_or_default();
        if text.is_empty() {
            continue;
        }
        let Ok(words) = line.Words() else {
            continue;
        };
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = 0.0_f64;
        let mut max_y = 0.0_f64;
        let mut any_word = false;
        for word in words {
            if let Ok(rect) = word.BoundingRect() {
                any_word = true;
                min_x = min_x.min(rect.X as f64);
                min_y = min_y.min(rect.Y as f64);
                max_x = max_x.max((rect.X + rect.Width) as f64);
                max_y = max_y.max((rect.Y + rect.Height) as f64);
            }
        }
        if !any_word {
            continue;
        }
        raw_lines.push(RawOcrLine {
            text,
            confidence: 0.92,
            x: (min_x / w).clamp(0.0, 1.0),
            y: (min_y / h).clamp(0.0, 1.0),
            w: ((max_x - min_x) / w).clamp(0.0, 1.0),
            h: ((max_y - min_y) / h).clamp(0.0, 1.0),
        });
    }

    enrich_raw_lines(image_bytes, raw_lines)
}

impl OcrEngine for WindowsMediaOcr {
    fn name(&self) -> &'static str {
        "Windows OCR"
    }

    fn is_ready(&self, _resource_dir: Option<&PathBuf>) -> bool {
        WinOcrEngine::TryCreateFromUserProfileLanguages().is_ok()
    }

    fn recognize(
        &self,
        image_bytes: &[u8],
        source_lang: &str,
        _resource_dir: Option<&PathBuf>,
    ) -> Result<OcrResult, String> {
        recognize_win_sync(image_bytes, source_lang)
    }
}
