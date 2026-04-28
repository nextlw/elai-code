//! Lock file: `.elai/auto-dream/.consolidate-lock` com mtime como lastAt.
//!
//! - `read_last_consolidated_at(root)` — lê mtime do lock; se ausente, retorna 0.
//! - `try_acquire_lock(root)` — touch o arquivo (atualiza mtime para now). Retorna
//!   o mtime ANTES do touch (para rollback). Se outro processo passou recente
//!   (< HOLDER_STALE_MS) → retorna None (lock ocupado).
//! - `rollback_lock(root, prior_mtime)` — restaura mtime para `prior_mtime`.
//! - `record_consolidation(root)` — touch (mtime = now), sem rollback semântico.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use filetime::FileTime;

pub const HOLDER_STALE_MS: u64 = 60 * 60 * 1000; // 60 min

pub fn lock_path(root: &Path) -> PathBuf {
    root.join(".elai").join("auto-dream").join(".consolidate-lock")
}

/// Lê mtime do lock em ms. 0 se ausente.
pub fn read_last_consolidated_at(root: &Path) -> std::io::Result<u64> {
    let path = lock_path(root);
    if !path.is_file() {
        return Ok(0);
    }
    let meta = std::fs::metadata(&path)?;
    let mtime = meta.modified()?;
    Ok(mtime
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64))
}

/// Tenta adquirir o lock. Retorna `Some(prior_mtime_ms)` em sucesso ou `None`
/// se outro processo passou recente (< `HOLDER_STALE_MS` atrás).
pub fn try_acquire_lock(root: &Path) -> std::io::Result<Option<u64>> {
    let path = lock_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let prior_mtime = read_last_consolidated_at(root)?;
    let now_ms = current_ms();
    if prior_mtime > 0 && now_ms.saturating_sub(prior_mtime) < HOLDER_STALE_MS {
        return Ok(None);
    }

    // Touch: cria se não existe + atualiza mtime para now.
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    set_mtime(&path, now_ms)?;
    Ok(Some(prior_mtime))
}

/// Restaura mtime para `prior_mtime_ms`. Usado em rollback de aborto.
pub fn rollback_lock(root: &Path, prior_mtime_ms: u64) -> std::io::Result<()> {
    let path = lock_path(root);
    if !path.is_file() {
        return Ok(());
    }
    set_mtime(&path, prior_mtime_ms)
}

/// Registra consolidação concluída: mtime = now.
pub fn record_consolidation(root: &Path) -> std::io::Result<()> {
    let path = lock_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    set_mtime(&path, current_ms())
}

/// Lista session files (em `<root>/.elai/sessions/*.json`) com mtime > since_ms.
/// Retorna nomes (sem extensão).
pub fn list_sessions_touched_since(root: &Path, since_ms: u64) -> std::io::Result<Vec<String>> {
    let dir = root.join(".elai").join("sessions");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        if mtime_ms > since_ms {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                out.push(stem.to_string());
            }
        }
    }
    Ok(out)
}

fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn set_mtime(path: &Path, ms: u64) -> std::io::Result<()> {
    let secs = (ms / 1000) as i64;
    let nanos = ((ms % 1000) * 1_000_000) as u32;
    let ft = FileTime::from_unix_time(secs, nanos);
    filetime::set_file_mtime(path, ft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_last_consolidated_at_returns_zero_when_no_lock() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(read_last_consolidated_at(tmp.path()).unwrap(), 0);
    }

    #[test]
    fn record_then_read_round_trip() {
        let tmp = TempDir::new().unwrap();
        record_consolidation(tmp.path()).unwrap();
        let mtime = read_last_consolidated_at(tmp.path()).unwrap();
        assert!(mtime > 0, "mtime should be positive after record");
    }

    #[test]
    fn try_acquire_when_recent_returns_none() {
        let tmp = TempDir::new().unwrap();
        record_consolidation(tmp.path()).unwrap();
        // Acquire immediately: lock was just set, holder not stale.
        let result = try_acquire_lock(tmp.path()).unwrap();
        assert!(result.is_none(), "expected None (lock busy), got {result:?}");
    }

    #[test]
    fn try_acquire_when_old_returns_some_with_prior_mtime() {
        let tmp = TempDir::new().unwrap();
        // Record, then backdate via rollback to 2h ago.
        record_consolidation(tmp.path()).unwrap();
        let two_hours_ago_ms = current_ms().saturating_sub(2 * 60 * 60 * 1000 + 1000);
        rollback_lock(tmp.path(), two_hours_ago_ms).unwrap();

        let result = try_acquire_lock(tmp.path()).unwrap();
        assert!(
            result.is_some(),
            "expected Some(prior_mtime) after stale lock"
        );
        let prior = result.unwrap();
        // prior should be close to two_hours_ago_ms (within 2s of rounding).
        assert!(
            prior.abs_diff(two_hours_ago_ms) < 2000,
            "prior mtime {prior} should be ~{two_hours_ago_ms}"
        );
    }

    #[test]
    fn rollback_restores_mtime() {
        let tmp = TempDir::new().unwrap();
        record_consolidation(tmp.path()).unwrap();
        let original = read_last_consolidated_at(tmp.path()).unwrap();

        // Acquire (updates mtime to now).
        let prior = try_acquire_lock(tmp.path())
            .unwrap()
            .unwrap_or(original);

        // Rollback.
        rollback_lock(tmp.path(), prior).unwrap();
        let after_rollback = read_last_consolidated_at(tmp.path()).unwrap();
        // Should be restored to prior (within 2s tolerance).
        assert!(
            after_rollback.abs_diff(prior) < 2000,
            "after rollback {after_rollback} should equal prior {prior}"
        );
    }

    #[test]
    fn list_sessions_touched_since_filters_by_mtime() {
        let tmp = TempDir::new().unwrap();
        let sessions_dir = tmp.path().join(".elai").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let now_ms = current_ms();
        let threshold_ms = now_ms.saturating_sub(5000); // 5 seconds ago

        // File "old" backdated to 10s ago — should NOT appear.
        let old_path = sessions_dir.join("old.json");
        std::fs::write(&old_path, b"{}").unwrap();
        let old_ms = now_ms.saturating_sub(10_000);
        set_mtime(&old_path, old_ms).unwrap();

        // Files "new1" and "new2" with mtime now — should appear.
        let new1 = sessions_dir.join("new1.json");
        let new2 = sessions_dir.join("new2.json");
        std::fs::write(&new1, b"{}").unwrap();
        std::fs::write(&new2, b"{}").unwrap();
        // Their mtime is already "now" from write; ensure > threshold.
        set_mtime(&new1, now_ms).unwrap();
        set_mtime(&new2, now_ms).unwrap();

        let mut found = list_sessions_touched_since(tmp.path(), threshold_ms).unwrap();
        found.sort();
        assert_eq!(found, vec!["new1", "new2"]);
    }
}
