# Streaming Buddy (Rust)

A local livestream chat assistant: reads chat, keeps a channel-specific **wiki** (games, platforms, persona, facts, events), enriches prompts with matched wiki entries, and replies via an external LLM (OpenRouter). Includes Twitch IRC integration, **encrypted settings secrets**, **wiki backups**, **auto-filling the wiki from important chat events** (subscriptions/raids/gifts), and a **separate VTuber overlay window** for OBS Studio capture.

> **GitHub/example version:** no personal data is included. Example wiki/personality/animation/event data is provided in [`seed/`](seed/). Runtime data (`data/`, DB, `master.key`, real backups) is gitignored.

## Features

- **Rust + Axum** web server with a local GUI (`http://127.0.0.1:8811`).
- **Encrypted settings at rest:** API keys and tokens (`openrouter_api_key`, `twitch_token`, etc.) are stored as `enc:v1:...` using **AES-256-GCM**. The key is derived (Argon2id) from `STREAMING_BUDDY_MASTER_KEY` (env var) or a generated `data/master.key`. Non-secret settings remain plaintext. Secrets are masked in API/GUI responses.
- **Wiki knowledge base** (categories: `game`, `platform`, `persona`, `fact`, `event`) with keyword search/matching used to ground AI answers.
- **Wiki backup/restore:** create timestamped JSON backups in `data/backups/`, list, restore (merge) or replace-all. CLI: `backup`, `backup-list`, `restore <file> [--replace]`.
- **Wiki auto-fill from chat/events:**
  - Twitch `USERNOTICE` parsing for **sub, resub, gift subs, raids, announcements** → recorded as `event` wiki entries (when `wiki_auto_events=1`).
  - Manual chat/command helpers: `!wikievent <title> | <content>` and `!addfact <text>`.
  - Conservative text extraction fallback for obvious subscription phrases.
- **Twitch IRC** (TLS, tags/commands) read + optional AI replies with reply modes and cooldown.
- **OpenRouter** chat completions; wiki-enriched system prompt with persona/character instructions.
- **VTuber in a separate window (OBS capture):** `/vtuber` opens as its own popup window so OBS can capture it independently. Press `G` on the VTuber page to toggle a green background for chroma workflows.

## Quick start (build & run)

Requirements: Rust toolchain (stable). The project uses `edition = "2021"`.

```powershell
cd "C:\AI_Projektit\Streaming buddy-rs"

# optional but recommended: set a strong master key (keeps encrypted secrets recoverable)
$env:STREAMING_BUDDY_MASTER_KEY = "<long-random-secret>"

# build
cargo build --release

# run server
./target/release/streaming_buddy_rs server --host 127.0.0.1 --port 8811
```

Open `http://127.0.0.1:8811`. In **Settings**: set OpenRouter API key/model/persona, Twitch credentials (optional), then save. Secrets are encrypted in the SQLite database (`data/streaming_buddy.db`).

## OBS Studio capture (VTuber)

1. Start the server and open the GUI.
2. Click **Open VTuber window (OBS capture)** — opens `http://127.0.0.1:8811/vtuber` as a separate popup window (required for independent capture).
3. In OBS Studio, add a **Window Capture** source and select the VTuber popup window. (Alternative: **Browser Source** pointing to `http://127.0.0.1:8811/vtuber`.)
4. Optional: focus the VTuber window and press **`G`** to toggle a green background for chroma keying.
5. The overlay shows simple ASCII/avatar states (idle/talk/celebrate/sleep) driven by recent AI activity as an MVP; see gaps below for full sprite/WebSocket parity.

## CLI

```text
streaming-buddy-rs server [--host 127.0.0.1] [--port 8811]
streaming-buddy-rs backup
streaming-buddy-rs backup-list
streaming-buddy-rs restore <filename.json> [--replace]
streaming-buddy-rs event <kind> <user> <detail>
streaming-buddy-rs ask "<text>"
```

## Example data (safe for GitHub)

- [`seed/wiki-example.json`](seed/wiki-example.json) — example wiki entries including **persona/personality**, rules, and an **event** example. No personal data.
- VTuber/character animations (documented set): `idle, blink, talk, happy, celebrate, sad, surprised, angry, wink, sleep, dance, wave, think, nod, shrug`. The Rust MVP `/vtuber` overlay demonstrates a subset (idle/talk/celebrate/sleep) as an OBS-capturable separate window.

## MVP gaps vs Python version

The Python `Streaming buddy` app includes additional UI/behavior that this Rust port does **not yet** fully implement:

- **Full PNG sprite upload/playback parity:** Python has sprite DB/API (`/api/sprites/*`), image uploads, per-animation frames/FPS/cycling, `/media` static serving, and Larry PIL sprite seeding. Rust MVP uses an embedded VTuber overlay page (ASCII/simple states), not the full sprite upload/playback pipeline.
- **WebSocket animation push:** Python/Rust MVP rely on simple polling/heuristics for state; no dedicated WebSocket channel for low-latency animation state changes.
- Some Python GUI controls and flows (advanced sprite management, all VTuber tab behaviors) are simplified in the embedded Rust GUI.

Core requirements covered in Rust: encrypted secrets at rest, wiki backup/create/list/restore, wiki auto-fill from Twitch USERNOTICE events (subs/raids/gifts) + `!wikievent`/`!addfact`, and VTuber in a **separate window** for OBS capture.

## Security notes

- **Never commit** `data/`, `master.key`, `.env`, or real databases/backups. `.gitignore` enforces this (`/data/*` ignored, `seed/` kept).
- Set a strong master key: `STREAMING_BUDDY_MASTER_KEY`. If unset, a random key is generated in `data/master.key` — **back it up securely**; losing it makes encrypted secrets unrecoverable.
- The app binds to `127.0.0.1` by default (local only). **No authentication** is included — do not expose publicly without adding auth/CSRF protections/TLS.

See [`SECURITY.md`](SECURITY.md) for details.

## No personal data checklist (before publishing)

- [ ] `data/` is not committed (contains `streaming_buddy.db`, `master.key`, `backups/` with real chat/wiki). Verify with `git status`.
- [ ] No real API keys/tokens in code, docs, or commits (OpenRouter, Twitch OAuth). Search history if needed.
- [ ] No personal usernames, channel names, chat logs, or private streamer facts in tracked files.
- [ ] `seed/wiki-example.json` contains only example/fictional placeholder data.
- [ ] `.env` files and local config overrides are not committed.
- [ ] Run `cargo check`/build artifacts (`target/`) are ignored.

## Project layout

- `src/crypto.rs` — AES-256-GCM encryption for secrets (`enc:v1:`).
- `src/db.rs` — SQLite (messages, wiki, settings), backups, event upsert.
- `src/context.rs` — keyword search + prompt building.
- `src/ai.rs` — OpenRouter client.
- `src/twitch.rs` — Twitch IRC (TLS) + USERNOTICE event parsing.
- `src/wiki_events.rs` — auto wiki fill from events/commands.
- `src/web.rs` — Axum API + embedded GUI + `/vtuber` overlay.
- `seed/wiki-example.json` — example data (no personal data).

## Further ideas

See [`RESEARCH.md`](RESEARCH.md) for researched additions (semantic wiki search, EventSub, sprite/WebSocket VTuber parity, PII redaction, hardening, tests).

## License

[MIT](LICENSE)
