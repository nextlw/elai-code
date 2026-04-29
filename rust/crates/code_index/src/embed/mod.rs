/// Errors that can occur during embedding.
#[derive(Debug)]
pub enum EmbedError {
    Io(std::io::Error),
    Backend(String),
    DimensionMismatch { expected: usize, got: usize },
    MissingApiKey(&'static str),
    Network(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Backend(msg) => write!(f, "backend error: {msg}"),
            Self::DimensionMismatch { expected, got } => {
                write!(f, "dimension mismatch: expected {expected}, got {got}")
            }
            Self::MissingApiKey(key) => write!(f, "missing API key: {key}"),
            Self::Network(msg) => write!(f, "network error: {msg}"),
        }
    }
}

impl std::error::Error for EmbedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for EmbedError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Trait for text embedding backends.
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    fn name(&self) -> &str;
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// Extension for embedders that also support image inputs (base64-encoded).
pub trait MultimodalEmbedder: Embedder {
    fn embed_images(&self, base64_images: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// Deterministic embedder via `xxh3(text)` — used in tests and offline fallback.
pub struct MockEmbedder {
    pub dim: usize,
}

impl MockEmbedder {
    #[must_use]
    pub const fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl Embedder for MockEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "mock"
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        use xxhash_rust::xxh3::xxh3_64;
        Ok(texts
            .iter()
            .map(|t| {
                let h = xxh3_64(t.as_bytes());
                (0..self.dim)
                    .map(|i| {
                        let bit = (h >> (i % 64)) & 1;
                        if bit == 0 { -1.0 } else { 1.0 }
                    })
                    .collect()
            })
            .collect())
    }
}

#[cfg(feature = "embed-fastembed")]
pub mod local;
#[cfg(feature = "embed-fastembed")]
pub use local::LocalFastEmbedder;

#[cfg(feature = "embed-http")]
pub mod ollama;
#[cfg(feature = "embed-http")]
pub use ollama::OllamaEmbedder;

#[cfg(feature = "embed-jina")]
pub mod jina;
#[cfg(feature = "embed-jina")]
pub use jina::JinaClipEmbedder;

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_embedder_is_deterministic() {
        let embedder = MockEmbedder::new(8);
        let text = "hello world".to_string();

        let v1 = embedder.embed_batch(std::slice::from_ref(&text)).unwrap();
        let v2 = embedder.embed_batch(std::slice::from_ref(&text)).unwrap();

        assert_eq!(v1, v2, "same text must produce identical embeddings");
    }

    #[test]
    fn mock_embedder_respects_dim() {
        let dim = 16;
        let embedder = MockEmbedder::new(dim);

        let result = embedder
            .embed_batch(&["a".to_string(), "b".to_string()])
            .unwrap();

        assert_eq!(result.len(), 2, "should return one embedding per text");
        assert_eq!(result[0].len(), dim, "first embedding has wrong dim");
        assert_eq!(result[1].len(), dim, "second embedding has wrong dim");
    }
}
