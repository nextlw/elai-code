//! Sink "linha viva" para CLI com TTY.
//!
//! Cada `emit` reposiciona o cursor com `\r` e limpa a linha com `\x1b[2K`,
//! depois reescreve `{label} {msg}` truncado pra largura do terminal. Múltiplas
//! tasks compartilhando o mesmo sink são serializadas: troca de owner emite
//! `\n` antes da nova linha pra commitar a anterior. `finalize` adiciona prefix
//! de status (`✓` / `✗` / `⊘`) e newline final.

use std::io::{self, Write};
use std::sync::Mutex;

use super::{ProgressSink, TaskStatus};

const CLEAR_LINE: &str = "\r\x1b[2K";

/// Largura usada pra truncar quando `$COLUMNS` não está exportada.
/// 100 cols cobre a maioria das janelas modernas e degrada bem em janelas
/// pequenas (truncate) e largas (sem truncate).
const FALLBACK_COLUMNS: usize = 100;

pub struct LiveStderrSink {
    inner: Mutex<Inner>,
    /// Largura fixa pra truncate. `None` = lê `$COLUMNS` em runtime. Usado
    /// em testes pra isolar do env compartilhado entre threads.
    fixed_width: Option<usize>,
}

struct Inner {
    /// Writer destino. Em produção é `Box::new(io::stderr())`; em testes é
    /// um buffer in-memory implementando `Write + Send`.
    writer: Box<dyn Write + Send>,
    /// Task que detém a linha viva atualmente. Quando outra task tenta
    /// emitir, primeiro fechamos a linha do owner anterior (`\n`).
    current_owner: Option<String>,
}

impl LiveStderrSink {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                writer: Box::new(io::stderr()),
                current_owner: None,
            }),
            fixed_width: None,
        }
    }

    /// Constrói com writer customizado. Usado em testes.
    pub fn with_writer<W: Write + Send + 'static>(writer: W) -> Self {
        Self {
            inner: Mutex::new(Inner {
                writer: Box::new(writer),
                current_owner: None,
            }),
            fixed_width: None,
        }
    }

    /// Para testes: força largura fixa, ignorando `$COLUMNS`.
    #[cfg(test)]
    fn with_writer_and_width<W: Write + Send + 'static>(writer: W, width: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                writer: Box::new(writer),
                current_owner: None,
            }),
            fixed_width: Some(width),
        }
    }

    fn line(&self, label: &str, msg: &str) -> String {
        let raw = if msg.is_empty() {
            label.to_string()
        } else {
            format!("{label} {msg}")
        };
        truncate_to_width(&raw, self.fixed_width.unwrap_or_else(terminal_width))
    }
}

impl Default for LiveStderrSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressSink for LiveStderrSink {
    fn emit(&self, task_id: &str, label: &str, msg: &str) {
        let line = self.line(label, msg);
        // try_lock: reentrante ou contendido → drop pra evitar deadlock.
        let Ok(mut guard) = self.inner.try_lock() else {
            return;
        };
        let needs_handover = guard
            .current_owner
            .as_deref()
            .is_some_and(|owner| owner != task_id);
        if needs_handover {
            let _ = guard.writer.write_all(b"\n");
        }
        let _ = write!(guard.writer, "{CLEAR_LINE}{line}");
        let _ = guard.writer.flush();
        guard.current_owner = Some(task_id.to_string());
    }

    fn finalize(&self, task_id: &str, label: &str, status: TaskStatus, summary: Option<&str>) {
        let prefix = status_prefix(status);
        let body = summary.unwrap_or("done");
        let line = self.line(&format!("{prefix} {label}"), body);
        let Ok(mut guard) = self.inner.try_lock() else {
            return;
        };
        // Se a linha viva é de outra task, não sobrescrevemos — emitimos numa
        // linha nova. Se é desta task (caso comum), reaproveitamos a linha.
        let owner_matches = guard
            .current_owner
            .as_deref()
            .is_some_and(|owner| owner == task_id);
        if !owner_matches && guard.current_owner.is_some() {
            let _ = guard.writer.write_all(b"\n");
        }
        let _ = writeln!(guard.writer, "{CLEAR_LINE}{line}");
        let _ = guard.writer.flush();
        if owner_matches {
            guard.current_owner = None;
        }
    }
}

const fn status_prefix(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Completed => "\u{2713}",        // ✓
        TaskStatus::Failed => "\u{2717}",           // ✗
        TaskStatus::Killed => "\u{2298}",           // ⊘
        TaskStatus::Pending | TaskStatus::Running => "\u{2026}", // …
    }
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(FALLBACK_COLUMNS)
}

/// Trunca por contagem de chars Unicode (não bytes) pra não cortar UTF-8 no
/// meio. Margem de 1 col à direita pra não disparar wrap em terminais que
/// avançam o cursor uma posição além da largura visível.
fn truncate_to_width(s: &str, width: usize) -> String {
    let limit = width.saturating_sub(1).max(1);
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= limit {
        return s.to_string();
    }
    let cut = limit.saturating_sub(1).max(1);
    let mut out: String = chars.iter().take(cut).collect();
    out.push('\u{2026}'); // …
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Writer in-memory thread-safe. `Box<Mutex<Vec<u8>>>` exposto via clone
    /// do `Arc` permite ler bytes escritos depois.
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

    /// Width grande o bastante pra nunca truncar nas mensagens dos testes.
    const WIDE: usize = 200;

    #[test]
    fn live_sink_overwrites_with_carriage_return() {
        let buf = SharedBuf::default();
        let sink = LiveStderrSink::with_writer_and_width(buf.clone(), WIDE);
        sink.emit("t1", "Indexing", "1%");
        sink.emit("t1", "Indexing", "2%");
        sink.emit("t1", "Indexing", "3%");
        let out = buf.output();
        assert_eq!(out.matches(CLEAR_LINE).count(), 3);
        assert!(!out.contains('\n'), "no newline between updates of same owner");
    }

    #[test]
    fn live_sink_switches_owner_emits_newline() {
        let buf = SharedBuf::default();
        let sink = LiveStderrSink::with_writer_and_width(buf.clone(), WIDE);
        sink.emit("t1", "Indexing", "1%");
        sink.emit("t2", "Verify", "scanning");
        let out = buf.output();
        // 1 newline entre owners; cada emit tem seu CLEAR_LINE.
        assert_eq!(out.matches('\n').count(), 1);
        assert_eq!(out.matches(CLEAR_LINE).count(), 2);
    }

    #[test]
    fn live_sink_finalize_emits_check() {
        let buf = SharedBuf::default();
        let sink = LiveStderrSink::with_writer_and_width(buf.clone(), WIDE);
        sink.emit("t1", "Indexing", "50%");
        sink.finalize("t1", "Indexing", TaskStatus::Completed, Some("3409 chunks"));
        let out = buf.output();
        assert!(out.contains("\u{2713}"), "expected ✓ in output: {out:?}");
        assert!(out.contains("3409 chunks"), "out: {out:?}");
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn live_sink_finalize_failure_emits_cross() {
        let buf = SharedBuf::default();
        let sink = LiveStderrSink::with_writer_and_width(buf.clone(), WIDE);
        sink.emit("t1", "Indexing", "10%");
        sink.finalize("t1", "Indexing", TaskStatus::Failed, Some("io error"));
        let out = buf.output();
        assert!(out.contains("\u{2717}"));
        assert!(out.contains("io error"));
    }

    #[test]
    fn truncate_respects_width() {
        // limit = 20 - 1 (margem) = 19; cut = 19 - 1 = 18 chars + …
        let truncated = truncate_to_width(&"x".repeat(50), 20);
        assert_eq!(truncated.chars().count(), 19);
        assert!(truncated.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_skips_when_short() {
        let s = "short message";
        assert_eq!(truncate_to_width(s, 100), s);
    }

    #[test]
    fn live_sink_truncates_long_messages() {
        let buf = SharedBuf::default();
        let sink = LiveStderrSink::with_writer_and_width(buf.clone(), 20);
        sink.emit("t1", "Indexing", &"y".repeat(50));
        let out = buf.output();
        assert!(out.contains('\u{2026}'), "expected ellipsis in: {out:?}");
    }
}
