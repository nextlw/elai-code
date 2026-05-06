use std::collections::HashMap;
use std::sync::Arc;

use runtime::{McpServerManager, ResponseCache};
use tokio::sync::Mutex;

use crate::auth::AuthState;
use crate::session_store::SessionStore;

/// OAuth PKCE state stored between /oauth/start and /oauth/callback.
pub struct OAuthPendingState {
    pub pkce: runtime::PkceCodePair,
    pub redirect_uri: String,
    pub provider: String,
    pub created_at: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionStore,
    pub auth: AuthState,
    pub version: Arc<String>,
    pub mcp: Arc<Mutex<McpServerManager>>,
    pub response_cache: Arc<Mutex<ResponseCache>>,
    pub oauth_pending: Arc<Mutex<HashMap<String, OAuthPendingState>>>,
}

impl AppState {
    #[must_use]
    pub fn new(token: String, mcp: McpServerManager) -> Self {
        Self {
            sessions: SessionStore::new(),
            auth: AuthState::new(token),
            version: Arc::new(env!("CARGO_PKG_VERSION").to_string()),
            mcp: Arc::new(Mutex::new(mcp)),
            response_cache: Arc::new(Mutex::new(ResponseCache::disabled())),
            oauth_pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
