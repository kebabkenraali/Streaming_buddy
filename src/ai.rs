use anyhow::{anyhow, Context, Result};
use serde_json::Value;

pub struct AiReply {
    pub text: String,
}

pub async fn chat_complete(
    api_key: &str,
    model: &str,
    messages: Vec<Value>,
) -> Result<AiReply> {
    if api_key.trim().is_empty() {
        anyhow::bail!(
            "OpenRouter API key is not set. Add it in Settings (encrypted at rest)."
        );
    }
    let client = reqwest::Client::builder()
        .user_agent("streaming-buddy-rs/0.1")
        .build()?;
    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": 400,
        "temperature": 0.7,
    });
    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(api_key.trim())
        .json(&body)
        .send()
        .await
        .context("OpenRouter request")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("OpenRouter HTTP {status}: {}", &text.chars().take(300).collect::<String>());
    }
    let v: Value = serde_json::from_str(&text).context("parse OpenRouter json")?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("Unexpected OpenRouter response"))?
        .trim()
        .to_string();
    if content.is_empty() {
        anyhow::bail!("OpenRouter returned an empty reply");
    }
    Ok(AiReply { text: content })
}
