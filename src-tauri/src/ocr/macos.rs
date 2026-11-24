use super::{image_suffix, OcrEngine};
use crate::image_proc::OcrResult;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct AppleVisionOcr;

fn is_executable(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = p.metadata() {
            return (meta.permissions().mode() & 0o111) != 0;
        }
    }
    true
}

pub fn find_vision_ocr_binary(resource_dir: Option<&PathBuf>) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(env_dir) = std::env::var("SMT_OCR_DIR") {
        let trimmed = env_dir.trim();
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed).join("vision_ocr"));
        }
    }
    if let Some(res) = resource_dir {
        candidates.push(res.join("resources").join("vision_ocr"));
        candidates.push(res.join("ocr").join("vision_ocr"));
        candidates.push(res.join("vision_ocr"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("../Resources/resources/vision_ocr"));
            candidates.push(dir.join("../Resources/ocr/vision_ocr"));
            candidates.push(dir.join("../Resources/vision_ocr"));
            candidates.push(dir.join("vision_ocr"));
        }
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest_dir.join("resources/vision_ocr"));
    candidates.push(manifest_dir.join("../ocr/vision_ocr"));

    candidates.into_iter().find(|p| is_executable(p))
}

impl OcrEngine for AppleVisionOcr {
    fn name(&self) -> &'static str {
        "Apple Vision"
    }

    fn is_ready(&self, resource_dir: Option<&PathBuf>) -> bool {
        find_vision_ocr_binary(resource_dir).is_some()
    }

    fn recognize(
        &self,
        image_bytes: &[u8],
        source_lang: &str,
        resource_dir: Option<&PathBuf>,
    ) -> Result<OcrResult, String> {
        let bin = find_vision_ocr_binary(resource_dir).ok_or_else(|| {
            "Binaire Apple Vision OCR (`vision_ocr`) introuvable dans le bundle.".to_string()
        })?;

        let suffix = image_suffix(image_bytes);
        let mut tmp = tempfile::Builder::new()
            .prefix("yomi_ocr_")
            .suffix(suffix)
            .tempfile()
            .map_err(|e| format!("Impossible de créer le fichier temporaire OCR : {}", e))?;
        tmp.write_all(image_bytes)
            .map_err(|e| format!("Erreur écriture image OCR : {}", e))?;
        tmp.flush()
            .map_err(|e| format!("Erreur flush image OCR : {}", e))?;

        let output = Command::new(&bin)
            .arg(tmp.path())
            .arg(source_lang)
            .output()
            .map_err(|e| format!("Échec lancement Apple Vision OCR : {}", e))?;

        if !output.status.success() {
            let err_txt = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if err_txt.is_empty() {
                "Erreur Apple Vision OCR".to_string()
            } else {
                err_txt
            });
        }

        let stdout_txt = String::from_utf8(output.stdout)
            .map_err(|e| format!("Sortie OCR non UTF-8 : {}", e))?;
        serde_json::from_str::<OcrResult>(&stdout_txt)
            .map_err(|e| format!("JSON Apple Vision OCR invalide : {}", e))
    }
}
