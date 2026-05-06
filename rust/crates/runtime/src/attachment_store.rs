//! Persistência de anexos binários (imagens / PDFs) em sidecar files
//! dentro do diretório da sessão.
//!
//! Conteúdo é content-addressable por SHA-256 — pastes idênticos dedupe
//! naturalmente. Apenas a referência (hash + metadados) viaja no JSON da
//! sessão (ver [`crate::session::ContentBlock::Image`] /
//! [`crate::session::ContentBlock::Document`]), mantendo `session.json`
//! enxuto e legível.
//!
//! Layout: `<session_dir>/attachments/<sha256_hex>` (sem extensão — o tipo
//! é resolvido via `media_type` armazenado no `ContentBlock`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Armazena anexos como sidecar files content-addressable dentro do
/// diretório da sessão.
pub struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    /// Cria o store apontando para `<session_dir>/attachments/`. Não cria
    /// o diretório imediatamente — `store` faz isso sob demanda.
    #[must_use]
    pub fn new(session_dir: impl AsRef<Path>) -> Self {
        Self {
            root: session_dir.as_ref().join("attachments"),
        }
    }

    /// Devolve o caminho raiz do store (geralmente `<session_dir>/attachments/`).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Persiste `bytes` e devolve o `sha256` em hex (64 chars).
    /// Operação idempotente: se um arquivo com o mesmo hash já existir, a
    /// gravação é pulada.
    pub fn store(&self, bytes: &[u8]) -> io::Result<String> {
        fs::create_dir_all(&self.root)?;
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash = format!("{:x}", hasher.finalize());
        let path = self.root.join(&hash);
        if !path.exists() {
            fs::write(&path, bytes)?;
        }
        Ok(hash)
    }

    /// Lê os bytes referenciados pelo `sha256` hex previamente armazenado.
    pub fn load(&self, sha256: &str) -> io::Result<Vec<u8>> {
        fs::read(self.root.join(sha256))
    }

    /// Devolve `true` se o arquivo correspondente ao hash existir.
    #[must_use]
    pub fn contains(&self, sha256: &str) -> bool {
        self.root.join(sha256).exists()
    }

    /// Apaga o diretório inteiro de anexos. Usado pelo `/clear` para
    /// liberar espaço e zerar a numeração de placeholders. No-op se o
    /// diretório ainda não foi criado.
    pub fn purge(&self) -> io::Result<()> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AttachmentStore;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_session_dir() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("attachment-store-test-{nanos}"));
        fs::create_dir_all(&dir).expect("create session dir");
        dir
    }

    #[test]
    fn store_then_load_round_trips_bytes() {
        let session_dir = temp_session_dir();
        let store = AttachmentStore::new(&session_dir);

        let bytes = b"\x89PNG\r\n\x1a\nfake-png-bytes".to_vec();
        let hash = store.store(&bytes).expect("store");
        // SHA-256 hex is 64 chars.
        assert_eq!(hash.len(), 64);

        let loaded = store.load(&hash).expect("load");
        assert_eq!(loaded, bytes);

        fs::remove_dir_all(&session_dir).ok();
    }

    #[test]
    fn store_is_idempotent_for_identical_bytes() {
        let session_dir = temp_session_dir();
        let store = AttachmentStore::new(&session_dir);

        let bytes = b"identical".to_vec();
        let hash_a = store.store(&bytes).expect("store first");
        let hash_b = store.store(&bytes).expect("store second");
        assert_eq!(hash_a, hash_b);

        // Apenas um arquivo deve ter sido criado.
        let entries: Vec<_> = fs::read_dir(store.root())
            .expect("read_dir")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect dir entries");
        assert_eq!(entries.len(), 1);

        fs::remove_dir_all(&session_dir).ok();
    }

    #[test]
    fn store_creates_distinct_files_for_distinct_bytes() {
        let session_dir = temp_session_dir();
        let store = AttachmentStore::new(&session_dir);

        let hash_a = store.store(b"a").expect("store a");
        let hash_b = store.store(b"b").expect("store b");
        assert_ne!(hash_a, hash_b);
        assert!(store.contains(&hash_a));
        assert!(store.contains(&hash_b));

        fs::remove_dir_all(&session_dir).ok();
    }

    #[test]
    fn purge_removes_directory() {
        let session_dir = temp_session_dir();
        let store = AttachmentStore::new(&session_dir);

        store.store(b"to be purged").expect("store");
        assert!(store.root().exists(), "root should exist after store");

        store.purge().expect("purge");
        assert!(!store.root().exists(), "root should be gone after purge");

        // Purga após dir ausente deve ser no-op.
        store.purge().expect("purge no-op when missing");

        fs::remove_dir_all(&session_dir).ok();
    }

    #[test]
    fn load_missing_hash_returns_io_error() {
        let session_dir = temp_session_dir();
        let store = AttachmentStore::new(&session_dir);

        let result = store.load("0".repeat(64).as_str());
        assert!(result.is_err(), "loading missing hash should error");

        fs::remove_dir_all(&session_dir).ok();
    }
}
