use std::path::Path;

use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};
use xxhash_rust::xxh3::xxh3_64;

use super::lang_query;
use super::Chunker;
use crate::{Chunk, ChunkKind, Lang};

const MAX_SNIPPET_BYTES: usize = 1500;

pub struct SemanticChunker;

impl SemanticChunker {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for SemanticChunker {
    fn default() -> Self {
        Self::new()
    }
}

fn lang_for(lang: Lang) -> Option<(Language, &'static str)> {
    match lang {
        Lang::Rust => Some((
            tree_sitter_rust::LANGUAGE.into(),
            lang_query::RUST_QUERY,
        )),
        Lang::TypeScript => Some((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            lang_query::TYPESCRIPT_QUERY,
        )),
        Lang::Tsx => Some((
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            lang_query::TYPESCRIPT_QUERY,
        )),
        Lang::JavaScript => Some((
            tree_sitter_javascript::LANGUAGE.into(),
            lang_query::JAVASCRIPT_QUERY,
        )),
        Lang::Python => Some((
            tree_sitter_python::LANGUAGE.into(),
            lang_query::PYTHON_QUERY,
        )),
        Lang::Go => Some((
            tree_sitter_go::LANGUAGE.into(),
            lang_query::GO_QUERY,
        )),
        _ => None,
    }
}

fn kind_from_node(node_kind: &str) -> ChunkKind {
    match node_kind {
        "method_definition" => ChunkKind::Method,
        "class_declaration"
        | "class_definition"
        | "interface_declaration"
        | "trait_item"
        | "struct_item"
        | "enum_item"
        | "enum_declaration"
        | "type_declaration" => ChunkKind::Class,
        "impl_item" => ChunkKind::Impl,
        _ => ChunkKind::Function,
    }
}

impl Chunker for SemanticChunker {
    fn chunk(&self, path: &Path, source: &str, lang: Lang) -> Vec<Chunk> {
        if source.is_empty() {
            return Vec::new();
        }
        let Some((language, query_src)) = lang_for(lang) else {
            return Vec::new();
        };

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return Vec::new();
        }
        let Some(tree) = parser.parse(source, None) else {
            return Vec::new();
        };
        let Ok(query) = Query::new(&language, query_src) else {
            return Vec::new();
        };

        let symbol_idx = query.capture_index_for_name("symbol");
        let Some(decl_idx) = query.capture_index_for_name("decl") else {
            return Vec::new();
        };

        let mut cursor = QueryCursor::new();
        let rel_path = path.to_string_lossy().to_string();
        let bytes = source.as_bytes();
        let mut chunks = Vec::new();

        let mut matches = cursor.matches(&query, tree.root_node(), bytes);
        while let Some(m) = matches.next() {
            let mut decl_node = None;
            let mut symbol_text: Option<String> = None;
            for cap in m.captures {
                if Some(cap.index) == symbol_idx {
                    if let Ok(t) = cap.node.utf8_text(bytes) {
                        symbol_text = Some(t.to_string());
                    }
                } else if cap.index == decl_idx {
                    decl_node = Some(cap.node);
                }
            }
            let Some(node) = decl_node else { continue };
            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            #[allow(clippy::cast_possible_truncation)]
            let line_start = (node.start_position().row + 1) as u32;
            #[allow(clippy::cast_possible_truncation)]
            let line_end = (node.end_position().row + 1) as u32;
            let slice = &source[start_byte..end_byte];
            let snippet = if slice.len() > MAX_SNIPPET_BYTES {
                let mut idx = MAX_SNIPPET_BYTES.min(slice.len());
                while idx > 0 && !slice.is_char_boundary(idx) {
                    idx -= 1;
                }
                slice[..idx].to_string()
            } else {
                slice.to_string()
            };
            chunks.push(Chunk {
                rel_path: rel_path.clone(),
                line_start,
                line_end,
                lang,
                symbol: symbol_text,
                kind: kind_from_node(node.kind()),
                content_hash: xxh3_64(slice.as_bytes()),
                snippet,
            });
        }
        chunks
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn rust_extracts_function_and_struct() {
        let c = SemanticChunker::new();
        let src = "pub struct Foo;\nfn bar() -> i32 { 42 }\n";
        let chunks = c.chunk(&PathBuf::from("a.rs"), src, Lang::Rust);
        let names: Vec<_> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(names.contains(&"Foo".to_string()));
        assert!(names.contains(&"bar".to_string()));
    }

    #[test]
    fn typescript_extracts_class_and_function() {
        let c = SemanticChunker::new();
        let src = "export class Bar {}\nfunction baz() { return 1; }\n";
        let chunks = c.chunk(&PathBuf::from("a.ts"), src, Lang::TypeScript);
        let names: Vec<_> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(names.contains(&"Bar".to_string()));
        assert!(names.contains(&"baz".to_string()));
    }

    #[test]
    fn python_extracts_function_and_class() {
        let c = SemanticChunker::new();
        let src = "class C:\n    pass\n\ndef foo():\n    return 1\n";
        let chunks = c.chunk(&PathBuf::from("a.py"), src, Lang::Python);
        let names: Vec<_> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(names.contains(&"C".to_string()));
        assert!(names.contains(&"foo".to_string()));
    }

    #[test]
    fn go_extracts_function() {
        let c = SemanticChunker::new();
        let src = "package main\nfunc Add(a, b int) int { return a + b }\n";
        let chunks = c.chunk(&PathBuf::from("a.go"), src, Lang::Go);
        let names: Vec<_> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(names.contains(&"Add".to_string()));
    }

    #[test]
    fn javascript_extracts_function() {
        let c = SemanticChunker::new();
        let src = "const greet = () => 'hi';\nfunction main() {}\n";
        let chunks = c.chunk(&PathBuf::from("a.js"), src, Lang::JavaScript);
        let names: Vec<_> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(names.contains(&"main".to_string()));
        assert!(!chunks.is_empty());
    }

    #[test]
    fn unsupported_lang_returns_empty() {
        let c = SemanticChunker::new();
        let chunks = c.chunk(&PathBuf::from("a.txt"), "hello", Lang::Plain);
        assert!(chunks.is_empty());
    }
}
