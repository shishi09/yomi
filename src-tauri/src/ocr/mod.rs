#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub mod linux;

use crate::image_proc::OcrResult;
use std::path::PathBuf;

pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &'static str;
    fn is_ready(&self, resource_dir: Option<&PathBuf>) -> bool;
    fn recognize(
        &self,
        image_bytes: &[u8],
        source_lang: &str,
        resource_dir: Option<&PathBuf>,
    ) -> Result<OcrResult, String>;
}

pub fn active_ocr_engine() -> &'static dyn OcrEngine {
    #[cfg(target_os = "macos")]
    {
        &macos::AppleVisionOcr
    }
    #[cfg(target_os = "windows")]
    {
        &windows::WindowsMediaOcr
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        &linux::TesseractOcr
    }
}

pub fn image_suffix(image_bytes: &[u8]) -> &'static str {
    if image_bytes.starts_with(b"\x89PNG") {
        ".png"
    } else if image_bytes.starts_with(b"\xff\xd8") {
        ".jpg"
    } else if image_bytes.len() >= 12
        && &image_bytes[0..4] == b"RIFF"
        && &image_bytes[8..12] == b"WEBP"
    {
        ".webp"
    } else if image_bytes.starts_with(b"GIF8") {
        ".gif"
    } else {
        ".img"
    }
}
