# Research: possible additions to Streaming Buddy

This document surveys practical extensions for a local livestream AI co-host (wiki-grounded answers, Twitch chat, VTuber overlay, events). Items are grouped by theme with notes on value, complexity, and privacy/safety.

## 1. Chat & platform integrations
- **Multi-platform chat:** YouTube Live chat, Kick, Discord relay. *Value:* reach; *Complexity:* medium/high (APIs, auth, rate limits). Keep credentials encrypted like Twitch.
- **Richer Twitch EventSub/WebSocket:** more reliable than IRC for some events; structured subscriptions, channel points, follows. *Complexity:* medium.
- **Moderator/role-aware replies:** only answer commands from mods/broadcaster; configurable allowlists. *Value:* safety.
- **Rate-limit & queue dashboard:** visualize cooldowns, dropped replies, and backoff when API limits hit.

## 2. Knowledge base (wiki) quality
- **Semantic/vector search (local embeddings):** improve matching beyond keywords (e.g., `sqlite-vec`/`qdrant` local, or ONNX embeddings). *Value:* high for fuzzy questions; *Complexity:* medium; keep fully local for privacy.
- **Source citations in replies:** have the model quote which wiki entry ID/title it used; show in GUI. *Value:* trust/debugging.
- **Importers:** CSV/JSON bulk import, optional read-only import from curated sources (game DB exports) — avoid scraping copyrighted text; respect licenses.
- **Wiki versioning/diff:** keep history per entry (audit of auto-filled events). *Value:* rollback after bad auto-fill.
- **Confidence gating:** if no strong wiki hit, force "I don't know / check with streamer" mode for factual categories.

## 3. Event automation & stream ops
- **Configurable event rules:** map `sub/resub/gift/raid/cheer/follow/raid` to wiki templates, on-screen alerts, or TTS hooks (opt-in).
- **Milestone counters:** "Nth subscriber today" in a local stats table; feed to persona context.
- **Command system:** `!lore <topic>`, `!game`, `!schedule` answered from wiki with stricter grounding; separate from free-form AI.
- **Clip/moment markers (metadata only):** log timestamps of notable events; no media capture by default (privacy).

## 4. VTuber / OBS presentation
- **True PNG sprite pipeline in Rust build:** upload frames, group/animation FPS, play state via WebSocket push (lower latency than polling).
- **Browser-source optimized overlay:** transparent mode, customizable CSS, chroma key toggle, resolution presets, "talk" mouth-sync heuristic from reply length/audio amplitude (if audio added later).
- **Expression selection by intent:** classify reply/event sentiment (happy/surprised/celebrate) to pick animation — rule-based first, model optional.
- **Local TTS (opt-in):** drive lip/talk animation; keep audio local where possible; document consent/stream disclosure.

## 5. AI/runtime quality
- **Streaming responses:** token streaming to GUI/Twitch with truncation safety.
- **Model failover:** fallback model/provider on errors; budget caps.
- **Prompt injection hardening:** treat chat/wiki as untrusted data; delimiters; never execute instructions from chat; strip secrets from prompts/logs.
- **Per-category grounding policies:** e.g., `event` facts can be quoted, `persona` is style-only.

## 6. Privacy, security, operations
- **At-rest encryption (implemented):** extend to full DB encryption (SQLCipher) if threat model requires.
- **Auth for GUI:** optional local password/token if bound beyond localhost.
- **PII redaction:** detect/emails/phones in logs and avoid sending to LLM; configurable retention.
- **Audit log:** who/what changed wiki/settings (important with chat auto-fill).
- **Portable export:** one-click "safe export" excluding secrets and chat PII.

## 7. Developer experience
- **Automated tests:** unit tests for crypto, wiki search scoring, event parsing, prompt assembly (with mocked LLM).
- **Schema migrations:** versioned migrations for SQLite.
- **Plugin/hooks:** small event bus (Rust traits) for custom handlers without forking core.

## Suggested priorities (pragmatic)
1. **Harden auto-events** (dedupe, rate-limit wiki writes, mod-only manual commands) + tests.
2. **Semantic wiki search** (local) for better factual grounding.
3. **VTuber sprite/animation parity** in Rust (WebSocket state, uploads) + OBS browser-source guide.
4. **EventSub integration** for reliable subscription/raid data.
5. **PII redaction + retention controls** before broader deployment.
