//! Chunking strategies: semantic (tree-sitter) and sliding-window.

mod window;
#[cfg(feature = "tree-sitter-langs")]
mod lang_query;
#[cfg(feature = "tree-sitter-langs")]
mod semantic;

pub use window::WindowChunker;
#[cfg(feature = "tree-sitter-langs")]
pub use semantic::SemanticChunker;

use std::path::Path;
use crate::{Chunk, Lang};

pub trait Chunker: Send + Sync {
    fn chunk(&self, path: &Path, source: &str, lang: Lang) -> Vec<Chunk>;
}

/// Combina `SemanticChunker` + `WindowChunker` como fallback.
pub struct DefaultChunker {
    #[cfg(feature = "tree-sitter-langs")]
    semantic: SemanticChunker,
    window: WindowChunker,
}

impl DefaultChunker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "tree-sitter-langs")]
            semantic: SemanticChunker::new(),
            window: WindowChunker::default(),
        }
    }
}

impl Default for DefaultChunker {
    fn default() -> Self {
        Self::new()
    }
}

impl Chunker for DefaultChunker {
    fn chunk(&self, path: &Path, source: &str, lang: Lang) -> Vec<Chunk> {
        // 1) Try semantic
        #[cfg(feature = "tree-sitter-langs")]
        {
            let chunks = self.semantic.chunk(path, source, lang);
            if !chunks.is_empty() {
                return chunks;
            }
        }
        // 2) Fallback window
        self.window.chunk(path, source, lang)
    }
}
