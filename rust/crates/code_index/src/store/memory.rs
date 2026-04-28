use std::sync::Mutex;

use super::{Filter, Hit, IndexPoint, StoreError, VectorStore};

/// In-memory vector store for tests and offline use.
pub struct MemoryStore {
    inner: Mutex<Vec<IndexPoint>>,
    dim: usize,
}

impl MemoryStore {
    #[must_use]
    pub fn new(dim: usize) -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
            dim,
        }
    }
}

impl VectorStore for MemoryStore {
    fn upsert(&self, points: Vec<IndexPoint>) -> Result<(), StoreError> {
        let mut guard = self.inner.lock().unwrap();
        for new_point in points {
            if new_point.embedding.len() != self.dim {
                return Err(StoreError::DimensionMismatch {
                    stored: self.dim,
                    requested: new_point.embedding.len(),
                });
            }
            if let Some(existing) = guard.iter_mut().find(|p| p.id == new_point.id) {
                *existing = new_point;
            } else {
                guard.push(new_point);
            }
        }
        Ok(())
    }

    fn delete_by_path(&self, rel_path: &str) -> Result<(), StoreError> {
        let mut guard = self.inner.lock().unwrap();
        guard.retain(|p| p.chunk.rel_path != rel_path);
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
        let guard = self.inner.lock().unwrap();
        let mut hits: Vec<Hit> = guard
            .iter()
            .filter(|p| match &filter {
                None => true,
                Some(f) => {
                    let path_ok = f
                        .rel_path_prefix
                        .as_ref()
                        .is_none_or(|pre| p.chunk.rel_path.starts_with(pre.as_str()));
                    let lang_ok = f.langs.is_empty() || f.langs.contains(&p.chunk.lang);
                    let kind_ok = f.kinds.is_empty() || f.kinds.contains(&p.chunk.kind);
                    path_ok && lang_ok && kind_ok
                }
            })
            .map(|p| Hit {
                chunk: p.chunk.clone(),
                score: cosine(vec, &p.embedding),
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        Ok(hits)
    }

    fn count(&self) -> Result<usize, StoreError> {
        Ok(self.inner.lock().unwrap().len())
    }

    fn clear(&self) -> Result<(), StoreError> {
        self.inner.lock().unwrap().clear();
        Ok(())
    }

    fn dim(&self) -> usize {
        self.dim
    }
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chunk, ChunkKind, Lang};

    fn make_chunk(rel_path: &str, lang: Lang) -> Chunk {
        Chunk {
            rel_path: rel_path.to_string(),
            line_start: 1,
            line_end: 10,
            lang,
            symbol: None,
            kind: ChunkKind::Plain,
            content_hash: 0,
            snippet: String::new(),
        }
    }

    fn unit_vec(dim: usize, hot_idx: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[hot_idx % dim] = 1.0;
        v
    }

    #[test]
    fn memory_store_upsert_query_round_trip() {
        let store = MemoryStore::new(4);

        let p1 = IndexPoint {
            id: 1,
            embedding: unit_vec(4, 0),
            chunk: make_chunk("a.rs", Lang::Rust),
        };
        let p2 = IndexPoint {
            id: 2,
            embedding: unit_vec(4, 1),
            chunk: make_chunk("b.rs", Lang::Rust),
        };
        let p3 = IndexPoint {
            id: 3,
            embedding: unit_vec(4, 2),
            chunk: make_chunk("c.rs", Lang::Rust),
        };

        store.upsert(vec![p1, p2, p3]).unwrap();
        assert_eq!(store.count().unwrap(), 3);

        // Query with vector closest to p1
        let hits = store.query(&unit_vec(4, 0), 3, None).unwrap();
        assert_eq!(hits.len(), 3);
        // Best match should be p1 (score ~1.0)
        assert_eq!(hits[0].chunk.rel_path, "a.rs");
        // Scores should be descending
        assert!(hits[0].score >= hits[1].score);
        assert!(hits[1].score >= hits[2].score);
    }

    #[test]
    fn memory_store_dim_mismatch_errors() {
        let store = MemoryStore::new(4);

        let bad = IndexPoint {
            id: 99,
            embedding: vec![1.0, 2.0], // dim=2, store expects 4
            chunk: make_chunk("x.rs", Lang::Rust),
        };
        assert!(matches!(
            store.upsert(vec![bad]),
            Err(StoreError::DimensionMismatch { stored: 4, requested: 2 })
        ));

        assert!(matches!(
            store.query(&[1.0, 2.0], 5, None),
            Err(StoreError::DimensionMismatch { stored: 4, requested: 2 })
        ));
    }

    #[test]
    fn memory_store_delete_by_path() {
        let store = MemoryStore::new(4);

        store
            .upsert(vec![
                IndexPoint {
                    id: 1,
                    embedding: unit_vec(4, 0),
                    chunk: make_chunk("keep.rs", Lang::Rust),
                },
                IndexPoint {
                    id: 2,
                    embedding: unit_vec(4, 1),
                    chunk: make_chunk("remove.rs", Lang::Rust),
                },
            ])
            .unwrap();

        store.delete_by_path("remove.rs").unwrap();
        assert_eq!(store.count().unwrap(), 1);

        let hits = store.query(&unit_vec(4, 0), 10, None).unwrap();
        assert!(hits.iter().all(|h| h.chunk.rel_path != "remove.rs"));
    }

    #[test]
    fn memory_store_filter_by_lang() {
        let store = MemoryStore::new(4);

        store
            .upsert(vec![
                IndexPoint {
                    id: 1,
                    embedding: unit_vec(4, 0),
                    chunk: make_chunk("a.rs", Lang::Rust),
                },
                IndexPoint {
                    id: 2,
                    embedding: unit_vec(4, 1),
                    chunk: make_chunk("b.ts", Lang::TypeScript),
                },
                IndexPoint {
                    id: 3,
                    embedding: unit_vec(4, 2),
                    chunk: make_chunk("c.py", Lang::Python),
                },
            ])
            .unwrap();

        let filter = Filter {
            langs: vec![Lang::Rust],
            ..Default::default()
        };
        let hits = store.query(&unit_vec(4, 0), 10, Some(filter)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk.rel_path, "a.rs");
    }
}
