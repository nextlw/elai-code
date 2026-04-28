//! Reporter de progresso amarrado a uma task.
//!
//! [`TaskProgressReporter`] implementa o trait [`crate::ProgressReporter`]
//! existente — call sites que aceitam `&dyn ProgressReporter` continuam
//! funcionando — mas DELEGA cada update a um [`ProgressSink`] que sabe
//! desenhar a "linha viva" no destino certo (CLI in-place, TUI `ChatEntry`,
//! testes coletando, etc.).
//!
//! # Pattern obrigatório
//!
//! Toda operação longa **deve** rodar dentro de [`with_task`] (ou
//! [`with_task_default`]):
//!
//! ```ignore
//! use runtime::{with_task_default, TaskType};
//!
//! with_task_default(
//!     TaskType::LocalWorkflow,
//!     "elai init",
//!     "Indexing",
//!     None, // parent_id
//!     |reporter| -> Result<(), MyError> {
//!         reporter.report("Walking files…");
//!         // ... long work ...
//!         reporter.report("Done.");
//!         Ok(())
//!     },
//! )?;
//! ```
//!
//! Em CLI puro, o sink default escolhe `LiveStderrSink` (TTY) ou
//! `PlainStderrSink` (não-TTY). No TUI, o startup chama
//! [`set_default_sink`] passando um `ChannelSink` próprio que emite
//! `TuiMsg::TaskProgress`.
#![allow(clippy::needless_pass_by_value)]
// `Arc<dyn ProgressSink>` é movido pra dentro do reporter / `with_task`;
// passar por referência exigiria clone do Arc no caller.

use std::io::IsTerminal;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use super::sinks::{LiveStderrSink, PlainStderrSink, ProgressSink};
use super::{generate_task_id, TaskState, TaskStatus, TaskType};
use crate::progress::ProgressReporter;
use crate::tasks::task_registry;

/// Janela de throttle para mensagens com barra de progresso.
const THROTTLE_WINDOW_MS: u128 = 80;

/// Reporter amarrado a uma `task_id`. Implementa `ProgressReporter` —
/// passe-o como `&dyn ProgressReporter` pra qualquer função existente.
pub struct TaskProgressReporter {
    task_id: String,
    label: String,
    sink: Arc<dyn ProgressSink>,
    state: Mutex<ReporterState>,
}

#[derive(Default)]
struct ReporterState {
    last_emitted_at: Option<Instant>,
    last_msg: Option<String>,
}

impl TaskProgressReporter {
    pub fn new(
        task_id: impl Into<String>,
        label: impl Into<String>,
        sink: Arc<dyn ProgressSink>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            label: label.into(),
            sink,
            state: Mutex::new(ReporterState::default()),
        }
    }

    #[must_use]
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl ProgressReporter for TaskProgressReporter {
    fn report(&self, msg: &str) {
        // Reentrância: se o lock já está tomado, dropa silenciosamente em vez
        // de deadlockar.
        let Ok(mut state) = self.state.try_lock() else {
            return;
        };

        if state.last_msg.as_deref() == Some(msg) {
            return; // dedup
        }

        let is_progress_bar = msg.contains('\u{2588}') || msg.contains('\u{2591}');
        let now = Instant::now();
        if is_progress_bar {
            if let Some(prev) = state.last_emitted_at {
                if now.duration_since(prev).as_millis() < THROTTLE_WINDOW_MS {
                    return;
                }
            }
        }

        state.last_emitted_at = Some(now);
        state.last_msg = Some(msg.to_string());
        drop(state); // libera antes de chamar sink (evita reentrância no próprio reporter)

        self.sink.emit(&self.task_id, &self.label, msg);
    }
}

// ---------------------------------------------------------------------------
// with_task helpers
// ---------------------------------------------------------------------------

/// Executa `f` dentro de uma task registrada. Cria `TaskState`, inicia
/// (`Running`), passa um `TaskProgressReporter` para a closure, e finaliza
/// (`Completed`/`Failed`/`Killed`) com `sink.finalize` apropriado.
///
/// - `Ok(_)` → `Completed`, sink recebe `summary = None` (caller pode usar
///   `reporter.report("Done — N items")` antes do return pra deixar texto final).
/// - `Err(e)` → `Failed`, summary = `format!("{e}")` truncado.
/// - panic → `Failed`, summary = `"panicked"`, panic é re-propagado.
pub fn with_task<T, E>(
    task_type: TaskType,
    description: impl Into<String>,
    label: impl Into<String>,
    parent_id: Option<String>,
    sink: Arc<dyn ProgressSink>,
    f: impl FnOnce(&TaskProgressReporter) -> Result<T, E>,
) -> Result<T, E>
where
    E: std::fmt::Display,
{
    let label = label.into();
    let task_id = generate_task_id(task_type);
    let state = TaskState::new_with_parent(
        task_id.clone(),
        task_type,
        description.into(),
        None,
        parent_id,
    );

    let registry = task_registry();
    // Falhas em registrar não devem matar a operação; logamos via finalize.
    if registry.register(state).is_ok() {
        let _ = registry.update_status(&task_id, TaskStatus::Running);
    }

    let reporter = TaskProgressReporter::new(task_id.clone(), label.clone(), sink.clone());

    let result = panic::catch_unwind(AssertUnwindSafe(|| f(&reporter)));

    match result {
        Ok(Ok(value)) => {
            let _ = registry.update_status(&task_id, TaskStatus::Completed);
            sink.finalize(&task_id, &label, TaskStatus::Completed, None);
            Ok(value)
        }
        Ok(Err(e)) => {
            let summary = truncate(&format!("{e}"), 200);
            let _ = registry.update_status(&task_id, TaskStatus::Failed);
            sink.finalize(&task_id, &label, TaskStatus::Failed, Some(&summary));
            Err(e)
        }
        Err(panic_payload) => {
            let _ = registry.update_status(&task_id, TaskStatus::Failed);
            sink.finalize(&task_id, &label, TaskStatus::Failed, Some("panicked"));
            panic::resume_unwind(panic_payload);
        }
    }
}

/// Wrapper sobre [`with_task`] que resolve o sink via [`default_sink`].
pub fn with_task_default<T, E>(
    task_type: TaskType,
    description: impl Into<String>,
    label: impl Into<String>,
    parent_id: Option<String>,
    f: impl FnOnce(&TaskProgressReporter) -> Result<T, E>,
) -> Result<T, E>
where
    E: std::fmt::Display,
{
    with_task(task_type, description, label, parent_id, default_sink(), f)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

// ---------------------------------------------------------------------------
// Default sink (slot global)
// ---------------------------------------------------------------------------

static DEFAULT_SINK: OnceLock<RwLock<Arc<dyn ProgressSink>>> = OnceLock::new();

fn slot() -> &'static RwLock<Arc<dyn ProgressSink>> {
    DEFAULT_SINK.get_or_init(|| RwLock::new(build_default_sink()))
}

fn build_default_sink() -> Arc<dyn ProgressSink> {
    if std::io::stderr().is_terminal() {
        Arc::new(LiveStderrSink::new())
    } else {
        Arc::new(PlainStderrSink::new())
    }
}

/// Sink default deste processo. Em CLI: detecta TTY automaticamente. Em TUI:
/// chame [`set_default_sink`] no startup pra substituir.
#[must_use]
pub fn default_sink() -> Arc<dyn ProgressSink> {
    slot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Substitui o sink default global. Use no startup do TUI antes de qualquer
/// `with_task` rodar.
pub fn set_default_sink(sink: Arc<dyn ProgressSink>) {
    *slot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = sink;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::sinks::{CollectedEvent, CollectingSink};

    fn count_emits(events: &[CollectedEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, CollectedEvent::Emit { .. }))
            .count()
    }

    #[test]
    fn dedup_identical_msg() {
        let sink = Arc::new(CollectingSink::new());
        let reporter =
            TaskProgressReporter::new("t1", "Indexing", sink.clone() as Arc<dyn ProgressSink>);
        reporter.report("Walking…");
        reporter.report("Walking…"); // idêntica → drop
        assert_eq!(count_emits(&sink.snapshot()), 1);
    }

    #[test]
    fn throttle_drops_progress_bar_within_window() {
        let sink = Arc::new(CollectingSink::new());
        let reporter =
            TaskProgressReporter::new("t1", "Indexing", sink.clone() as Arc<dyn ProgressSink>);
        // Mensagens com █ contam como progress bar, sujeitas a throttle.
        reporter.report("Embedding [\u{2588}\u{2591}\u{2591}] 1%");
        reporter.report("Embedding [\u{2588}\u{2591}\u{2591}] 2%");
        reporter.report("Embedding [\u{2588}\u{2591}\u{2591}] 3%");
        // Esperamos 1 emit (a primeira), as outras dropadas pelo throttle.
        assert_eq!(count_emits(&sink.snapshot()), 1);
    }

    #[test]
    fn throttle_passes_phase_transitions() {
        let sink = Arc::new(CollectingSink::new());
        let reporter =
            TaskProgressReporter::new("t1", "Indexing", sink.clone() as Arc<dyn ProgressSink>);
        // Sem progress bar — sempre emite.
        reporter.report("Walking files…");
        reporter.report("Chunking files…");
        reporter.report("Embedding…");
        assert_eq!(count_emits(&sink.snapshot()), 3);
    }

    #[test]
    fn task_progress_reporter_is_progress_reporter() {
        // Garante compat com call sites que aceitam &dyn ProgressReporter.
        fn takes_reporter(r: &dyn ProgressReporter) {
            r.report("hello");
        }
        let sink = Arc::new(CollectingSink::new());
        let reporter = TaskProgressReporter::new("t1", "L", sink.clone() as Arc<dyn ProgressSink>);
        takes_reporter(&reporter);
        let events = sink.snapshot();
        assert!(matches!(&events[0], CollectedEvent::Emit { msg, .. } if msg == "hello"));
    }

    #[test]
    fn with_task_ok_marks_completed_and_finalizes() {
        let sink = Arc::new(CollectingSink::new());
        let result = with_task::<i32, std::io::Error>(
            TaskType::LocalWorkflow,
            "test op",
            "Test",
            None,
            sink.clone(),
            |reporter| {
                assert!(!reporter.task_id().is_empty());
                reporter.report("doing work");
                Ok(42)
            },
        );
        assert_eq!(result.unwrap(), 42);
        let events = sink.snapshot();
        let last = events.last().unwrap();
        assert!(matches!(
            last,
            CollectedEvent::Finalize {
                status: TaskStatus::Completed,
                ..
            }
        ));
    }

    #[test]
    fn with_task_err_marks_failed_with_summary() {
        let sink = Arc::new(CollectingSink::new());
        let result = with_task::<(), &str>(
            TaskType::LocalWorkflow,
            "test op",
            "Test",
            None,
            sink.clone(),
            |_| Err("boom"),
        );
        assert!(result.is_err());
        let events = sink.snapshot();
        let last = events.last().unwrap();
        match last {
            CollectedEvent::Finalize {
                status,
                summary,
                ..
            } => {
                assert_eq!(*status, TaskStatus::Failed);
                assert_eq!(summary.as_deref(), Some("boom"));
            }
            _ => panic!("expected Finalize, got {last:?}"),
        }
    }

    #[test]
    fn with_task_panic_marks_failed_and_rethrows() {
        let sink = Arc::new(CollectingSink::new());
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            with_task::<(), std::io::Error>(
                TaskType::LocalWorkflow,
                "test op",
                "Test",
                None,
                sink.clone(),
                |_| -> Result<(), std::io::Error> {
                    panic!("boom");
                },
            )
        }));
        assert!(result.is_err(), "panic should be re-thrown");
        let events = sink.snapshot();
        let last = events.last().unwrap();
        assert!(matches!(
            last,
            CollectedEvent::Finalize {
                status: TaskStatus::Failed,
                ..
            }
        ));
    }

    #[test]
    fn with_task_passes_parent_id_to_state() {
        let sink: Arc<dyn ProgressSink> = Arc::new(CollectingSink::new());
        let _ = with_task::<String, std::io::Error>(
            TaskType::LocalWorkflow,
            "child op",
            "Child",
            Some("parent-xyz".to_string()),
            sink,
            |reporter| {
                let registry = task_registry();
                let state = registry.get(reporter.task_id()).unwrap();
                assert_eq!(state.parent_id.as_deref(), Some("parent-xyz"));
                Ok(reporter.task_id().to_string())
            },
        );
    }

    #[test]
    fn set_default_sink_replaces_slot() {
        let custom = Arc::new(CollectingSink::new());
        set_default_sink(custom.clone());
        // Roda uma task com default_sink — eventos caem no `custom`.
        let _ = with_task_default::<(), std::io::Error>(
            TaskType::LocalWorkflow,
            "default sink test",
            "DST",
            None,
            |r| {
                r.report("hi");
                Ok(())
            },
        );
        let events = custom.snapshot();
        assert!(events.iter().any(|e| matches!(e, CollectedEvent::Emit { msg, .. } if msg == "hi")));
        // Restaura sink padrão para não vazar pros próximos testes.
        set_default_sink(build_default_sink());
    }
}
