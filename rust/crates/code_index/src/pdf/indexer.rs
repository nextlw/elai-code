use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use xxhash_rust::xxh3::xxh3_64;

use crate::embed::MultimodalEmbedder;
use crate::store::{IndexPoint, StoreError, VectorStore};
use crate::walker::walk_project;
use crate::{Chunk, ChunkKind, Lang};

use super::chunker::PdfChunker;
use super::extractor::{self, ExtractError};

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum PdfIndexError {
    Extract(ExtractError),
    Embed(crate::embed::EmbedError),
    Store(StoreError),
}

impl std::fmt::Display for PdfIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extract(e) => write!(f, "extraction error: {e}"),
            Self::Embed(e) => write!(f, "embed error: {e}"),
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for PdfIndexError {}

impl From<ExtractError> for PdfIndexError {
    fn from(e: ExtractError) -> Self { Self::Extract(e) }
}

impl From<crate::embed::EmbedError> for PdfIndexError {
    fn from(e: crate::embed::EmbedError) -> Self { Self::Embed(e) }
}

impl From<StoreError> for PdfIndexError {
    fn from(e: StoreError) -> Self { Self::Store(e) }
}

// ─── Stats ────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct PdfIndexerStats {
    pub pdfs_indexed: usize,
    pub text_chunks: usize,
    pub image_chunks: usize,
    pub elapsed_ms: u128,
}

// ─── PdfIndexer ───────────────────────────────────────────────────────────────

/// Indexa PDFs numa `VectorStore` dedicada via `jina-clip-v2` (512d).
/// Texto de páginas → `ChunkKind::PdfPage`.
/// Imagens embutidas → `ChunkKind::PdfImage`.
pub struct PdfIndexer {
    pub root: PathBuf,
    pub embedder: Arc<dyn MultimodalEmbedder>,
    pub store: Arc<dyn VectorStore>,
    pub chunker: PdfChunker,
}

impl PdfIndexer {
    pub fn new(
        root: impl Into<PathBuf>,
        embedder: Arc<dyn MultimodalEmbedder>,
        store: Arc<dyn VectorStore>,
    ) -> Self {
        Self {
            root: root.into(),
            embedder,
            store,
            chunker: PdfChunker::new(),
        }
    }

    /// Indexa todos os PDFs encontrados em `self.root`.
    pub fn index_all(&self) -> Result<PdfIndexerStats, PdfIndexError> {
        let start = Instant::now();
        let pdfs: Vec<PathBuf> = walk_project(&self.root)
            .into_iter()
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("pdf"))
                    .unwrap_or(false)
            })
            .collect();

        let mut total = PdfIndexerStats::default();
        for abs_path in &pdfs {
            let rel = abs_path.strip_prefix(&self.root).unwrap_or(abs_path).to_path_buf();
            let s = self.index_pdf(&rel)?;
            total.pdfs_indexed += 1;
            total.text_chunks += s.text_chunks;
            total.image_chunks += s.image_chunks;
        }
        total.elapsed_ms = start.elapsed().as_millis();
        Ok(total)
    }

    /// Indexa um único PDF por caminho relativo ao `root`.
    pub fn index_pdf(&self, rel: &Path) -> Result<PdfIndexerStats, PdfIndexError> {
        let start = Instant::now();
        let abs = self.root.join(rel);
        let rel_str = rel.to_string_lossy().into_owned();

        self.store.delete_by_path(&rel_str)?;

        let mut stats = PdfIndexerStats { pdfs_indexed: 1, ..Default::default() };

        // ── Texto ──────────────────────────────────────────────────────────
        let pages = extractor::extract_text(&abs)?;
        let text_chunks: Vec<Chunk> = pages
            .iter()
            .flat_map(|p| self.chunker.chunk_page(rel, p.page_num, &p.text))
            .collect();

        if !text_chunks.is_empty() {
            let snippets: Vec<String> = text_chunks.iter().map(|c| c.snippet.clone()).collect();
            let embeddings = self.embedder.embed_batch(&snippets)?;
            let points: Vec<IndexPoint> = text_chunks
                .into_iter()
                .enumerate()
                .zip(embeddings)
                .map(|((idx, chunk), emb)| IndexPoint {
                    #[allow(clippy::cast_possible_truncation)]
                    id: Chunk::id_for(&chunk.rel_path, idx as u32),
                    embedding: emb,
                    chunk,
                })
                .collect();
            stats.text_chunks = points.len();
            self.store.upsert(points)?;
        }

        // ── Imagens ────────────────────────────────────────────────────────
        let images = extractor::extract_images(&abs)?;
        if !images.is_empty() {
            let base64_images: Vec<String> =
                images.iter().map(|img| B64.encode(&img.bytes)).collect();
            let embeddings = self.embedder.embed_images(&base64_images)?;

            // Offset de 100_000 para evitar colisão de id com chunks de texto
            let points: Vec<IndexPoint> = images
                .iter()
                .enumerate()
                .zip(embeddings)
                .map(|((idx, img), emb)| {
                    let content_hash = xxh3_64(&img.bytes);
                    #[allow(clippy::cast_possible_truncation)]
                    let id = Chunk::id_for(&rel_str, 100_000u32.saturating_add(idx as u32));
                    IndexPoint {
                        id,
                        embedding: emb,
                        chunk: Chunk {
                            rel_path: rel_str.clone(),
                            line_start: img.page_num,
                            line_end: img.page_num,
                            lang: Lang::Pdf,
                            symbol: None,
                            kind: ChunkKind::PdfImage,
                            content_hash,
                            snippet: format!("Image on page {}", img.page_num),
                        },
                    }
                })
                .collect();

            stats.image_chunks = points.len();
            self.store.upsert(points)?;
        }

        stats.elapsed_ms = start.elapsed().as_millis();
        Ok(stats)
    }
}
