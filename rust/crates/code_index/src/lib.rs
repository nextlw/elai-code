pub mod chunker;
pub mod embed;
pub mod facts;
pub mod indexer;
pub mod progress;
pub mod store;
pub mod walker;

pub use chunker::{Chunker, DefaultChunker, WindowChunker};
#[cfg(feature = "tree-sitter-langs")]
pub use chunker::SemanticChunker;
pub use embed::{EmbedError, Embedder, MockEmbedder};
#[cfg(feature = "embed-fastembed")]
pub use embed::LocalFastEmbedder;
#[cfg(feature = "embed-http")]
pub use embed::OllamaEmbedder;
pub use facts::{collect_facts, DirSummary, ProjectFacts, TopSymbol};
pub use indexer::{IndexError, IndexPhase, IndexProgress, Indexer};
pub use progress::{progress_bar, progress_bar_labeled, NoopReporter, ProgressReporter};
pub use store::{Filter, Hit, IndexPoint, MemoryStore, StoreError, VectorStore};
#[cfg(feature = "vec-sqlite")]
pub use store::SqliteVecStore;
pub use walker::{walk_project, walk_project_with, IgnoreRules, WalkOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Markdown,
    Toml,
    Json,
    Plain,
}

impl Lang {
    #[must_use]
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" => Self::Python,
            "go" => Self::Go,
            "md" | "markdown" => Self::Markdown,
            "toml" => Self::Toml,
            "json" => Self::Json,
            _ => Self::Plain,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    Function,
    Method,
    Class,
    Impl,
    Module,
    Window,
    Plain,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    pub rel_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub lang: Lang,
    pub symbol: Option<String>,
    pub kind: ChunkKind,
    pub content_hash: u64,
    pub snippet: String,
}

impl Chunk {
    /// Determinístico: `xxh3(rel_path)` ^ `chunk_idx` (estável entre runs).
    #[must_use]
    pub fn id_for(rel_path: &str, chunk_idx: u32) -> u64 {
        use xxhash_rust::xxh3::xxh3_64;
        xxh3_64(rel_path.as_bytes()) ^ u64::from(chunk_idx)
    }
}

#[derive(Debug, Default, Clone)]
pub struct IndexerStats {
    pub files_indexed: usize,
    pub chunks_indexed: usize,
    pub bytes_processed: usize,
    pub elapsed_ms: u128,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_id_is_deterministic() {
        let id1 = Chunk::id_for("a/b.rs", 0);
        let id2 = Chunk::id_for("a/b.rs", 0);
        assert_eq!(id1, id2, "id_for must be deterministic");

        // Different inputs must produce different ids
        let id3 = Chunk::id_for("a/b.rs", 1);
        assert_ne!(id1, id3);
    }

    #[test]
    fn lang_from_extension_covers_known_extensions() {
        assert_eq!(Lang::from_extension("rs"), Lang::Rust);
        assert_eq!(Lang::from_extension("ts"), Lang::TypeScript);
        assert_eq!(Lang::from_extension("tsx"), Lang::Tsx);
        assert_eq!(Lang::from_extension("js"), Lang::JavaScript);
        assert_eq!(Lang::from_extension("jsx"), Lang::JavaScript);
        assert_eq!(Lang::from_extension("mjs"), Lang::JavaScript);
        assert_eq!(Lang::from_extension("cjs"), Lang::JavaScript);
        assert_eq!(Lang::from_extension("py"), Lang::Python);
        assert_eq!(Lang::from_extension("go"), Lang::Go);
        assert_eq!(Lang::from_extension("md"), Lang::Markdown);
        assert_eq!(Lang::from_extension("markdown"), Lang::Markdown);
        assert_eq!(Lang::from_extension("toml"), Lang::Toml);
        assert_eq!(Lang::from_extension("json"), Lang::Json);
        assert_eq!(Lang::from_extension("xyz"), Lang::Plain);
        // Case insensitive
        assert_eq!(Lang::from_extension("RS"), Lang::Rust);
    }
}
