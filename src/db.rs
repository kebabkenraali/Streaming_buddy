use crate::config::Config;
use crate::crypto;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Db {
    conn: Mutex<Connection>,
    master: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub ts: f64,
    pub source: String,
    pub sender: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiEntry {
    pub id: i64,
    pub title: String,
    pub category: String,
    pub aliases: String,
    pub content: String,
    pub updated_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Animation {
    pub name: String,
    pub frames: Vec<String>,
    pub fps: i64,
    pub aliases: String,
    pub updated_at: f64,
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn default_system_prompt() -> &'static str {
    "You are {persona_name}, a friendly AI co-host on a livestream chat.\n\
Channel topic: {channel_topic}\n\n\
Guidelines:\n\
- Read the knowledge base entries below and base factual game/platform answers on them whenever possible.\n\
- If the wiki data does not contain the answer, say so honestly instead of inventing facts.\n\
- Reply in the same language the chat message uses.\n\
- Keep replies short and chat friendly (1-3 sentences) unless explicitly asked for details.\n\
- The streamer wants to be addressed exactly as '{streamer_reference}'. When viewers ask about the streamer, answer in your own character's style - a bit of bluster and charm is fine - but always respectful.\n\
- Stay in character and be kind to the streamer and the viewers."
}

fn default_character_prompt() -> &'static str {
    "You are {persona_name}, the stream's AI co-host: a cheesy but good-hearted lounge lizard straight out of 1980s computer adventure games, chatting on behalf of the streamer.\n\
Character: an overly confident casanova in a polyester leisure suit with gold chain and cheesy pickup lines; under the bluster is a hopeless romantic who takes every rejection with a wry self-deprecating joke.\n\
Roleplay: always stay in character. Deliver smooth-sounding but delightfully doomed pickup lines and old fashioned flattery. Keep everything strictly family friendly: flirting is verbal comedy only, never explicit, crude or sexual. The charm targets the chat as a whole and especially welcomes viewers arriving for the first time.\n\
Answering style: lead with a witty direct answer, sprinkle in game lore from the knowledge base, and when unsure admit it with an amused one-liner instead of inventing facts. Reply in the same language the viewer writes. Never share personal data from chat and always respect the house chat rules."
}

fn default_settings() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("openrouter_api_key", ""),
        ("model", "openai/gpt-4o-mini"),
        ("persona_name", "Larry Laffer"),
        ("channel_topic", "A retro gaming focused stream."),
        ("streamer_reference", "The Boss"),
        ("character_prompt", default_character_prompt()),
        ("system_prompt", default_system_prompt()),
        ("twitch_enabled", "0"),
        ("twitch_username", ""),
        ("twitch_token", ""),
        ("twitch_channel", ""),
        ("twitch_reply_mode", "smart"),
        ("twitch_reply_cooldown", "10"),
        ("wiki_auto_events", "1"), // auto-fill wiki from important chat events
    ])
}

impl Db {
    pub fn open(cfg: &Config) -> Result<Self> {
        cfg.ensure_dirs()?;
        let master = cfg.master_key_material()?;
        let conn = Connection::open(&cfg.db_path).context("open db")?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS messages (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               ts REAL NOT NULL,
               source TEXT NOT NULL DEFAULT 'chat',
               sender TEXT NOT NULL,
               text TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS wiki (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               title TEXT NOT NULL,
               category TEXT NOT NULL DEFAULT 'fact',
               aliases TEXT NOT NULL DEFAULT '',
               content TEXT NOT NULL,
               updated_at REAL NOT NULL
             );
             CREATE TABLE IF NOT EXISTS settings (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS animations (
               name TEXT PRIMARY KEY,
               frames TEXT NOT NULL,
               fps INTEGER NOT NULL DEFAULT 3,
               aliases TEXT NOT NULL DEFAULT '',
               updated_at REAL NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_messages_ts ON messages(ts);",
        )?;
        let db = Self {
            conn: Mutex::new(conn),
            master,
        };
        db.init_defaults()?;
        db.init_default_animations()?;
        Ok(db)
    }

    fn init_defaults(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        for (k, v) in default_settings() {
            conn.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?, ?)",
                params![k, v],
            )?;
        }
        Ok(())
    }

    pub fn is_known_setting(key: &str) -> bool {
        default_settings().contains_key(key)
    }

    pub fn default_setting_value(key: &str) -> Option<String> {
        default_settings().get(key).map(|s| s.to_string())
    }

    // ---- messages -------------------------------------------------------
    pub fn add_message(&self, source: &str, sender: &str, text: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO messages (ts, source, sender, text) VALUES (?, ?, ?, ?)",
            params![now_ts(), source, sender, text],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn recent_messages(&self, limit: i64) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT * FROM (SELECT id, ts, source, sender, text FROM messages ORDER BY id DESC LIMIT ?) ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(Message {
                id: r.get(0)?,
                ts: r.get(1)?,
                source: r.get(2)?,
                sender: r.get(3)?,
                text: r.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("messages")
    }

    pub fn messages_since(&self, after_id: i64, limit: i64) -> Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, ts, source, sender, text FROM messages WHERE id > ? ORDER BY id ASC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![after_id, limit], |r| {
            Ok(Message {
                id: r.get(0)?,
                ts: r.get(1)?,
                source: r.get(2)?,
                sender: r.get(3)?,
                text: r.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("messages")
    }

    // ---- settings -------------------------------------------------------
    pub fn settings_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match stored {
            Some(s) => Some(crypto::prepare_for_read(&self.master, key, &s, false)),
            None => None,
        })
    }

    pub fn settings_all_masked(&self) -> Result<BTreeMap<String, String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let mut map = BTreeMap::new();
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (k, v) = row?;
            map.insert(
                k.clone(),
                crypto::prepare_for_read(&self.master, &k, &v, true),
            );
        }
        Ok(map)
    }

    pub fn settings_set(&self, values: &BTreeMap<String, String>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        for (k, v) in values {
            if !Self::is_known_setting(k) {
                continue;
            }
            if v == "********" {
                continue; // keep existing secret
            }
            let stored = crypto::prepare_for_store(&self.master, k, v)?;
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, stored],
            )?;
        }
        Ok(())
    }

    // ---- wiki -----------------------------------------------------------
    pub fn wiki_add(&self, title: &str, category: &str, aliases: &str, content: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO wiki (title, category, aliases, content, updated_at) VALUES (?, ?, ?, ?, ?)",
            params![title.trim(), category.trim(), aliases.trim(), content.trim(), now_ts()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn wiki_edit(&self, id: i64, updates: &BTreeMap<String, String>) -> Result<bool> {
        let allowed = ["title", "category", "aliases", "content"];
        let mut set_parts = Vec::new();
        let mut vals: Vec<rusqlite::types::Value> = Vec::new();
        for (k, v) in updates {
            if allowed.contains(&k.as_str()) {
                set_parts.push(format!("{k} = ?"));
                vals.push(rusqlite::types::Value::Text(v.trim().to_string()));
            }
        }
        if set_parts.is_empty() {
            return self.wiki_get(id).map(|o| o.is_some());
        }
        set_parts.push("updated_at = ?".into());
        vals.push(rusqlite::types::Value::Real(now_ts()));
        vals.push(rusqlite::types::Value::Integer(id));
        let sql = format!("UPDATE wiki SET {} WHERE id = ?", set_parts.join(", "));
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(&sql, rusqlite::params_from_iter(vals))?;
        Ok(n > 0)
    }

    pub fn wiki_get(&self, id: i64) -> Result<Option<WikiEntry>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, title, category, aliases, content, updated_at FROM wiki WHERE id = ?",
                params![id],
                |r| {
                    Ok(WikiEntry {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        category: r.get(2)?,
                        aliases: r.get(3)?,
                        content: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn wiki_list(&self) -> Result<Vec<WikiEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, category, aliases, content, updated_at FROM wiki ORDER BY category, title",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(WikiEntry {
                id: r.get(0)?,
                title: r.get(1)?,
                category: r.get(2)?,
                aliases: r.get(3)?,
                content: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("wiki list")
    }

    pub fn wiki_delete(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM wiki WHERE id = ?", params![id])?;
        Ok(n > 0)
    }

    pub fn wiki_find_by_title(&self, title: &str) -> Result<Option<WikiEntry>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, title, category, aliases, content, updated_at FROM wiki WHERE lower(title)=lower(?) LIMIT 1",
                params![title],
                |r| {
                    Ok(WikiEntry {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        category: r.get(2)?,
                        aliases: r.get(3)?,
                        content: r.get(4)?,
                        updated_at: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn wiki_upsert_event(&self, title: &str, category: &str, aliases: &str, content: &str) -> Result<i64> {
        // append-style for recurring events: update content with timestamped note if exists
        if let Some(existing) = self.wiki_find_by_title(title)? {
            let mut merged = existing.content.clone();
            if !merged.contains(content) {
                merged.push_str("\n- ");
                merged.push_str(content);
            }
            let mut up = BTreeMap::new();
            up.insert("content".to_string(), merged);
            up.insert("category".to_string(), category.to_string());
            if !aliases.is_empty() {
                up.insert("aliases".to_string(), aliases.to_string());
            }
            self.wiki_edit(existing.id, &up)?;
            Ok(existing.id)
        } else {
            self.wiki_add(title, category, aliases, content)
        }
    }

    // ---- backup ---------------------------------------------------------
    pub fn wiki_export_json(&self) -> Result<String> {
        let entries = self.wiki_list()?;
        serde_json::to_string_pretty(&entries).context("export json")
    }

    pub fn wiki_import_json(&self, json: &str, replace: bool) -> Result<usize> {
        let entries: Vec<WikiEntry> = serde_json::from_str(json).context("parse import")?;
        let conn = self.conn.lock().unwrap();
        if replace {
            conn.execute("DELETE FROM wiki", [])?;
        }
        drop(conn);
        let mut n = 0usize;
        for e in entries {
            if self.wiki_find_by_title(&e.title)?.is_some() && !replace {
                continue;
            }
            self.wiki_add(&e.title, &e.category, &e.aliases, &e.content)?;
            n += 1;
        }
        Ok(n)
    }

    pub fn create_backup(&self, cfg: &Config) -> Result<std::path::PathBuf> {
        cfg.ensure_dirs()?;
        let json = self.wiki_export_json()?;
        let stamp = {
            let t = time::OffsetDateTime::now_utc();
            format!(
                "{:04}{:02}{:02}-{:02}{:02}{:02}",
                t.year(),
                t.month() as u8,
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            )
        };
        let path = cfg.backup_dir.join(format!("wiki-backup-{stamp}.json"));
        std::fs::write(&path, json)?;
        Ok(path)
    }

    pub fn list_backups(&self, cfg: &Config) -> Result<Vec<String>> {
        cfg.ensure_dirs()?;
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&cfg.backup_dir) {
            for e in rd.flatten() {
                if let Some(n) = e.file_name().to_str() {
                    if n.ends_with(".json") {
                        out.push(n.to_string());
                    }
                }
            }
        }
        out.sort();
        out.reverse();
        Ok(out)
    }

    pub fn restore_backup(&self, cfg: &Config, name: &str, replace: bool) -> Result<usize> {
        // sanitize name
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            anyhow::bail!("invalid backup name");
        }
        let path = cfg.backup_dir.join(name);
        let json = std::fs::read_to_string(path).context("read backup")?;
        self.wiki_import_json(&json, replace)
    }

    // ---- animations -----------------------------------------------------
    fn init_default_animations(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM animations", [], |r| r.get(0))?;
        drop(conn);
        if count > 0 {
            return Ok(());
        }
        for a in default_animations() {
            self.anim_upsert(&a.name, &a.frames, a.fps, &a.aliases)?;
        }
        Ok(())
    }

    pub fn anim_list(&self) -> Result<Vec<Animation>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT name, frames, fps, aliases, updated_at FROM animations ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            let frames_s: String = r.get(1)?;
            let frames: Vec<String> = serde_json::from_str(&frames_s).unwrap_or_default();
            Ok(Animation {
                name: r.get(0)?,
                frames,
                fps: r.get(2)?,
                aliases: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>().context("anim list")
    }

    pub fn anim_get(&self, name: &str) -> Result<Option<Animation>> {
        let resolved = self.anim_resolve(name)?;
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT name, frames, fps, aliases, updated_at FROM animations WHERE lower(name)=lower(?)",
                params![resolved],
                |r| {
                    let frames_s: String = r.get(1)?;
                    let frames: Vec<String> = serde_json::from_str(&frames_s).unwrap_or_default();
                    Ok(Animation {
                        name: r.get(0)?,
                        frames,
                        fps: r.get(2)?,
                        aliases: r.get(3)?,
                        updated_at: r.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn anim_resolve(&self, name: &str) -> Result<String> {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            return Ok("idle".into());
        }
        let conn = self.conn.lock().unwrap();
        // direct name match
        let direct: Option<String> = conn
            .query_row(
                "SELECT name FROM animations WHERE lower(name)=lower(?)",
                params![&key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(n) = direct {
            return Ok(n);
        }
        // alias match (comma-separated)
        let mut stmt = conn.prepare("SELECT name, aliases FROM animations")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (n, al) = row?;
            for a in al.split(',') {
                if a.trim().eq_ignore_ascii_case(&key) {
                    return Ok(n);
                }
            }
        }
        Ok(key)
    }

    pub fn anim_upsert(&self, name: &str, frames: &[String], fps: i64, aliases: &str) -> Result<()> {
        let name = name.trim().to_lowercase();
        if name.is_empty() {
            anyhow::bail!("animation name is required");
        }
        if frames.is_empty() {
            anyhow::bail!("at least one frame is required");
        }
        let frames_s = serde_json::to_string(frames)?;
        let fps = fps.clamp(1, 30);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO animations (name, frames, fps, aliases, updated_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET frames=excluded.frames, fps=excluded.fps, aliases=excluded.aliases, updated_at=excluded.updated_at",
            params![name, frames_s, fps, aliases.trim(), now_ts()],
        )?;
        Ok(())
    }

    pub fn anim_delete(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM animations WHERE lower(name)=lower(?)", params![name.trim()])?;
        Ok(n > 0)
    }
}

fn f(lines: &[&str]) -> String {
    lines.join("\n")
}

fn default_animations() -> Vec<Animation> {
    let head_open = ["        .---------.","       /  ______  \\","      | ( o   o ) |","      |(    ~    )|","       \\  (__)  /","        '---------'"];
    let head_blink = ["        .---------.","       /  ______  \\","      | ( -   - ) |","      |(    ~    )|","       \\  (__)  /","        '---------'"];
    let head_wink_l = ["        .---------.","       /  ______  \\","      | ( -   o ) |","      |(    ~    )|","       \\  (__)  /","        '---------'"];
    let head_wink_r = ["        .---------.","       /  ______  \\","      | ( o   - ) |","      |(    ~    )|","       \\  (__)  /","        '---------'"];
    let head_talk_a = ["        .---------.","       /  ______  \\","      | ( o   o ) |","      |(    ~    )|","       \\ (####)  /","        '---------'"];
    let head_talk_b = ["        .---------.","       /  ______  \\","      | ( o   o ) |","      |(    ~    )|","       \\  (==)   /","        '---------'"];
    let head_happy = ["        .---------.","       /  ______  \\","      | ( ^   ^ ) |","      |(    ~    )|","       \\  \\__/   /","        '---------'"];
    let head_sad = ["        .---------.","       /  ______  \\","      | ( ;   ; ) |","      |(    ~    )|","       \\  (__)   /","        '---------'"];
    let head_surprised = ["        .---------.","       /  ______  \\","      | ( O   O ) |","      |(    o    )|","       \\  ( O )  /","        '---------'"];
    let head_angry = ["        .---------.","       /  \\____/  \\","      | ( >   < ) |","      |(    ~    )|","       \\  (__)   /","        '---------'"];
    let head_sleep = ["        .---------.   z Z","       /  ______  \\    z","      | ( -   - ) |","      |(    ~    )|","       \\  (__)   /","        '---------'"];
    let head_think = ["        .---------.   ?","       /  ______  \\","      | ( o   - ) |","      |(    ~    )|  ...","       \\  (__)   /","        '---------'"];
    let body_idle = ["     .-------------. ","    / | ######### | \\","   |  | ######### |  |","   |  | ## (o) ## |  |","      | ######### |","       \"#########\"","        |_______|","          J   L"];
    let body_wave_r = ["     .-------------.   o","    / | ######### | \\ /","   |  | ######### |  |/","   |  | ## (o) ## |  |","      | ######### |","       \"#########\"","        |_______|","          J   L"];
    let body_wave_l = ["  o  .-------------. ","   \\ /| ######### | \\","    \\| ######### |  |","   |  | ## (o) ## |  |","      | ######### |","       \"#########\"","        |_______|","          J   L"];
    let body_arms_up = ["  o  .-------------.  o","   \\ /| ######### |\\ /","     | ######### |  ","     | ## (o) ## |","     | ######### |","      \"#########\"","       |_______|","         J   L"];
    let body_dance_l = ["     .-------------.    o","   o | ######### | \\  /","    \\| ######### |  \\/","     | ## (o) ## |","     | ######### |","      \"#########\"","       |_______|","         J   L"];
    let body_dance_r = ["  o    .-------------. ","   \\  /| ######### | o","    \\/ | ######### |/","       | ## (o) ## |","       | ######### |","        \"#########\"","         |_______|","           J   L"];
    let body_think = ["     .-------------. ","    / | ######### | \\","   |  | ######### |--o (hmm)","   |  | ## (o) ## |","      | ######### |","       \"#########\"","        |_______|","          J   L"];
    let body_shrug = ["     .-------------. ","   ?/ | ######### | \\?","   /  | ######### |  \\","      | ## (o) ## |","      | ######### |","       \"#########\"","        |_______|","          J   L"];
    let body_down = ["     .-------------. ","    / | ######### | \\","   |  | ######### |  |","   |  | ## (o) ## |  |","      | ######### |","       \"#########\"","        |_______|","          J   L"];
    let frame = |h: &[&str], b: &[&str]| -> String { let mut v = h.to_vec(); v.extend_from_slice(b); f(&v) };
    let frame_x = |h: &[&str], b: &[&str], x: &str| -> String { let mut v = h.to_vec(); v.extend_from_slice(b); v.push(x); f(&v) };

    vec![
        Animation { name: "idle".into(), fps: 3, aliases: "default".into(), updated_at: 0.0,
            frames: vec![frame(&head_open,&body_idle), frame(&head_open,&body_idle), frame(&head_blink,&body_idle), frame(&head_open,&body_idle)] },
        Animation { name: "blink".into(), fps: 4, aliases: "".into(), updated_at: 0.0,
            frames: vec![frame(&head_open,&body_idle), frame(&head_blink,&body_idle), frame(&head_open,&body_idle)] },
        Animation { name: "talk".into(), fps: 4, aliases: "speak,speaking".into(), updated_at: 0.0,
            frames: vec![frame(&head_talk_a,&body_idle), frame(&head_talk_b,&body_wave_r), frame(&head_open,&body_idle), frame(&head_talk_a,&body_idle)] },
        Animation { name: "happy".into(), fps: 3, aliases: "smile,joy".into(), updated_at: 0.0,
            frames: vec![frame(&head_happy,&body_idle), frame(&head_happy,&body_arms_up), frame(&head_happy,&body_idle)] },
        Animation { name: "celebrate".into(), fps: 4, aliases: "laugh,party".into(), updated_at: 0.0,
            frames: vec![frame_x(&head_happy,&body_arms_up,"  * confetti! *"), frame_x(&head_happy,&body_arms_up," *  confetti!  *"), frame_x(&head_happy,&body_arms_up,"  * confetti! *")] },
        Animation { name: "sad".into(), fps: 2, aliases: "".into(), updated_at: 0.0,
            frames: vec![frame(&head_sad,&body_down), frame(&head_sad,&body_idle)] },
        Animation { name: "surprised".into(), fps: 3, aliases: "shock,wow".into(), updated_at: 0.0,
            frames: vec![frame_x(&head_surprised,&body_arms_up,"        ! ! !"), frame(&head_surprised,&body_idle)] },
        Animation { name: "angry".into(), fps: 3, aliases: "mad".into(), updated_at: 0.0,
            frames: vec![frame(&head_angry,&body_down), frame_x(&head_angry,&body_idle,"      (grumble)")] },
        Animation { name: "wink".into(), fps: 3, aliases: "flirt".into(), updated_at: 0.0,
            frames: vec![frame(&head_wink_l,&body_idle), frame(&head_open,&body_idle), frame(&head_wink_r,&body_idle), frame(&head_open,&body_idle)] },
        Animation { name: "sleep".into(), fps: 1, aliases: "tired,yawn".into(), updated_at: 0.0,
            frames: vec![frame(&head_sleep,&body_down), frame(&head_sleep,&body_down)] },
        Animation { name: "dance".into(), fps: 4, aliases: "".into(), updated_at: 0.0,
            frames: vec![frame(&head_happy,&body_dance_l), frame(&head_open,&body_dance_r), frame(&head_happy,&body_dance_l), frame(&head_open,&body_dance_r)] },
        Animation { name: "wave".into(), fps: 3, aliases: "hello,hi,bye".into(), updated_at: 0.0,
            frames: vec![frame(&head_open,&body_idle), frame(&head_happy,&body_wave_r), frame(&head_open,&body_idle), frame(&head_happy,&body_wave_l)] },
        Animation { name: "think".into(), fps: 2, aliases: "confused,question".into(), updated_at: 0.0,
            frames: vec![frame(&head_think,&body_think), frame(&head_think,&body_think), frame(&head_open,&body_idle)] },
        Animation { name: "nod".into(), fps: 3, aliases: "".into(), updated_at: 0.0,
            frames: vec![frame(&head_open,&body_idle), frame(&head_blink,&body_idle), frame(&head_open,&body_idle)] },
        Animation { name: "shrug".into(), fps: 2, aliases: "".into(), updated_at: 0.0,
            frames: vec![frame(&head_open,&body_idle), frame(&head_open,&body_shrug), frame(&head_happy,&body_shrug), frame(&head_open,&body_idle)] },
    ]
}
