mod memory;
pub use memory::MemoryStore;

#[cfg(feature = "vec-sqlite")]
pub mod sqlite_vec;
#[cfg(feature = "vec-sqlite")]
pub use sqlite_vec::SqliteVecStore;

use crate::Chunk;

/// Errors that can occur in a [`VectorStore`].
#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    DimensionMismatch { stored: usize, requested: usize },
    Backend(String),
    NotFound,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::DimensionMismatch { stored, requested } => write!(
                f,
                "dimension mismatch: store has {stored}, got {requested}"
            ),
            Self::Backend(msg) => write!(f, "backend error: {msg}"),
            Self::NotFound => write!(f, "item not found"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A point to be stored in the vector store.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexPoint {
    pub id: u64,
    pub embedding: Vec<f32>,
    pub chunk: Chunk,
}

/// Filter criteria for vector queries.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub rel_path_prefix: Option<String>,
    pub langs: Vec<crate::Lang>,
    pub kinds: Vec<crate::ChunkKind>,
}

/// A search result from a vector query.
#[derive(Debug, Clone)]
pub struct Hit {
    pub chunk: Chunk,
    /// Similarity score; higher is better. Backends convert from distance if needed.
    pub score: f32,
}

/// Trait for vector stores that can upsert, delete, query, and count chunks.
pub trait VectorStore: Send + Sync {
    fn upsert(&self, points: Vec<IndexPoint>) -> Result<(), StoreError>;
    fn delete_by_path(&self, rel_path: &str) -> Result<(), StoreError>;
    fn query(&self, vec: &[f32], k: usize, filter: Option<Filter>) -> Result<Vec<Hit>, StoreError>;
    fn count(&self) -> Result<usize, StoreError>;
    fn clear(&self) -> Result<(), StoreError>;
    fn dim(&self) -> usize;
}
