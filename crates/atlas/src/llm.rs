//! Pluggable LLM backend. Enum dispatch (no `async-trait` dep, dyn-safe by
//! construction): local Ollama (reuses hifz-core's client), any
//! OpenAI/Anthropic-compatible `/chat/completions` endpoint, or a test
//! stub. `from_env` selects; absence is fine — extraction has a
//! deterministic no-LLM fallback.

use anyhow::{Context, Result};
use hifz_core::ollama::OllamaClient;

pub enum LlmBackend {
    Ollama(OllamaClient),
    OpenAiCompat {
        base_url: String,
        api_key: String,
        model: String,
        client: reqwest::Client,
    },
    /// Deterministic canned response — test seam only.
    Stub(String),
}

impl LlmBackend {
    /// `ATLAS_LLM=openai` + `ATLAS_LLM_BASE`/`ATLAS_LLM_KEY`/`ATLAS_LLM_MODEL`
    /// → cloud; else `OLLAMA_URL` (+`OLLAMA_MODEL`) → local Ollama; else
    /// `None` (caller uses the no-LLM fallback).
    pub fn from_env() -> Option<Self> {
        if std::env::var("ATLAS_LLM").ok().as_deref() == Some("openai") {
            let base_url = std::env::var("ATLAS_LLM_BASE").ok()?;
            let api_key = std::env::var("ATLAS_LLM_KEY").unwrap_or_default();
            let model = std::env::var("ATLAS_LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());
            return Some(LlmBackend::OpenAiCompat {
                base_url,
                api_key,
                model,
                client: reqwest::Client::new(),
            });
        }
        if let Ok(url) = std::env::var("OLLAMA_URL") {
            let model = std::env::var("OLLAMA_MODEL").ok();
            return Some(LlmBackend::Ollama(OllamaClient::new(Some(url), model)));
        }
        None
    }

    pub async fn complete(&self, system: &str, user: &str) -> Result<String> {
        match self {
            LlmBackend::Ollama(c) => c.complete(system, user).await,
            LlmBackend::Stub(s) => Ok(s.clone()),
            LlmBackend::OpenAiCompat {
                base_url,
                api_key,
                model,
                client,
            } => {
                let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
                let body = serde_json::json!({
                    "model": model,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user}
                    ],
                    "temperature": 0
                });
                let mut req = client.post(&url).json(&body);
                if !api_key.is_empty() {
                    req = req.bearer_auth(api_key);
                }
                let resp = req.send().await.context("LLM request failed")?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    anyhow::bail!(
                        "LLM {status}: {}",
                        text.chars().take(200).collect::<String>()
                    );
                }
                let v: serde_json::Value =
                    serde_json::from_str(&text).context("LLM non-JSON response")?;
                v["choices"][0]["message"]["content"]
                    .as_str()
                    .map(|s| s.to_string())
                    .context("LLM response missing choices[0].message.content")
            }
        }
    }
}

/// Strip ```json … ``` fences / leading prose so `serde_json` can parse a
/// model reply that wrapped its JSON.
pub fn strip_json_fence(s: &str) -> &str {
    let s = s.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    // If there's leading prose, slice from the first `{` to the last `}`.
    match (s.find('{'), s.rfind('}')) {
        (Some(a), Some(b)) if b >= a => &s[a..=b],
        _ => s.trim(),
    }
}
