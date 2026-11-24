use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::config::target_language_name;
use crate::glossary::{format_glossary_for_prompt, Glossary};
use crate::image_proc::Bubble;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationItem {
    pub fr: String,
    pub who: String,
    pub skip: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptBubbleItem<'a> {
    pub id: usize,
    pub text: &'a str,
}

fn is_skip_token(who_text: &str) -> bool {
    matches!(
        who_text.trim().to_ascii_lowercase().as_str(),
        "ignore" | "ignorer" | "skip" | "dessin" | "art" | "bruit" | "ocr noise"
    )
}

fn is_skip_item(skip_raw: Option<&Value>, fr_text: &str, who_text: &str) -> bool {
    if let Some(val) = skip_raw {
        if let Some(b) = val.as_bool() {
            if b {
                return true;
            }
        } else if let Some(s) = val.as_str() {
            if matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "oui") {
                return true;
            }
        } else if let Some(n) = val.as_i64() {
            if n == 1 {
                return true;
            }
        }
    }
    fr_text.is_empty() && is_skip_token(who_text)
}

pub fn can_skip_bubble(bubble: &Bubble) -> bool {
    let re_word = Regex::new(r"[A-Za-z]{2,}").unwrap();
    let real_words = re_word.find_iter(&bubble.text).count();
    if (bubble.style == "bubble_light" || bubble.style == "bubble_dark")
        && bubble.min_conf >= 0.85
        && real_words >= 4
    {
        return false;
    }
    true
}

pub fn repair_json_text(text: &str) -> String {
    let re_comma = Regex::new(r",\s*([}\]])").unwrap();
    let step1 = re_comma.replace_all(text, "$1");
    let re_keys = Regex::new(r"([{\[,]\s*)(translations|id|who|fr)(\s*:)").unwrap();
    re_keys.replace_all(&step1, r#"$1"$2"$3"#).to_string()
}

pub fn parse_translation_items(raw_list: Option<&Value>) -> BTreeMap<usize, TranslationItem> {
    let mut out = BTreeMap::new();
    let Some(Value::Array(items)) = raw_list else {
        return out;
    };
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let Some(id_val) = obj.get("id") else {
            continue;
        };
        let bid_opt = id_val
            .as_u64()
            .map(|u| u as usize)
            .or_else(|| id_val.as_str().and_then(|s| s.trim().parse::<usize>().ok()));
        let Some(bid) = bid_opt else {
            continue;
        };
        let fr_text = obj
            .get("fr")
            .or_else(|| obj.get("tr"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let who_text = obj
            .get("who")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let skip = is_skip_item(obj.get("skip"), &fr_text, &who_text);
        out.insert(
            bid,
            TranslationItem {
                fr: if skip { String::new() } else { fr_text },
                who: who_text,
                skip,
            },
        );
    }
    out
}

fn extract_quoted_field(frag: &str, field_name: &str) -> Option<String> {
    let pat = format!(
        r#"(?s)(?:"{f}"|\b{f}\b)\s*:\s*"((?:\\.|[^"\\])*)"\s*(?:[,}}]|"?(?:id|who|fr|skip)"?\s*:)"#,
        f = field_name
    );
    if let Ok(re) = Regex::new(&pat) {
        if let Some(caps) = re.captures(frag) {
            let raw = caps.get(1)?.as_str();
            let quoted = format!("\"{}\"", raw);
            if let Ok(s) = serde_json::from_str::<String>(&quoted) {
                return Some(s.trim().to_string());
            }
            return Some(
                raw.replace("\\\"", "\"")
                    .replace("\\n", "\n")
                    .replace("\\t", "\t")
                    .trim()
                    .to_string(),
            );
        }
    }
    None
}

fn recover_translation_items(raw_text: &str) -> Vec<Value> {
    let mut recovered = Vec::new();
    let re_obj = Regex::new(r"\{[^{}]*\}").unwrap();
    let re_id = Regex::new(r#"(?:"id"|\bid\b)\s*:\s*(\d+)"#).unwrap();
    let re_skip = Regex::new(r#"(?i)(?:"skip"|\bskip\b)\s*:\s*(true|false)"#).unwrap();

    for m in re_obj.find_iter(raw_text) {
        let frag = m.as_str();
        let repaired = repair_json_text(frag);
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&repaired) {
            if let Some(bid) = obj.get("id").and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            }) {
                let fr_s = obj.get("fr").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let who_s = obj.get("who").and_then(Value::as_str).unwrap_or("").trim().to_string();
                let skip_b = is_skip_item(obj.get("skip"), &fr_s, &who_s);
                if obj.contains_key("fr") || skip_b {
                    recovered.push(serde_json::json!({
                        "id": bid,
                        "who": who_s,
                        "fr": if skip_b { String::new() } else { fr_s },
                        "skip": skip_b,
                    }));
                    continue;
                }
            }
        }

        let Some(id_caps) = re_id.captures(frag) else {
            continue;
        };
        let Ok(bid) = id_caps[1].parse::<u64>() else {
            continue;
        };
        let fr_val = extract_quoted_field(frag, "fr");
        let who_val = extract_quoted_field(frag, "who").unwrap_or_default();
        let skip_val = re_skip
            .captures(frag)
            .and_then(|c| c.get(1))
            .map(|m| Value::String(m.as_str().to_string()));
        let skip_b = is_skip_item(skip_val.as_ref(), fr_val.as_deref().unwrap_or(""), &who_val);
        if fr_val.is_none() && !skip_b {
            continue;
        }
        recovered.push(serde_json::json!({
            "id": bid,
            "who": who_val,
            "fr": if skip_b { "" } else { fr_val.as_deref().unwrap_or("") },
            "skip": skip_b,
        }));
    }
    recovered
}

pub fn strip_markdown_fences(raw_text: &str) -> String {
    let re = Regex::new(r"(?s)^```(?:json)?\s*|\s*```$").unwrap();
    re.replace_all(raw_text.trim(), "").trim().to_string()
}

pub fn extract_json_object(raw_text: &str) -> Result<Value, String> {
    let cleaned = strip_markdown_fences(raw_text);
    if let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(&cleaned) {
        return Ok(parsed);
    }

    let start = cleaned.find('{');
    let end = cleaned.rfind('}');
    let candidate = match (start, end) {
        (Some(s), Some(e)) if e > s => &cleaned[s..=e],
        (Some(s), _) => &cleaned[s..],
        _ => cleaned.as_str(),
    };

    for attempt in [candidate.to_string(), repair_json_text(candidate)] {
        if let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(&attempt) {
            return Ok(parsed);
        }
    }

    let recovered = recover_translation_items(&cleaned);
    if !recovered.is_empty() {
        return Ok(serde_json::json!({ "translations": recovered }));
    }
    Err("Réponse JSON invalide du modèle de traduction.".to_string())
}

pub fn build_manga_system_prompt(
    source_lang: &str,
    use_vision: bool,
    glossary: Option<&Glossary>,
    target_lang: &str,
) -> String {
    let src_label = match source_lang {
        "en" => "anglais",
        "ja" => "japonais",
        "auto" => "anglais ou japonais",
        other => other,
    };
    let tgt_code = {
        let t = target_lang.trim().to_ascii_lowercase();
        if t.is_empty() { "fr".to_string() } else { t }
    };
    let tgt_name = target_language_name(&tgt_code);
    let glossary_block = format_glossary_for_prompt(glossary);

    if tgt_code != "fr" {
        let src_en = match source_lang {
            "en" => "English",
            "ja" => "Japanese",
            "auto" => "English or Japanese",
            other => other,
        };
        let vis_en = if use_vision {
            "\nVISION (ANNOTATED PAGE PROVIDED):\n\
             - Inspect the attached manga page: each bubble is boxed in red with its id number.\n\
             - Infer speaker gender, age, register, and facial expression, and fix minor OCR glitches by reading the red boxes directly.\n\
             - OCR false positives: if a red box covers pure artwork or misread Japanese SFX, return {\"id\": ..., \"who\": \"ignore\", \"fr\": \"\", \"skip\": true}.\n\
             - Never skip real dialogue, narration, or English SFX/notes.\n"
        } else {
            ""
        };
        return format!(
            "You are a professional manga localizer ({} -> {}).\n\
             Read ALL bubbles first to understand the scene before translating them into natural, idiomatic spoken {}.\n\
             {}\n\
             RULES:\n\
             1. Write natural spoken {} dialogue (never literal word-for-word). Match character tone and pronouns (HE/SHE).\n\
             2. Join split sentences smoothly across consecutive ids.\n\
             3. Keep proper nouns and Japanese terms like Golden Week, senpai, bento, ramen, sensei when appropriate.\n\
             4. Fill `who` (2-4 words) first, then write the translated dialogue inside the JSON field named `fr`.\n\
             5. Translate ALL received ids (unless visually skipped in Vision mode). Return ONLY valid JSON: \
             {{\"translations\": [{{\"id\": 0, \"who\": \"...\", \"fr\": \"...\"}}]}}\n\n\
             {}",
            src_en, tgt_name, tgt_name, vis_en, tgt_name, glossary_block
        );
    }

    let vision_block = if use_vision {
        "\nVISION (IMAGE ANNOTÉE FOURNIE) :\n\
         - Regarde attentivement la planche de manga jointe : chaque bulle est encadrée en rouge avec son numéro d'id (0, 1, 2...).\n\
         - Identifie qui parle à qui (fille/garçon -> accords grammaticaux exacts ; ado/ami -> tutoiement ; adulte/inconnu -> vouvoiement) et observe l'expression des visages (gêne, ironie, panique, cri).\n\
         - Relis directement le texte dans chaque cadre rouge sur l'image pour corriger toute erreur d'OCR !\n\
         - Faux positifs OCR : si un cadre rouge recouvre en réalité du DESSIN pur ou une ONOMATOPÉE japonaise (katakana/hiragana) mal lue par l'OCR en chiffres/lettres parasites (ex. フン lu '7%', ポカポカ lu 'TitT', D2: Drn), renvoie {\"id\": ..., \"who\": \"ignore\", \"fr\": \"\", \"skip\": true} afin de ne rien afficher par-dessus le dessin.\n\
         - Ne mets JAMAIS \"skip\": true sur une vraie bulle de dialogue, pensée, encadré narratif ou vraie note/onomatopée en anglais (ex. '*BUTTER KNIFE.', 'MAMA!!', 'PHEW', 'Gloom').\n"
    } else {
        ""
    };

    format!(
        "Tu es un adaptateur professionnel de manga ({} -> français), dans le style des éditions françaises (Pika, Kurokawa, Ki-oon).\n\
         Tu reçois les bulles d'un manga dans l'ordre de lecture (droite à gauche). Lis d'abord TOUTES les bulles pour saisir la scène et qui parle avant de traduire.\n\
         {}\n\
         RÈGLES D'ADAPTATION :\n\
         1. Écris un français ORAL, NATUREL et IDIOMATIQUE (comme parlent vraiment des lycéens ou une famille en France). Ne traduis JAMAIS mot à mot et évite les calques de l'anglais.\n\
         2. Phrases coupées sur 2 bulles : si deux id qui se suivent forment clairement une seule phrase coupée sur deux bulles, traduis l'ensemble de manière fluide en répartissant naturellement la phrase française entre les deux id.\n\
         3. Casse normale : l'OCR est souvent en MAJUSCULES ; écris en minuscules françaises normales (majuscule en début de phrase uniquement, sauf cris/titres).\n\
         4. Concision : reste court et percutant pour que le texte tienne dans une bulle de manga.\n\
         5. Correction OCR : si un mot source est légèrement abîmé par l'OCR (ex. 'KOM' -> 'Komi', 'DEMONS' -> 'démoniaque'), corrige-le grâce au contexte.\n\
         6. Analyse qui parle et le ton : pour chaque bulle, remplis d'abord `who` en 2 à 4 mots (ex. \"Komi, gênée\", \"Mère de Komi, polie\", \"Narrateur, ironique\", \"Katai, sérieux\") afin de choisir le bon genre (masculin/féminin) et le bon registre (tu/vous), puis écris la réplique française naturelle dans `fr`.\n\
         7. Traduis absolument TOUS les id reçus sans en sauter aucun (y compris les notes *... et petits textes ; sauf faux positif visuel marqué `\"skip\": true` en mode Vision). \
         Dans le texte français de `fr`, utilise des guillemets français `« ... »` (jamais de guillemets droits `\"` non échappés). \
         Réponds UNIQUEMENT avec un objet JSON valide (sans texte avant ni après) de la forme exacte : \
         {{\"translations\": [{{\"id\": 0, \"who\": \"...\", \"fr\": \"...\"}}]}}\n\n\
         CONVENTIONS MANGA VF :\n\
         - Termes japonais conservés tels quels : Golden Week (JAMAIS 'Semaine d'or'), senpai, kôhai, bento, ramen, onsen, konbini, matsuri, sensei.\n\
         - Tutoiement par défaut : en manga, tutoie TOUJOURS par défaut (`tu`, `t'`, `te`, `s'il te plaît`, `arrête`, `regarde`). N'utilise « vous » que si un personnage parle clairement à un professeur, un inconnu âgé ou un groupe (pluriel) ; en cas de doute, utilise « tu » ! Supprime -san/-kun/-chan quand c'est plus fluide en français.\n\
         - Pronoms narratifs : respecte STRICTEMENT `HE` = il / `SHE` = elle (ex. `\"WHAT A CUTIE,\" SHE THOUGHT.` -> `« Qu'elle est mignonne... » pensa-t-elle.`, JAMAIS `pensa-t-il`).\n\n\
         {}EXEMPLES DE BONNE ADAPTATION (à suivre) :\n\
         - \"WAH, YOU SCARED ME!!\" -> \"Ouah, tu m'as fait flipper !!\" (PAS \"Vous m'avez fait sursauter\")\n\
         - \"STOP! PLEASE, STOP!\" -> \"Arrête ! S'il te plaît, arrête !\" (PAS \"Arrêtez ! S'il vous plaît\")\n\
         - '\"WHAT A CUTIE,\" SHE THOUGHT.' -> '« Qu\\'elle est mignonne... » pensa-t-elle.' (respect strict de SHE -> elle)\n\
         - '\"THAT WAS HER OWN DAMN FAULT FOR BLASTING \"KITANO MIKI\" THROUGH MY HOOD AT NIGHT.\"' -> \"C'est entièrement sa faute si elle balançait « Kitano Miki » à fond dans mon quartier en pleine nuit.\" (PAS \"dans mon capot\")\n\
         - \"OUR BOSS IS STILL BAWLING OVER HOW YOU BROKE HER BELOVED BIKE!!\" -> \"Notre cheffe pleurniche encore parce que t'as bousillé sa précieuse moto !!\"\n\
         - \"*BUTTER KNIFE.\" -> \"*Couteau à beurre.\"\n\
         - \"OH DEARIE ME, YOU'RE SO NERVOUS.\" -> \"Oh ma chérie, tu es toute nerveuse.\"\n\
         - \"GOLDEN WEEK!\" -> \"La Golden Week !\" (PAS \"Semaine d'or !\")\n\
         - \"WAIT, I DON'T WANT TO BE SEEN GETTING ALL HYPED WITH MY FAMILY!\" -> \"Hors de question qu'on me voie tout excité avec ma famille !\" (PAS \"Attends, je ne veux pas être vu devenir tout excité...\")\n\
         - \"SO THEY BECOME LOW-SPIRITED!\" -> \"Du coup, ils font la tête !\" (PAS \"Donc ils deviennent bas d'esprit !\")\n\
         - \"ARE YOU FOR REAL?!\" -> \"Tu rigoles ?!\" (PAS \"Es-tu pour vrai ?!\")\n\
         - \"I'M GONNA PASS OUT...\" -> \"Je vais tourner de l'œil...\" (PAS \"Je vais passer dehors...\")",
        src_label, vision_block, glossary_block
    )
}

pub fn build_user_prompt(
    payload_items: &[PromptBubbleItem<'_>],
    context_history: &[String],
    use_vision: bool,
    target_lang: &str,
) -> String {
    let tgt_code = {
        let t = target_lang.trim().to_ascii_lowercase();
        if t.is_empty() { "fr".to_string() } else { t }
    };
    let tgt_name = target_language_name(&tgt_code);

    let clean_ctx: Vec<String> = context_history
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|s| s.chars().take(160).collect())
        .collect();

    let mut parts = Vec::new();
    if !clean_ctx.is_empty() {
        let ctx_lines: Vec<String> = clean_ctx.iter().map(|c| format!("- {}", c)).collect();
        parts.push(format!(
            "CONTEXTE DES PAGES PRÉCÉDENTES (pour garder le même ton, genre et tu/vous — NE PAS retraduire) :\n{}\n",
            ctx_lines.join("\n")
        ));
    }

    if tgt_code == "fr" {
        if use_vision {
            parts.push("Traduis en français les bulles numérotées dans l'image jointe (texte OCR indicatif ci-dessous) :\n".to_string());
        } else {
            parts.push("Traduis ces bulles de manga en français :\n".to_string());
        }
    } else if use_vision {
        parts.push(format!(
            "Translate into {} the numbered bubbles in the attached image (OCR text below for reference):\n",
            tgt_name
        ));
    } else {
        parts.push(format!("Translate these manga bubbles into {}:\n", tgt_name));
    }

    parts.push(serde_json::to_string(payload_items).unwrap_or_else(|_| "[]".to_string()));
    parts.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_recovery_handles_trailing_commas_and_truncation() {
        let broken = "```json\n{\"translations\": [{\"id\": 0, \"who\": \"Komi\", \"fr\": \"Salut !\",}, {\"id\": 1, \"who\": \"Tadano\", \"fr\": \"Ça va ?\"}\n";
        let obj = extract_json_object(broken).unwrap();
        let parsed = parse_translation_items(obj.get("translations"));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[&0].fr, "Salut !");
        assert_eq!(parsed[&1].fr, "Ça va ?");
    }
}
