//! Encrypt/decrypt sensitive setting values (API keys, Twitch token, etc.).
//!
//! Format stored in DB/file as: `enc:v1:<base64(nonce || ciphertext)>`
//! Key: Argon2id-derived from master key material (config) + fixed salt context,
//! used with AES-256-GCM.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Context};
use argon2::Argon2;
use base64::Engine;
use rand::RngCore;

const PREFIX: &str = "enc:v1:";
const SALT: &[u8] = b"streaming-buddy-settings-v1";

fn derive_key(master: &[u8]) -> anyhow::Result<[u8; 32]> {
    let mut out = [0u8; 32];
    Argon2::default()
        .hash_password_into(master, SALT, &mut out)
        .map_err(|e| anyhow!("kdf: {e}"))?;
    Ok(out)
}

pub fn encrypt(master: &[u8], plaintext: &str) -> anyhow::Result<String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }
    let key = derive_key(master)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("{e}"))?;
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| anyhow!("encrypt failed"))?;
    let mut blob = nonce_bytes.to_vec();
    blob.extend_from_slice(&ct);
    let b64 = base64::engine::general_purpose::STANDARD.encode(blob);
    Ok(format!("{PREFIX}{b64}"))
}

pub fn decrypt(master: &[u8], stored: &str) -> anyhow::Result<String> {
    if stored.is_empty() {
        return Ok(String::new());
    }
    if !stored.starts_with(PREFIX) {
        // plaintext legacy value (migration-friendly)
        return Ok(stored.to_string());
    }
    let b64 = &stored[PREFIX.len()..];
    let blob = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("base64 decode")?;
    if blob.len() < 12 {
        return Err(anyhow!("ciphertext too short"));
    }
    let (nonce_bytes, ct) = blob.split_at(12);
    let key = derive_key(master)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("{e}"))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let pt = cipher
        .decrypt(nonce, ct)
        .map_err(|_| anyhow!("decrypt failed (wrong master key or corrupted data)"))?;
    String::from_utf8(pt).context("utf8")
}

/// Keys whose values must be encrypted at rest.
pub fn is_secret_key(key: &str) -> bool {
    matches!(
        key,
        "openrouter_api_key" | "twitch_token" | "twitch_client_secret"
    )
}

pub fn prepare_for_store(master: &[u8], key: &str, value: &str) -> anyhow::Result<String> {
    if value == "********" {
        // sentinel: caller should not overwrite
        return Ok(value.to_string());
    }
    if is_secret_key(key) && !value.is_empty() {
        encrypt(master, value)
    } else {
        Ok(value.to_string())
    }
}

pub fn prepare_for_read(master: &[u8], key: &str, stored: &str, mask: bool) -> String {
    if is_secret_key(key) {
        match decrypt(master, stored) {
            Ok(pt) if !pt.is_empty() => {
                if mask {
                    "********".to_string()
                } else {
                    pt
                }
            }
            _ => String::new(),
        }
    } else {
        stored.to_string()
    }
}
