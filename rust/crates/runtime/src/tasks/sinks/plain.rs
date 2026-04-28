//! Sink append-only para CLI sem TTY (CI, piping pra arquivo).
//!
//! `\r` + ANSI escapes não fazem sentido aqui: ferramentas downstream esperam
//! texto plano linha a linha. Estratégia: emitir mensagens com `%` apenas
//! quando o delta vs última emissão for ≥ 5%; mensagens sem `%` (transições
//! de fase, "Walking…", "Done.") sempre emitem.

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Mutex;

use super::{ProgressSink, TaskStatus};

const PCT_DELTA_THRESHOLD: u32 = 5;

pub struct PlainStderrSink {
    inner: Mutex<Inner>,
}

struct Inner {
    writer: Box<dyn Write + Send>,
    last_pct: HashMap<String, u32>,
}

impl PlainStderrSink {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                writer: Box::new(io::stderr()),
                last_pct: HashMap::new(),
            }),
        }
    }

    /// Constrói com writer customizado. Usado em testes.
    pub fn with_writer<W: Write + Send + 'static>(writer: W) -> Self {
        Self {
            inner: Mutex::new(Inner {
                writer: Box::new(writer),
                last_pct: HashMap::new(),
            }),
        }
    }
}

impl Default for PlainStderrSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressSink for PlainStderrSink {
    fn emit(&self, task_id: &str, label: &str, msg: &str) {
        let Ok(mut guard) = self.inner.try_lock() else {
            return;
        };
        if let Some(pct) = extract_pct(msg) {
            let last = guard.last_pct.get(task_id).copied();
            let should_emit = match last {
                None => true,
                Some(prev) => pct == 100 || pct.abs_diff(prev) >= PCT_DELTA_THRESHOLD,
            };
            if !should_emit {
                return;
            }
            guard.last_pct.insert(task_id.to_string(), pct);
        }
        let _ = writeln!(guard.writer, "[task {task_id} {label}] {msg}");
        let _ = guard.writer.flush();
    }

    fn finalize(&self, task_id: &str, label: &str, status: TaskStatus, summary: Option<&str>) {
        let Ok(mut guard) = self.inner.try_lock() else {
            return;
        };
        let body = summary.unwrap_or("done");
        let _ = writeln!(
            guard.writer,
            "[task {task_id} {label}] {} {body}",
            status_word(status)
        );
        let _ = guard.writer.flush();
        guard.last_pct.remove(task_id);
    }
}

const fn status_word(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Killed => "killed",
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
    }
}

/// Extrai o primeiro `\d+%` de uma mensagem (ex: `"Embedding [██░░] 45% 1500/3409"` → `Some(45)`).
/// Tolera N até 999.
fn extract_pct(msg: &str) -> Option<u32> {
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'%' {
                return msg[start..i].parse::<u32>().ok();
            }
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl SharedBuf {
        fn output(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    #[test]
    fn extract_pct_finds_first_percent() {
        assert_eq!(extract_pct("[█] 45% 1500/3409"), Some(45));
        assert_eq!(extract_pct("100% 3409/3409"), Some(100));
        assert_eq!(extract_pct("Walking files..."), None);
        assert_eq!(extract_pct("0%"), Some(0));
        assert_eq!(extract_pct("file 50.txt at 45%"), Some(45));
    }

    #[test]
    fn plain_sink_skips_under_5pct_delta() {
        let buf = SharedBuf::default();
        let sink = PlainStderrSink::with_writer(buf.clone());
        sink.emit("t1", "Indexing", "1%");
        sink.emit("t1", "Indexing", "2%"); // delta=1 → drop
        sink.emit("t1", "Indexing", "3%"); // delta=2 → drop
        sink.emit("t1", "Indexing", "6%"); // delta=5 → emit
        let out = buf.output();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "lines: {lines:?}");
        assert!(lines[0].contains("1%"));
        assert!(lines[1].contains("6%"));
    }

    #[test]
    fn plain_sink_always_emits_non_progress_msgs() {
        let buf = SharedBuf::default();
        let sink = PlainStderrSink::with_writer(buf.clone());
        sink.emit("t1", "Indexing", "10%");
        sink.emit("t1", "Indexing", "Walking files..."); // sem %, sempre emite
        sink.emit("t1", "Indexing", "Chunking files...");
        let out = buf.output();
        assert_eq!(out.lines().count(), 3);
    }

    #[test]
    fn plain_sink_emits_final_100pct() {
        let buf = SharedBuf::default();
        let sink = PlainStderrSink::with_writer(buf.clone());
        sink.emit("t1", "Indexing", "98%");
        sink.emit("t1", "Indexing", "99%"); // delta=1 → drop
        sink.emit("t1", "Indexing", "100%"); // 100 sempre emite
        let out = buf.output();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("100%"));
    }

    #[test]
    fn plain_sink_finalize_writes_status_word() {
        let buf = SharedBuf::default();
        let sink = PlainStderrSink::with_writer(buf.clone());
        sink.finalize("t1", "Indexing", TaskStatus::Completed, Some("3409 chunks"));
        let out = buf.output();
        assert!(out.contains("completed"));
        assert!(out.contains("3409 chunks"));
    }
}
