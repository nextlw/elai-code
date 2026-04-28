//! Auto-dream: consolidação automática de memória.
//!
//! Gates (em ordem de custo):
//!   1. Time gate: hours since lastAt >= min_hours
//!   2. Scan throttle: 10 min entre scans falhos
//!   3. Session gate: sessions touched since lastAt >= min_sessions
//!   4. Lock acquire: outro processo não está mid-consolidation

mod config;
mod lock;
mod prompt;

pub use config::AutoDreamConfig;
pub use lock::{
    list_sessions_touched_since, read_last_consolidated_at, record_consolidation, rollback_lock,
    try_acquire_lock, HOLDER_STALE_MS,
};
pub use prompt::build_consolidation_prompt;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const SESSION_SCAN_INTERVAL_MS: u64 = 10 * 60 * 1000;

/// Estado por-processo do scan throttle. Em produção usaria estado da sessão;
/// para v1 mantemos estático.
static LAST_SESSION_SCAN_AT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq)]
pub enum AutoDreamDecision {
    /// Não deve rodar (gate fechado, throttle, lock ocupado, etc).
    Skip { reason: SkipReason },
    /// Deve rodar. Lock adquirido; caller deve chamar `record_consolidation`
    /// no sucesso ou `rollback_lock(prior_mtime)` no abort.
    Fire {
        session_ids: Vec<String>,
        hours_since_last: f64,
        prior_mtime_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    TimeGateNotPassed { hours_since: u64 },
    ScanThrottle { remaining_ms: u64 },
    NotEnoughSessions { count: u32, required: u32 },
    LockBusy,
    Io(String),
}

/// Avalia gates e adquire lock se OK. NÃO executa o dream — apenas decide.
/// O caller (post-turn hook) usa `Fire` para invocar o agent forked.
pub fn evaluate(root: &Path, cfg: &AutoDreamConfig) -> AutoDreamDecision {
    if !cfg.enabled {
        return AutoDreamDecision::Skip {
            reason: SkipReason::Disabled,
        };
    }

    // 1. Time gate
    let last_at = match lock::read_last_consolidated_at(root) {
        Ok(v) => v,
        Err(e) => {
            return AutoDreamDecision::Skip {
                reason: SkipReason::Io(e.to_string()),
            }
        }
    };
    let now_ms = current_ms();
    let hours_since = if last_at == 0 {
        f64::INFINITY
    } else {
        (now_ms.saturating_sub(last_at) as f64) / 3_600_000.0
    };
    if hours_since < f64::from(cfg.min_hours) {
        return AutoDreamDecision::Skip {
            reason: SkipReason::TimeGateNotPassed {
                hours_since: hours_since as u64,
            },
        };
    }

    // 2. Scan throttle
    let last_scan = LAST_SESSION_SCAN_AT.load(Ordering::Relaxed);
    let since_scan = now_ms.saturating_sub(last_scan);
    if since_scan < SESSION_SCAN_INTERVAL_MS && last_scan > 0 {
        return AutoDreamDecision::Skip {
            reason: SkipReason::ScanThrottle {
                remaining_ms: SESSION_SCAN_INTERVAL_MS - since_scan,
            },
        };
    }
    LAST_SESSION_SCAN_AT.store(now_ms, Ordering::Relaxed);

    // 3. Session gate
    let session_ids = match lock::list_sessions_touched_since(root, last_at) {
        Ok(v) => v,
        Err(e) => {
            return AutoDreamDecision::Skip {
                reason: SkipReason::Io(e.to_string()),
            }
        }
    };
    if (session_ids.len() as u32) < cfg.min_sessions {
        return AutoDreamDecision::Skip {
            reason: SkipReason::NotEnoughSessions {
                count: session_ids.len() as u32,
                required: cfg.min_sessions,
            },
        };
    }

    // 4. Lock acquire
    let prior_mtime = match lock::try_acquire_lock(root) {
        Ok(Some(m)) => m,
        Ok(None) => {
            return AutoDreamDecision::Skip {
                reason: SkipReason::LockBusy,
            }
        }
        Err(e) => {
            return AutoDreamDecision::Skip {
                reason: SkipReason::Io(e.to_string()),
            }
        }
    };

    AutoDreamDecision::Fire {
        session_ids,
        hours_since_last: hours_since,
        prior_mtime_ms: prior_mtime,
    }
}

fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Execute consolidation after gates have passed.
/// Currently a stub — full implementation deferred to v0.8.0.
pub fn execute_consolidation(
    _root: &Path,
    _session_ids: &[String],
    reporter: &dyn crate::ProgressReporter,
) -> Result<(), Box<dyn std::error::Error>> {
    reporter.report("[auto-dream] consolidation stub — implementation deferred to v0.8.0");
    Ok(())
}

#[cfg(test)]
pub fn reset_scan_throttle_for_tests() {
    LAST_SESSION_SCAN_AT.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn evaluate_disabled_returns_skip() {
        let tmp = TempDir::new().unwrap();
        let cfg = AutoDreamConfig {
            enabled: false,
            ..Default::default()
        };
        match evaluate(tmp.path(), &cfg) {
            AutoDreamDecision::Skip {
                reason: SkipReason::Disabled,
            } => {}
            other => panic!("expected Skip(Disabled), got {other:?}"),
        }
    }

    #[test]
    fn evaluate_time_gate_blocks_when_recent() {
        let tmp = TempDir::new().unwrap();
        // Recently consolidated (record_consolidation now).
        lock::record_consolidation(tmp.path()).unwrap();
        reset_scan_throttle_for_tests();
        let cfg = AutoDreamConfig::default();
        match evaluate(tmp.path(), &cfg) {
            AutoDreamDecision::Skip {
                reason: SkipReason::TimeGateNotPassed { .. },
            } => {}
            other => panic!("expected TimeGateNotPassed, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_session_gate_blocks_when_few_sessions() {
        let tmp = TempDir::new().unwrap();
        // No prior lock + no sessions.
        reset_scan_throttle_for_tests();
        let cfg = AutoDreamConfig {
            min_sessions: 5,
            ..Default::default()
        };
        match evaluate(tmp.path(), &cfg) {
            AutoDreamDecision::Skip {
                reason: SkipReason::NotEnoughSessions { count: 0, required: 5 },
            } => {}
            other => panic!("expected NotEnoughSessions(0/5), got {other:?}"),
        }
    }
}
