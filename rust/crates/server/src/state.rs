use std::collections::HashMap;
use std::sync::Arc;

use runtime::{McpServerManager, ResponseCache};
use tokio::sync::{broadcast, Mutex, RwLock};
use uuid::Uuid;

use crate::auth::{jwks::JwkSet, AuthState};
use crate::db::PgPool;
use crate::session_store::SessionStore;

pub type ConversationChannels = Arc<RwLock<HashMap<Uuid, broadcast::Sender<String>>>>;

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
    pub db: PgPool,
    pub jwks: Arc<RwLock<JwkSet>>,
    pub clerk_webhook_secret: String,
    conversation_channels: ConversationChannels,
}

impl AppState {
    #[must_use]
    pub fn new(
        token: String,
        mcp: McpServerManager,
        db: PgPool,
        jwks: JwkSet,
        clerk_webhook_secret: String,
    ) -> Self {
        Self {
            sessions: SessionStore::new(),
            auth: AuthState::new(token),
            version: Arc::new(env!("CARGO_PKG_VERSION").to_string()),
            mcp: Arc::new(Mutex::new(mcp)),
            response_cache: Arc::new(Mutex::new(ResponseCache::disabled())),
            oauth_pending: Arc::new(Mutex::new(HashMap::new())),
            db,
            jwks: Arc::new(RwLock::new(jwks)),
            clerk_webhook_secret,
            conversation_channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get_or_create_channel(&self, conversation_id: Uuid) -> broadcast::Sender<String> {
        {
            let channels = self.conversation_channels.read().await;
            if let Some(sender) = channels.get(&conversation_id) {
                return sender.clone();
            }
        }

        let mut channels = self.conversation_channels.write().await;
        channels
            .entry(conversation_id)
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    }
}
