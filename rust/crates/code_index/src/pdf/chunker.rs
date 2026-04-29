use std::path::Path;

use xxhash_rust::xxh3::xxh3_64;

use crate::{Chunk, ChunkKind, Lang};

/// Chunker para texto de páginas PDF.
/// Divide o texto em janelas de `chunk_size` caracteres com passo `step`.
/// Cada chunk preserva o número de página como `line_start` / `line_end`.
pub struct PdfChunker {
    pub chunk_size: usize,
    pub step: usize,
}

impl PdfChunker {
    pub const DEFAULT_CHUNK_SIZE: usize = 300;

    #[must_use]
    pub fn new() -> Self {
        Self {
            chunk_size: Self::DEFAULT_CHUNK_SIZE,
            step: Self::DEFAULT_CHUNK_SIZE,
        }
    }

    #[must_use]
    pub fn with_overlap(chunk_size: usize, overlap: usize) -> Self {
        let step = chunk_size.saturating_sub(overlap).max(1);
        Self { chunk_size, step }
    }

    /// Chunkeia `text` da `page_num` em `Vec<Chunk>`.
    #[must_use]
    pub fn chunk_page(&self, rel_path: &Path, page_num: u32, text: &str) -> Vec<Chunk> {
        if text.trim().is_empty() {
            return Vec::new();
        }

        let chars: Vec<char> = text.chars().collect();
        let mut chunks = Vec::new();
        let mut start = 0;

        while start < chars.len() {
            let end = (start + self.chunk_size).min(chars.len());
            let snippet: String = chars[start..end].iter().collect();
            let snippet = snippet.trim().to_string();

            if !snippet.is_empty() {
                let content_hash = xxh3_64(snippet.as_bytes());
                chunks.push(Chunk {
                    rel_path: rel_path.to_string_lossy().into_owned(),
                    line_start: page_num,
                    line_end: page_num,
                    lang: Lang::Pdf,
                    symbol: None,
                    kind: ChunkKind::PdfPage,
                    content_hash,
                    snippet,
                });
            }

            if end == chars.len() {
                break;
            }
            start += self.step;
        }

        chunks
    }
}

impl Default for PdfChunker {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_page_text_into_correct_size() {
        let chunker = PdfChunker::new();
        let text = "a".repeat(700);
        let chunks = chunker.chunk_page(Path::new("doc.pdf"), 1, &text);
        // 700 chars / 300 step = 3 chunks (0..300, 300..600, 600..700)
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].snippet.len(), 300);
        assert_eq!(chunks[1].snippet.len(), 300);
        assert_eq!(chunks[2].snippet.len(), 100);
    }

    #[test]
    fn chunks_carry_correct_lang_and_kind() {
        let chunker = PdfChunker::new();
        let chunks = chunker.chunk_page(Path::new("x.pdf"), 5, "hello world");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].lang, Lang::Pdf);
        assert_eq!(chunks[0].kind, ChunkKind::PdfPage);
        assert_eq!(chunks[0].line_start, 5);
        assert_eq!(chunks[0].line_end, 5);
    }

    #[test]
    fn empty_text_produces_no_chunks() {
        let chunker = PdfChunker::new();
        let chunks = chunker.chunk_page(Path::new("x.pdf"), 1, "   ");
        assert!(chunks.is_empty());
    }

    #[test]
    fn overlap_produces_more_chunks() {
        let chunker = PdfChunker::with_overlap(100, 50);
        let text = "x".repeat(200);
        let chunks = chunker.chunk_page(Path::new("x.pdf"), 1, &text);
        // step = 50, so: 0..100, 50..150, 100..200, 150..200 → 4 chunks
        assert!(chunks.len() > 2);
    }
}
