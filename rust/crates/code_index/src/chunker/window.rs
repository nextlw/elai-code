use std::path::Path;

use xxhash_rust::xxh3::xxh3_64;

use crate::{Chunk, ChunkKind, Lang};
use super::Chunker;

const TARGET_BYTES: usize = 2048;
const OVERLAP_BYTES: usize = 256;
const MAX_SNIPPET_BYTES: usize = 1500;

pub struct WindowChunker {
    pub target_bytes: usize,
    pub overlap_bytes: usize,
}

impl Default for WindowChunker {
    fn default() -> Self {
        Self {
            target_bytes: TARGET_BYTES,
            overlap_bytes: OVERLAP_BYTES,
        }
    }
}

impl Chunker for WindowChunker {
    fn chunk(&self, path: &Path, source: &str, lang: Lang) -> Vec<Chunk> {
        if source.is_empty() {
            return Vec::new();
        }
        let rel_path = path.to_string_lossy().to_string();
        let mut chunks = Vec::new();
        let bytes = source.as_bytes();
        let mut start = 0_usize;
        while start < bytes.len() {
            let end = (start + self.target_bytes).min(bytes.len());
            let real_end = find_char_boundary(source, end);
            let real_start = find_char_boundary(source, start);
            let slice = &source[real_start..real_end];
            #[allow(clippy::cast_possible_truncation)]
            let line_start = source[..real_start].matches('\n').count() as u32 + 1;
            #[allow(clippy::cast_possible_truncation)]
            let line_end = source[..real_end].matches('\n').count() as u32 + 1;
            let snippet = if slice.len() > MAX_SNIPPET_BYTES {
                slice[..find_char_boundary(slice, MAX_SNIPPET_BYTES)].to_string()
            } else {
                slice.to_string()
            };
            chunks.push(Chunk {
                rel_path: rel_path.clone(),
                line_start,
                line_end,
                lang,
                symbol: None,
                kind: if matches!(
                    lang,
                    Lang::Plain | Lang::Markdown | Lang::Toml | Lang::Json
                ) {
                    ChunkKind::Plain
                } else {
                    ChunkKind::Window
                },
                content_hash: xxh3_64(slice.as_bytes()),
                snippet,
            });
            if real_end >= bytes.len() {
                break;
            }
            start = real_end.saturating_sub(self.overlap_bytes);
            if start <= real_start {
                start = real_end; // avoid infinite loop on tiny files
            }
        }
        chunks
    }
}

fn find_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(idx) && idx < s.len() {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn empty_source_yields_no_chunks() {
        let c = WindowChunker::default();
        assert!(c.chunk(&PathBuf::from("foo.rs"), "", Lang::Rust).is_empty());
    }

    #[test]
    fn small_source_yields_single_chunk() {
        let c = WindowChunker::default();
        let chunks = c.chunk(&PathBuf::from("foo.rs"), "fn main() {}", Lang::Rust);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].kind, ChunkKind::Window);
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn large_source_yields_multiple_chunks_with_overlap() {
        let c = WindowChunker {
            target_bytes: 100,
            overlap_bytes: 20,
        };
        let source = "x".repeat(500); // 500 bytes
        let chunks = c.chunk(&PathBuf::from("a.txt"), &source, Lang::Plain);
        assert!(
            chunks.len() >= 5,
            "expected at least 5 chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(chunk.snippet.len() <= 1500);
        }
    }

    #[test]
    fn plain_lang_uses_plain_kind() {
        let c = WindowChunker::default();
        let chunks = c.chunk(
            &PathBuf::from("readme.md"),
            "# Hello\n\nWorld",
            Lang::Markdown,
        );
        assert!(matches!(chunks[0].kind, ChunkKind::Plain));
    }
}
