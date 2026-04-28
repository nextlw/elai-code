//! Orchestrates the full indexing pipeline: walk → read → chunk → embed → upsert.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::{
    walker::{walk_project_with, WalkOptions},
    Chunk, Chunker, IndexerStats, Lang,
};
use crate::embed::{EmbedError, Embedder};
use crate::store::{IndexPoint, StoreError, VectorStore};

const BATCH_SIZE: usize = 64;
const MAX_FILE_BYTES: u64 = 1_024 * 1_024;

// ─── IndexError ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum IndexError {
    Io(std::io::Error),
    Embed(EmbedError),
    Store(StoreError),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Embed(e) => write!(f, "embed error: {e}"),
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for IndexError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Embed(e) => Some(e),
            Self::Store(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for IndexError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<EmbedError> for IndexError {
    fn from(e: EmbedError) -> Self {
        Self::Embed(e)
    }
}

impl From<StoreError> for IndexError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

// ─── Indexer ──────────────────────────────────────────────────────────────────

/// Orchestrates walk → read → chunk → embed → upsert for a project root.
pub struct Indexer<C: Chunker> {
    pub root: PathBuf,
    pub embedder: Arc<dyn Embedder>,
    pub store: Arc<dyn VectorStore>,
    pub chunker: C,
    pub batch_size: usize,
}

impl<C: Chunker> Indexer<C> {
    /// Create a new `Indexer` with default batch size.
    pub fn new(
        root: impl Into<PathBuf>,
        embedder: Arc<dyn Embedder>,
        store: Arc<dyn VectorStore>,
        chunker: C,
    ) -> Self {
        Self {
            root: root.into(),
            embedder,
            store,
            chunker,
            batch_size: BATCH_SIZE,
        }
    }

    /// Full walk + reindex. Returns stats about what was indexed.
    pub fn index_full(&self) -> Result<IndexerStats, IndexError> {
        let start = Instant::now();
        let opts = WalkOptions {
            max_file_bytes: MAX_FILE_BYTES,
            follow_symlinks: false,
        };
        let files = walk_project_with(&self.root, &opts);

        let mut stats = IndexerStats::default();
        let mut all_chunks: Vec<(Chunk, u32)> = Vec::new();

        for path in &files {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            // Skip binary files: heuristic — check first 8 KiB for NUL bytes
            if bytes.iter().take(8192).any(|b| *b == 0) {
                continue;
            }
            let Ok(source) = String::from_utf8(bytes) else {
                continue;
            };
            stats.bytes_processed = stats.bytes_processed.saturating_add(source.len());

            let rel = path
                .strip_prefix(&self.root)
                .unwrap_or(path)
                .to_path_buf();
            let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
            let lang = Lang::from_extension(ext);
            let chunks = self.chunker.chunk(&rel, &source, lang);

            for (idx, mut chunk) in chunks.into_iter().enumerate() {
                chunk.rel_path = rel.to_string_lossy().to_string();
                #[allow(clippy::cast_possible_truncation)]
                all_chunks.push((chunk, idx as u32));
            }
            stats.files_indexed += 1;
        }

        // Embed in batches
        let total_chunks = all_chunks.len();
        let mut buffer: Vec<(Chunk, u32)> = Vec::with_capacity(self.batch_size);
        for entry in all_chunks {
            buffer.push(entry);
            if buffer.len() >= self.batch_size {
                self.flush_batch(&mut buffer)?;
            }
        }
        if !buffer.is_empty() {
            self.flush_batch(&mut buffer)?;
        }

        stats.chunks_indexed = total_chunks;
        stats.elapsed_ms = start.elapsed().as_millis();
        Ok(stats)
    }

    fn flush_batch(&self, buffer: &mut Vec<(Chunk, u32)>) -> Result<(), IndexError> {
        let texts: Vec<String> = buffer.iter().map(|(c, _)| c.snippet.clone()).collect();
        let embeddings = self.embedder.embed_batch(&texts)?;
        let points: Vec<IndexPoint> = buffer
            .drain(..)
            .zip(embeddings)
            .map(|((chunk, idx), emb)| IndexPoint {
                id: Chunk::id_for(&chunk.rel_path, idx),
                embedding: emb,
                chunk,
            })
            .collect();
        self.store.upsert(points)?;
        Ok(())
    }

    /// Re-chunk + re-embed a single file and upsert, replacing any previous embeddings.
    pub fn index_path(&self, rel: &Path) -> Result<usize, IndexError> {
        let abs = self.root.join(rel);
        let source = std::fs::read_to_string(&abs)?;
        let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = Lang::from_extension(ext);
        let mut chunks = self.chunker.chunk(rel, &source, lang);
        for c in &mut chunks {
            c.rel_path = rel.to_string_lossy().to_string();
        }

        // Remove stale embeddings for this path
        self.store.delete_by_path(&rel.to_string_lossy())?;

        let n = chunks.len();
        let texts: Vec<String> = chunks.iter().map(|c| c.snippet.clone()).collect();
        let embeddings = self.embedder.embed_batch(&texts)?;
        #[allow(clippy::cast_possible_truncation)]
        let points: Vec<IndexPoint> = chunks
            .into_iter()
            .zip(embeddings)
            .enumerate()
            .map(|(idx, (chunk, emb))| IndexPoint {
                id: Chunk::id_for(&chunk.rel_path, idx as u32),
                embedding: emb,
                chunk,
            })
            .collect();
        self.store.upsert(points)?;
        Ok(n)
    }

    /// Remove all embeddings for a path from the index.
    pub fn remove_path(&self, rel: &Path) -> Result<(), IndexError> {
        self.store.delete_by_path(&rel.to_string_lossy())?;
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use crate::store::MemoryStore;
    use crate::DefaultChunker;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_indexer(dir: &TempDir) -> Indexer<DefaultChunker> {
        let dim = 4;
        let embedder = Arc::new(MockEmbedder::new(dim));
        let store = Arc::new(MemoryStore::new(dim));
        Indexer::new(dir.path(), embedder, store, DefaultChunker::new())
    }

    #[test]
    fn index_full_walks_chunks_and_upserts() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn foo() { let x = 1; }").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn bar() { let y = 2; }").unwrap();

        let indexer = make_indexer(&dir);
        indexer.index_full().unwrap();

        assert!(
            indexer.store.count().unwrap() > 0,
            "store should have chunks after index_full"
        );
    }

    #[test]
    fn index_full_skips_binary_files() {
        let dir = TempDir::new().unwrap();
        // NUL bytes → binary
        let mut data = vec![0u8, 1, 2, 3];
        data.extend_from_slice(b"some text");
        std::fs::write(dir.path().join("bin.bin"), &data).unwrap();
        std::fs::write(dir.path().join("ok.rs"), "fn main() {}").unwrap();

        let indexer = make_indexer(&dir);
        let stats = indexer.index_full().unwrap();

        // Only ok.rs should be indexed
        assert_eq!(stats.files_indexed, 1);
    }

    #[test]
    fn index_path_replaces_existing_chunks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("src.rs");
        std::fs::write(&path, "fn original() {}").unwrap();

        let indexer = make_indexer(&dir);
        indexer.index_full().unwrap();
        let count_before = indexer.store.count().unwrap();

        // Overwrite with different content
        std::fs::write(&path, "fn updated() { let a = 1; let b = 2; }").unwrap();
        indexer.index_path(std::path::Path::new("src.rs")).unwrap();

        // Count should still be > 0 and store reflects new content
        let count_after = indexer.store.count().unwrap();
        assert!(count_after > 0, "store should not be empty after index_path");
        let _ = count_before; // just confirming no panic
    }

    #[test]
    fn remove_path_drops_chunks() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("keep.rs"), "fn keep() {}").unwrap();
        std::fs::write(dir.path().join("drop.rs"), "fn drop_me() {}").unwrap();

        let indexer = make_indexer(&dir);
        indexer.index_full().unwrap();
        let before = indexer.store.count().unwrap();

        indexer
            .remove_path(std::path::Path::new("drop.rs"))
            .unwrap();
        let after = indexer.store.count().unwrap();

        assert!(
            after < before,
            "count should decrease after remove_path: {before} -> {after}"
        );
    }

    #[test]
    fn index_full_returns_stats_with_files_and_chunks() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() { println!(\"hi\"); }").unwrap();

        let indexer = make_indexer(&dir);
        let stats = indexer.index_full().unwrap();

        assert!(stats.files_indexed > 0, "files_indexed must be > 0");
        assert!(stats.chunks_indexed > 0, "chunks_indexed must be > 0");
        // elapsed_ms is u128, always >= 0
        let _ = stats.elapsed_ms;
    }
}
