use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use unicode_normalization::UnicodeNormalization;

pub static GLOSSARIES_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryCharacter {
    pub name: String,
    #[serde(default)]
    pub gender: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Glossary {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub match_keywords: Vec<String>,
    #[serde(default)]
    pub characters: Vec<GlossaryCharacter>,
    #[serde(default)]
    pub terms: BTreeMap<String, String>,
    #[serde(default)]
    pub style_notes: String,
}

pub fn default_komi_glossary() -> Glossary {
    let mut terms = BTreeMap::new();
    terms.insert("Golden Week".to_string(), "Golden Week".to_string());
    terms.insert("Comm.".to_string(), "Chap.".to_string());
    terms.insert("Hard liner".to_string(), "Puriste".to_string());

    Glossary {
        slug: "komi-cant-communicate".to_string(),
        title: "Komi cherche ses mots (Komi Can't Communicate)".to_string(),
        match_keywords: vec![
            "komi".to_string(),
            "komi-san".to_string(),
            "komi-can-t-communicate".to_string(),
            "komi-cherche-ses-mots".to_string(),
        ],
        characters: vec![
            GlossaryCharacter {
                name: "Shôko Komi".to_string(),
                gender: "F".to_string(),
                notes: "Héroïne très timide/muette (communique par carnet ou murmures). Tutoie Tadano, Najimi et sa famille ; vouvoie les inconnus.".to_string(),
            },
            GlossaryCharacter {
                name: "Hitohito Tadano".to_string(),
                gender: "M".to_string(),
                notes: "Lycéen gentil et attentif. Masculin (« pensé-t-il » quand narré au masculin). Tutoie Komi, Najimi et Katai.".to_string(),
            },
            GlossaryCharacter {
                name: "Najimi Osana".to_string(),
                gender: "NB".to_string(),
                notes: "Ami(e) d'enfance extraverti(e) et taquin(e). Ton familier, tutoie tout le monde.".to_string(),
            },
            GlossaryCharacter {
                name: "Shôsuke Komi".to_string(),
                gender: "M".to_string(),
                notes: "Petit frère de Shôko, laconique et stoïque. Masculin.".to_string(),
            },
            GlossaryCharacter {
                name: "Shûko Komi".to_string(),
                gender: "F".to_string(),
                notes: "Mère de Shôko, très joyeuse et bavarde. Féminin, tutoie sa famille (« ma chérie »).".to_string(),
            },
            GlossaryCharacter {
                name: "Masayoshi Komi".to_string(),
                gender: "M".to_string(),
                notes: "Père de Shôko, calme et silencieux. Masculin.".to_string(),
            },
            GlossaryCharacter {
                name: "Makoto Katai".to_string(),
                gender: "M".to_string(),
                notes: "Grand garçon musclé mais très timide. Masculin, poli avec ses camarades.".to_string(),
            },
            GlossaryCharacter {
                name: "Ai Katai".to_string(),
                gender: "F".to_string(),
                notes: "Petite sœur de Makoto, look de délinquante, protectrice avec son frère (« cheffe »). Féminin.".to_string(),
            },
            GlossaryCharacter {
                name: "Rumiko Manbagi".to_string(),
                gender: "F".to_string(),
                notes: "Gyaru expressive et chaleureuse, amie proche de Komi. Féminin, ton oral vif.".to_string(),
            },
            GlossaryCharacter {
                name: "Ren Yamai".to_string(),
                gender: "F".to_string(),
                notes: "Admiratrice excessive de Komi. Féminin.".to_string(),
            },
            GlossaryCharacter {
                name: "Himiko Agari".to_string(),
                gender: "F".to_string(),
                notes: "Anxieuse et timide. Féminin.".to_string(),
            },
            GlossaryCharacter {
                name: "Shisuto Naruse".to_string(),
                gender: "M".to_string(),
                notes: "Narcissique théâtral. Masculin.".to_string(),
            },
            GlossaryCharacter {
                name: "Chûsaku Kometani".to_string(),
                gender: "M".to_string(),
                notes: "Commentateur pince-sans-rire. Masculin.".to_string(),
            },
        ],
        terms,
        style_notes: "Tutoiement naturel entre lycéens/amis et en famille ; vouvoiement envers les adultes/inconnus. Respecter strictement les pronoms HE (il) / SHE (elle) dans la narration (ex. « SHE THOUGHT » -> « pensa-t-elle »). Supprimer -san/-kun/-chan quand c'est plus fluide en français.".to_string(),
    }
}

pub fn sanitize_glossary_slug(raw: &str) -> String {
    let ascii: String = raw
        .nfkd()
        .filter(|c| c.is_ascii())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let re = Regex::new(r"[^a-z0-9]+").unwrap();
    let mut slug = re.replace_all(&ascii, "-").trim_matches('-').to_string();
    if slug == "auto" || slug == "none" {
        slug = format!("{}-series", slug);
    }
    slug.chars().take(80).collect()
}

pub fn normalize_glossary(mut g: Glossary) -> Result<Glossary, String> {
    let title = g.title.trim().to_string();
    let slug_raw = if g.slug.trim().is_empty() {
        title.clone()
    } else {
        g.slug.clone()
    };
    let slug = sanitize_glossary_slug(&slug_raw);
    if slug.is_empty() {
        return Err("Le titre ou slug du glossaire est requis.".to_string());
    }
    g.slug = slug.clone();
    g.title = if title.is_empty() { slug } else { title };
    g.match_keywords = g
        .match_keywords
        .into_iter()
        .map(|k| k.trim().to_ascii_lowercase())
        .filter(|k| !k.is_empty())
        .collect();
    g.characters = g
        .characters
        .into_iter()
        .filter_map(|mut c| {
            c.name = c.name.trim().to_string();
            if c.name.is_empty() {
                return None;
            }
            c.gender = c.gender.trim().to_ascii_uppercase();
            c.notes = c.notes.trim().to_string();
            Some(c)
        })
        .collect();
    g.terms = g
        .terms
        .into_iter()
        .filter_map(|(k, v)| {
            let kt = k.trim().to_string();
            let vt = v.trim().to_string();
            if kt.is_empty() || vt.is_empty() {
                None
            } else {
                Some((kt, vt))
            }
        })
        .collect();
    g.style_notes = g.style_notes.trim().to_string();
    Ok(g)
}

pub fn ensure_default_glossaries(data_dir: &Path, bundled_examples_dir: Option<&Path>) {
    let gloss_dir = data_dir.join("glossaries");
    let _ = fs::create_dir_all(&gloss_dir);

    if let Some(ex_dir) = bundled_examples_dir {
        if let Ok(entries) = fs::read_dir(ex_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Some(name) = p.file_name() {
                        let target = gloss_dir.join(name);
                        if !target.exists() {
                            let _ = fs::copy(&p, &target);
                        }
                    }
                }
            }
        }
    }

    let komi_path = gloss_dir.join("komi-cant-communicate.json");
    if !komi_path.exists() {
        if let Ok(json_str) = serde_json::to_string_pretty(&default_komi_glossary()) {
            let _ = fs::write(&komi_path, json_str);
        }
    }
}

pub fn list_glossaries(data_dir: &Path) -> Vec<Glossary> {
    let _guard = GLOSSARIES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let gloss_dir = data_dir.join("glossaries");
    let Ok(entries) = fs::read_dir(&gloss_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    paths.sort();

    let mut out = Vec::new();
    for p in paths {
        if let Ok(raw) = fs::read_to_string(&p) {
            if let Ok(g) = serde_json::from_str::<Glossary>(&raw) {
                if let Ok(norm) = normalize_glossary(g) {
                    out.push(norm);
                }
            }
        }
    }
    out
}

pub fn load_glossary(data_dir: &Path, slug: &str) -> Option<Glossary> {
    let clean = sanitize_glossary_slug(slug);
    if clean.is_empty() {
        return None;
    }
    let path = data_dir.join("glossaries").join(format!("{}.json", clean));
    let _guard = GLOSSARIES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let raw = fs::read_to_string(&path).ok()?;
    let g = serde_json::from_str::<Glossary>(&raw).ok()?;
    normalize_glossary(g).ok()
}

pub fn save_glossary(data_dir: &Path, input: Glossary) -> Result<Glossary, String> {
    let normalized = normalize_glossary(input)?;
    let _guard = GLOSSARIES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let gloss_dir = data_dir.join("glossaries");
    let _ = fs::create_dir_all(&gloss_dir);
    let path = gloss_dir.join(format!("{}.json", normalized.slug));
    let txt = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
    fs::write(&path, txt).map_err(|e| e.to_string())?;
    Ok(normalized)
}

pub fn delete_glossary(data_dir: &Path, slug: &str) -> bool {
    let clean = sanitize_glossary_slug(slug);
    if clean.is_empty() {
        return false;
    }
    let path = data_dir.join("glossaries").join(format!("{}.json", clean));
    let _guard = GLOSSARIES_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if path.exists() {
        fs::remove_file(&path).is_ok()
    } else {
        false
    }
}

pub fn resolve_glossary(
    data_dir: &Path,
    glossary_slug: &str,
    page_url: &str,
    page_title: &str,
) -> Option<Glossary> {
    let slug = glossary_slug.trim().to_ascii_lowercase();
    if slug == "none" {
        return None;
    }
    if !slug.is_empty() && slug != "auto" {
        return load_glossary(data_dir, &slug);
    }

    let hay_raw = format!("{} {}", page_url, page_title).to_ascii_lowercase();
    if hay_raw.trim().is_empty() {
        return None;
    }
    let re_sep = Regex::new(r"[^a-z0-9]+").unwrap();
    let hay_norm = re_sep.replace_all(&hay_raw, " ").to_string();

    for gl in list_glossaries(data_dir) {
        let mut candidates = gl.match_keywords.clone();
        candidates.push(gl.slug.clone());
        candidates.push(gl.title.clone());
        for kw in candidates {
            let kw_low = kw.trim().to_ascii_lowercase();
            if kw_low.is_empty() {
                continue;
            }
            if hay_raw.contains(&kw_low) {
                return Some(gl);
            }
            let kw_norm = re_sep.replace_all(&kw_low, " ").trim().to_string();
            if !kw_norm.is_empty() {
                let pat = format!(r"(?:^|\s){}(?:\s|$)", regex::escape(&kw_norm));
                if let Ok(re) = Regex::new(&pat) {
                    if re.is_match(&hay_norm) {
                        return Some(gl);
                    }
                }
            }
        }
    }
    None
}

pub fn compute_glossary_hash(glossary: Option<&Glossary>) -> String {
    let Some(gl) = glossary else {
        return "none".to_string();
    };
    let serialized = serde_json::to_string(gl).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hex::encode(hasher.finalize());
    digest.chars().take(8).collect()
}

pub fn format_glossary_for_prompt(glossary: Option<&Glossary>) -> String {
    let Some(gl) = glossary else {
        return String::new();
    };
    let mut lines = vec![format!("GLOSSAIRE SÉRIE ACTIVE ({}) :", gl.title)];
    if !gl.characters.is_empty() {
        lines.push("Personnages (respecter genre et registre) :".to_string());
        for c in &gl.characters {
            let g_tag = if c.gender.is_empty() {
                String::new()
            } else {
                format!(" ({})", c.gender)
            };
            let notes = if c.notes.is_empty() {
                String::new()
            } else {
                format!(" : {}", c.notes)
            };
            lines.push(format!("- {}{}{}", c.name, g_tag, notes));
        }
    }
    if !gl.terms.is_empty() {
        lines.push("Termes spécifiques :".to_string());
        for (k, v) in &gl.terms {
            lines.push(format!("- \"{}\" -> \"{}\"", k, v));
        }
    }
    if !gl.style_notes.is_empty() {
        lines.push(format!("Style série : {}", gl.style_notes));
    }
    lines.push(String::new());
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_komi_created_and_resolved_by_url() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_default_glossaries(tmp.path(), None);
        let list = list_glossaries(tmp.path());
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].slug, "komi-cant-communicate");

        let hit = resolve_glossary(tmp.path(), "auto", "https://example.org/komi-san/ch1", "Komi");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().slug, "komi-cant-communicate");

        let miss = resolve_glossary(tmp.path(), "none", "https://example.org/komi-san/ch1", "Komi");
        assert!(miss.is_none());
    }
}
