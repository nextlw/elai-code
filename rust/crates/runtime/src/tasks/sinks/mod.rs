//! Sinks de progresso — onde a "linha viva" de uma task aparece.
//!
//! O `TaskProgressReporter` (em [`super::progress`]) delega cada update ao
//! sink atual. CLI puro recebe `LiveStderrSink` (linha que se sobrescreve via
//! `\r\x1b[2K`); ambientes não-TTY recebem `PlainStderrSink` (append-only com
//! throttle por delta de %); o TUI substitui o sink default por um
//! `ChannelSink` próprio (vive em `elai-cli`) que envia `TuiMsg::TaskProgress`.

use std::sync::Mutex;

use super::TaskStatus;

mod live;
mod plain;

pub use live::LiveStderrSink;
pub use plain::PlainStderrSink;

/// Receptor de updates de progresso de uma `TaskProgressReporter`.
///
/// Implementações DEVEM ser thread-safe e tolerar mensagens vindas de
/// múltiplas tasks concorrentes (com `task_id` distintos).
pub trait ProgressSink: Send + Sync {
    /// Emite uma "linha viva" — substitui a anterior pra esse `task_id`.
    fn emit(&self, task_id: &str, label: &str, msg: &str);

    /// Fecha a linha viva da task (newline final ou marcador de status).
    /// Chamado uma vez por task pelo helper `with_task`.
    fn finalize(&self, task_id: &str, label: &str, status: TaskStatus, summary: Option<&str>);
}

/// Sink silencioso — descarta tudo. Use em testes ou modo quiet.
#[derive(Default)]
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn emit(&self, _task_id: &str, _label: &str, _msg: &str) {}
    fn finalize(&self, _task_id: &str, _label: &str, _status: TaskStatus, _summary: Option<&str>) {}
}

/// Sink que acumula `(task_id, label, msg, kind)` em memória. Usado em testes
/// para inspecionar a sequência de eventos sem TTY real.
#[derive(Default)]
pub struct CollectingSink {
    events: Mutex<Vec<CollectedEvent>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectedEvent {
    Emit {
        task_id: String,
        label: String,
        msg: String,
    },
    Finalize {
        task_id: String,
        label: String,
        status: TaskStatus,
        summary: Option<String>,
    },
}

impl CollectingSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<CollectedEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn drain(&self) -> Vec<CollectedEvent> {
        std::mem::take(
            &mut *self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl ProgressSink for CollectingSink {
    fn emit(&self, task_id: &str, label: &str, msg: &str) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(CollectedEvent::Emit {
                task_id: task_id.to_string(),
                label: label.to_string(),
                msg: msg.to_string(),
            });
    }

    fn finalize(&self, task_id: &str, label: &str, status: TaskStatus, summary: Option<&str>) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(CollectedEvent::Finalize {
                task_id: task_id.to_string(),
                label: label.to_string(),
                status,
                summary: summary.map(String::from),
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sink_does_nothing() {
        let sink = NoopSink;
        sink.emit("t1", "Indexing", "10%");
        sink.finalize("t1", "Indexing", TaskStatus::Completed, Some("done"));
    }

    #[test]
    fn collecting_sink_records_events() {
        let sink = CollectingSink::new();
        sink.emit("t1", "Indexing", "10%");
        sink.emit("t1", "Indexing", "50%");
        sink.finalize("t1", "Indexing", TaskStatus::Completed, Some("done"));
        let events = sink.snapshot();
        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], CollectedEvent::Emit { msg, .. } if msg == "10%"));
        assert!(matches!(&events[2], CollectedEvent::Finalize { status: TaskStatus::Completed, .. }));
    }

    #[test]
    fn collecting_sink_drain_clears_state() {
        let sink = CollectingSink::new();
        sink.emit("t1", "Indexing", "x");
        assert_eq!(sink.drain().len(), 1);
        assert!(sink.snapshot().is_empty());
    }
}
