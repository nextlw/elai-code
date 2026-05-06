use std::sync::Arc;

use crate::auth::AuthState;
use crate::db::SessionStore;

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionStore,
    pub auth: AuthState,
    pub version: Arc<String>,
}

impl AppState {
    #[must_use]
    pub fn new(token: String) -> Self {
        Self {
            sessions: SessionStore::new(),
            auth: AuthState::new(token),
            version: Arc::new(env!("CARGO_PKG_VERSION").to_string()),
        }
    }
}
