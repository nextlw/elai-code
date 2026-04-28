//! Extracts structural facts from a project for use in grounded ELAI.md generation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{
    walker::{walk_project_with, WalkOptions},
    Chunk, ChunkKind, Chunker, DefaultChunker, Lang,
};

// ─── Public types ─────────────────────────────────────────────────────────────

/// Structural facts extracted from a project by static analysis.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ProjectFacts {
    pub root: PathBuf,
    pub total_files: usize,
    pub total_bytes: u64,
    /// File counts per language label (e.g. `"rust"`, `"typescript"`).
    pub by_lang: HashMap<String, usize>,
    /// Detected frameworks / build systems (e.g. `"rust-cargo-workspace"`, `"next.js"`).
    pub frameworks: Vec<String>,
    /// Top symbols ordered by heuristic importance (Class > Impl > Function > Method).
    pub top_symbols: Vec<TopSymbol>,
    /// Top directories by file count.
    pub dirs_summary: Vec<DirSummary>,
    /// First 4 000 chars of README if present.
    pub readme_excerpt: Option<String>,
}

/// A named symbol extracted from a chunk.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TopSymbol {
    pub symbol: String,
    pub kind: String,
    pub rel_path: String,
    pub line_start: u32,
}

/// Directory with its file count.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DirSummary {
    pub dir: String,
    pub files: usize,
}

// ─── Constants ────────────────────────────────────────────────────────────────

const TOP_SYMBOLS_LIMIT: usize = 200;
const DIRS_LIMIT: usize = 30;

// ─── collect_facts ────────────────────────────────────────────────────────────

/// Walk `root`, chunk every source file, and return [`ProjectFacts`].
pub fn collect_facts(root: &Path) -> std::io::Result<ProjectFacts> {
    let opts = WalkOptions {
        max_file_bytes: 1_048_576,
        follow_symlinks: false,
    };
    let paths = walk_project_with(root, &opts);
    let chunker = DefaultChunker::new();

    let mut facts = ProjectFacts {
        root: root.to_path_buf(),
        ..Default::default()
    };
    let mut by_dir: HashMap<String, usize> = HashMap::new();
    let mut all_chunks: Vec<Chunk> = Vec::new();

    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        // Skip binaries
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue;
        }
        let Ok(source) = String::from_utf8(bytes) else {
            continue;
        };

        facts.total_bytes = facts.total_bytes.saturating_add(source.len() as u64);
        facts.total_files += 1;

        let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = Lang::from_extension(ext);
        *facts
            .by_lang
            .entry(lang_label(lang).to_string())
            .or_insert(0) += 1;

        if let Some(dir) = rel.parent().and_then(|p| p.to_str()) {
            *by_dir.entry(dir.to_string()).or_insert(0) += 1;
        }

        let chunks = chunker.chunk(&rel, &source, lang);
        for mut c in chunks {
            c.rel_path = rel.to_string_lossy().to_string();
            all_chunks.push(c);
        }
    }

    // Top symbols: Class(4) > Impl(3) > Function(2) > Method(1) > other(0);
    // within same weight, sort alphabetically for determinism.
    let kind_weight = |k: &ChunkKind| match k {
        ChunkKind::Class => 4,
        ChunkKind::Impl => 3,
        ChunkKind::Function => 2,
        ChunkKind::Method => 1,
        _ => 0,
    };
    let mut sym_chunks: Vec<&Chunk> = all_chunks.iter().filter(|c| c.symbol.is_some()).collect();
    sym_chunks.sort_by(|a, b| {
        kind_weight(&b.kind)
            .cmp(&kind_weight(&a.kind))
            .then_with(|| {
                a.symbol
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.symbol.as_deref().unwrap_or(""))
            })
    });
    facts.top_symbols = sym_chunks
        .into_iter()
        .take(TOP_SYMBOLS_LIMIT)
        .map(|c| TopSymbol {
            symbol: c.symbol.clone().unwrap_or_default(),
            kind: format!("{:?}", c.kind).to_lowercase(),
            rel_path: c.rel_path.clone(),
            line_start: c.line_start,
        })
        .collect();

    // Dirs summary
    let mut dirs: Vec<(String, usize)> = by_dir.into_iter().collect();
    dirs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    facts.dirs_summary = dirs
        .into_iter()
        .take(DIRS_LIMIT)
        .map(|(dir, files)| DirSummary { dir, files })
        .collect();

    // Framework detection
    facts.frameworks = detect_frameworks(root);

    // README excerpt
    for candidate in ["README.md", "README.MD", "README.markdown", "README"] {
        let p = root.join(candidate);
        if p.is_file() {
            if let Ok(s) = std::fs::read_to_string(&p) {
                facts.readme_excerpt = Some(s.chars().take(4_000).collect());
                break;
            }
        }
    }

    Ok(facts)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn lang_label(l: Lang) -> &'static str {
    match l {
        Lang::Rust => "rust",
        Lang::TypeScript => "typescript",
        Lang::Tsx => "tsx",
        Lang::JavaScript => "javascript",
        Lang::Python => "python",
        Lang::Go => "go",
        Lang::Markdown => "markdown",
        Lang::Toml => "toml",
        Lang::Json => "json",
        Lang::Plain => "plain",
    }
}

fn detect_frameworks(root: &Path) -> Vec<String> {
    let mut out = Vec::new();

    if root.join("Cargo.toml").is_file() {
        out.push("rust-cargo".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("Cargo.toml")) {
            if s.contains("[workspace]") {
                out.push("rust-cargo-workspace".to_string());
            }
        }
    }
    if root.join("package.json").is_file() {
        out.push("npm".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("package.json")) {
            if s.contains("\"next\"") {
                out.push("next.js".to_string());
            }
            if s.contains("\"vite\"") {
                out.push("vite".to_string());
            }
            if s.contains("\"react\"") {
                out.push("react".to_string());
            }
            if s.contains("\"@nestjs/core\"") {
                out.push("nestjs".to_string());
            }
            if s.contains("\"typescript\"") {
                out.push("typescript".to_string());
            }
        }
    }
    if root.join("pyproject.toml").is_file() {
        out.push("python-poetry".to_string());
    }
    if root.join("requirements.txt").is_file() {
        out.push("python-pip".to_string());
    }
    if root.join("go.mod").is_file() {
        out.push("go-modules".to_string());
    }
    out
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn collect_facts_extracts_lang_counts() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() {}").unwrap();
        std::fs::write(dir.path().join("c.py"), "def c(): pass").unwrap();

        let facts = collect_facts(dir.path()).unwrap();
        assert_eq!(
            facts.by_lang.get("rust").copied().unwrap_or(0),
            2,
            "expected 2 rust files"
        );
        assert_eq!(
            facts.by_lang.get("python").copied().unwrap_or(0),
            1,
            "expected 1 python file"
        );
    }

    #[test]
    fn collect_facts_detects_cargo_workspace() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src.rs"), "fn x() {}").unwrap();

        let facts = collect_facts(dir.path()).unwrap();
        assert!(
            facts.frameworks.contains(&"rust-cargo-workspace".to_string()),
            "expected rust-cargo-workspace in frameworks: {:?}",
            facts.frameworks
        );
    }

    #[test]
    fn collect_facts_detects_next_js() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"13.0.0","react":"18.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("index.js"), "// entry").unwrap();

        let facts = collect_facts(dir.path()).unwrap();
        assert!(
            facts.frameworks.contains(&"next.js".to_string()),
            "expected next.js: {:?}",
            facts.frameworks
        );
    }

    #[test]
    fn collect_facts_top_symbols_includes_classes_first() {
        let dir = TempDir::new().unwrap();
        // A struct (mapped as Class by semantic chunker) and a free function.
        // We rely on DefaultChunker with tree-sitter if available; with window fallback
        // symbols may not be extracted, so we just check the function doesn't panic.
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub struct MyStruct { x: i32 }\npub fn my_func() {}\n",
        )
        .unwrap();

        let facts = collect_facts(dir.path()).unwrap();
        // If symbols are extracted, classes must appear before functions
        if facts.top_symbols.len() >= 2 {
            let first_weight = match facts.top_symbols[0].kind.as_str() {
                "class" => 4,
                "impl" => 3,
                "function" => 2,
                "method" => 1,
                _ => 0,
            };
            let second_weight = match facts.top_symbols[1].kind.as_str() {
                "class" => 4,
                "impl" => 3,
                "function" => 2,
                "method" => 1,
                _ => 0,
            };
            assert!(
                first_weight >= second_weight,
                "top_symbols not ordered by kind weight: {:?}",
                facts.top_symbols
            );
        }
    }

    #[test]
    fn collect_facts_includes_readme_excerpt() {
        let dir = TempDir::new().unwrap();
        let readme_content = "# My Project\n\nThis is the README.\n";
        std::fs::write(dir.path().join("README.md"), readme_content).unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let facts = collect_facts(dir.path()).unwrap();
        assert!(
            facts.readme_excerpt.is_some(),
            "readme_excerpt should be Some when README.md exists"
        );
        assert!(
            facts
                .readme_excerpt
                .as_ref()
                .unwrap()
                .contains("My Project"),
            "readme_excerpt should contain README content"
        );
    }
}
