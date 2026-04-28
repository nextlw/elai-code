use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use super::{TaskHandle, TaskState, TaskStatus, TaskType};

#[derive(Debug)]
pub enum TaskRegistryError {
    NotFound(String),
    AlreadyExists(String),
    NotTerminal(String),
}

impl std::fmt::Display for TaskRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "task not found: {id}"),
            Self::AlreadyExists(id) => write!(f, "task already exists: {id}"),
            Self::NotTerminal(id) => write!(f, "task is not in a terminal state: {id}"),
        }
    }
}

impl std::error::Error for TaskRegistryError {}

struct TaskEntry {
    state: TaskState,
    handle: Arc<Mutex<TaskHandle>>,
}

#[derive(Default)]
pub struct TaskRegistry {
    tasks: RwLock<HashMap<String, TaskEntry>>,
}

impl TaskRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra estado novo; retorna `Arc<Mutex<TaskHandle>>` para o caller spawnar.
    pub fn register(
        &self,
        state: TaskState,
    ) -> Result<Arc<Mutex<TaskHandle>>, TaskRegistryError> {
        let mut guard = self.tasks.write().unwrap();
        if guard.contains_key(&state.id) {
            return Err(TaskRegistryError::AlreadyExists(state.id.clone()));
        }
        let handle = Arc::new(Mutex::new(TaskHandle::new(state.id.clone())));
        guard.insert(
            state.id.clone(),
            TaskEntry {
                state,
                handle: handle.clone(),
            },
        );
        Ok(handle)
    }

    pub fn get(&self, task_id: &str) -> Option<TaskState> {
        self.tasks
            .read()
            .unwrap()
            .get(task_id)
            .map(|e| e.state.clone())
    }

    pub fn list_by_type(&self, task_type: TaskType) -> Vec<TaskState> {
        self.tasks
            .read()
            .unwrap()
            .values()
            .filter(|e| e.state.task_type == task_type)
            .map(|e| e.state.clone())
            .collect()
    }

    pub fn list_active(&self) -> Vec<TaskState> {
        self.tasks
            .read()
            .unwrap()
            .values()
            .filter(|e| !e.state.status.is_terminal())
            .map(|e| e.state.clone())
            .collect()
    }

    pub fn list_children(&self, parent_id: &str) -> Vec<TaskState> {
        self.tasks
            .read()
            .unwrap()
            .values()
            .filter(|e| e.state.parent_id.as_deref() == Some(parent_id))
            .map(|e| e.state.clone())
            .collect()
    }

    pub fn update_status(
        &self,
        task_id: &str,
        status: TaskStatus,
    ) -> Result<(), TaskRegistryError> {
        let mut guard = self.tasks.write().unwrap();
        let entry = guard
            .get_mut(task_id)
            .ok_or_else(|| TaskRegistryError::NotFound(task_id.to_string()))?;
        entry.state.status = status;
        if status.is_terminal() && entry.state.end_time_ms.is_none() {
            entry.state.end_time_ms = Some(super::now_ms());
        }
        Ok(())
    }

    pub fn append_output(&self, task_id: &str, bytes_added: u64) -> Result<(), TaskRegistryError> {
        let mut guard = self.tasks.write().unwrap();
        let entry = guard
            .get_mut(task_id)
            .ok_or_else(|| TaskRegistryError::NotFound(task_id.to_string()))?;
        entry.state.output_offset = entry.state.output_offset.saturating_add(bytes_added);
        Ok(())
    }

    /// Sinaliza abort para o handle; o caller é responsável por respeitar.
    pub fn kill(&self, task_id: &str) -> Result<(), TaskRegistryError> {
        let guard = self.tasks.read().unwrap();
        let entry = guard
            .get(task_id)
            .ok_or_else(|| TaskRegistryError::NotFound(task_id.to_string()))?;
        entry.handle.lock().unwrap().signal_abort();
        Ok(())
    }

    /// Remove apenas tasks em estado terminal.
    pub fn evict(&self, task_id: &str) -> Result<TaskState, TaskRegistryError> {
        let mut guard = self.tasks.write().unwrap();
        let entry = guard
            .get(task_id)
            .ok_or_else(|| TaskRegistryError::NotFound(task_id.to_string()))?;
        if !entry.state.status.is_terminal() {
            return Err(TaskRegistryError::NotTerminal(task_id.to_string()));
        }
        Ok(guard.remove(task_id).unwrap().state)
    }

    pub fn count(&self) -> usize {
        self.tasks.read().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{generate_task_id, TaskType};

    fn make_state(task_type: TaskType) -> TaskState {
        let id = generate_task_id(task_type);
        TaskState::new(id, task_type, "test task".to_string(), None)
    }

    #[test]
    fn register_then_get_returns_state() {
        let reg = TaskRegistry::new();
        let state = make_state(TaskType::LocalBash);
        let id = state.id.clone();
        reg.register(state).unwrap();
        let got = reg.get(&id).unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.task_type, TaskType::LocalBash);
        assert_eq!(got.status, TaskStatus::Pending);
    }

    #[test]
    fn register_duplicate_errors() {
        let reg = TaskRegistry::new();
        let state = make_state(TaskType::LocalBash);
        let id = state.id.clone();
        reg.register(state).unwrap();
        // Build a second state with the same id then override id to guarantee duplicate
        let mut dup = TaskState::new(id.clone(), TaskType::LocalBash, "dup".to_string(), None);
        dup.id = id;
        let err = reg.register(dup).unwrap_err();
        assert!(matches!(err, TaskRegistryError::AlreadyExists(_)));
    }

    #[test]
    fn update_status_to_completed_sets_end_time() {
        let reg = TaskRegistry::new();
        let state = make_state(TaskType::Dream);
        let id = state.id.clone();
        reg.register(state).unwrap();
        assert!(reg.get(&id).unwrap().end_time_ms.is_none());
        reg.update_status(&id, TaskStatus::Completed).unwrap();
        let got = reg.get(&id).unwrap();
        assert_eq!(got.status, TaskStatus::Completed);
        assert!(got.end_time_ms.is_some());
    }

    #[test]
    fn kill_signals_abort_in_handle() {
        let reg = TaskRegistry::new();
        let state = make_state(TaskType::LocalBash);
        let id = state.id.clone();
        let handle_arc = reg.register(state).unwrap();
        assert!(!handle_arc.lock().unwrap().is_aborted());
        reg.kill(&id).unwrap();
        assert!(handle_arc.lock().unwrap().is_aborted());
    }

    #[test]
    fn evict_running_task_errors_with_not_terminal() {
        let reg = TaskRegistry::new();
        let state = make_state(TaskType::LocalBash);
        let id = state.id.clone();
        reg.register(state).unwrap();
        reg.update_status(&id, TaskStatus::Running).unwrap();
        let err = reg.evict(&id).unwrap_err();
        assert!(matches!(err, TaskRegistryError::NotTerminal(_)));
    }

    #[test]
    fn evict_completed_task_succeeds() {
        let reg = TaskRegistry::new();
        let state = make_state(TaskType::LocalBash);
        let id = state.id.clone();
        reg.register(state).unwrap();
        reg.update_status(&id, TaskStatus::Completed).unwrap();
        let evicted = reg.evict(&id).unwrap();
        assert_eq!(evicted.id, id);
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn list_by_type_filters_correctly() {
        let reg = TaskRegistry::new();
        let bash1 = make_state(TaskType::LocalBash);
        let bash2 = make_state(TaskType::LocalBash);
        let dream = make_state(TaskType::Dream);
        reg.register(bash1).unwrap();
        reg.register(bash2).unwrap();
        reg.register(dream).unwrap();
        let bash_list = reg.list_by_type(TaskType::LocalBash);
        assert_eq!(bash_list.len(), 2);
        let dream_list = reg.list_by_type(TaskType::Dream);
        assert_eq!(dream_list.len(), 1);
    }

    #[test]
    fn list_active_excludes_terminal() {
        let reg = TaskRegistry::new();
        let active = make_state(TaskType::LocalBash);
        let done = make_state(TaskType::Dream);
        let active_id = active.id.clone();
        let done_id = done.id.clone();
        reg.register(active).unwrap();
        reg.register(done).unwrap();
        reg.update_status(&active_id, TaskStatus::Running).unwrap();
        reg.update_status(&done_id, TaskStatus::Completed).unwrap();
        let actives = reg.list_active();
        assert_eq!(actives.len(), 1);
        assert_eq!(actives[0].id, active_id);
    }
}
