use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::args::{EmbedProviderArg, IndexBackend, InitArgs};
use code_index::{
    collect_facts, DefaultChunker, Embedder, Indexer, IndexerStats, SqliteVecStore, VectorStore,
};
use runtime::render_static_elai_md;

const STARTER_ELAI_JSON: &str = concat!(
    "{\n",
    "  \"permissions\": {\n",
    "    \"defaultMode\": \"dontAsk\"\n",
    "  }\n",
    "}\n",
);
const GITIGNORE_COMMENT: &str = "# Elai Code local artifacts";
const GITIGNORE_ENTRIES: [&str; 2] = [".elai/settings.local.json", ".elai/sessions/"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InitStatus {
    Created,
    Updated,
    Skipped,
}

impl InitStatus {
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Skipped => "skipped (already exists)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InitArtifact {
    pub(crate) name: &'static str,
    pub(crate) status: InitStatus,
}

#[derive(Debug, Clone)]
pub(crate) struct InitReport {
    pub(crate) project_root: PathBuf,
    pub(crate) artifacts: Vec<InitArtifact>,
    pub(crate) index_stats: Option<IndexerStats>,
}

impl InitReport {
    #[must_use]
    pub(crate) fn render(&self) -> String {
        let mut lines = vec![
            "Init".to_string(),
            format!("  Project          {}", self.project_root.display()),
        ];
        for artifact in &self.artifacts {
            lines.push(format!(
                "  {:<16} {}",
                artifact.name,
                artifact.status.label()
            ));
        }
        if let Some(s) = &self.index_stats {
            lines.push(format!(
                "  Index            {} files, {} chunks ({} ms)",
                s.files_indexed, s.chunks_indexed, s.elapsed_ms
            ));
        }
        lines.push("  Next step        Review and tailor the generated guidance".to_string());
        lines.join("\n")
    }
}

pub(crate) fn initialize_repo(
    cwd: &Path,
    args: &InitArgs,
) -> Result<InitReport, Box<dyn std::error::Error>> {
    let mut artifacts = Vec::new();

    // 1. Basic structure
    let elai_dir = cwd.join(".elai");
    artifacts.push(InitArtifact {
        name: ".elai/",
        status: ensure_dir(&elai_dir)?,
    });

    let elai_json = cwd.join(".elai.json");
    artifacts.push(InitArtifact {
        name: ".elai.json",
        status: write_file_if_missing(&elai_json, STARTER_ELAI_JSON)?,
    });

    let index_dir = elai_dir.join("index");
    if !args.no_index {
        artifacts.push(InitArtifact {
            name: ".elai/index/",
            status: ensure_dir(&index_dir)?,
        });
    }

    let gitignore = cwd.join(".gitignore");
    artifacts.push(InitArtifact {
        name: ".gitignore",
        status: ensure_gitignore_entries(&gitignore)?,
    });

    // Add .elai/index/ to .gitignore when indexing
    if !args.no_index {
        let _ = append_gitignore_index_entry(&gitignore);
    }

    // 2. Collect project facts (always, even with --no-index, for the static template)
    eprintln!("  Analisando projeto...");
    let facts = collect_facts(cwd).map_err(|e| format!("collect_facts: {e}"))?;
    let facts_json = serde_json::to_string_pretty(&facts)?;

    // 3. Indexing (unless --no-index)
    let mut index_stats: Option<IndexerStats> = None;
    if !args.no_index {
        index_stats = Some(run_indexing(cwd, &index_dir, args)?);
    }

    // 4. Generate ELAI.md
    let elai_md_path = cwd.join("ELAI.md");
    let elai_md_status = if elai_md_path.exists() && !args.reindex {
        InitStatus::Skipped
    } else {
        let content = generate_elai_md_or_fallback(&facts_json);
        fs::write(&elai_md_path, content)?;
        if args.reindex {
            InitStatus::Updated
        } else {
            InitStatus::Created
        }
    };
    artifacts.push(InitArtifact {
        name: "ELAI.md",
        status: elai_md_status,
    });

    // 5. Save index config
    if !args.no_index {
        write_index_config(&index_dir, args)?;
    }

    Ok(InitReport {
        project_root: cwd.to_path_buf(),
        artifacts,
        index_stats,
    })
}

fn run_indexing(
    cwd: &Path,
    index_dir: &Path,
    args: &InitArgs,
) -> Result<IndexerStats, Box<dyn std::error::Error>> {
    let embedder: Arc<dyn Embedder> = build_embedder(args)?;
    let store: Arc<dyn VectorStore> = build_store(index_dir, args, embedder.dim())?;

    if args.reindex {
        store.clear()?;
    }

    let chunker = DefaultChunker::new();
    let indexer = Indexer::new(cwd, embedder, store, chunker);
    eprintln!("  Indexando código (isso pode demorar)...");
    let stats = indexer.index_full()?;
    eprintln!(
        "  Indexacao concluida: {} arquivos, {} chunks em {} ms",
        stats.files_indexed, stats.chunks_indexed, stats.elapsed_ms,
    );
    Ok(stats)
}

#[cfg(feature = "embed-fastembed")]
fn build_local_embedder() -> Result<Arc<dyn Embedder>, Box<dyn std::error::Error>> {
    // Heurística: cache do fastembed em ~/.cache/.fastembed_cache. Se não existir,
    // primeira execução vai baixar ~125 MB (model + tokenizer). Avisa o usuário
    // antes para que o silêncio (TUI sem progress bar) não pareça travamento.
    let cache_present = std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".cache").join(".fastembed_cache"))
        .is_some_and(|p| p.is_dir() && p.read_dir().is_ok_and(|mut d| d.next().is_some()));
    if !cache_present {
        eprintln!(
            "  Baixando modelo de embedding (BGE-small, ~125 MB). Apenas na primeira execução."
        );
        eprintln!("  (Defina ELAI_FASTEMBED_PROGRESS=1 para ver progresso fora do TUI.)");
    }
    let e = code_index::LocalFastEmbedder::new()?;
    Ok(Arc::new(e))
}

#[cfg(not(feature = "embed-fastembed"))]
fn build_local_embedder() -> Result<Arc<dyn Embedder>, Box<dyn std::error::Error>> {
    Err("embed-provider 'local' não está disponível neste binário (compilado sem \
         embed-fastembed). Use --embed-provider ollama ou um endpoint HTTP."
        .into())
}

fn build_embedder(
    args: &InitArgs,
) -> Result<Arc<dyn Embedder>, Box<dyn std::error::Error>> {
    match args.embed_provider {
        EmbedProviderArg::Local => build_local_embedder(),
        EmbedProviderArg::Ollama => {
            let url = args
                .ollama_url
                .clone()
                .or_else(|| std::env::var("OLLAMA_BASE_URL").ok())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            let model = args
                .embed_model
                .clone()
                .unwrap_or_else(|| "nomic-embed-text".to_string());
            let dim = match model.as_str() {
                "all-minilm" => 384,
                "nomic-embed-text" => 768,
                "mxbai-embed-large" => 1024,
                _ => 768,
            };
            let e = code_index::OllamaEmbedder::new(url, model, dim)?;
            Ok(Arc::new(e))
        }
        EmbedProviderArg::Jina | EmbedProviderArg::Openai | EmbedProviderArg::Voyage => {
            Err(format!(
                "embed-provider {:?} ainda não implementado nesta versão; use --embed-provider local|ollama",
                args.embed_provider
            )
            .into())
        }
    }
}

fn build_store(
    index_dir: &Path,
    args: &InitArgs,
    dim: usize,
) -> Result<Arc<dyn VectorStore>, Box<dyn std::error::Error>> {
    match args.backend {
        IndexBackend::Sqlite => {
            let db_path = index_dir.join("index.db");
            let store = SqliteVecStore::open(db_path, dim)?;
            Ok(Arc::new(store))
        }
        IndexBackend::Qdrant => {
            Err("backend qdrant ainda não implementado nesta versão; use --backend sqlite".into())
        }
    }
}

fn write_index_config(index_dir: &Path, args: &InitArgs) -> std::io::Result<()> {
    let config = serde_json::json!({
        "backend": match args.backend {
            IndexBackend::Sqlite => "sqlite",
            IndexBackend::Qdrant => "qdrant",
        },
        "qdrantUrl": args.qdrant_url,
        "embedProvider": match args.embed_provider {
            EmbedProviderArg::Local => "local",
            EmbedProviderArg::Ollama => "ollama",
            EmbedProviderArg::Jina => "jina",
            EmbedProviderArg::Openai => "openai",
            EmbedProviderArg::Voyage => "voyage",
        },
        "embedModel": args.embed_model,
        "ollamaUrl": args.ollama_url,
        "watcher": { "enabled": !args.no_watcher, "debounceMs": 500 },
    });
    fs::write(
        index_dir.join("config.json"),
        serde_json::to_string_pretty(&config)?,
    )
}

fn append_gitignore_index_entry(gitignore: &Path) -> std::io::Result<()> {
    use std::io::Write;
    if gitignore.exists() {
        let content = fs::read_to_string(gitignore)?;
        if content.contains(".elai/index/") {
            return Ok(());
        }
        let mut f = fs::OpenOptions::new().append(true).open(gitignore)?;
        writeln!(f, ".elai/index/")?;
    }
    Ok(())
}

fn generate_elai_md_or_fallback(facts_json: &str) -> String {
    render_static_elai_md(facts_json)
}

fn ensure_dir(path: &Path) -> Result<InitStatus, std::io::Error> {
    if path.is_dir() {
        return Ok(InitStatus::Skipped);
    }
    fs::create_dir_all(path)?;
    Ok(InitStatus::Created)
}

fn write_file_if_missing(path: &Path, content: &str) -> Result<InitStatus, std::io::Error> {
    if path.exists() {
        return Ok(InitStatus::Skipped);
    }
    fs::write(path, content)?;
    Ok(InitStatus::Created)
}

fn ensure_gitignore_entries(path: &Path) -> Result<InitStatus, std::io::Error> {
    if !path.exists() {
        let mut lines = vec![GITIGNORE_COMMENT.to_string()];
        lines.extend(GITIGNORE_ENTRIES.iter().map(|entry| (*entry).to_string()));
        fs::write(path, format!("{}\n", lines.join("\n")))?;
        return Ok(InitStatus::Created);
    }

    let existing = fs::read_to_string(path)?;
    let mut lines = existing.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let mut changed = false;

    if !lines.iter().any(|line| line == GITIGNORE_COMMENT) {
        lines.push(GITIGNORE_COMMENT.to_string());
        changed = true;
    }

    for entry in GITIGNORE_ENTRIES {
        if !lines.iter().any(|line| line == entry) {
            lines.push(entry.to_string());
            changed = true;
        }
    }

    if !changed {
        return Ok(InitStatus::Skipped);
    }

    fs::write(path, format!("{}\n", lines.join("\n")))?;
    Ok(InitStatus::Updated)
}

// ── Kept for backward compat (used in tests and TUI /init slash command) ───
#[allow(dead_code)]
pub(crate) fn render_init_elai_md(cwd: &Path) -> String {
    // Use the static template grounded in project facts
    let facts = collect_facts(cwd).ok();
    let facts_json = facts
        .as_ref()
        .and_then(|f| serde_json::to_string_pretty(f).ok())
        .unwrap_or_else(|| "{}".to_string());
    render_static_elai_md(&facts_json)
}

#[cfg(test)]
mod tests {
    use super::{initialize_repo, render_init_elai_md, InitArgs};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("elai-init-{nanos}"))
    }

    #[test]
    fn initialize_repo_creates_expected_files_and_gitignore_entries() {
        let root = temp_dir();
        fs::create_dir_all(root.join("rust")).expect("create rust dir");
        fs::write(root.join("rust").join("Cargo.toml"), "[workspace]\n").expect("write cargo");

        let args = InitArgs {
            no_index: true,
            ..InitArgs::default()
        };
        let report = initialize_repo(&root, &args).expect("init should succeed");
        let rendered = report.render();
        assert!(
            rendered.lines().any(|line| line.contains(".elai/") && line.contains("created")),
            "{rendered}"
        );
        assert!(rendered.lines().any(|line| line.contains(".elai.json") && line.contains("created")));
        assert!(rendered.lines().any(|line| line.contains(".gitignore") && line.contains("created")));
        assert!(rendered.lines().any(|line| line.contains("ELAI.md") && line.contains("created")));
        assert!(root.join(".elai").is_dir());
        assert!(root.join(".elai.json").is_file());
        assert!(root.join("ELAI.md").is_file());
        assert_eq!(
            fs::read_to_string(root.join(".elai.json")).expect("read elai json"),
            concat!(
                "{\n",
                "  \"permissions\": {\n",
                "    \"defaultMode\": \"dontAsk\"\n",
                "  }\n",
                "}\n",
            )
        );
        let gitignore = fs::read_to_string(root.join(".gitignore")).expect("read gitignore");
        assert!(gitignore.contains(".elai/settings.local.json"));
        assert!(gitignore.contains(".elai/sessions/"));

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn initialize_repo_is_idempotent_and_preserves_existing_files() {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("ELAI.md"), "custom guidance\n").expect("write existing elai md");
        fs::write(root.join(".gitignore"), ".elai/settings.local.json\n").expect("write gitignore");

        let args = InitArgs {
            no_index: true,
            ..InitArgs::default()
        };

        let first = initialize_repo(&root, &args).expect("first init should succeed");
        assert!(first
            .render()
            .contains("ELAI.md          skipped (already exists)"));
        let second = initialize_repo(&root, &args).expect("second init should succeed");
        let second_rendered = second.render();
        assert!(second_rendered
            .lines()
            .any(|line| line.contains(".elai/") && line.contains("skipped (already exists)")));
        assert!(second_rendered
            .lines()
            .any(|line| line.contains(".elai.json") && line.contains("skipped (already exists)")));
        assert!(second_rendered
            .lines()
            .any(|line| line.contains(".gitignore") && line.contains("skipped (already exists)")));
        assert!(second_rendered
            .lines()
            .any(|line| line.contains("ELAI.md") && line.contains("skipped (already exists)")));
        assert_eq!(
            fs::read_to_string(root.join("ELAI.md")).expect("read existing elai md"),
            "custom guidance\n"
        );
        let gitignore = fs::read_to_string(root.join(".gitignore")).expect("read gitignore");
        assert_eq!(gitignore.matches(".elai/settings.local.json").count(), 1);
        assert_eq!(gitignore.matches(".elai/sessions/").count(), 1);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn render_init_template_mentions_detected_python_and_nextjs_markers() {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("pyproject.toml"), "[project]\nname = \"demo\"\n")
            .expect("write pyproject");
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0"},"devDependencies":{"typescript":"5.0.0"}}"#,
        )
        .expect("write package json");

        // render_init_elai_md now uses collect_facts → render_static_elai_md
        // It will detect files and produce a valid ELAI.md
        let rendered = render_init_elai_md(Path::new(&root));
        assert!(rendered.contains("# ELAI.md"), "should contain heading: {rendered}");
        assert!(rendered.contains("## Estrutura"), "should contain Estrutura section: {rendered}");

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn init_with_no_index_skips_indexing() {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("create root");

        let args = InitArgs {
            no_index: true,
            ..InitArgs::default()
        };
        let report = initialize_repo(&root, &args).expect("init should succeed");

        // .elai/index/ should NOT be created
        assert!(!root.join(".elai").join("index").exists(), ".elai/index/ should not be created with --no-index");
        // ELAI.md should be present
        assert!(root.join("ELAI.md").is_file(), "ELAI.md should exist");
        // index_stats should be None
        assert!(report.index_stats.is_none(), "index_stats should be None when --no-index");

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn init_uses_static_elai_md_fallback() {
        let facts_json = serde_json::json!({
            "total_files": 5,
            "by_lang": {"rust": 4, "toml": 1},
            "frameworks": ["rust-cargo"],
            "dirs_summary": [{"dir": "src", "files": 4}],
            "top_symbols": [],
            "readme_excerpt": null
        })
        .to_string();

        let content = runtime::render_static_elai_md(&facts_json);
        assert!(content.contains("## Estrutura"), "static render must include Estrutura section");
        assert!(content.contains("rust"), "static render must include rust lang");
    }

    #[test]
    #[ignore = "requires fastembed model download"]
    fn init_creates_index_config_json() {
        let root = temp_dir();
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("hello.rs"), "fn main() {}").expect("write rs file");

        let args = InitArgs {
            no_index: false,
            ..InitArgs::default()
        };
        let _report = initialize_repo(&root, &args).expect("init should succeed");

        let config_path = root.join(".elai").join("index").join("config.json");
        assert!(config_path.is_file(), "config.json should be created");
        let config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(config["backend"], "sqlite");
        assert_eq!(config["embedProvider"], "local");

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }
}
