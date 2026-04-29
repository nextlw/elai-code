use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};

use super::{EmbedError, Embedder, MultimodalEmbedder};

const JINA_API_URL: &str = "https://api.jina.ai/v1/embeddings";
const MODEL: &str = "jina-clip-v2";
const DIM: usize = 512;
const BATCH_SIZE: usize = 32;
const MAX_RETRIES: u32 = 3;

// ─── Request / Response ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct JinaRequest {
    model: &'static str,
    input: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct JinaResponse {
    data: Vec<JinaEmbedding>,
}

#[derive(Deserialize)]
struct JinaEmbedding {
    embedding: Vec<f32>,
}

// ─── JinaClipEmbedder ────────────────────────────────────────────────────────

/// Embedder multimodal via `jina-clip-v2` (512d).
/// Suporta texto (`Embedder`) e imagens base64 (`MultimodalEmbedder`).
/// API key via env var `JINA_API_KEY`.
pub struct JinaClipEmbedder {
    client: Client,
    api_key: String,
}

impl JinaClipEmbedder {
    pub fn new() -> Result<Self, EmbedError> {
        let api_key = std::env::var("JINA_API_KEY")
            .map_err(|_| EmbedError::MissingApiKey("JINA_API_KEY"))?;
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| EmbedError::Network(e.to_string()))?;
        Ok(Self { client, api_key })
    }

    fn post_batch(&self, input: Vec<serde_json::Value>) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = JinaRequest { model: MODEL, input };
        let mut last_err = String::new();
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
            }
            let res = self
                .client
                .post(JINA_API_URL)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send();
            match res {
                Ok(r) if r.status().is_success() => {
                    let parsed: JinaResponse = r
                        .json()
                        .map_err(|e| EmbedError::Backend(e.to_string()))?;
                    return Ok(parsed.data.into_iter().map(|d| d.embedding).collect());
                }
                Ok(r) => {
                    last_err = format!("HTTP {}", r.status());
                }
                Err(e) => {
                    last_err = e.to_string();
                }
            }
        }
        Err(EmbedError::Network(last_err))
    }
}

impl Embedder for JinaClipEmbedder {
    fn dim(&self) -> usize {
        DIM
    }

    fn name(&self) -> &str {
        MODEL
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH_SIZE) {
            let input = chunk
                .iter()
                .map(|t| serde_json::Value::String(t.clone()))
                .collect();
            all.extend(self.post_batch(input)?);
        }
        Ok(all)
    }
}

impl MultimodalEmbedder for JinaClipEmbedder {
    fn embed_images(&self, base64_images: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut all = Vec::with_capacity(base64_images.len());
        for chunk in base64_images.chunks(BATCH_SIZE) {
            let input = chunk
                .iter()
                .map(|b64| serde_json::json!({ "image": b64 }))
                .collect();
            all.extend(self.post_batch(input)?);
        }
        Ok(all)
    }
}
