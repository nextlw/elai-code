//! `LocalFastEmbedder` — embeddings via crate fastembed (ONNX, BGE-small por padrão).
//!
//! Usa fastembed v5 com backend ONNX Runtime. O modelo é baixado automaticamente
//! para o cache do `HuggingFace` (~80 MB na primeira execução).

use std::path::PathBuf;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use super::{EmbedError, Embedder};

/// Diretório onde fastembed armazena modelos ONNX. Centralizamos em `~/.elai/`
/// para que `elai uninstall` (que limpa `~/.elai/`) também leve o cache.
/// Override via `ELAI_FASTEMBED_CACHE_DIR`.
fn fastembed_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ELAI_FASTEMBED_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".elai").join("fastembed_cache")
}

/// Modelo padrão (~80MB ONNX, 384-dim).
pub const DEFAULT_MODEL: EmbeddingModel = EmbeddingModel::BGESmallENV15;

pub struct LocalFastEmbedder {
    inner: Mutex<TextEmbedding>,
    dim: usize,
    model_name: String,
}

impl LocalFastEmbedder {
    /// Carrega modelo padrão (BGE-small-en-v1.5). Primeira run baixa ~80MB para cache.
    pub fn new() -> Result<Self, EmbedError> {
        Self::with_model(DEFAULT_MODEL)
    }

    pub fn with_model(model: EmbeddingModel) -> Result<Self, EmbedError> {
        let model_name = format!("{model:?}");
        // Progress bar do fastembed (indicatif) escreveria direto no stderr e
        // corromperia qualquer TUI ativo. Mantemos silencioso por padrão; o
        // caller (init.rs) imprime sua própria mensagem antes da chamada.
        // Override com `ELAI_FASTEMBED_PROGRESS=1` para debug fora do TUI.
        let show_progress = std::env::var_os("ELAI_FASTEMBED_PROGRESS").is_some();
        let cache_dir = fastembed_cache_dir();
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            return Err(EmbedError::Backend(format!(
                "create fastembed cache dir {}: {e}",
                cache_dir.display()
            )));
        }
        let init = TextInitOptions::new(model)
            .with_show_download_progress(show_progress)
            .with_cache_dir(cache_dir);
        let inner =
            TextEmbedding::try_new(init).map_err(|e| EmbedError::Backend(e.to_string()))?;
        // Heurística de dimensão por variante.
        let dim = dim_for_model(&model_name);
        Ok(Self {
            inner: Mutex::new(inner),
            dim,
            model_name,
        })
    }
}

fn dim_for_model(name: &str) -> usize {
    if name.contains("BGESmall") || name.contains("AllMiniLM") {
        384
    } else if name.contains("BGEBase") || name.contains("NomicEmbed") {
        768
    } else if name.contains("BGELarge") || name.contains("MxbaiEmbedLarge") {
        1024
    } else {
        384 // fallback conservador
    }
}

impl Embedder for LocalFastEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &str {
        &self.model_name
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| EmbedError::Backend(format!("mutex poisoned: {e}")))?;
        guard
            .embed(texts, None)
            .map_err(|e| EmbedError::Backend(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Marcado #[ignore] porque baixa modelo ONNX ~80MB. Rodar manual com --ignored.
    #[test]
    #[ignore = "downloads ~80MB ONNX model on first run"]
    fn local_embedder_produces_384_dim_vectors() {
        let e = LocalFastEmbedder::new().expect("fastembed should init");
        assert_eq!(e.dim(), 384);
        let v = e.embed_batch(&["hello".into(), "world".into()]).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].len(), 384);
        // Determinístico: mesmo input → mesmo output.
        let v2 = e.embed_batch(&["hello".into()]).unwrap();
        for (a, b) in v[0].iter().zip(&v2[0]) {
            assert!((a - b).abs() < 1e-5);
        }
    }
}
