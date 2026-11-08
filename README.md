# Streaming Buddy (Rust)

**Local VTuber streaming chat helper and buddy for streamers — local wiki knowledge base + OpenRouter.ai AI chat (proof of concept).**

Streaming Buddy is a local co-host app for livestreams: it keeps a channel-specific **wiki**, enriches AI prompts with matched wiki entries, replies via **OpenRouter.ai**, integrates with **Twitch chat/events**, and provides a **separate VTuber overlay window** for **OBS Studio** capture. Settings secrets are encrypted at rest.

> **No personal data included in this repository.** Example wiki/personality/animation/event data lives in [`seed/`](seed/). Runtime data (`data/` DB, `master.key`, backups, chat logs) is gitignored.

## Key features

- **Local web GUI** (Axum): `http://127.0.0.1:8811` (configurable host/port).
- **OpenRouter.ai AI chat**: wiki-grounded prompt building (persona/character instructions, streamer reference, recent context, matched wiki hits).
- **Local wiki knowledge base**: categories `game`, `platform`, `persona`, `fact`, `event` (CRUD + search). Used to ground factual answers.
- **Encrypted settings at rest**: `openrouter_api_key`, `twitch_token`, etc. stored as `enc:v1:<...>` using **AES-256-GCM**; key derived via **Argon2id** from `STREAMING_BUDDY_MASTER_KEY` or generated `data/master.key`. Secrets are **masked** in API/GUI responses.
- **Wiki backup & restore**: timestamped JSON backups in `data/backups/` (create/list/restore merge or replace). CLI: `backup`, `backup-list`, `restore <file> [--replace]`.
- **Wiki auto-fill from important chat events**:
  - Twitch IRC (TLS) + `USERNOTICE` parsing for **subscription, resub, gift subs, raid, announcement** → recorded as `event` wiki entries when `wiki_auto_events=1`.
  - Manual commands: `!wikievent <title> | <content>` and `!addfact <text>`.
- **Twitch integration**: connect/start/stop/status, reply modes/cooldown (configurable). Start without credentials returns a clear error.
- **VTuber in a separate window (OBS capture)**: `/vtuber` opens as its own popup window (independent capture). Press **`G`** to toggle green background for chroma.
- **VTuber animations (ASCII + image)**: 15 default ASCII animations — `idle, blink, talk, happy, celebrate, sad, surprised, angry, wink, sleep, dance, wave, think, nod, shrug` (+ aliases like `smile/joy`, `speak`, `hello/hi/bye`, `shock/wow`, `flirt`, etc.). Animations are stored in the local DB and rendered by `/vtuber`.
  - **ASCII**: frames separated by a line containing `---FRAME---`.
  - **Image**: upload image frames (PNG/JPG/GIF/WEBP) in the Animations tab or via API; frames are saved under `data/images/animations/<name>/` (gitignored) and served at `/media/...`.
- **Create/edit animations in the app**: GUI **Animations** tab + API to list/get/create/update/delete animations, upload image frames, and control VTuber play state (`/api/vtuber/play`, `/api/vtuber/state`).

## Download / run (Windows EXE)

A prebuilt Windows x64 package (when included in `dist/`) contains `streaming_buddy_rs.exe` and `start-server.bat`.

1. Extract the zip.
2. (Recommended) set a master key for encrypted secrets:
   ```powershell
   setx STREAMING_BUDDY_MASTER_KEY "<long-random-secret>"
   ```
   Or run with `$env:STREAMING_BUDDY_MASTER_KEY="..."` in the current shell. If unset, the app generates `data/master.key` — **back it up securely** (losing it makes encrypted secrets unrecoverable).
3. Run `start-server.bat` (or `streaming_buddy_rs.exe server --host 127.0.0.1 --port 8811`).
4. Open `http://127.0.0.1:8811`.

## Build from source

Requirements: Rust stable toolchain.

```powershell
git clone https://github.com/kebabkenraali/Streaming_buddy.git
cd Streaming_buddy
cargo build --release
./target/release/streaming_buddy_rs.exe server --host 127.0.0.1 --port 8811
```

## OBS Studio (VTuber capture)

1. Start the server and open the GUI.
2. Click **Open VTuber window (OBS capture)** — opens `/vtuber` as a **separate popup window**.
3. In OBS: add **Window Capture** for that popup (or **Browser Source** → `http://127.0.0.1:8811/vtuber`).
4. Optional: press **`G`** on the VTuber window for green/chroma background.
5. Animations are loaded from `/api/animations`; AI replies trigger `talk` (state via `/api/vtuber/state`). Use the Animations tab to edit/create and **Play**.

## CLI

```text
streaming-buddy-rs server [--host 127.0.0.1] [--port 8811]
streaming-buddy-rs backup
streaming-buddy-rs backup-list
streaming-buddy-rs restore <filename.json> [--replace]
streaming-buddy-rs event <kind> <user> <detail>
streaming-buddy-rs ask "<text>"
```

## API (selected)

- `GET/POST /api/chat`, `GET /api/messages`
- `GET/POST /api/wiki`, `GET/PUT/DELETE /api/wiki/:id`, `GET /api/wiki/search?q=...`
- `GET/POST /api/animations`, `GET/PUT/DELETE /api/animations/:name`, `POST /api/animations/:name/upload` (multipart image frames)
- `GET /api/vtuber/state`, `POST /api/vtuber/play`
- `GET/POST /api/settings` (secrets masked)
- `POST /api/backup/create`, `GET /api/backup/list`, `POST /api/backup/restore`
- `POST /api/twitch/start|stop`, `GET /api/twitch/status`
- `GET /media/...` (local images under `data/images/`, gitignored)

## Example data (safe)

[`seed/wiki-example.json`](seed/wiki-example.json) contains **two example entries per category** (`persona`, `platform`, `game`, `fact`, `event`) — **10 entries total**, fictional/no personal data. On a fresh install (empty wiki DB) the app auto-imports this seed if present; you can also use it as a backup/import reference.

## Security & privacy

- **Never commit** `data/`, `master.key`, `.env`, real DBs/backups, or personal chat logs. `.gitignore` enforces this.
- Bind to `127.0.0.1` by default. **No authentication** is included — do not expose publicly without auth/CSRF/TLS.
- Chat/wiki are untrusted input (prompt-injection risk); review auto-filled events and harden before production use.

See [`SECURITY.md`](SECURITY.md) and [`RESEARCH.md`](RESEARCH.md).

## Project layout

- `src/crypto.rs` — secret encryption (`enc:v1:`)
- `src/db.rs` — SQLite (messages/wiki/settings/animations), backups
- `src/context.rs` — wiki search + prompt building
- `src/ai.rs` — OpenRouter client
- `src/twitch.rs` — Twitch IRC + USERNOTICE parsing
- `src/wiki_events.rs` — event/command → wiki auto-fill
- `src/web.rs` — Axum API + embedded GUI + `/vtuber` + animation endpoints
- `src/main.rs` — CLI/server
- `seed/` — example data (no personal data)
- `dist/` — optional prebuilt Windows EXE zip

## License

[MIT](LICENSE)
