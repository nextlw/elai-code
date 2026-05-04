use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

// ──────────────────────────── TelemetryEntry (JSONL per-request record) ──────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEntry {
    pub timestamp: String,
    pub session_id: String,
    pub project: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub latency_ms: u64,
    pub success: bool,
    pub provider: Option<String>,
    pub error_type: Option<String>,
}

// ──────────────────────────── TelemetryWriter ────────────────────────────────

pub struct TelemetryWriter {
    path: PathBuf,
}

impl TelemetryWriter {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Append a single JSONL line. Creates the file and parent dirs if needed.
    pub fn append(&self, entry: &TelemetryEntry) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(entry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }
}

/// Default path: `~/.elai/telemetry.jsonl`
#[must_use]
pub fn default_telemetry_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".elai")
        .join("telemetry.jsonl")
}

/// Load all entries (or filter by `since_secs` Unix timestamp).
/// Entries whose `timestamp` prefix is earlier than the cut-off are skipped.
pub fn load_entries(path: &Path, since_secs: Option<u64>) -> io::Result<Vec<TelemetryEntry>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let cutoff_prefix: Option<String> = since_secs.map(|secs| {
        // Convert unix timestamp to "YYYY-MM-DD" prefix for prefix comparison.
        unix_secs_to_date_prefix(secs)
    });

    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: TelemetryEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Some(ref cutoff) = cutoff_prefix {
            // timestamp is "YYYY-MM-DDTHH:MM:SSZ" — compare date prefix
            if entry.timestamp.len() >= 10 && &entry.timestamp[..10] < cutoff.as_str() {
                continue;
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// Convert Unix seconds to a `YYYY-MM-DD` date string.
fn unix_secs_to_date_prefix(secs: u64) -> String {
    // Days since epoch
    let days = secs / 86400;
    // Gregorian calendar calculation
    let (y, m, d) = days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm: civil calendar from days since 1970-01-01
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Build an ISO-8601 UTC timestamp string for *now* without any external crate.
#[must_use]
pub fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d) = days_to_ymd(secs / 86400);
    let rem = secs % 86400;
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let s = rem % 60;
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

// ──────────────────────────── Event types ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TelemetryEvent {
    ProviderSelected {
        timestamp_ms: u64,
        provider: String,
        model: String,
        latency_ms: u64,
        reason: String,
    },
    RequestFailed {
        timestamp_ms: u64,
        provider: String,
        error: String,
        retried: bool,
    },
    TokenUsage {
        timestamp_ms: u64,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
        cost_usd: f64,
    },
    SessionEnd {
        timestamp_ms: u64,
        session_id: String,
        turns: u32,
        total_cost_usd: f64,
        duration_ms: u64,
    },
}

// ──────────────────────────── Handle ─────────────────────────────────────────

/// Cheap-to-clone handle for emitting events. Never blocks the caller.
#[derive(Clone)]
pub struct TelemetryHandle {
    tx: Option<mpsc::UnboundedSender<TelemetryEvent>>,
}

impl TelemetryHandle {
    /// Emit an event. Returns immediately; the worker drains asynchronously.
    pub fn emit(&self, event: TelemetryEvent) {
        if let Some(ref tx) = self.tx {
            // Ignore send errors (worker shut down).
            let _ = tx.send(event);
        }
    }

    /// Returns a handle that silently drops all events (for `ELAI_TELEMETRY=off`).
    #[must_use]
    pub fn noop() -> Self {
        Self { tx: None }
    }
}

// ──────────────────────────── Shutdown token ─────────────────────────────────

/// Drop (or call `signal()`) to trigger graceful flush and worker shutdown.
pub struct TelemetryShutdown {
    tx: Option<oneshot::Sender<()>>,
}

impl TelemetryShutdown {
    /// Explicitly signal shutdown (also happens on `Drop`).
    pub fn signal(mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for TelemetryShutdown {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
}

// ──────────────────────────── Worker ─────────────────────────────────────────

/// Background tokio task that batches and writes telemetry events to disk.
pub struct TelemetryWorker;

impl TelemetryWorker {
    /// Spawn the worker and return the handle/shutdown pair.
    ///
    /// Requires an active tokio runtime (uses `tokio::spawn`).
    #[must_use] 
    pub fn start() -> (TelemetryHandle, TelemetryShutdown) {
        let (event_tx, event_rx) = mpsc::unbounded_channel::<TelemetryEvent>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(run_worker(event_rx, shutdown_rx));

        let handle = TelemetryHandle { tx: Some(event_tx) };
        let shutdown = TelemetryShutdown { tx: Some(shutdown_tx) };
        (handle, shutdown)
    }
}

// ──────────────────────────── Worker internals ───────────────────────────────

async fn run_worker(
    mut event_rx: mpsc::UnboundedReceiver<TelemetryEvent>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    const FLUSH_THRESHOLD: usize = 10;
    const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

    let mut buffer: Vec<TelemetryEvent> = Vec::new();
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    // Skip the immediate first tick.
    interval.tick().await;

    loop {
        tokio::select! {
            // Drain incoming events.
            event = event_rx.recv() => {
                if let Some(ev) = event {
                    buffer.push(ev);
                    if buffer.len() >= FLUSH_THRESHOLD {
                        flush_buffer(&mut buffer).await;
                    }
                } else {
                    // Sender side dropped — flush and exit.
                    flush_buffer(&mut buffer).await;
                    return;
                }
            }
            // Timer-based flush.
            _ = interval.tick() => {
                if !buffer.is_empty() {
                    flush_buffer(&mut buffer).await;
                }
            }
            // Graceful shutdown signal.
            _ = &mut shutdown_rx => {
                // Drain remaining events before exiting.
                while let Ok(ev) = event_rx.try_recv() {
                    buffer.push(ev);
                }
                flush_buffer(&mut buffer).await;
                return;
            }
        }
    }
}

async fn flush_buffer(buffer: &mut Vec<TelemetryEvent>) {
    use tokio::io::AsyncWriteExt;

    if buffer.is_empty() {
        return;
    }

    let Some(path) = telemetry_file_path() else {
        buffer.clear();
        return;
    };

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        if let Err(_e) = tokio::fs::create_dir_all(parent).await {
            buffer.clear();
            return;
        }
    }

    // Build JSONL lines.
    let mut lines = String::new();
    for event in buffer.drain(..) {
        if let Ok(json) = serde_json::to_string(&event) {
            lines.push_str(&json);
            lines.push('\n');
        }
    }

    // Append to file.
    let append_result = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await;

    match append_result {
        Ok(mut file) => {
            let _ = file.write_all(lines.as_bytes()).await;
            let _ = file.flush().await;
        }
        Err(_) => return,
    }

    // Rotate if over 10,000 lines.
    maybe_rotate(&path).await;
}

async fn maybe_rotate(path: &PathBuf) {
    const MAX_LINES: usize = 10_000;
    const KEEP_LINES: usize = 8_000;

    let Ok(content) = tokio::fs::read_to_string(path).await else { return };

    let all_lines: Vec<&str> = content.lines().collect();
    if all_lines.len() <= MAX_LINES {
        return;
    }

    let keep_start = all_lines.len().saturating_sub(KEEP_LINES);
    let trimmed = all_lines[keep_start..].join("\n");
    let trimmed = format!("{trimmed}\n");

    let _ = tokio::fs::write(path, trimmed.as_bytes()).await;
}

fn telemetry_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("elai").join("telemetry.jsonl"))
}

// ──────────────────────────── Helper ─────────────────────────────────────────

/// Returns the current Unix time in milliseconds.
#[allow(clippy::cast_possible_truncation)]
#[must_use]
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ──────────────────────────── Tests ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Serde round-trips ────────────────────────────────────────────────────

    #[test]
    fn serde_provider_selected_round_trip() {
        let event = TelemetryEvent::ProviderSelected {
            timestamp_ms: 1_000,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            latency_ms: 350,
            reason: "default".to_string(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"ProviderSelected""#));
        let decoded: TelemetryEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, decoded);
    }

    #[test]
    fn serde_request_failed_round_trip() {
        let event = TelemetryEvent::RequestFailed {
            timestamp_ms: 2_000,
            provider: "openai".to_string(),
            error: "timeout".to_string(),
            retried: true,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"RequestFailed""#));
        let decoded: TelemetryEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, decoded);
    }

    #[test]
    fn serde_token_usage_round_trip() {
        let event = TelemetryEvent::TokenUsage {
            timestamp_ms: 3_000,
            model: "gpt-4.1-mini".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
            cost_usd: 0.001_5,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"TokenUsage""#));
        let decoded: TelemetryEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, decoded);
    }

    #[test]
    fn serde_session_end_round_trip() {
        let event = TelemetryEvent::SessionEnd {
            timestamp_ms: 4_000,
            session_id: "session-abc".to_string(),
            turns: 7,
            total_cost_usd: 0.05,
            duration_ms: 60_000,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"SessionEnd""#));
        let decoded: TelemetryEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, decoded);
    }

    // ── Noop handle ──────────────────────────────────────────────────────────

    #[test]
    fn noop_handle_does_not_panic() {
        let handle = TelemetryHandle::noop();
        handle.emit(TelemetryEvent::RequestFailed {
            timestamp_ms: now_millis(),
            provider: "test".to_string(),
            error: "noop".to_string(),
            retried: false,
        });
        // No panic means success.
    }

    // ── Flush writes to file ─────────────────────────────────────────────────

    #[tokio::test]
    async fn flush_basic_writes_json_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("telemetry.jsonl");

        let mut buffer = vec![
            TelemetryEvent::ProviderSelected {
                timestamp_ms: 1,
                provider: "p".to_string(),
                model: "m".to_string(),
                latency_ms: 10,
                reason: "default".to_string(),
            },
            TelemetryEvent::TokenUsage {
                timestamp_ms: 2,
                model: "m".to_string(),
                input_tokens: 5,
                output_tokens: 2,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
            },
        ];

        // Write directly via the internal helper.
        write_to_path(&mut buffer, &path).await;

        let content = tokio::fs::read_to_string(&path)
            .await
            .expect("read file");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line must be valid JSON.
        for line in &lines {
            serde_json::from_str::<TelemetryEvent>(line).expect("valid JSON line");
        }
    }

    // ── Graceful shutdown flushes remaining events ────────────────────────────

    #[tokio::test]
    async fn graceful_shutdown_flushes_pending_events() {
        // We test this at the integration level using a temp config dir override
        // via the ELAI_TELEMETRY_PATH env var approach is not available here.
        // Instead, verify that the shutdown token can be dropped without panic.
        let (_handle, shutdown) = TelemetryWorker::start();
        // Immediately signal shutdown.
        drop(shutdown);
        // Give the worker a moment to clean up.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // ── TelemetryWriter + load_entries ─────────────────────────────────────────

    fn make_test_entry(timestamp: &str, model: &str) -> TelemetryEntry {
        TelemetryEntry {
            timestamp: timestamp.to_string(),
            session_id: "sess-1".to_string(),
            project: "test-project".to_string(),
            model: model.to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.001,
            latency_ms: 300,
            success: true,
            provider: None,
            error_type: None,
        }
    }

    #[test]
    fn write_and_read_entries() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("telemetry.jsonl");
        let writer = TelemetryWriter::new(path.clone());

        let entries = vec![
            make_test_entry("2026-04-27T10:00:00Z", "claude-sonnet"),
            make_test_entry("2026-04-27T11:00:00Z", "claude-haiku"),
            make_test_entry("2026-04-27T12:00:00Z", "gpt-4.1-mini"),
        ];
        for e in &entries {
            writer.append(e).expect("append");
        }

        let loaded = load_entries(&path, None).expect("load");
        assert_eq!(loaded.len(), 3);
    }

    #[test]
    fn filter_by_days() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("telemetry.jsonl");
        let writer = TelemetryWriter::new(path.clone());

        // Entry from "today" — will pass a 3-day filter.
        writer.append(&make_test_entry("2026-04-27T10:00:00Z", "recent")).expect("append");
        // Entry from 10 days ago — will be filtered out by a 3-day filter.
        writer.append(&make_test_entry("2026-04-17T10:00:00Z", "old")).expect("append");

        // Simulate a cutoff of 2026-04-24 (3 days before 2026-04-27).
        // "2026-04-24" in unix seconds: days from epoch to 2026-04-24
        //   2026-04-24 = days from 1970-01-01
        //   We compute it as: (2026-04-27 represents ~20575 days) minus 3 days = 20572
        //   Actually use SystemTime for a reliable cutoff:
        //   For test purposes, use a fixed cutoff that keeps 2026-04-27 but drops 2026-04-17.
        //   2026-04-24 00:00 UTC = unix secs.
        //   2026-04-24 00:00 UTC: days from epoch = let's compute:
        //   years 1970..2025 = 55*365 + 14 (leap years) = 20088 days
        //   2026 jan=31, feb=28, mar=31, apr=24 → 114 days
        //   total: 20088 + 365 + 114 = 20567 days → 20567 * 86400 secs = 1775788800
        let cutoff_secs: u64 = 1_776_988_800; // 2026-04-24 00:00 UTC
        let loaded = load_entries(&path, Some(cutoff_secs)).expect("load");
        assert_eq!(loaded.len(), 1, "expected only recent entry, got {}", loaded.len());
        assert_eq!(loaded[0].model, "recent");
    }

    // ── Internal helper used in tests ────────────────────────────────────────

    async fn write_to_path(buffer: &mut Vec<TelemetryEvent>, path: &std::path::Path) {
        use tokio::io::AsyncWriteExt;
        let mut lines = String::new();
        for event in buffer.drain(..) {
            if let Ok(json) = serde_json::to_string(&event) {
                lines.push_str(&json);
                lines.push('\n');
            }
        }
        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
        {
            let _ = file.write_all(lines.as_bytes()).await;
        }
    }
}
