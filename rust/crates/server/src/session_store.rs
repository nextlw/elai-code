use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use runtime::{PermissionMode, Session as RuntimeSession, TokenUsage};
use tokio::sync::{broadcast, oneshot, Mutex, RwLock};

use crate::streaming::ServerEvent;

const BROADCAST_CAPACITY: usize = 256;

pub type SessionId = String;

/// In-memory store. Structured so a SQLite-backed impl can replace it later.
#[derive(Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<SessionId, Arc<SessionData>>>>,
}

impl SessionStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn insert(&self, session: Arc<SessionData>) {
        self.inner.write().await.insert(session.id.clone(), session);
    }

    pub async fn get(&self, id: &str) -> Option<Arc<SessionData>> {
        self.inner.read().await.get(id).cloned()
    }

    pub async fn remove(&self, id: &str) -> Option<Arc<SessionData>> {
        self.inner.write().await.remove(id)
    }

    pub async fn list(&self) -> Vec<Arc<SessionData>> {
        let mut sessions: Vec<_> = self.inner.read().await.values().cloned().collect();
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        sessions
    }
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SessionData {
    pub id: SessionId,
    pub created_at: u64,
    pub cwd: String,
    /// Mutable runtime fields.
    pub runtime_state: Mutex<RuntimeSessionState>,
    pub events: broadcast::Sender<ServerEvent>,
    /// Buffer of past events for `?since=N` reconnection support.
    pub event_history: RwLock<Vec<ServerEvent>>,
    pub seq_counter: AtomicU64,
    pub pending_permissions: Mutex<HashMap<String, PendingPermission>>,
    pub current_turn: Mutex<Option<TurnHandle>>,
}

pub struct RuntimeSessionState {
    pub conversation: RuntimeSession,
    pub model: String,
    pub permission_mode: PermissionMode,
    pub usage: TokenUsage,
    pub allow_patterns: Vec<String>,
    pub deny_patterns: Vec<String>,
}

pub struct PendingPermission {
    pub tool_name: String,
    pub input: String,
    pub required_mode: PermissionMode,
    pub responder: oneshot::Sender<PermissionDecisionPayload>,
}

#[derive(Debug, Clone)]
pub enum PermissionDecisionPayload {
    Allow,
    Deny { reason: String },
}

pub struct TurnHandle {
    pub turn_id: String,
    pub cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl SessionData {
    #[must_use]
    pub fn new(id: SessionId, cwd: String, model: String, permission_mode: PermissionMode) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            id,
            created_at: now_millis(),
            cwd,
            runtime_state: Mutex::new(RuntimeSessionState {
                conversation: RuntimeSession::new(),
                model,
                permission_mode,
                usage: TokenUsage::default(),
                allow_patterns: Vec::new(),
                deny_patterns: Vec::new(),
            }),
            events,
            event_history: RwLock::new(Vec::new()),
            seq_counter: AtomicU64::new(1),
            pending_permissions: Mutex::new(HashMap::new()),
            current_turn: Mutex::new(None),
        }
    }

    pub fn next_seq(&self) -> u64 {
        self.seq_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Append to history and broadcast. Receivers that lag silently lose events
    /// but can recover via `?since=N` against `event_history`.
    pub async fn publish(&self, event: ServerEvent) {
        self.event_history.write().await.push(event.clone());
        let _ = self.events.send(event);
    }
}

#[must_use]
pub fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}
