use crate::db::{Db, WikiEntry};
use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

static TOKEN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^\w\-]+").unwrap());

fn stopwords() -> HashSet<&'static str> {
    [
        "the", "and", "for", "you", "your", "that", "this", "with", "was", "are", "have",
        "has", "had", "but", "not", "what", "how", "who", "did", "does", "its", "him",
        "her", "she", "his", "one", "can", "get", "got", "will", "und", "der", "die",
        "das", "ja", "on", "että", "oli", "kuin", "vai", "mitä", "kuka", "voi", "sen",
        "gg", "lol", "xd", "brb", "afk",
    ]
    .into_iter()
    .collect()
}

pub fn extract_keywords(text: &str) -> Vec<String> {
    let sw = stopwords();
    let lower = text.to_lowercase();
    TOKEN_RE
        .split(&lower)
        .filter(|t| t.len() >= 2 && !sw.contains(*t))
        .map(|s| s.to_string())
        .collect()
}

fn count_matches(token: &str, hay: &str) -> i32 {
    if token.is_empty() {
        return 0;
    }
    if token.chars().count() < 4 {
        // whole word boundary for short tokens
        let pat = format!(r"\b{}\b", regex::escape(token));
        if let Ok(re) = Regex::new(&pat) {
            return re.find_iter(hay).count() as i32;
        }
    }
    hay.matches(token).count() as i32
}

#[derive(Debug, Clone, Serialize)]
pub struct ScoredEntry {
    #[serde(flatten)]
    pub entry: WikiEntry,
    pub score: i32,
}

pub fn wiki_search(db: &Db, tokens: &[String], max_results: usize, min_score: i32) -> Result<Vec<ScoredEntry>> {
    if tokens.is_empty() {
        return Ok(vec![]);
    }
    let entries = db.wiki_list()?;
    let mut scored = Vec::new();
    for e in entries {
        let title = e.title.to_lowercase();
        let aliases = e.aliases.to_lowercase();
        let content = e.content.to_lowercase();
        let mut score = 0i32;
        for t in tokens {
            let ct = count_matches(t, &title);
            let ca = count_matches(t, &aliases);
            let cc = count_matches(t, &content);
            if ct > 0 {
                score += 4;
            }
            if ca > 0 {
                score += 3;
            }
            if cc > 0 {
                score += cc.min(3);
            }
        }
        if score >= min_score {
            scored.push(ScoredEntry { entry: e, score });
        }
    }
    scored.sort_by(|a, b| b.score.cmp(&a.score));
    scored.truncate(max_results);
    Ok(scored)
}

pub fn find_hits(db: &Db, text: &str) -> Result<Vec<ScoredEntry>> {
    wiki_search(db, &extract_keywords(text), 5, 2)
}

fn format_map(template: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

pub fn build_messages(
    db: &Db,
    text: &str,
    hits: &[ScoredEntry],
    settings: &BTreeMap<String, String>,
) -> Result<Vec<serde_json::Value>> {
    let persona = settings.get("persona_name").cloned().unwrap_or_else(|| "Buddy".into());
    let topic = settings.get("channel_topic").cloned().unwrap_or_default();
    let reference = settings
        .get("streamer_reference")
        .cloned()
        .unwrap_or_else(|| "The Boss".into());
    let mut vars = BTreeMap::new();
    vars.insert("persona_name", persona.clone());
    vars.insert("channel_topic", topic);
    vars.insert("streamer_reference", reference);

    let sys_tpl = settings
        .get("system_prompt")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| crate::db::Db::default_setting_value("system_prompt").unwrap_or_default());
    let char_tpl = settings
        .get("character_prompt")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| crate::db::Db::default_setting_value("character_prompt").unwrap_or_default());

    let base = format_map(&sys_tpl, &vars);
    let character = format_map(&char_tpl, &vars);
    let mut parts = vec![
        base,
        "---\nCharacter and roleplay instructions:\n".to_string() + &character,
    ];

    // recent chat
    let recent = db.recent_messages(30)?;
    let history: Vec<String> = recent
        .iter()
        .filter(|m| m.source == "chat" || m.source == "chat_file" || m.source == "twitch")
        .map(|m| format!("{}: {}", m.sender, m.text))
        .collect();
    let tail: Vec<&String> = history.iter().rev().take(20).collect();
    if !tail.is_empty() {
        let lines: Vec<&str> = tail.into_iter().rev().map(|s| s.as_str()).collect();
        parts.push("Recent chat log (oldest first):\n".to_string() + &lines.join("\n"));
        let _ = lines; // silence
    }

    if !hits.is_empty() {
        let mut lines = Vec::new();
        for h in hits {
            let alias = if h.entry.aliases.is_empty() {
                String::new()
            } else {
                format!(" (aliases: {})", h.entry.aliases)
            };
            lines.push(format!(
                "- [{}] {}{}: {}",
                h.entry.category, h.entry.title, alias, h.entry.content
            ));
        }
        parts.push("Knowledge base entries matched to this message:\n".to_string() + &lines.join("\n"));
    } else {
        parts.push("No knowledge base entries matched this message. Answer from general knowledge only, and keep it light.".into());
    }

    Ok(vec![
        serde_json::json!({"role":"system","content": parts.join("\n\n")}),
        serde_json::json!({"role":"user","content": text}),
    ])
}
