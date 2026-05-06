use std::sync::Arc;

use runtime::{PermissionMode, PermissionPromptDecision, PermissionPrompter, PermissionRequest};
use tokio::sync::oneshot;

use crate::db::{PendingPermission, PermissionDecisionPayload, SessionData};
use crate::streaming::ServerEvent;

/// Sync prompter that emits an SSE `permission_request` event and blocks waiting
/// for the HTTP `decide` endpoint to deliver an answer via a oneshot channel.
///
/// Must be invoked from inside a `spawn_blocking` task — uses `Handle::current().block_on`.
pub struct HttpPermissionPrompter {
    pub session: Arc<SessionData>,
    pub turn_id: String,
}

impl PermissionPrompter for HttpPermissionPrompter {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => {
                return PermissionPromptDecision::Deny {
                    reason: "no async runtime available".to_string(),
                }
            }
        };
        let request_id = ulid::Ulid::new().to_string();
        let (tx, rx) = oneshot::channel::<PermissionDecisionPayload>();

        let session = Arc::clone(&self.session);
        let req_id = request_id.clone();
        let tool_name = request.tool_name.clone();
        let input = request.input.clone();
        let required_mode = request.required_mode;

        handle.block_on(async {
            session.pending_permissions.lock().await.insert(
                req_id.clone(),
                PendingPermission {
                    tool_name: tool_name.clone(),
                    input: input.clone(),
                    required_mode,
                    responder: tx,
                },
            );
            let seq = session.next_seq();
            session
                .publish(ServerEvent::PermissionRequest {
                    seq,
                    session_id: session.id.clone(),
                    request_id: req_id,
                    tool_name,
                    input,
                    required_mode: mode_label(required_mode).to_string(),
                })
                .await;
        });

        match handle.block_on(rx) {
            Ok(PermissionDecisionPayload::Allow) => PermissionPromptDecision::Allow,
            Ok(PermissionDecisionPayload::Deny { reason }) => {
                PermissionPromptDecision::Deny { reason }
            }
            Err(_) => PermissionPromptDecision::Deny {
                reason: "permission request cancelled".to_string(),
            },
        }
    }
}

#[must_use]
pub fn mode_label(mode: PermissionMode) -> &'static str {
    mode.as_str()
}
