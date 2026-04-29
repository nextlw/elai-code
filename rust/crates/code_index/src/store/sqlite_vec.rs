//! `SqliteVecStore`: backend baseado em `SQLite` com KNN plano em Rust.
//!
//! Schema:
//!   - tabela regular `chunks`: metadata (`id`, `rel_path`, `line_start`, `line_end`,
//!     `lang`, `symbol`, `kind`, `content_hash`, `snippet`, `indexed_at`).
//!   - tabela regular `embeddings`: (`id` INTEGER PRIMARY KEY, `data` BLOB NOT NULL) —
//!     vetor `f32` serializado como little-endian bytes.
//!   - rowid alinhado entre as duas tabelas via `id` explícito.
//!
//! Abordagem de carregamento: **Rust-fallback** — embeddings persistidos como BLOBs
//! em tabela `SQLite` padrão; KNN calculado em Rust por similaridade de cosseno.
//! Aceitável até ~10 k chunks; escalável para `sqlite-vec` num upgrade futuro
//! sem mudar a interface pública (`VectorStore`).
//!
//! A crate `sqlite-vec` expõe apenas `sqlite3_vec_init` via `extern` FFI, o que exige
//! `unsafe`. Como o workspace proíbe `unsafe_code` em nível `forbid` (passado pela
//! linha de comando, não sobreposto por `#[allow]`), adoptamos esta abordagem pura-Rust
//! que é segura e correcta dentro das restrições do projecto.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::{Chunk, ChunkKind, Lang};

use super::{Filter, Hit, IndexPoint, StoreError, VectorStore};

// ─── Schema ───────────────────────────────────────────────────────────────────

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS chunks (
    id           INTEGER PRIMARY KEY,
    rel_path     TEXT    NOT NULL,
    line_start   INTEGER NOT NULL,
    line_end     INTEGER NOT NULL,
    lang         TEXT    NOT NULL,
    symbol       TEXT,
    kind         TEXT    NOT NULL,
    content_hash INTEGER NOT NULL,
    snippet      TEXT    NOT NULL,
    indexed_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chunks_rel_path ON chunks(rel_path);
CREATE TABLE IF NOT EXISTS embeddings (
    id   INTEGER PRIMARY KEY,
    data BLOB    NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

// ─── Store ────────────────────────────────────────────────────────────────────

pub struct SqliteVecStore {
    conn: Mutex<Connection>,
    dim: usize,
    path: PathBuf,
}

impl std::fmt::Debug for SqliteVecStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteVecStore")
            .field("dim", &self.dim)
            .field("path", &self.path)
            .field("conn", &"Mutex<Connection>")
            .finish()
    }
}

impl SqliteVecStore {
    /// Abre ou cria store em `path`. `dim` é validado contra dim já gravada (se existir).
    pub fn open(path: impl Into<PathBuf>, dim: usize) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(StoreError::Io)?;
            }
        }

        let conn =
            Connection::open(&path).map_err(|e| StoreError::Backend(e.to_string()))?;

        conn.execute_batch(SCHEMA)
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        // Valida / registra dim
        let stored_dim = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'dim'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        if let Some(stored) = stored_dim {
            let stored_dim: usize = stored.parse().unwrap_or(0);
            if stored_dim != dim {
                return Err(StoreError::DimensionMismatch {
                    stored: stored_dim,
                    requested: dim,
                });
            }
        } else {
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('dim', ?1)",
                params![dim.to_string()],
            )
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
            dim,
            path,
        })
    }

    /// Caminho do arquivo de banco de dados.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// ─── Helpers internos ─────────────────────────────────────────────────────────

fn embedding_to_blob(emb: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(emb.len() * 4);
    for &f in emb {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn lang_to_str(l: Lang) -> &'static str {
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
        Lang::Pdf => "pdf",
    }
}

fn lang_from_str(s: &str) -> Lang {
    match s {
        "rust" => Lang::Rust,
        "typescript" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "javascript" => Lang::JavaScript,
        "python" => Lang::Python,
        "go" => Lang::Go,
        "markdown" => Lang::Markdown,
        "toml" => Lang::Toml,
        "json" => Lang::Json,
        "pdf" => Lang::Pdf,
        _ => Lang::Plain,
    }
}

fn kind_to_str(k: ChunkKind) -> &'static str {
    match k {
        ChunkKind::Function => "function",
        ChunkKind::Method => "method",
        ChunkKind::Class => "class",
        ChunkKind::Impl => "impl",
        ChunkKind::Module => "module",
        ChunkKind::Window => "window",
        ChunkKind::Plain => "plain",
        ChunkKind::PdfPage => "pdf_page",
        ChunkKind::PdfImage => "pdf_image",
    }
}

fn kind_from_str(s: &str) -> ChunkKind {
    match s {
        "function" => ChunkKind::Function,
        "method" => ChunkKind::Method,
        "class" => ChunkKind::Class,
        "impl" => ChunkKind::Impl,
        "module" => ChunkKind::Module,
        "window" => ChunkKind::Window,
        "pdf_page" => ChunkKind::PdfPage,
        "pdf_image" => ChunkKind::PdfImage,
        _ => ChunkKind::Plain,
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| {
            // Seconds since epoch fit in i64 for centuries; cast is intentional.
            #[allow(clippy::cast_possible_wrap)]
            let secs = d.as_secs() as i64;
            secs
        })
}

// ─── VectorStore impl ─────────────────────────────────────────────────────────

impl VectorStore for SqliteVecStore {
    fn dim(&self) -> usize {
        self.dim
    }

    fn upsert(&self, points: Vec<IndexPoint>) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        for p in points {
            if p.embedding.len() != self.dim {
                return Err(StoreError::DimensionMismatch {
                    stored: self.dim,
                    requested: p.embedding.len(),
                });
            }
            // u64 ids and content_hash stored as i64 (SQLite has no unsigned integer type).
            // The bit pattern is preserved; reads back with cast_unsigned().
            #[allow(clippy::cast_possible_wrap)]
            let id_i64 = p.id as i64;
            #[allow(clippy::cast_possible_wrap)]
            let hash_i64 = p.chunk.content_hash as i64;

            tx.execute(
                "INSERT OR REPLACE INTO chunks \
                 (id, rel_path, line_start, line_end, lang, symbol, kind, content_hash, snippet, indexed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id_i64,
                    p.chunk.rel_path,
                    p.chunk.line_start,
                    p.chunk.line_end,
                    lang_to_str(p.chunk.lang),
                    p.chunk.symbol,
                    kind_to_str(p.chunk.kind),
                    hash_i64,
                    p.chunk.snippet,
                    now_unix(),
                ],
            )
            .map_err(|e| StoreError::Backend(e.to_string()))?;

            let blob = embedding_to_blob(&p.embedding);
            tx.execute(
                "INSERT OR REPLACE INTO embeddings (id, data) VALUES (?1, ?2)",
                params![id_i64, blob],
            )
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        tx.commit().map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }

    fn delete_by_path(&self, rel_path: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id FROM chunks WHERE rel_path = ?1")
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        let ids: Vec<i64> = stmt
            .query_map(params![rel_path], |r| r.get(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?
            .filter_map(Result::ok)
            .collect();
        for id in ids {
            conn.execute("DELETE FROM embeddings WHERE id = ?1", params![id])
                .map_err(|e| StoreError::Backend(e.to_string()))?;
            conn.execute("DELETE FROM chunks WHERE id = ?1", params![id])
                .map_err(|e| StoreError::Backend(e.to_string()))?;
        }
        Ok(())
    }

    fn query(
        &self,
        vec: &[f32],
        k: usize,
        filter: Option<Filter>,
    ) -> Result<Vec<Hit>, StoreError> {
        if vec.len() != self.dim {
            return Err(StoreError::DimensionMismatch {
                stored: self.dim,
                requested: vec.len(),
            });
        }
        let conn = self.conn.lock().unwrap();

        // Carrega todos os registros e aplica filtro + KNN em Rust.
        let sql = "
            SELECT c.id, c.rel_path, c.line_start, c.line_end,
                   c.lang, c.symbol, c.kind, c.content_hash, c.snippet,
                   e.data
            FROM chunks c
            JOIN embeddings e ON e.id = c.id
        ";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let rel_path: String = row.get(1)?;
                let line_start: u32 = row.get(2)?;
                let line_end: u32 = row.get(3)?;
                let lang: String = row.get(4)?;
                let symbol: Option<String> = row.get(5)?;
                let kind: String = row.get(6)?;
                let content_hash: i64 = row.get(7)?;
                let snippet: String = row.get(8)?;
                let blob: Vec<u8> = row.get(9)?;
                // Bit-pattern round-trip: written as i64, read back as u64.
                #[allow(clippy::cast_sign_loss)]
                let content_hash_u64 = content_hash as u64;
                Ok((
                    Chunk {
                        rel_path,
                        line_start,
                        line_end,
                        lang: lang_from_str(&lang),
                        symbol,
                        kind: kind_from_str(&kind),
                        content_hash: content_hash_u64,
                        snippet,
                    },
                    blob,
                ))
            })
            .map_err(|e| StoreError::Backend(e.to_string()))?;

        let mut hits: Vec<Hit> = Vec::new();
        for row in rows {
            let (chunk, blob) = row.map_err(|e| StoreError::Backend(e.to_string()))?;

            // Aplica filtro.
            if let Some(ref f) = filter {
                let path_ok = f
                    .rel_path_prefix
                    .as_ref()
                    .is_none_or(|p| chunk.rel_path.starts_with(p.as_str()));
                let lang_ok = f.langs.is_empty() || f.langs.contains(&chunk.lang);
                let kind_ok = f.kinds.is_empty() || f.kinds.contains(&chunk.kind);
                if !(path_ok && lang_ok && kind_ok) {
                    continue;
                }
            }

            let emb = blob_to_embedding(&blob);
            let score = cosine(vec, &emb);
            hits.push(Hit { chunk, score });
        }

        // Ordena por score decrescente e trunca em k.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        Ok(hits)
    }

    fn count(&self) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        // COUNT(*) is always non-negative; truncation only possible on 16-bit targets.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let count = n as usize;
        Ok(count)
    }

    fn clear(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM embeddings", [])
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        conn.execute("DELETE FROM chunks", [])
            .map_err(|e| StoreError::Backend(e.to_string()))?;
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chunk, ChunkKind, Lang};
    use tempfile::TempDir;

    fn make_chunk(rel_path: &str, lang: Lang, kind: ChunkKind) -> Chunk {
        Chunk {
            rel_path: rel_path.to_string(),
            line_start: 1,
            line_end: 10,
            lang,
            symbol: Some(rel_path.to_string()),
            kind,
            content_hash: 0,
            snippet: String::new(),
        }
    }

    /// Vetor unitário com `hot_idx` ativo.
    fn unit_vec(dim: usize, hot_idx: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[hot_idx % dim] = 1.0;
        v
    }

    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn open_creates_schema_and_persists_dim() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");

        // Primeira abertura deve funcionar.
        {
            let store = SqliteVecStore::open(&db_path, 4).unwrap();
            assert_eq!(store.dim(), 4);
        }

        // Reabrir com mesmo dim deve funcionar.
        {
            let store = SqliteVecStore::open(&db_path, 4).unwrap();
            assert_eq!(store.dim(), 4);
        }

        // Reabrir com dim diferente deve falhar com DimensionMismatch.
        let err = SqliteVecStore::open(&db_path, 8).unwrap_err();
        assert!(
            matches!(err, StoreError::DimensionMismatch { stored: 4, requested: 8 }),
            "expected DimensionMismatch, got {err:?}"
        );
    }

    #[test]
    fn upsert_then_query_returns_top_k_ordered() {
        let dir = TempDir::new().unwrap();
        let store = SqliteVecStore::open(dir.path().join("q.db"), 4).unwrap();

        let pa = IndexPoint {
            id: 1,
            embedding: unit_vec(4, 0), // [1,0,0,0]
            chunk: make_chunk("a.rs", Lang::Rust, ChunkKind::Function),
        };
        let pb = IndexPoint {
            id: 2,
            embedding: unit_vec(4, 1), // [0,1,0,0]
            chunk: make_chunk("b.rs", Lang::Rust, ChunkKind::Function),
        };
        let pc = IndexPoint {
            id: 3,
            embedding: unit_vec(4, 2), // [0,0,1,0]
            chunk: make_chunk("c.rs", Lang::Rust, ChunkKind::Function),
        };
        store.upsert(vec![pa, pb, pc]).unwrap();

        // Query com vetor próximo de pb -> b.rs deve vir primeiro.
        let hits = store.query(&unit_vec(4, 1), 3, None).unwrap();
        assert!(!hits.is_empty(), "expected hits");
        assert_eq!(hits[0].chunk.rel_path, "b.rs", "b.rs should be top result");

        // Scores devem ser decrescentes.
        for w in hits.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "scores must be descending: {} >= {}",
                w[0].score,
                w[1].score
            );
        }
    }

    #[test]
    fn upsert_with_wrong_dim_errors() {
        let dir = TempDir::new().unwrap();
        let store = SqliteVecStore::open(dir.path().join("d.db"), 4).unwrap();

        let bad = IndexPoint {
            id: 1,
            embedding: vec![1.0, 2.0], // dim=2
            chunk: make_chunk("x.rs", Lang::Rust, ChunkKind::Plain),
        };
        assert!(matches!(
            store.upsert(vec![bad]),
            Err(StoreError::DimensionMismatch { stored: 4, requested: 2 })
        ));
    }

    #[test]
    fn delete_by_path_removes_all_chunks_for_that_path() {
        let dir = TempDir::new().unwrap();
        let store = SqliteVecStore::open(dir.path().join("del.db"), 4).unwrap();

        store
            .upsert(vec![
                IndexPoint {
                    id: 1,
                    embedding: unit_vec(4, 0),
                    chunk: make_chunk("keep.rs", Lang::Rust, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 2,
                    embedding: unit_vec(4, 1),
                    chunk: make_chunk("remove.rs", Lang::Rust, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 3,
                    embedding: unit_vec(4, 2),
                    chunk: make_chunk("remove.rs", Lang::Rust, ChunkKind::Plain),
                },
            ])
            .unwrap();

        assert_eq!(store.count().unwrap(), 3);
        store.delete_by_path("remove.rs").unwrap();
        assert_eq!(store.count().unwrap(), 1);

        let hits = store.query(&unit_vec(4, 0), 10, None).unwrap();
        assert!(
            hits.iter().all(|h| h.chunk.rel_path != "remove.rs"),
            "remove.rs should be gone"
        );
    }

    #[test]
    fn count_reflects_inserts_and_deletes() {
        let dir = TempDir::new().unwrap();
        let store = SqliteVecStore::open(dir.path().join("cnt.db"), 4).unwrap();

        assert_eq!(store.count().unwrap(), 0);

        store
            .upsert(vec![
                IndexPoint {
                    id: 1,
                    embedding: unit_vec(4, 0),
                    chunk: make_chunk("a.rs", Lang::Rust, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 2,
                    embedding: unit_vec(4, 1),
                    chunk: make_chunk("b.rs", Lang::Rust, ChunkKind::Plain),
                },
            ])
            .unwrap();
        assert_eq!(store.count().unwrap(), 2);

        store.delete_by_path("a.rs").unwrap();
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn clear_empties_store() {
        let dir = TempDir::new().unwrap();
        let store = SqliteVecStore::open(dir.path().join("clr.db"), 4).unwrap();

        store
            .upsert(vec![
                IndexPoint {
                    id: 1,
                    embedding: unit_vec(4, 0),
                    chunk: make_chunk("a.rs", Lang::Rust, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 2,
                    embedding: unit_vec(4, 1),
                    chunk: make_chunk("b.rs", Lang::Rust, ChunkKind::Plain),
                },
            ])
            .unwrap();

        assert_eq!(store.count().unwrap(), 2);
        store.clear().unwrap();
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn query_with_lang_filter_excludes_other_langs() {
        let dir = TempDir::new().unwrap();
        let store = SqliteVecStore::open(dir.path().join("lf.db"), 4).unwrap();

        store
            .upsert(vec![
                IndexPoint {
                    id: 1,
                    embedding: unit_vec(4, 0),
                    chunk: make_chunk("a.rs", Lang::Rust, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 2,
                    embedding: unit_vec(4, 1),
                    chunk: make_chunk("b.ts", Lang::TypeScript, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 3,
                    embedding: unit_vec(4, 2),
                    chunk: make_chunk("c.py", Lang::Python, ChunkKind::Plain),
                },
            ])
            .unwrap();

        let filter = Filter {
            langs: vec![Lang::Rust],
            ..Default::default()
        };
        let hits = store.query(&unit_vec(4, 0), 10, Some(filter)).unwrap();
        assert_eq!(hits.len(), 1, "only Rust hits expected");
        assert_eq!(hits[0].chunk.rel_path, "a.rs");
    }

    #[test]
    fn query_with_path_prefix_filter_works() {
        let dir = TempDir::new().unwrap();
        let store = SqliteVecStore::open(dir.path().join("pf.db"), 4).unwrap();

        store
            .upsert(vec![
                IndexPoint {
                    id: 1,
                    embedding: unit_vec(4, 0),
                    chunk: make_chunk("src/a.rs", Lang::Rust, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 2,
                    embedding: unit_vec(4, 1),
                    chunk: make_chunk("tests/b.rs", Lang::Rust, ChunkKind::Plain),
                },
                IndexPoint {
                    id: 3,
                    embedding: unit_vec(4, 2),
                    chunk: make_chunk("src/c.rs", Lang::Rust, ChunkKind::Plain),
                },
            ])
            .unwrap();

        let filter = Filter {
            rel_path_prefix: Some("src/".to_string()),
            ..Default::default()
        };
        let hits = store.query(&unit_vec(4, 0), 10, Some(filter)).unwrap();
        assert_eq!(hits.len(), 2, "only src/ hits expected");
        assert!(hits.iter().all(|h| h.chunk.rel_path.starts_with("src/")));
    }
}
