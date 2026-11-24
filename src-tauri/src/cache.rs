use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::config::{model_supports_vision, AppConfig};
use crate::image_proc::{group_lines_into_bubbles, should_use_vision, Bubble, OcrResult};
use crate::ocr::active_ocr_engine;

pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_PREFETCH_ENTRIES: usize = 4;

#[derive(Debug, Clone)]
pub struct PrefetchEntry {
    pub key: (String, String),
    pub img_bytes: Vec<u8>,
    pub ocr_out: OcrResult,
    pub ocr_ms: u64,
    pub bubbles: Vec<Bubble>,
}

#[derive(Default)]
pub struct PrefetchCache {
    entries: Mutex<VecDeque<PrefetchEntry>>,
    inflight: Mutex<HashSet<(String, String)>>,
}

impl PrefetchCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take(&self, image_url: &str, source_lang: &str) -> Option<PrefetchEntry> {
        let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let pos = guard
            .iter()
            .position(|e| e.key.0 == image_url && e.key.1 == source_lang)?;
        guard.remove(pos)
    }

    pub fn insert(&self, entry: PrefetchEntry) {
        let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = guard.iter().position(|e| e.key == entry.key) {
            guard.remove(pos);
        }
        guard.push_back(entry);
        while guard.len() > MAX_PREFETCH_ENTRIES {
            guard.pop_front();
        }
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.inflight.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn try_begin_inflight(&self, image_url: &str, source_lang: &str) -> bool {
        let key = (image_url.to_string(), source_lang.to_string());
        {
            let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if entries.iter().any(|e| e.key == key) {
                return false;
            }
        }
        let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if inflight.contains(&key) {
            return false;
        }
        inflight.insert(key);
        true
    }

    pub fn end_inflight(&self, image_url: &str, source_lang: &str) {
        let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        inflight.remove(&(image_url.to_string(), source_lang.to_string()));
    }
}

pub fn is_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_global_ipv4(v4),
        IpAddr::V6(v6) => is_global_ipv6(v6),
    }
}

fn is_global_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    if ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
    {
        return false;
    }
    // 100.64.0.0/10 shared address space (CGNAT)
    if o[0] == 100 && (o[1] & 0b1100_0000) == 64 {
        return false;
    }
    // 198.18.0.0/15 benchmarking, 0.0.0.0/8 "this network", 240.0.0.0/4 reserved
    if (o[0] == 198 && (o[1] == 18 || o[1] == 19)) || o[0] == 0 || o[0] >= 240 {
        return false;
    }
    true
}

fn is_global_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_global_ipv4(v4);
    }
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    let segs = ip.segments();
    // Unique local fc00::/7
    if (segs[0] & 0xfe00) == 0xfc00 {
        return false;
    }
    // Link-local fe80::/10
    if (segs[0] & 0xffc0) == 0xfe80 {
        return false;
    }
    // Documentation 2001:db8::/32
    if segs[0] == 0x2001 && segs[1] == 0x0db8 {
        return false;
    }
    true
}

pub async fn validate_public_image_url(raw_url: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(raw_url).map_err(|_| "URL d'image invalide.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL d'image invalide.".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL d'image invalide.".to_string())?
        .trim()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    if host.is_empty()
        || host == "localhost"
        || host == "localhost.localdomain"
        || host.ends_with(".local")
    {
        return Err("Hôte local interdit pour image_url.".to_string());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_global_ip(ip) {
            return Err("Adresse IP non publique interdite pour image_url.".to_string());
        }
        return Ok(parsed);
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("Impossible de résoudre l'hôte image ({}) : {}", host, e))?;
    let mut any = false;
    for sa in addrs {
        any = true;
        if !is_global_ip(sa.ip()) {
            return Err("Adresse IP non publique interdite pour image_url.".to_string());
        }
    }
    if !any {
        return Err("Aucune adresse IP résolue pour image_url.".to_string());
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImagePayloadInput {
    #[serde(default)]
    pub image_base64: String,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub referer: String,
}

pub async fn decode_image_payload(input: &ImagePayloadInput) -> Result<Vec<u8>, String> {
    let img_b64 = input.image_base64.trim();
    if !img_b64.is_empty() {
        let raw_b64 = match img_b64.split_once(',') {
            Some((_, after)) => after,
            None => img_b64,
        };
        let raw = base64::engine::general_purpose::STANDARD
            .decode(raw_b64.trim())
            .map_err(|e| format!("Base64 invalide : {}", e))?;
        if raw.len() > MAX_IMAGE_BYTES {
            return Err("Image trop volumineuse (> 20 Mo).".to_string());
        }
        return Ok(raw);
    }

    let img_url = input.image_url.trim();
    if img_url.is_empty() {
        return Err("Aucune image fournie (image_base64 ou image_url requis).".to_string());
    }

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let mut current_url = img_url.to_string();
    for _hop in 0..6 {
        let valid = validate_public_image_url(&current_url).await?;
        let mut req = client
            .get(valid.clone())
            .header(
                "User-Agent",
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
            )
            .header("Accept", "image/avif,image/webp,image/apng,image/*,*/*;q=0.8")
            .header("Accept-Language", "fr-FR,fr;q=0.9,en;q=0.8");
        let ref_str = input.referer.trim();
        if !ref_str.is_empty() {
            req = req.header("Referer", ref_str);
            if let Ok(rp) = url::Url::parse(ref_str) {
                if let Some(h) = rp.host_str() {
                    let origin = match rp.port() {
                        Some(p) => format!("{}://{}:{}", rp.scheme(), h, p),
                        None => format!("{}://{}", rp.scheme(), h),
                    };
                    req = req.header("Origin", origin);
                }
            }
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("IMAGE_FETCH_FAILED : {}", e))?;
        if resp.status().is_redirection() {
            if let Some(loc) = resp.headers().get("location").and_then(|v| v.to_str().ok()) {
                let next_url = valid
                    .join(loc.trim())
                    .map_err(|_| "Redirection URL d'image invalide.".to_string())?;
                current_url = next_url.to_string();
                continue;
            }
            return Err("IMAGE_FETCH_FAILED : redirection sans cible.".to_string());
        }
        if !resp.status().is_success() {
            return Err(format!(
                "IMAGE_FETCH_FAILED ({}) : impossible de télécharger l'image côté serveur.",
                resp.status().as_u16()
            ));
        }
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if !ctype.is_empty()
            && !(ctype.starts_with("image/")
                || ctype == "application/octet-stream"
                || ctype == "binary/octet-stream")
        {
            return Err(format!("Type MIME non-image refusé ({}).", ctype));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("IMAGE_FETCH_FAILED : {}", e))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("Image distante trop volumineuse (> 20 Mo).".to_string());
        }
        return Ok(bytes.to_vec());
    }

    Err("Trop de redirections pour l'image.".to_string())
}

pub fn get_cache_path(
    cache_dir: &Path,
    image_bytes: &[u8],
    model: &str,
    source_lang: &str,
    target_lang: &str,
    vision: bool,
    glossary_hash: &str,
) -> PathBuf {
    let mut img_hasher = Sha256::new();
    img_hasher.update(image_bytes);
    let img_hash: String = hex::encode(img_hasher.finalize()).chars().take(16).collect();

    let key_src = format!(
        "{}|{}|{}|{}|v10som|gl={}",
        model,
        source_lang,
        target_lang,
        if vision { "vis1" } else { "vis0" },
        glossary_hash
    );
    let mut meta_hasher = Sha256::new();
    meta_hasher.update(key_src.as_bytes());
    let meta_hash: String = hex::encode(meta_hasher.finalize()).chars().take(10).collect();

    cache_dir.join(format!("{}_{}.json", img_hash, meta_hash))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPageResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub bubbles: Vec<Bubble>,
    pub image_width: u32,
    pub image_height: u32,
    pub model: String,
    pub source_lang: String,
    pub target_lang: String,
    pub vision_used: bool,
    pub glossary_used: Option<String>,
    pub cached: bool,
    pub ocr_ms: u64,
    pub translate_ms: u64,
}

pub fn find_disk_cache_hit(
    cache_dir: &Path,
    image_bytes: &[u8],
    model: &str,
    source_lang: &str,
    target_lang: &str,
    vision_mode: &str,
    glossary_hash: &str,
    cfg: &AppConfig,
) -> Option<CachedPageResult> {
    if !model_supports_vision(model, cfg) || vision_mode == "off" {
        let p = get_cache_path(
            cache_dir, image_bytes, model, source_lang, target_lang, false, glossary_hash,
        );
        if let Ok(raw) = fs::read_to_string(p) {
            return serde_json::from_str(&raw).ok();
        }
        return None;
    }
    if vision_mode == "on" {
        let p = get_cache_path(
            cache_dir, image_bytes, model, source_lang, target_lang, true, glossary_hash,
        );
        if let Ok(raw) = fs::read_to_string(p) {
            return serde_json::from_str(&raw).ok();
        }
        return None;
    }
    let p_vis = get_cache_path(
        cache_dir, image_bytes, model, source_lang, target_lang, true, glossary_hash,
    );
    if let Ok(raw) = fs::read_to_string(p_vis) {
        if let Ok(parsed) = serde_json::from_str::<CachedPageResult>(&raw) {
            return Some(parsed);
        }
    }
    let p_txt = get_cache_path(
        cache_dir, image_bytes, model, source_lang, target_lang, false, glossary_hash,
    );
    if let Ok(raw) = fs::read_to_string(p_txt) {
        if let Ok(parsed) = serde_json::from_str::<CachedPageResult>(&raw) {
            if !should_use_vision("auto", model, &parsed.bubbles, cfg) {
                return Some(parsed);
            }
        }
    }
    None
}

pub fn clear_disk_cache(cache_dir: &Path) -> usize {
    let mut deleted = 0usize;
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("json") && fs::remove_file(&p).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

pub fn prune_disk_cache(cache_dir: &Path, max_files: usize, max_age_days: u64) -> usize {
    let Ok(rd) = fs::read_dir(cache_dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let max_age = Duration::from_secs(max_age_days * 86400);
    let mut entries: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut deleted = 0usize;

    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(mtime) = p.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if now.duration_since(mtime).unwrap_or_default() > max_age {
            if fs::remove_file(&p).is_ok() {
                deleted += 1;
            }
        } else {
            entries.push((mtime, p));
        }
    }

    if entries.len() > max_files {
        entries.sort_by_key(|(mt, _)| *mt);
        let excess = entries.len() - max_files;
        for (_, p) in entries.into_iter().take(excess) {
            if fs::remove_file(&p).is_ok() {
                deleted += 1;
            }
        }
    }
    deleted
}

pub fn schedule_prefetch_next(
    prefetch_spec: Option<&ImagePayloadInput>,
    source_lang: &str,
    prefetch: Arc<PrefetchCache>,
    resource_dir: Option<PathBuf>,
) {
    let Some(spec) = prefetch_spec.cloned() else {
        return;
    };
    let img_url = spec.image_url.trim().to_string();
    let Ok(parsed) = url::Url::parse(&img_url) else {
        return;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return;
    }
    if !prefetch.try_begin_inflight(&img_url, source_lang) {
        return;
    }
    let src_lang = source_lang.to_string();
    tokio::spawn(async move {
        let res = async {
            let img_bytes = decode_image_payload(&spec).await.ok()?;
            let bytes_clone = img_bytes.clone();
            let lang_clone = src_lang.clone();
            let res_dir = resource_dir.clone();
            let t0 = std::time::Instant::now();
            let ocr_out = tokio::task::spawn_blocking(move || {
                active_ocr_engine().recognize(&bytes_clone, &lang_clone, res_dir.as_ref())
            })
            .await
            .ok()?
            .ok()?;
            let ocr_ms = t0.elapsed().as_millis() as u64;
            let bubbles = group_lines_into_bubbles(&ocr_out.lines, &src_lang);
            Some(PrefetchEntry {
                key: (img_url.clone(), src_lang.clone()),
                img_bytes,
                ocr_out,
                ocr_ms,
                bubbles,
            })
        }
        .await;

        if let Some(entry) = res {
            prefetch.insert(entry);
        }
        prefetch.end_inflight(&img_url, &src_lang);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ssrf_guard_blocks_localhost_and_private_ips() {
        assert!(validate_public_image_url("http://127.0.0.1/test.png").await.is_err());
        assert!(validate_public_image_url("http://localhost:8765/yomi.user.js").await.is_err());
        assert!(validate_public_image_url("http://192.168.1.10/page.jpg").await.is_err());
        assert!(validate_public_image_url("http://10.0.0.1/page.jpg").await.is_err());
        assert!(validate_public_image_url("file:///etc/passwd").await.is_err());
        assert!(is_global_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_global_ip("169.254.169.254".parse().unwrap()));
    }
}
