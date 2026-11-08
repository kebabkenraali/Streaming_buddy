//! Auto-fill the wiki from important chat/stream events.
//!
//! Primary sources:
//! - Twitch IRC `USERNOTICE` (subscriptions, resubs, gift subs, raids, ...)
//! - Manual chat commands: `!wikievent <title> | <content>` (moderator/streamer use)
//!
//! Creates/updates wiki entries under category `event` so the AI can reference
//! important channel moments (who subscribed, raids, milestones).

use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    pub kind: String,      // sub, resub, subgift, raid, cheer, manual, ...
    pub user: String,
    pub channel: String,
    pub detail: String,    // human-readable
    pub system_msg: String,
}

pub fn enabled(db: &Db) -> bool {
    db.settings_get("wiki_auto_events")
        .ok()
        .flatten()
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

fn stamp() -> String {
    let t = OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}Z",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute()
    )
}

fn kind_fi(kind: &str) -> &'static str {
    match kind {
        "sub" => "tilaus",
        "resub" => "jatkotilaus",
        "subgift" | "submysterygift" | "giftpaidupgrade" => "lahjatilaus",
        "raid" => "raidi",
        "cheer" | "bits" => "bitit",
        "announcement" => "ilmoitus",
        _ => "tapahtuma",
    }
}

/// Record an important event into the wiki (category `event`) and message log.
pub fn record_event(db: &Db, ev: &ChatEvent) -> Result<Option<i64>> {
    if !enabled(db) {
        return Ok(None);
    }
    let fi = kind_fi(&ev.kind);
    let title = match ev.kind.as_str() {
        "raid" => format!("Raid: {}", ev.user),
        "sub" | "resub" => format!("Subscription: {}", ev.user),
        "subgift" | "submysterygift" | "giftpaidupgrade" => {
            format!("Gift sub: {}", ev.user)
        }
        _ => format!("Event: {} ({})", ev.user, ev.kind),
    };
    let aliases = format!(
        "{},{},{}",
        ev.kind, fi, ev.user.to_lowercase()
    );
    let note = if ev.detail.is_empty() {
        format!("[{}] {} - {}", stamp(), ev.kind, ev.system_msg)
    } else {
        format!("[{}] {} - {}", stamp(), ev.kind, ev.detail)
    };
    let content = if ev.system_msg.is_empty() {
        format!("Important stream event ({fi}). {note}")
    } else {
        format!(
            "Important stream event ({fi}). System: {}. {note}",
            ev.system_msg
        )
    };
    let _ = db.add_message("event", &ev.user, &format!("{}: {}", ev.kind, ev.detail));
    let id = db.wiki_upsert_event(&title, "event", &aliases, &content)?;
    Ok(Some(id))
}

/// Parse a manual wiki-event command from chat.
/// Format: `!wikievent <title> | <content>`
/// Returns true if handled.
pub fn try_manual_command(db: &Db, sender: &str, text: &str) -> Result<bool> {
    let t = text.trim();
    let (cmd, rest) = if let Some(r) = t.strip_prefix("!wikievent") {
        ("wikievent", r.trim())
    } else if let Some(r) = t.strip_prefix("!addfact") {
        ("addfact", r.trim())
    } else {
        return Ok(false);
    };
    if rest.is_empty() {
        return Ok(true); // consumed but incomplete
    }
    let (title, content) = if let Some((a, b)) = rest.split_once('|') {
        (a.trim().to_string(), b.trim().to_string())
    } else if cmd == "addfact" {
        // !addfact <content> -> title auto
        let short: String = rest.chars().take(48).collect();
        (format!("Fact: {short}"), rest.to_string())
    } else {
        (format!("Event: {rest}"), rest.to_string())
    };
    let ev = ChatEvent {
        kind: "manual".into(),
        user: sender.to_string(),
        channel: String::new(),
        detail: content.clone(),
        system_msg: format!("manual via !{cmd}"),
    };
    // for addfact use category fact, for wikievent event
    if cmd == "addfact" {
        let _ = db.add_message("event", sender, &format!("!addfact: {content}"));
        let id = db.wiki_upsert_event(&title, "fact", "manual,fact", &content)?;
        let _ = id;
    } else {
        record_event(db, &ev)?;
    }
    Ok(true)
}

/// Best-effort extraction from plain chat text (manual mode fallback).
/// Looks for explicit subscription/event keywords; conservative to avoid noise.
pub fn try_extract_from_text(db: &Db, sender: &str, text: &str) -> Result<bool> {
    if !enabled(db) {
        return Ok(false);
    }
    let lower = text.to_lowercase();
    // explicit markers only
    let kind = if lower.contains("just subscribed") || lower.contains("uusi tilaaja") || lower.contains("tilasi kanavan") {
        Some("sub")
    } else if lower.contains("subscribed for") || lower.contains("jatkoi tilausta") {
        Some("resub")
    } else if lower.contains("gifted a sub") || lower.contains("lahjoitti tilauksen") {
        Some("subgift")
    } else if lower.contains("raiding with") || lower.contains("raidasi") {
        None // raids usually come as events; avoid false positives
    } else {
        None
    };
    if let Some(k) = kind {
        let ev = ChatEvent {
            kind: k.into(),
            user: sender.to_string(),
            channel: String::new(),
            detail: text.to_string(),
            system_msg: "extracted from chat text".into(),
        };
        record_event(db, &ev)?;
        return Ok(true);
    }
    Ok(false)
}
