//! Minimal Twitch IRC client (read + reply + event parsing).
//!
//! Connects to irc.chat.twitch.tv:6697 (TLS), requests tags/commands capabilities
//! so USERNOTICE (subs, raids, gifts) can be parsed for wiki event auto-fill.

use crate::ai;
use crate::context;
use crate::db::Db;
use crate::wiki_events::{self, ChatEvent};
use anyhow::Result;
use native_tls::TlsConnector;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_native_tls::TlsConnector as TokioTls;

pub struct TwitchHandle {
    pub running: Arc<AtomicBool>,
    pub connected: Arc<AtomicBool>,
    stop_tx: mpsc::Sender<()>,
}

impl TwitchHandle {
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = self.stop_tx.try_send(());
    }
    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "running": self.running.load(Ordering::SeqCst),
            "connected": self.connected.load(Ordering::SeqCst),
        })
    }
}

fn normalize_token(token: &str) -> String {
    let t = token.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.to_lowercase().starts_with("oauth:") {
        t.to_string()
    } else {
        format!("oauth:{t}")
    }
}

pub fn should_reply(text: &str, mode: &str, persona_name: &str) -> bool {
    let mode = mode.trim().to_lowercase();
    if mode == "off" {
        return false;
    }
    let lower = text.to_lowercase();
    let first = persona_name.split_whitespace().next().unwrap_or("").to_lowercase();
    let is_mention = !first.is_empty() && lower.contains(&first);
    let is_question = text.contains('?');
    match mode.as_str() {
        "mentions" => is_mention,
        "questions" => is_question,
        "all" => true,
        _ => is_mention || is_question, // smart
    }
}

fn parse_tags(prefix: &str) -> BTreeMap<String, String> {
    // prefix starts with @...
    let mut map = BTreeMap::new();
    let s = prefix.strip_prefix('@').unwrap_or(prefix);
    let tag_part = s.split_whitespace().next().unwrap_or(s);
    for kv in tag_part.split(';') {
        if let Some((k, v)) = kv.split_once('=') {
            // unescape Twitch tag values (minimal)
            let v = v.replace("\\s", " ").replace("\\:", ";").replace("\\\\", "\\").replace("\\r", "\r").replace("\\n", "\n");
            map.insert(k.to_string(), v);
        }
    }
    map
}

fn parse_privmsg(line: &str, tags: &BTreeMap<String, String>) -> Option<(String, String, String)> {
    // [:@tags ]:nick!user@host PRIVMSG #chan :text
    let (head, text) = line.split_once(" PRIVMSG ")?;
    let text = text.split_once(" :").map(|x| x.1).unwrap_or("").trim().to_string();
    let nick = if let Some(dn) = tags.get("display-name") {
        if !dn.is_empty() { dn.clone() } else { extract_nick(head) }
    } else {
        extract_nick(head)
    };
    let chan = text_channel(head, text.is_empty());
    Some((nick, chan, text))
}

fn extract_nick(head: &str) -> String {
    // head may contain tags; nick is after last ' :'? actually ":nick!..."
    let core = head.rsplit(' ').next().unwrap_or(head);
    let core = core.strip_prefix(':').unwrap_or(core);
    core.split('!').next().unwrap_or("unknown").to_string()
}

fn text_channel(_head: &str, _empty: bool) -> String {
    String::new()
}

fn parse_usernotice(line: &str, tags: &BTreeMap<String, String>) -> Option<ChatEvent> {
    // USERNOTICE #channel [:message]
    let msg_id = tags.get("msg-id").cloned().unwrap_or_default();
    let login = tags.get("login").or_else(|| tags.get("display-name")).cloned().unwrap_or_else(|| "unknown".into());
    let system_msg = tags.get("system-msg").cloned().unwrap_or_default();
    let user_msg = line.split_once(" USERNOTICE ").and_then(|(_, rest)| rest.split_once(" :").map(|x| x.1.to_string())).unwrap_or_default();
    let kind = match msg_id.as_str() {
        "sub" => "sub",
        "resub" => "resub",
        "subgift" => "subgift",
        "submysterygift" => "submysterygift",
        "giftpaidupgrade" | "anongiftpaidupgrade" => "giftpaidupgrade",
        "raid" => "raid",
        "announcement" => "announcement",
        _ => if msg_id.is_empty() { return None; } else { "event" },
    };
    let detail = if !user_msg.trim().is_empty() {
        format!("{system_msg} | {user_msg}")
    } else if !system_msg.is_empty() {
        system_msg.clone()
    } else {
        format!("{kind} by {login}")
    };
    Some(ChatEvent {
        kind: kind.into(),
        user: login,
        channel: String::new(),
        detail,
        system_msg,
    })
}

pub async fn spawn(db: Arc<Db>) -> Result<TwitchHandle> {
    let settings = db_settings(&db)?;
    let username = settings.get("twitch_username").cloned().unwrap_or_default();
    let token = settings.get("twitch_token").cloned().unwrap_or_default();
    let channel = settings.get("twitch_channel").cloned().unwrap_or_default();
    let mode = settings.get("twitch_reply_mode").cloned().unwrap_or_else(|| "smart".into());
    let cooldown: u64 = settings
        .get("twitch_reply_cooldown")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let persona = settings.get("persona_name").cloned().unwrap_or_else(|| "Buddy".into());

    if username.trim().is_empty() || token.trim().is_empty() || channel.trim().is_empty() {
        anyhow::bail!("Twitch needs username, oauth token and channel in Settings.");
    }

    let running = Arc::new(AtomicBool::new(true));
    let connected = Arc::new(AtomicBool::new(false));
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    let handle = TwitchHandle {
        running: running.clone(),
        connected: connected.clone(),
        stop_tx,
    };
    let _ = db.add_message("system", "twitch", "Connecting to Twitch IRC...");

    let run_flag = running.clone();
    let conn_flag = connected.clone();
    tokio::spawn(async move {
        while run_flag.load(Ordering::SeqCst) {
            let r = session_loop_inner(
                db.clone(),
                &username,
                &token,
                &channel,
                &mode,
                cooldown,
                &persona,
                run_flag.clone(),
                conn_flag.clone(),
                &mut stop_rx,
            )
            .await;
            conn_flag.store(false, Ordering::SeqCst);
            if !run_flag.load(Ordering::SeqCst) {
                break;
            }
            if let Err(e) = r {
                let _ = db.add_message(
                    "system",
                    "twitch",
                    &format!("Twitch error: {e}; reconnecting in 5s"),
                );
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = stop_rx.recv() => { run_flag.store(false, Ordering::SeqCst); break; }
            }
        }
        conn_flag.store(false, Ordering::SeqCst);
        let _ = db.add_message("system", "twitch", "Twitch IRC stopped.");
    });

    Ok(handle)
}

fn db_settings(db: &Db) -> Result<BTreeMap<String, String>> {
    let mut m = BTreeMap::new();
    for k in [
        "twitch_username","twitch_token","twitch_channel","twitch_reply_mode",
        "twitch_reply_cooldown","persona_name","openrouter_api_key","model",
        "wiki_auto_events",
    ] {
        if let Some(v) = db.settings_get(k)? { m.insert(k.into(), v); }
    }
    Ok(m)
}

// (see session_loop_inner below)
async fn session_loop_inner(
    db: Arc<Db>,
    username: &str,
    token: &str,
    channel: &str,
    mode: &str,
    cooldown: u64,
    persona: &str,
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    stop_rx: &mut mpsc::Receiver<()>,
) -> Result<()> {
    let addr = "irc.chat.twitch.tv:6697";
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let cx = TlsConnector::builder().build()?;
    let cx = TokioTls::from(cx);
    let stream = cx.connect("irc.chat.twitch.tv", tcp).await?;
    let (rd, wr) = tokio::io::split(stream);
    let mut lines = BufReader::new(rd).lines();
    let wr = Arc::new(tokio::sync::Mutex::new(wr));
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
    {
        let wr2 = wr.clone();
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                let mut w = wr2.lock().await;
                let _ = w.write_all(line.as_bytes()).await;
                let _ = w.write_all(b"\r\n").await;
                let _ = w.flush().await;
            }
        });
    }

    let pass = normalize_token(token);
    let nick = username.trim().to_lowercase();
    let chan = channel.trim().trim_start_matches('#').to_lowercase();
    let send = |s: String, tx: &mpsc::Sender<String>| { let _ = tx.try_send(s); };
    send(format!("PASS {pass}"), &out_tx);
    send(format!("NICK {nick}"), &out_tx);
    send("CAP REQ :twitch.tv/tags twitch.tv/commands".into(), &out_tx);
    send(format!("JOIN #{chan}"), &out_tx);

    let last_reply = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(cooldown + 1)));
    let chan_owned = chan.clone();
    let mode_owned = mode.to_string();
    let persona_owned = persona.to_string();

    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            _ = stop_rx.recv() => { running.store(false, Ordering::SeqCst); break; }
            line = lines.next_line() => {
                let line = match line? { Some(l) => l, None => break };
                if line.starts_with("PING") {
                    let payload = line.strip_prefix("PING").unwrap_or(" :tmi.twitch.tv");
                    let _ = out_tx.try_send(format!("PONG{payload}"));
                    continue;
                }
                // parse tags if present
                let (tags, rest) = if line.starts_with('@') {
                    if let Some((a,b)) = line.split_once(' ') { (parse_tags(a), b.to_string()) } else { (BTreeMap::new(), line.clone()) }
                } else { (BTreeMap::new(), line.clone()) };
                let full = if tags.is_empty() { line.clone() } else { format!("@tags {}", rest) };
                let _ = full;
                if rest.contains(" 001 ") || rest.contains(" 376 ") || rest.contains("End of /NAMES") || rest.contains(" JOIN #") {
                    connected.store(true, Ordering::SeqCst);
                }
                if rest.contains(" USERNOTICE ") {
                    if let Some(ev) = parse_usernotice(&rest, &tags) {
                        let mut ev = ev; ev.channel = chan_owned.clone();
                        let _ = wiki_events::record_event(&db, &ev);
                    }
                    continue;
                }
                if rest.contains(" PRIVMSG ") {
                    if let Some((nick, _c, text)) = parse_privmsg(&rest, &tags) {
                        if text.is_empty() { continue; }
                        let _ = db.add_message("twitch", &nick, &text);
                        // manual wiki commands from chat
                        if let Ok(true) = wiki_events::try_manual_command(&db, &nick, &text) {
                            continue;
                        }
                        let _ = wiki_events::try_extract_from_text(&db, &nick, &text);
                        if should_reply(&text, &mode_owned, &persona_owned) {
                            let ok = {
                                let mut lr = last_reply.lock().unwrap();
                                if lr.elapsed() >= Duration::from_secs(cooldown) {
                                    *lr = Instant::now(); true
                                } else { false }
                            };
                            if ok {
                                let db2 = db.clone();
                                let tx2 = out_tx.clone();
                                let t = text.clone();
                                let reply_chan = chan_owned.clone();
                                tokio::spawn(async move {
                                    if let Ok((reply, _)) = run_ai(&db2, &t).await {
                                        let msg: String = reply.chars().take(480).collect();
                                        let _ = tx2
                                            .send(format!(
                                                "PRIVMSG #{} :{}",
                                                reply_chan,
                                                msg.replace('\n', " ")
                                            ))
                                            .await;
                                    }
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub async fn run_ai(db: &Db, text: &str) -> Result<(String, Vec<crate::context::ScoredEntry>)> {
    let settings = db_settings(db)?;
    let hits = context::find_hits(db, text)?;
    let messages = context::build_messages(db, text, &hits, &settings)?;
    let api_key = settings.get("openrouter_api_key").cloned().unwrap_or_default();
    let model = settings.get("model").cloned().unwrap_or_else(|| "openai/gpt-4o-mini".into());
    let reply = ai::chat_complete(&api_key, &model, messages).await?;
    let persona = settings.get("persona_name").cloned().unwrap_or_else(|| "Buddy".into());
    let _ = db.add_message("ai", &persona, &reply.text);
    Ok((reply.text, hits))
}
