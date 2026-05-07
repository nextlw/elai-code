//! `OllamaEmbedder` — embeddings via Ollama local (sem chave de API).
//!
//! API: `POST {base_url}/api/embed`
//!   body: `{ "model": "nomic-embed-text", "input": ["texto", ...] }`
//!   resp: `{ "model": "...", "embeddings": [[...], ...] }`

use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::json;

use super::{EmbedError, Embedder};

pub const DEFAULT_BASE_URL: &str = "http://localhost:11434";
pub const DEFAULT_MODEL: &str = "nomic-embed-text";
pub const DEFAULT_DIM: usize = 768;

#[derive(serde::Deserialize)]
struct OllamaResp {
    embeddings: Vec<Vec<f32>>,
}

pub struct OllamaEmbedder {
    client: Client,
    base_url: String,
    model: String,
    dim: usize,
}

impl OllamaEmbedder {
    /// Cria embedder com base URL + model. `dim` deve bater com o modelo escolhido
    /// (768 nomic-embed-text, 1024 mxbai-embed-large, 384 all-minilm).
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        dim: usize,
    ) -> Result<Self, EmbedError> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let model = model.into();
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| EmbedError::Backend(e.to_string()))?;
        // Healthcheck rápido: GET /api/tags. Se falhar, erro claro.
        let tags_url = format!("{base_url}/api/tags");
        match client
            .get(&tags_url)
            .timeout(Duration::from_secs(3))
            .send()
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => {
                return Err(EmbedError::Network(format!(
                    "Ollama at {base_url} responded {} on /api/tags. Is `ollama serve` running?",
                    r.status()
                )));
            }
            Err(e) => {
                return Err(EmbedError::Network(format!(
                    "Cannot reach Ollama at {base_url}: {e}. \
                     Start with `ollama serve` or use --embed-provider local."
                )));
            }
        }
        Ok(Self {
            client,
            base_url,
            model,
            dim,
        })
    }

    /// Constructor padrão: localhost + nomic-embed-text + 768-dim.
    pub fn default_local() -> Result<Self, EmbedError> {
        Self::new(DEFAULT_BASE_URL, DEFAULT_MODEL, DEFAULT_DIM)
    }
}

impl Embedder for OllamaEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &str {
        &self.model
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/api/embed", self.base_url);
        let body = json!({ "model": self.model, "input": texts });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| EmbedError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(EmbedError::Backend(format!(
                "Ollama embed {status}: {text}"
            )));
        }
        let parsed: OllamaResp = resp
            .json()
            .map_err(|e| EmbedError::Backend(format!("decode ollama response: {e}")))?;
        if parsed.embeddings.len() != texts.len() {
            return Err(EmbedError::Backend(format!(
                "Ollama returned {} embeddings for {} inputs",
                parsed.embeddings.len(),
                texts.len()
            )));
        }
        for emb in &parsed.embeddings {
            if emb.len() != self.dim {
                return Err(EmbedError::DimensionMismatch {
                    expected: self.dim,
                    got: emb.len(),
                });
            }
        }
        Ok(parsed.embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_network_error_when_ollama_not_reachable() {
        // Porta com 99% de chance de estar fechada
        let result = OllamaEmbedder::new("http://localhost:1", "nomic-embed-text", 768);
        assert!(matches!(result, Err(EmbedError::Network(_))));
    }

    // Marcado #[ignore]: requer `ollama serve` + `ollama pull nomic-embed-text` rodando.
    #[test]
    #[ignore = "requires running Ollama with nomic-embed-text"]
    fn ollama_embed_returns_768_dim() {
        let e = OllamaEmbedder::default_local().expect("ollama should be running");
        let v = e.embed_batch(&["hello world".into()]).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), 768);
    }
}
