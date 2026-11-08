# Security notes

## Secrets (implemented)
- `openrouter_api_key`, `twitch_token` (and similar secret keys) are **encrypted at rest** as `enc:v1:<base64(nonce||ciphertext)>` using **AES-256-GCM**.
- The encryption key is derived with **Argon2id** from master key material:
  - `STREAMING_BUDDY_MASTER_KEY` environment variable (recommended), OR
  - `data/master.key` (auto-generated, base64, 32 random bytes) if the env var is not set.
- **Back up `master.key` / the master passphrase securely.** Without it, encrypted values cannot be recovered.
- The web API **masks** secrets in responses (`********`); masked values are not overwritten on save.

## Network exposure
- Default bind is `127.0.0.1` (local machine only).
- There is **no authentication** on the GUI/API in this version. Do **not** bind to `0.0.0.0` or expose via port forwarding/tunnels without adding auth, CSRF protections, and TLS.

## Chat/wiki as untrusted input
- Chat messages and wiki content may contain prompt-injection attempts. The system treats wiki as context; consider stricter grounding/citation and filtering for production use.
- Auto wiki-fill from chat events is **convenience automation** — review `event` entries, restrict manual commands to trusted users/mods in future hardening, and keep backups.

## Data handling
- `data/streaming_buddy.db` and `data/backups/` may contain chat logs and channel info — **gitignored** by default. Do not publish them.
- When sharing/exporting, use the backup JSON and review for personal data.

## Reporting
For a public GitHub release, add a vulnerability reporting policy and review dependencies (`cargo audit`) regularly.
