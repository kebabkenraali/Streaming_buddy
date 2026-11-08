use crate::config::Config;
use crate::db::Db;
use crate::{context, twitch, wiki_events};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Serialize, Deserialize)]
pub struct VtuberState {
    pub anim: String,
    pub until_ms: i64,
}

impl Default for VtuberState {
    fn default() -> Self {
        Self {
            anim: "idle".into(),
            until_ms: 0,
        }
    }
}

pub struct AppState {
    pub db: Arc<Db>,
    pub cfg: Config,
    pub twitch: Mutex<Option<twitch::TwitchHandle>>,
    pub vtuber: Mutex<VtuberState>,
}

#[derive(Deserialize)]
struct AfterQ {
    after: Option<i64>,
}
#[derive(Deserialize)]
struct ChatIn {
    sender: Option<String>,
    text: String,
    answer: Option<bool>,
}
#[derive(Deserialize)]
struct WikiIn {
    title: String,
    category: Option<String>,
    aliases: Option<String>,
    content: String,
}
#[derive(Deserialize)]
struct WikiUpdate {
    title: Option<String>,
    category: Option<String>,
    aliases: Option<String>,
    content: Option<String>,
}
#[derive(Deserialize)]
struct SettingsIn {
    values: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct BackupRestore {
    name: String,
    replace: Option<bool>,
}
#[derive(Serialize)]
struct ApiErr {
    detail: String,
}

fn err(code: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ApiErr>) {
    (code, Json(ApiErr { detail: msg.into() }))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/vtuber", get(vtuber_page))
        .route("/api/messages", get(get_messages))
        .route("/api/chat", post(post_chat))
        .route("/api/wiki", get(get_wiki).post(add_wiki))
        .route("/api/wiki/search", get(wiki_search))
        .route("/api/wiki/:id", get(get_wiki_one).put(update_wiki).delete(delete_wiki))
        .route("/api/animations", get(list_animations).post(upsert_animation))
        .route("/api/animations/:name", get(get_animation).put(put_animation).delete(delete_animation))
        .route("/api/vtuber/state", get(vtuber_state))
        .route("/api/vtuber/play", post(vtuber_play))
        .route("/api/settings", get(get_settings).post(post_settings))
        .route("/api/backup/create", post(backup_create))
        .route("/api/backup/list", get(backup_list))
        .route("/api/backup/restore", post(backup_restore))
        .route("/api/twitch/start", post(twitch_start))
        .route("/api/twitch/stop", post(twitch_stop))
        .route("/api/twitch/status", get(twitch_status))
        .route("/api/server/status", get(server_status))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn vtuber_page() -> Html<&'static str> {
    Html(VTUBER_HTML)
}

async fn get_messages(
    State(st): State<Arc<AppState>>,
    Query(q): Query<AfterQ>,
) -> impl IntoResponse {
    let res = match q.after {
        Some(a) if a > 0 => st.db.messages_since(a, 200),
        _ => st.db.recent_messages(100),
    };
    match res {
        Ok(m) => (StatusCode::OK, Json(json!({"messages": m}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn post_chat(
    State(st): State<Arc<AppState>>,
    Json(p): Json<ChatIn>,
) -> impl IntoResponse {
    let text = p.text.trim().to_string();
    if text.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Message text is empty.").into_response();
    }
    let sender = p.sender.unwrap_or_else(|| "Viewer".into());
    let sender = if sender.trim().is_empty() { "Viewer".into() } else { sender };
    let _ = st.db.add_message("chat", &sender, &text);
    // wiki auto-fill from manual chat commands/events
    let _ = wiki_events::try_manual_command(&st.db, &sender, &text);
    let _ = wiki_events::try_extract_from_text(&st.db, &sender, &text);
    if !p.answer.unwrap_or(true) {
        return (StatusCode::OK, Json(json!({"reply": null, "hits": []}))).into_response();
    }
    // trigger talk animation while AI responds
    {
        let mut v = st.vtuber.lock().unwrap();
        v.anim = "talk".into();
        v.until_ms = now_ms() + 8000;
    }
    match twitch::run_ai(&st.db, &text).await {
        Ok((reply, hits)) => (StatusCode::OK, Json(json!({"reply": reply, "hits": hits}))).into_response(),
        Err(e) => {
            let mut v = st.vtuber.lock().unwrap();
            v.anim = "idle".into();
            v.until_ms = 0;
            err(StatusCode::BAD_GATEWAY, e.to_string()).into_response()
        }
    }
}

async fn get_wiki(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    match st.db.wiki_list() {
        Ok(entries) => (StatusCode::OK, Json(json!({"entries": entries}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn add_wiki(State(st): State<Arc<AppState>>, Json(p): Json<WikiIn>) -> impl IntoResponse {
    let title = p.title.trim();
    let content = p.content.trim();
    if title.is_empty() || content.is_empty() {
        return err(StatusCode::BAD_REQUEST, "Title and content are required.").into_response();
    }
    let allowed = ["game", "platform", "persona", "fact", "event"];
    let category = p.category.unwrap_or_else(|| "fact".into());
    if !allowed.contains(&category.as_str()) {
        return err(StatusCode::BAD_REQUEST, "Unknown category.").into_response();
    }
    match st.db.wiki_add(title, &category, p.aliases.as_deref().unwrap_or(""), content) {
        Ok(id) => match st.db.wiki_get(id) {
            Ok(entry) => (StatusCode::OK, Json(json!({"entry": entry}))).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn update_wiki(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(p): Json<WikiUpdate>,
) -> impl IntoResponse {
    if !st.db.wiki_get(id).ok().flatten().is_some() {
        return err(StatusCode::NOT_FOUND, "Entry not found.").into_response();
    }
    let mut up = BTreeMap::new();
    if let Some(v) = p.title { up.insert("title".into(), v); }
    if let Some(v) = p.category { up.insert("category".into(), v); }
    if let Some(v) = p.aliases { up.insert("aliases".into(), v); }
    if let Some(v) = p.content { up.insert("content".into(), v); }
    match st.db.wiki_edit(id, &up) {
        Ok(_) => match st.db.wiki_get(id) {
            Ok(entry) => (StatusCode::OK, Json(json!({"entry": entry}))).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_wiki_one(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match st.db.wiki_get(id) {
        Ok(Some(entry)) => (StatusCode::OK, Json(json!({"entry": entry}))).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "Entry not found.").into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn delete_wiki(
    State(st): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match st.db.wiki_delete(id) {
        Ok(true) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "Entry not found.").into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct SearchQ { q: Option<String> }
async fn wiki_search(
    State(st): State<Arc<AppState>>,
    Query(q): Query<SearchQ>,
) -> impl IntoResponse {
    let text = q.q.unwrap_or_default();
    let keywords = context::extract_keywords(&text);
    match context::find_hits(&st.db, &text) {
        Ok(hits) => (StatusCode::OK, Json(json!({"keywords": keywords, "hits": hits}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_settings(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    match st.db.settings_all_masked() {
        Ok(s) => (StatusCode::OK, Json(json!({"settings": s}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn post_settings(State(st): State<Arc<AppState>>, Json(p): Json<SettingsIn>) -> impl IntoResponse {
    match st.db.settings_set(&p.values) {
        Ok(_) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn backup_create(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    match st.db.create_backup(&st.cfg) {
        Ok(path) => (StatusCode::OK, Json(json!({"ok": true, "file": path.file_name().and_then(|s| s.to_str()).unwrap_or("backup.json")}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn backup_list(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    match st.db.list_backups(&st.cfg) {
        Ok(files) => (StatusCode::OK, Json(json!({"files": files}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn backup_restore(State(st): State<Arc<AppState>>, Json(p): Json<BackupRestore>) -> impl IntoResponse {
    match st.db.restore_backup(&st.cfg, &p.name, p.replace.unwrap_or(false)) {
        Ok(n) => (StatusCode::OK, Json(json!({"ok": true, "imported": n}))).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn twitch_start(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    // stop existing
    {
        let mut g = st.twitch.lock().unwrap();
        if let Some(h) = g.take() { h.stop(); }
    }
    match twitch::spawn(st.db.clone()).await {
        Ok(h) => { *st.twitch.lock().unwrap() = Some(h); (StatusCode::OK, Json(json!({"ok": true}))).into_response() }
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn twitch_stop(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let mut g = st.twitch.lock().unwrap();
    if let Some(h) = g.take() { h.stop(); }
    let _ = st.db.settings_set(&BTreeMap::from([("twitch_enabled".into(), "0".into())]));
    (StatusCode::OK, Json(json!({"ok": true})))
}

async fn twitch_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let s = st.twitch.lock().unwrap().as_ref().map(|h| h.status()).unwrap_or(json!({"running":false,"connected":false}));
    (StatusCode::OK, Json(s))
}

async fn server_status(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let tw = st.twitch.lock().unwrap().as_ref().map(|h| h.status()).unwrap_or(json!({"running":false,"connected":false}));
    (StatusCode::OK, Json(json!({"ok": true, "twitch": tw, "version": "rust"})))
}

// ---- animations / vtuber ------------------------------------------------
#[derive(Deserialize)]
struct AnimIn {
    name: String,
    frames: Option<Vec<String>>,
    frames_text: Option<String>,
    fps: Option<i64>,
    aliases: Option<String>,
}
#[derive(Deserialize)]
struct PlayIn {
    anim: String,
    hold_ms: Option<i64>,
}

fn parse_frames(p: &AnimIn) -> Vec<String> {
    if let Some(f) = &p.frames {
        if !f.is_empty() {
            return f.clone();
        }
    }
    let t = p.frames_text.clone().unwrap_or_default();
    if t.contains("---FRAME---") {
        t.split("---FRAME---")
            .map(|s| s.trim_matches(['\r', '\n']).to_string())
            .filter(|s| !s.trim().is_empty())
            .collect()
    } else if t.trim_start().starts_with('[') {
        serde_json::from_str(&t).unwrap_or_default()
    } else if t.trim().is_empty() {
        vec![]
    } else {
        vec![t.trim_matches(['\r', '\n']).to_string()]
    }
}

async fn list_animations(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    match st.db.anim_list() {
        Ok(anims) => (StatusCode::OK, Json(json!({"animations": anims}))).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_animation(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match st.db.anim_get(&name) {
        Ok(Some(a)) => (StatusCode::OK, Json(json!({"animation": a}))).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "Animation not found.").into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn upsert_animation(State(st): State<Arc<AppState>>, Json(p): Json<AnimIn>) -> impl IntoResponse {
    let frames = parse_frames(&p);
    match st.db.anim_upsert(&p.name, &frames, p.fps.unwrap_or(3), p.aliases.as_deref().unwrap_or("")) {
        Ok(_) => match st.db.anim_get(&p.name) {
            Ok(a) => (StatusCode::OK, Json(json!({"ok": true, "animation": a}))).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        },
        Err(e) => err(StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn put_animation(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(mut p): Json<AnimIn>,
) -> impl IntoResponse {
    if p.name.trim().is_empty() {
        p.name = name;
    }
    upsert_animation(State(st), Json(p)).await
}

async fn delete_animation(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    match st.db.anim_delete(&name) {
        Ok(true) => (StatusCode::OK, Json(json!({"ok": true}))).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "Animation not found.").into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn vtuber_state(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    let mut v = st.vtuber.lock().unwrap().clone();
    let now = now_ms();
    if v.until_ms > 0 && now >= v.until_ms {
        v.anim = "idle".into();
        v.until_ms = 0;
        *st.vtuber.lock().unwrap() = v.clone();
    }
    (StatusCode::OK, Json(json!({"anim": v.anim, "until_ms": v.until_ms, "now_ms": now})))
}

async fn vtuber_play(State(st): State<Arc<AppState>>, Json(p): Json<PlayIn>) -> impl IntoResponse {
    let anim = if p.anim.trim().is_empty() { "idle".into() } else { p.anim };
    let hold = p.hold_ms.unwrap_or(4000).max(0);
    let resolved = st.db.anim_resolve(&anim).unwrap_or(anim.clone());
    let mut v = st.vtuber.lock().unwrap();
    v.anim = resolved.clone();
    v.until_ms = if hold > 0 { now_ms() + hold } else { 0 };
    (StatusCode::OK, Json(json!({"ok": true, "anim": resolved, "until_ms": v.until_ms})))
}

// -------------------- Embedded GUI templates ---------------------------------

const VTUBER_HTML: &str = r########"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Streaming Buddy - VTuber</title>
<style>
  html,body{margin:0;height:100%;background:transparent;overflow:hidden;font-family:Segoe UI,system-ui,sans-serif}
  body.chroma{background:#00ff00}
  #wrap{display:flex;flex-direction:column;align-items:center;justify-content:center;height:100%;gap:8px}
  #avatar{min-width:360px;min-height:360px;display:flex;align-items:center;justify-content:center}
  pre{margin:0;font:14px/1.15 Consolas,'Courier New',monospace;color:#e8eef7;text-shadow:0 1px 2px #000a;white-space:pre}
  #label{color:#fff;background:#0008;padding:4px 10px;border-radius:999px;font-size:13px}
  .hint{position:fixed;bottom:8px;left:8px;color:#fffc;background:#0006;padding:4px 8px;border-radius:6px;font-size:11px}
</style></head><body>
<div id="wrap"><div id="avatar"><pre id="art"></pre></div><div id="label">Larry - idle</div></div>
<div class="hint">OBS: capture this window. Press G for green background. Animations load from /api/animations.</div>
<script>
let anims={};
let anim='idle',i=0,fps=3,lastState='';
async function loadAnims(){
  try{const r=await fetch('/api/animations'); if(!r.ok) return; const d=await r.json();
    const map={}; (d.animations||[]).forEach(a=>{ map[a.name]=a; (a.aliases||'').split(',').forEach(x=>{const k=x.trim().toLowerCase(); if(k) map[k]=a;}); });
    if(Object.keys(map).length) anims=map;
  }catch(e){}
}
async function pollState(){
  try{const r=await fetch('/api/vtuber/state'); if(r.ok){ const d=await r.json(); if(d.anim && d.anim!==lastState){ anim=d.anim; lastState=anim; i=0; } }}
  catch(e){}
}
function tick(){
  const a=anims[anim]||anims['idle'];
  if(a && a.frames && a.frames.length){ document.getElementById('art').textContent=a.frames[i % a.frames.length]; fps=a.fps||3; }
  else { document.getElementById('art').textContent='(loading animations...)'; }
  i++;
  document.getElementById('label').textContent='Larry - '+anim;
  setTimeout(tick, Math.max(80, Math.floor(1000/Math.max(1,fps))));
}
loadAnims().then(()=>{ pollState(); tick(); });
setInterval(loadAnims, 10000);
setInterval(pollState, 1200);
document.addEventListener('keydown',e=>{if(e.key==='g'||e.key==='G')document.body.classList.toggle('chroma');});
</script></body></html>"########;

const INDEX_HTML: &str = r########"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Streaming Buddy (Rust)</title>
<style>
:root{color-scheme:dark}*{box-sizing:border-box}body{margin:0;font:14px/1.4 Segoe UI,system-ui,sans-serif;background:#14161c;color:#e6e6e6}
header{display:flex;gap:16px;align-items:center;padding:10px 18px;background:#1c1f27;border-bottom:1px solid #2c313d}
header h1{font-size:17px;margin:0}nav button{background:#242936;color:#cfd8e6;border:1px solid #333a4a;border-radius:6px;padding:6px 14px;cursor:pointer}
nav button.active{background:#2f6feb;border-color:#2f6feb;color:#fff}.st{margin-left:auto;font-size:12px;color:#8ea0b5}
main{max-width:1100px;margin:0 auto;padding:18px}section{display:none}.active-sec{display:block}
.panel{background:#1c1f27;border:1px solid #2c313d;border-radius:8px;padding:14px;margin-bottom:14px}
#log{height:340px;overflow:auto;background:#14171d;border:1px solid #2c313d;border-radius:6px;padding:8px;display:flex;flex-direction:column;gap:6px}
.msg{padding:6px 10px;border-radius:6px}.msg.chat{background:#232836}.msg.ai{background:#14304a;border-left:3px solid #2f6feb}.msg.event{background:#2a2a14;border-left:3px solid #d4a017}
.who{font-weight:700;margin-right:6px}input,select,textarea{background:#14171d;color:#e6e6e6;border:1px solid #333a4a;border-radius:6px;padding:7px 9px}
.row{display:flex;gap:8px;flex-wrap:wrap;align-items:center}.primary{background:#2f6feb;color:#fff;border:0;border-radius:6px;padding:8px 16px;cursor:pointer}
.warn{background:#a83232;color:#fff;border:0;border-radius:6px;padding:8px 14px;cursor:pointer}
.mini{background:#242936;color:#cfd;border:1px solid #333a4a;border-radius:6px;padding:4px 10px;cursor:pointer;font-size:12px}
.danger{background:none;color:#e46b6b;border:1px solid #5a3030;border-radius:6px;padding:4px 10px;cursor:pointer;font-size:12px}
table{width:100%;border-collapse:collapse;font-size:13px}th,td{padding:6px 8px;border-bottom:1px solid #2c313d;text-align:left}th{color:#8ea0b5}
.field{display:flex;flex-direction:column;gap:4px;margin-bottom:8px}.grid2{display:grid;grid-template-columns:1fr 1fr;gap:0 14px}
.hint{font-size:12px;color:#8ea0b5;margin:6px 0}.err{color:#ff8f8f;font-size:13px;min-height:16px}.ok{color:#8fe4a0;font-size:13px}
</style></head><body>
<header><h1>Streaming Buddy (Rust)</h1>
<nav>
<button data-tab="chat" class="active">Chat</button>
<button data-tab="wiki">Wiki</button>
<button data-tab="anim">Animations</button>
<button data-tab="backup">Backup</button>
<button data-tab="settings">Settings</button>
</nav><span class="st" id="st">connecting...</span></header>
<main>
<section id="tab-chat" class="active-sec"><div class="panel">
<div id="log"></div>
<form id="cf" class="row"><input id="sender" value="Streamer" style="width:120px">
<input id="msg" placeholder="Message... (!wikievent Title | content  OR  !addfact text)" style="flex:1" autocomplete="off">
<label><input type="checkbox" id="ai" checked> AI</label><button class="primary">Send</button></form>
<div class="err" id="cerr"></div>
<div class="row"><button class="mini" type="button" id="openvt">Open VTuber window (OBS capture)</button>
<span class="hint">Separate window for OBS Studio (Window Capture / Browser Source).</span></div>
</div></section>

<section id="tab-wiki"><div class="panel">
<table><thead><tr><th>Id</th><th>Cat</th><th>Title</th><th>Aliases</th><th></th></tr></thead><tbody id="wtb"></tbody></table>
<div class="hint" id="wcount"></div></div>
<div class="panel"><h3 id="wtitle">Add entry</h3>
<form id="wf" class="row"><input id="wti" placeholder="Title" required>
<select id="wcat"><option>game</option><option>platform</option><option>persona</option><option selected>fact</option><option>event</option></select>
<input id="wal" placeholder="Aliases" style="flex:1"><input id="wco" placeholder="Content / facts" style="flex:2" required>
<button class="primary" id="wsave">Add</button><button class="mini" type="button" id="wcancel" style="display:none">Cancel</button></form>
<div class="err" id="werr"></div></div></section>

<section id="tab-anim"><div class="panel"><h3>VTuber ASCII animations</h3>
<p class="hint">Create and edit animations used by the VTuber window (<code>/vtuber</code>, separate OBS-capturable window). Frames are ASCII art; separate multiple frames with a line containing only <code>---FRAME---</code>.</p>
<div class="row"><button class="mini" id="arefresh">Refresh</button><button class="mini" id="anew">New animation</button><button class="primary" id="openvt2">Open VTuber window</button></div>
<table style="margin-top:10px"><thead><tr><th>Name</th><th>FPS</th><th>Aliases</th><th>Frames</th><th></th></tr></thead><tbody id="atb"></tbody></table>
<div class="err" id="alistErr"></div></div>
<div class="panel"><h3 id="atitle">Create / edit animation</h3>
<div class="grid2">
<div class="field"><label>Name (id)</label><input id="aname" placeholder="idle, talk, happy..."></div>
<div class="field"><label>FPS</label><input id="afps" type="number" min="1" max="30" value="3"></div>
</div>
<div class="field"><label>Aliases (comma-separated)</label><input id="aalias" placeholder="smile,joy"></div>
<div class="field"><label>Frames (separate with a line: ---FRAME---)</label><textarea id="aframes" rows="12" style="font-family:Consolas,monospace;font-size:12px;line-height:1.15"></textarea></div>
<div class="row"><button class="primary" id="asave">Save animation</button><button class="mini" id="aplay">Play in VTuber</button></div>
<div class="ok" id="aok"></div><div class="err" id="aerr"></div>
</div></section>

<section id="tab-backup"><div class="panel"><h3>Wiki backup</h3>
<div class="row"><button class="primary" id="bcreate">Create backup now</button>
<button class="mini" id="brefresh">Refresh list</button></div>
<div class="ok" id="bok"></div><div class="err" id="berr"></div>
<table style="margin-top:10px"><thead><tr><th>File</th><th></th></tr></thead><tbody id="btb"></tbody></table>
<p class="hint">Backups are JSON exports in <code>data/backups/</code>. Restore merges by default (or replace all).</p>
</div></section>

<section id="tab-settings"><div class="panel"><h3>Settings (secrets encrypted at rest)</h3>
<div class="grid2">
<div class="field"><label>OpenRouter API key</label><input id="skey" type="password" placeholder="sk-or-..."><span class="hint" id="khint"></span></div>
<div class="field"><label>Model</label><input id="smodel" placeholder="openai/gpt-4o-mini"></div>
<div class="field"><label>Persona name</label><input id="spersona"></div>
<div class="field"><label>Channel topic</label><input id="stopic"></div>
<div class="field"><label>Streamer reference</label><input id="sref"></div>
<div class="field"><label>Auto-fill wiki from events (1/0)</label><input id="sauto" placeholder="1"></div>
</div>
<div class="field"><label>Character prompt</label><textarea id="schar" rows="5"></textarea></div>
<div class="field"><label>System prompt</label><textarea id="ssys" rows="5"></textarea></div>
<h4>Twitch (token encrypted)</h4>
<div class="grid2">
<div class="field"><label>Username</label><input id="tuser"></div>
<div class="field"><label>OAuth token</label><input id="ttok" type="password"></div>
<div class="field"><label>Channel</label><input id="tchan"></div>
<div class="field"><label>Reply mode</label><select id="tmode"><option value="off">off</option><option value="mentions">mentions</option><option value="questions">questions</option><option value="smart" selected>smart</option><option value="all">all</option></select></div>
</div>
<div class="row"><button class="primary" id="ssave">Save settings</button>
<button class="mini" id="tstart">Twitch connect</button><button class="warn" id="tstop">Twitch stop</button></div>
<div class="ok" id="sok"></div><div class="err" id="serr"></div>
<p class="hint">Secrets use <code>enc:v1:...</code> (AES-256-GCM). Set <code>STREAMING_BUDDY_MASTER_KEY</code> or let the app generate <code>data/master.key</code>.</p>
</div></section>
</main>
<script>
const $=id=>document.getElementById(id);
async function api(u,o){const r=await fetch(u,o);let b=null;try{b=await r.json()}catch{}if(!r.ok)throw new Error((b&&b.detail)||('HTTP '+r.status));return b}
const post=(u,d)=>api(u,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(d)});
const esc=s=>s.replace(/[&<>\"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','\"':'&quot;'}[c]));
document.querySelectorAll('nav button').forEach(b=>b.onclick=()=>{document.querySelectorAll('nav button').forEach(x=>x.classList.remove('active'));document.querySelectorAll('main section').forEach(x=>x.classList.remove('active-sec'));b.classList.add('active');$('tab-'+b.dataset.tab).classList.add('active-sec');if(b.dataset.tab==='wiki')loadWiki();if(b.dataset.tab==='backup')loadBackups();if(b.dataset.tab==='settings')loadSettings();if(b.dataset.tab==='anim')loadAnims();});
let last=0;
function addMsg(m){const d=document.createElement('div');d.className='msg '+(m.source||'chat');d.innerHTML='<span class="who">'+esc(m.sender)+':</span>'+esc(m.text);$('log').appendChild(d);$('log').scrollTop=$('log').scrollHeight;}
async function poll(){try{const d=await api('/api/messages?after='+last);(d.messages||[]).forEach(m=>{addMsg(m);last=Math.max(last,m.id)});$('st').textContent='connected';}catch(e){$('st').textContent='offline';}}
$('cf').onsubmit=async e=>{e.preventDefault();$('cerr').textContent='';const t=$('msg').value.trim();if(!t)return;try{await post('/api/chat',{sender:$('sender').value,text:t,answer:$('ai').checked});$('msg').value='';await poll();}catch(err){$('cerr').textContent=err.message;}};
$('openvt').onclick=()=>window.open('/vtuber','sbvtuber','width=420,height=520,menubar=no,toolbar=no,location=no,status=no');
let editId=null;
async function loadWiki(){try{const d=await api('/api/wiki');const tb=$('wtb');tb.innerHTML='';(d.entries||[]).forEach(e=>{const tr=document.createElement('tr');tr.innerHTML='<td>'+e.id+'</td><td>'+esc(e.category)+'</td><td>'+esc(e.title)+'</td><td>'+esc(e.aliases)+'</td><td><button class="mini" data-e="'+e.id+'">Edit</button> <button class="danger" data-d="'+e.id+'">Del</button></td>';tb.appendChild(tr);});tb.querySelectorAll('[data-e]').forEach(b=>b.onclick=()=>startEdit(Number(b.dataset.e),(d.entries||[])));tb.querySelectorAll('[data-d]').forEach(b=>b.onclick=async()=>{if(!confirm('Delete?'))return;await api('/api/wiki/'+b.dataset.d,{method:'DELETE'});loadWiki();});$('wcount').textContent=(d.entries||[]).length+' entries';}catch(e){$('werr').textContent=e.message;}}
function startEdit(id,entries){const e=entries.find(x=>x.id===id);if(!e)return;editId=id;$('wti').value=e.title;$('wcat').value=e.category;$('wal').value=e.aliases;$('wco').value=e.content;$('wtitle').textContent='Edit #'+id;$('wsave').textContent='Save';$('wcancel').style.display='';}
function resetW(){editId=null;$('wtitle').textContent='Add entry';$('wsave').textContent='Add';$('wcancel').style.display='none';$('wti').value='';$('wal').value='';$('wco').value='';}
$('wcancel').onclick=resetW;
$('wf').onsubmit=async e=>{e.preventDefault();$('werr').textContent='';const p={title:$('wti').value,category:$('wcat').value,aliases:$('wal').value,content:$('wco').value};try{if(editId)await api('/api/wiki/'+editId,{method:'PUT',headers:{'Content-Type':'application/json'},body:JSON.stringify(p)});else await post('/api/wiki',p);resetW();loadWiki();}catch(err){$('werr').textContent=err.message;}};
async function loadBackups(){try{const d=await api('/api/backup/list');const tb=$('btb');tb.innerHTML='';(d.files||[]).forEach(f=>{const tr=document.createElement('tr');tr.innerHTML='<td>'+esc(f)+'</td><td><button class="mini" data-r="'+esc(f)+'">Restore (merge)</button> <button class="danger" data-rp="'+esc(f)+'">Replace all</button></td>';tb.appendChild(tr);});tb.querySelectorAll('[data-r]').forEach(b=>b.onclick=async()=>{if(!confirm('Restore (merge)?'))return;await post('/api/backup/restore',{name:b.dataset.r,replace:false});$('bok').textContent='Restored.';loadWiki();});tb.querySelectorAll('[data-rp]').forEach(b=>b.onclick=async()=>{if(!confirm('REPLACE all wiki entries with this backup?'))return;await post('/api/backup/restore',{name:b.dataset.rp,replace:true});$('bok').textContent='Restored (replaced).';loadWiki();});}catch(e){$('berr').textContent=e.message;}}
$('bcreate').onclick=async()=>{$('bok').textContent='';$('berr').textContent='';try{const d=await post('/api/backup/create',{});$('bok').textContent='Created: '+d.file;loadBackups();}catch(e){$('berr').textContent=e.message;}};
$('brefresh').onclick=loadBackups;
async function loadSettings(){try{const d=await api('/api/settings');const s=d.settings||{};$('skey').value=s.openrouter_api_key||'';$('smodel').value=s.model||'';$('spersona').value=s.persona_name||'';$('stopic').value=s.channel_topic||'';$('sref').value=s.streamer_reference||'';$('sauto').value=s.wiki_auto_events||'1';$('schar').value=s.character_prompt||'';$('ssys').value=s.system_prompt||'';$('tuser').value=s.twitch_username||'';$('ttok').value=s.twitch_token||'';$('tchan').value=s.twitch_channel||'';$('tmode').value=s.twitch_reply_mode||'smart';$('khint').textContent=s.openrouter_api_key?'Key stored (encrypted).':'';}catch(e){$('serr').textContent=e.message;}}
$('ssave').onclick=async()=>{$('sok').textContent='';$('serr').textContent='';try{await post('/api/settings',{values:{openrouter_api_key:$('skey').value,model:$('smodel').value,persona_name:$('spersona').value,channel_topic:$('stopic').value,streamer_reference:$('sref').value,wiki_auto_events:$('sauto').value,character_prompt:$('schar').value,system_prompt:$('ssys').value,twitch_username:$('tuser').value,twitch_token:$('ttok').value,twitch_channel:$('tchan').value,twitch_reply_mode:$('tmode').value}});$('sok').textContent='Saved (secrets encrypted).';}catch(e){$('serr').textContent=e.message;}};
$('tstart').onclick=async()=>{try{await post('/api/twitch/start',{});$('sok').textContent='Twitch connecting...';}catch(e){$('serr').textContent=e.message;}};
$('tstop').onclick=async()=>{try{await post('/api/twitch/stop',{});$('sok').textContent='Twitch stopped.';}catch(e){$('serr').textContent=e.message;}};
$('openvt2') && ($('openvt2').onclick=$('openvt').onclick);
async function loadAnims(){try{const d=await api('/api/animations');const tb=$('atb');tb.innerHTML='';(d.animations||[]).forEach(a=>{const tr=document.createElement('tr');const fr=(a.frames||[]).length;tr.innerHTML='<td>'+esc(a.name)+'</td><td>'+a.fps+'</td><td>'+esc(a.aliases||'')+'</td><td>'+fr+'</td><td><button class="mini" data-ae="'+esc(a.name)+'">Edit</button> <button class="mini" data-ap="'+esc(a.name)+'">Play</button> <button class="danger" data-ad="'+esc(a.name)+'">Del</button></td>';tb.appendChild(tr);});tb.querySelectorAll('[data-ae]').forEach(b=>b.onclick=()=>editAnim(b.dataset.ae));tb.querySelectorAll('[data-ap]').forEach(b=>b.onclick=()=>playAnim(b.dataset.ap));tb.querySelectorAll('[data-ad]').forEach(b=>b.onclick=async()=>{if(!confirm('Delete animation '+b.dataset.ad+'?'))return;await api('/api/animations/'+encodeURIComponent(b.dataset.ad),{method:'DELETE'});loadAnims();});}catch(e){$('alistErr').textContent=e.message;}}
async function editAnim(name){try{const d=await api('/api/animations/'+encodeURIComponent(name));const a=d.animation;if(!a)return;$('atitle').textContent='Edit: '+a.name;$('aname').value=a.name;$('afps').value=a.fps||3;$('aalias').value=a.aliases||'';$('aframes').value=(a.frames||[]).join('\n---FRAME---\n');$('aok').textContent='';$('aerr').textContent='';}catch(e){$('aerr').textContent=e.message;}}
async function playAnim(name){try{await post('/api/vtuber/play',{anim:name,hold_ms:8000});$('aok').textContent='Playing: '+name;}catch(e){$('aerr').textContent=e.message;}}
$('arefresh').onclick=loadAnims;
$('anew').onclick=()=>{$('atitle').textContent='Create / edit animation';$('aname').value='';$('afps').value='3';$('aalias').value='';$('aframes').value='';$('aok').textContent='';$('aerr').textContent='';};
$('asave').onclick=async()=>{$('aok').textContent='';$('aerr').textContent='';try{const p={name:$('aname').value.trim(),fps:Number($('afps').value||3),aliases:$('aalias').value,frames_text:$('aframes').value};if(!p.name)throw new Error('Name required');await post('/api/animations',p);$('aok').textContent='Saved: '+p.name;loadAnims();}catch(e){$('aerr').textContent=e.message;}};
$('aplay').onclick=()=>playAnim($('aname').value.trim()||'idle');
poll(); setInterval(poll,3000);
</script></body></html>"########;
