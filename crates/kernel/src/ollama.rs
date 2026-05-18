use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Optional Ollama client for LLM-powered features (compression, consolidation).
/// Only instantiated when OLLAMA_URL is set. All features gracefully degrade without it.
pub struct OllamaClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: Option<ChatResponseMessage>,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: String,
}

impl OllamaClient {
    pub fn new(base_url: Option<String>, model: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.unwrap_or_else(|| "http://localhost:11434".to_string()),
            model: model.unwrap_or_else(|| "qwen2.5:7b".to_string()),
        }
    }

    /// Send a completion request to Ollama.
    pub async fn complete(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt.to_string(),
                },
            ],
            stream: false,
        };

        let response = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
            .await?;

        let body: ChatResponse = response.json().await?;
        Ok(body.message.map(|m| m.content).unwrap_or_default())
    }

    /// Check if Ollama is reachable.
    pub async fn is_available(&self) -> bool {
        self.client.get(&self.base_url).send().await.is_ok()
    }
}
