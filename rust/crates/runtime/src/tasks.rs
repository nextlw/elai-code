//! Background task system — porte do contrato Claude Code (Task.ts/tasks.ts).
//!
//! Cada tarefa async tem id determinístico (prefix por `TaskType` + 8 chars),
//! status track-able (pending → running → completed|failed|killed), output
//! buffer em arquivo, e mecanismo de abort/cleanup.

pub mod output;
pub mod progress;
pub mod registry;
pub mod sinks;

pub use output::{task_output_path, TaskOutputWriter};
pub use progress::{
    default_sink, set_default_sink, with_task, with_task_default, TaskProgressReporter,
};
pub use registry::{TaskRegistry, TaskRegistryError};
pub use sinks::{
    CollectedEvent, CollectingSink, LiveStderrSink, NoopSink, PlainStderrSink, ProgressSink,
};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Process-wide singleton da `TaskRegistry`. Inicializada lazy na primeira
/// chamada de `task_registry()`. Permite que ferramentas (bash, agent,
/// dream, etc.) registrem tasks sem ter que passar a registry como argumento
/// por toda a árvore de chamadas.
static GLOBAL_REGISTRY: OnceLock<Arc<TaskRegistry>> = OnceLock::new();

/// Retorna a `TaskRegistry` global, criando-a no primeiro acesso.
#[must_use]
pub fn task_registry() -> Arc<TaskRegistry> {
    GLOBAL_REGISTRY
        .get_or_init(|| Arc::new(TaskRegistry::new()))
        .clone()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    LocalBash,
    LocalAgent,
    RemoteAgent,
    InProcessTeammate,
    LocalWorkflow,
    MonitorMcp,
    Dream,
}

impl TaskType {
    #[must_use]
    pub const fn id_prefix(self) -> char {
        match self {
            Self::LocalBash => 'b',
            Self::LocalAgent => 'a',
            Self::RemoteAgent => 'r',
            Self::InProcessTeammate => 't',
            Self::LocalWorkflow => 'w',
            Self::MonitorMcp => 'm',
            Self::Dream => 'd',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Killed,
}

impl TaskStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Killed)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskState {
    pub id: String,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub description: String,
    pub tool_use_id: Option<String>,
    /// Sub-task aponta pra task pai (`None` = raiz). Permite que ferramentas
    /// listem a árvore de progresso (ex: download de modelo embaixo de
    /// indexação) sem inventar mecanismos paralelos.
    #[serde(default)]
    pub parent_id: Option<String>,
    pub start_time_ms: u64,
    pub end_time_ms: Option<u64>,
    pub total_paused_ms: u64,
    pub output_file: std::path::PathBuf,
    pub output_offset: u64,
    pub notified: bool,
}

impl TaskState {
    #[must_use]
    pub fn new(
        id: String,
        task_type: TaskType,
        description: String,
        tool_use_id: Option<String>,
    ) -> Self {
        Self::new_with_parent(id, task_type, description, tool_use_id, None)
    }

    #[must_use]
    pub fn new_with_parent(
        id: String,
        task_type: TaskType,
        description: String,
        tool_use_id: Option<String>,
        parent_id: Option<String>,
    ) -> Self {
        let output_file = task_output_path(&id);
        Self {
            id,
            task_type,
            status: TaskStatus::Pending,
            description,
            tool_use_id,
            parent_id,
            start_time_ms: now_ms(),
            end_time_ms: None,
            total_paused_ms: 0,
            output_file,
            output_offset: 0,
            notified: false,
        }
    }
}

pub struct TaskHandle {
    pub task_id: String,
    pub abort: Arc<AtomicBool>,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}

impl TaskHandle {
    #[must_use]
    pub fn new(task_id: String) -> Self {
        Self {
            task_id,
            abort: Arc::new(AtomicBool::new(false)),
            cleanup: None,
        }
    }

    #[must_use]
    pub fn with_cleanup(mut self, f: impl FnOnce() + Send + 'static) -> Self {
        self.cleanup = Some(Box::new(f));
        self
    }

    pub fn signal_abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.abort.load(Ordering::SeqCst)
    }

    pub fn run_cleanup(&mut self) {
        if let Some(f) = self.cleanup.take() {
            f();
        }
    }
}

impl std::fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskHandle")
            .field("task_id", &self.task_id)
            .field("aborted", &self.is_aborted())
            .finish_non_exhaustive()
    }
}

/// Gera ID com prefix do `TaskType` + 8 chars alfanuméricos lowercase.
/// 36^8 ≈ 2.8 trilhões de combinações.
#[must_use]
pub fn generate_task_id(task_type: TaskType) -> String {
    use std::fs::File;
    use std::io::Read;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let prefix = task_type.id_prefix();
    let mut buf = [0_u8; 8];
    #[cfg(unix)]
    {
        let _ = File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf));
    }
    #[cfg(not(unix))]
    {
        let seed = now_ms();
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = (seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407)
                >> (i * 8)) as u8;
        }
    }
    let mut id = String::with_capacity(9);
    id.push(prefix);
    for b in &buf {
        id.push(ALPHABET[(*b as usize) % ALPHABET.len()] as char);
    }
    id
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn task_status_terminal_check() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Killed.is_terminal());
    }

    #[test]
    fn generate_task_id_has_prefix_and_length() {
        let cases = [
            (TaskType::LocalBash, 'b'),
            (TaskType::LocalAgent, 'a'),
            (TaskType::RemoteAgent, 'r'),
            (TaskType::InProcessTeammate, 't'),
            (TaskType::LocalWorkflow, 'w'),
            (TaskType::MonitorMcp, 'm'),
            (TaskType::Dream, 'd'),
        ];
        for (task_type, expected_prefix) in cases {
            let id = generate_task_id(task_type);
            assert_eq!(id.len(), 9, "id={id} should be 9 chars");
            assert_eq!(
                id.chars().next().unwrap(),
                expected_prefix,
                "id={id} wrong prefix"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_alphanumeric()),
                "id={id} contains non-alphanumeric"
            );
        }
    }

    #[test]
    fn generate_task_id_is_unique() {
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            let id = generate_task_id(TaskType::LocalBash);
            assert!(seen.insert(id.clone()), "duplicate id: {id}");
        }
    }

    #[test]
    fn task_registry_singleton_is_shared() {
        // Duas chamadas retornam Arc apontando para a mesma instância.
        let r1 = task_registry();
        let r2 = task_registry();
        assert!(Arc::ptr_eq(&r1, &r2), "singleton must return same Arc instance");

        // Estado é compartilhado: registrar via uma alça é visível na outra.
        let state = TaskState::new(
            generate_task_id(TaskType::LocalBash),
            TaskType::LocalBash,
            "singleton test".to_string(),
            None,
        );
        let id = state.id.clone();
        let _ = r1.register(state);
        assert!(r2.get(&id).is_some(), "registered task should be visible via second handle");
    }
}
