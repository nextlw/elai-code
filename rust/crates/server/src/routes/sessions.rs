use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_stream::stream;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use runtime::{
    compact_session, CompactionConfig, ContentBlock, ConversationMessage, ConversationRuntime,
    MessageRole, PermissionMode, PermissionPolicy, TurnSummary,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tools::GlobalToolRegistry;

use crate::db::{PermissionDecisionPayload, SessionData, TurnHandle};
use crate::permission_bridge::{mode_label, HttpPermissionPrompter};
use crate::runtime_bridge::{ServerApiClient, ServerToolExecutor};
use crate::state::AppState;
use crate::streaming::{ServerEvent, SessionSnapshot};

#[derive(Debug, Serialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}

pub type ApiError = (StatusCode, Json<ApiErrorBody>);

pub fn api_error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(ApiErrorBody {
            error: ApiErrorDetail {
                code: code.to_string(),
                message: message.into(),
            },
        }),
    )
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub session: SessionSnapshot,
}

#[derive(Debug, Serialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionSnapshot>,
}

#[derive(Debug, Serialize)]
pub struct SessionDetailsResponse {
    pub session: SessionSnapshot,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Deserialize)]
pub struct PatchSessionRequest {
    pub model: Option<String>,
    pub permission_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub turn_id: String,
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub since: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct CostResponse {
    pub total_tokens: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub estimated_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct PendingPermissionInfo {
    pub request_id: String,
    pub tool_name: String,
    pub input: String,
    pub required_mode: String,
}

#[derive(Debug, Serialize)]
pub struct PendingPermissionsResponse {
    pub pending: Vec<PendingPermissionInfo>,
}

#[derive(Debug, Deserialize)]
pub struct DecideRequest {
    pub outcome: String,
    pub reason: Option<String>,
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), ApiError> {
    let id = ulid::Ulid::new().to_string();
    let cwd = payload.cwd.unwrap_or_else(|| ".".to_string());
    let model = payload
        .model
        .unwrap_or_else(|| api::suggested_default_model().to_string());
    let permission_mode = parse_permission_mode(payload.permission_mode.as_deref())
        .unwrap_or(PermissionMode::WorkspaceWrite);

    let session = Arc::new(SessionData::new(
        id.clone(),
        cwd.clone(),
        model.clone(),
        permission_mode,
    ));
    state.sessions.insert(Arc::clone(&session)).await;

    let snapshot = snapshot_for(&session).await;
    Ok((
        StatusCode::CREATED,
        Json(CreateSessionResponse {
            session_id: id,
            session: snapshot,
        }),
    ))
}

pub async fn list_sessions(State(state): State<AppState>) -> Json<ListSessionsResponse> {
    let mut snapshots = Vec::new();
    for s in state.sessions.list().await {
        snapshots.push(snapshot_for(&s).await);
    }
    Json(ListSessionsResponse { sessions: snapshots })
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionDetailsResponse>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let snapshot = snapshot_for(&session).await;
    let messages = session.runtime_state.lock().await.conversation.messages.clone();
    Ok(Json(SessionDetailsResponse { session: snapshot, messages }))
}

pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .sessions
        .remove(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn patch_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<PatchSessionRequest>,
) -> Result<Json<SessionSnapshot>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    {
        let mut state_guard = session.runtime_state.lock().await;
        if let Some(model) = payload.model {
            state_guard.model = model;
        }
        if let Some(mode_str) = payload.permission_mode {
            let mode = parse_permission_mode(Some(mode_str.as_str())).ok_or_else(|| {
                api_error(StatusCode::BAD_REQUEST, "invalid_mode", "unknown permission mode")
            })?;
            state_guard.permission_mode = mode;
        }
    }
    Ok(Json(snapshot_for(&session).await))
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<SendMessageResponse>), ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let turn_id = ulid::Ulid::new().to_string();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    {
        let mut current = session.current_turn.lock().await;
        if current.is_some() {
            return Err(api_error(
                StatusCode::CONFLICT,
                "turn_in_progress",
                "another turn is already running for this session",
            ));
        }
        *current = Some(TurnHandle {
            turn_id: turn_id.clone(),
            cancel_flag: Arc::clone(&cancel_flag),
        });
    }

    // Snapshot inputs for the spawn_blocking task.
    let (model, permission_mode, prior_session) = {
        let guard = session.runtime_state.lock().await;
        (
            guard.model.clone(),
            guard.permission_mode,
            guard.conversation.clone(),
        )
    };

    let seq = session.next_seq();
    session
        .publish(ServerEvent::TurnStarted {
            seq,
            session_id: session.id.clone(),
            turn_id: turn_id.clone(),
        })
        .await;

    let user_input = payload.content.clone();
    let session_for_task = Arc::clone(&session);
    let turn_id_for_task = turn_id.clone();

    tokio::spawn(async move {
        run_turn_task(
            session_for_task,
            turn_id_for_task,
            user_input,
            model,
            permission_mode,
            prior_session,
        )
        .await;
    });

    Ok((StatusCode::ACCEPTED, Json(SendMessageResponse { turn_id })))
}

#[allow(clippy::too_many_lines)]
async fn run_turn_task(
    session: Arc<SessionData>,
    turn_id: String,
    user_input: String,
    model: String,
    permission_mode: PermissionMode,
    prior_session: runtime::Session,
) {
    // run_turn is sync: hop to a blocking thread but stay inside the tokio runtime
    // so HttpPermissionPrompter can use Handle::current().
    let session_for_block = Arc::clone(&session);
    let turn_id_for_block = turn_id.clone();
    let result: Result<TurnSummary, String> = tokio::task::spawn_blocking(move || {
        let registry = GlobalToolRegistry::builtin();
        let api_client = ServerApiClient::new(model.clone(), registry.clone())
            .map_err(|e| format!("failed to init api client: {e}"))?;
        let tool_executor = ServerToolExecutor::new(registry);

        let mut policy = PermissionPolicy::new(permission_mode);
        // Default: bash + write tools require danger-full-access; so prompt will fire under
        // workspace-write. (Read tools default to DangerFullAccess too in current API,
        // matching the runtime test expectations.)
        for tool_name in ["bash", "write_file", "edit_file"] {
            policy = policy.with_tool_requirement(tool_name, PermissionMode::DangerFullAccess);
        }

        let mut runtime_obj = ConversationRuntime::new(
            prior_session,
            api_client,
            tool_executor,
            policy,
            Vec::new(),
        )
        .with_model_name(model);

        let mut prompter = HttpPermissionPrompter {
            session: Arc::clone(&session_for_block),
            turn_id: turn_id_for_block.clone(),
        };

        let result = runtime_obj
            .run_turn(user_input, Some(&mut prompter))
            .map_err(|e| e.to_string());
        if result.is_ok() {
            // Persist updated session messages back via a side channel.
            let updated = runtime_obj.session().clone();
            stash_updated_session(&session_for_block, updated);
        }
        result
    })
    .await
    .unwrap_or_else(|join_err| Err(format!("turn task panicked: {join_err}")));

    // Replay summary as SSE events.
    match result {
        Ok(summary) => {
            for msg in &summary.assistant_messages {
                for block in &msg.blocks {
                    match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            let seq = session.next_seq();
                            session
                                .publish(ServerEvent::TextDelta {
                                    seq,
                                    session_id: session.id.clone(),
                                    turn_id: turn_id.clone(),
                                    text: text.clone(),
                                })
                                .await;
                        }
                        ContentBlock::Thinking { thinking } if !thinking.is_empty() => {
                            let seq = session.next_seq();
                            session
                                .publish(ServerEvent::ThinkingDelta {
                                    seq,
                                    session_id: session.id.clone(),
                                    turn_id: turn_id.clone(),
                                    thinking: thinking.clone(),
                                })
                                .await;
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let seq = session.next_seq();
                            session
                                .publish(ServerEvent::ToolUseStarted {
                                    seq,
                                    session_id: session.id.clone(),
                                    turn_id: turn_id.clone(),
                                    tool_call_id: id.clone(),
                                    tool_name: name.clone(),
                                })
                                .await;
                            let seq = session.next_seq();
                            session
                                .publish(ServerEvent::ToolUseInputDelta {
                                    seq,
                                    session_id: session.id.clone(),
                                    turn_id: turn_id.clone(),
                                    tool_call_id: id.clone(),
                                    partial_json: input.clone(),
                                })
                                .await;
                        }
                        _ => {}
                    }
                }
                let summary_text = summarize_message(msg);
                let seq = session.next_seq();
                session
                    .publish(ServerEvent::MessageAppended {
                        seq,
                        session_id: session.id.clone(),
                        role: role_label(msg.role).to_string(),
                        text_summary: summary_text,
                    })
                    .await;
            }
            for msg in &summary.tool_results {
                for block in &msg.blocks {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } = block
                    {
                        let seq = session.next_seq();
                        session
                            .publish(ServerEvent::ToolResult {
                                seq,
                                session_id: session.id.clone(),
                                turn_id: turn_id.clone(),
                                tool_call_id: tool_use_id.clone(),
                                output: output.clone(),
                                is_error: *is_error,
                            })
                            .await;
                    }
                }
            }
            let seq = session.next_seq();
            session
                .publish(ServerEvent::UsageDelta {
                    seq,
                    session_id: session.id.clone(),
                    turn_id: turn_id.clone(),
                    input_tokens: summary.usage.input_tokens,
                    output_tokens: summary.usage.output_tokens,
                })
                .await;
            // Update stored usage.
            {
                let mut guard = session.runtime_state.lock().await;
                guard.usage.input_tokens = guard.usage.input_tokens.saturating_add(summary.usage.input_tokens);
                guard.usage.output_tokens = guard.usage.output_tokens.saturating_add(summary.usage.output_tokens);
            }
            let seq = session.next_seq();
            session
                .publish(ServerEvent::TurnCompleted {
                    seq,
                    session_id: session.id.clone(),
                    turn_id: turn_id.clone(),
                })
                .await;
        }
        Err(error) => {
            let seq = session.next_seq();
            session
                .publish(ServerEvent::TurnError {
                    seq,
                    session_id: session.id.clone(),
                    turn_id: turn_id.clone(),
                    error,
                })
                .await;
        }
    }

    *session.current_turn.lock().await = None;
}

/// Side channel: write the updated runtime Session back into shared state.
///
/// Used from inside `spawn_blocking` where we cannot await the async Mutex.
fn stash_updated_session(session: &Arc<SessionData>, updated: runtime::Session) {
    // Use blocking_lock — safe because we are inside spawn_blocking.
    let mut guard = session.runtime_state.blocking_lock();
    guard.conversation = updated;
}

pub async fn cancel_turn(
    State(state): State<AppState>,
    Path((id, turn_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let mut current = session.current_turn.lock().await;
    match current.as_ref() {
        Some(handle) if handle.turn_id == turn_id => {
            handle.cancel_flag.store(true, Ordering::Relaxed);
            *current = None;
            drop(current);
            let seq = session.next_seq();
            session
                .publish(ServerEvent::TurnCancelled {
                    seq,
                    session_id: session.id.clone(),
                    turn_id,
                })
                .await;
            Ok(StatusCode::NO_CONTENT)
        }
        _ => Err(api_error(
            StatusCode::NOT_FOUND,
            "turn_not_found",
            "no active turn with that id",
        )),
    }
}

pub async fn stream_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<EventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let since = query.since.unwrap_or(0);
    let backlog: Vec<ServerEvent> = session
        .event_history
        .read()
        .await
        .iter()
        .filter(|e| e.seq() >= since)
        .cloned()
        .collect();
    let snapshot_event = ServerEvent::Snapshot {
        seq: 0,
        session_id: session.id.clone(),
        session: snapshot_for(&session).await,
    };
    let mut receiver = session.events.subscribe();

    let stream = stream! {
        if let Ok(event) = to_sse(&snapshot_event) {
            yield Ok::<Event, Infallible>(event);
        }
        for event in backlog {
            if let Ok(sse) = to_sse(&event) {
                yield Ok::<Event, Infallible>(sse);
            }
        }
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if event.seq() < since {
                        continue;
                    }
                    if let Ok(sse) = to_sse(&event) {
                        yield Ok::<Event, Infallible>(sse);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

pub async fn get_cost(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CostResponse>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let guard = session.runtime_state.lock().await;
    let input = guard.usage.input_tokens;
    let output = guard.usage.output_tokens;
    Ok(Json(CostResponse {
        total_tokens: input.saturating_add(output),
        input_tokens: input,
        output_tokens: output,
        estimated_usd: estimate_usd(&guard.model, input, output),
    }))
}

pub async fn list_pending_permissions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PendingPermissionsResponse>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;
    let guard = session.pending_permissions.lock().await;
    let pending = guard
        .iter()
        .map(|(req_id, p)| PendingPermissionInfo {
            request_id: req_id.clone(),
            tool_name: p.tool_name.clone(),
            input: p.input.clone(),
            required_mode: mode_label(p.required_mode).to_string(),
        })
        .collect();
    Ok(Json(PendingPermissionsResponse { pending }))
}

pub async fn decide_permission(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(payload): Json<DecideRequest>,
) -> Result<StatusCode, ApiError> {
    // Find session that owns this request_id.
    for session in state.sessions.list().await {
        let mut guard = session.pending_permissions.lock().await;
        if let Some(pending) = guard.remove(&request_id) {
            let payload_decision = match payload.outcome.as_str() {
                "allow" => PermissionDecisionPayload::Allow,
                "deny" => PermissionDecisionPayload::Deny {
                    reason: payload.reason.unwrap_or_else(|| "denied".to_string()),
                },
                _ => {
                    // Restore so the caller can retry with a valid outcome.
                    guard.insert(request_id.clone(), pending);
                    return Err(api_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_outcome",
                        "outcome must be 'allow' or 'deny'",
                    ));
                }
            };
            let _ = pending.responder.send(payload_decision);
            return Ok(StatusCode::NO_CONTENT);
        }
    }
    Err(api_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "permission request not found",
    ))
}

// ── clone / compact / export / resume ───────────────────────────────

#[derive(Debug, Serialize)]
pub struct CloneSessionResponse {
    pub session_id: String,
    pub session: SessionSnapshot,
}

pub async fn clone_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<CloneSessionResponse>), ApiError> {
    let source = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let new_id = ulid::Ulid::new().to_string();
    let (cwd, model, permission_mode, conversation) = {
        let guard = source.runtime_state.lock().await;
        (
            source.cwd.clone(),
            guard.model.clone(),
            guard.permission_mode,
            guard.conversation.clone(),
        )
    };

    let new_session = Arc::new(crate::db::SessionData::new(
        new_id.clone(),
        cwd,
        model,
        permission_mode,
    ));
    {
        let mut guard = new_session.runtime_state.lock().await;
        guard.conversation = conversation;
    }
    state.sessions.insert(Arc::clone(&new_session)).await;

    let snapshot = snapshot_for(&new_session).await;
    Ok((
        StatusCode::CREATED,
        Json(CloneSessionResponse {
            session_id: new_id,
            session: snapshot,
        }),
    ))
}

#[derive(Debug, Serialize)]
pub struct CompactSessionResponse {
    pub status: String,
    pub removed_message_count: usize,
}

pub async fn compact_session_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CompactSessionResponse>, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let (conversation, model) = {
        let guard = session.runtime_state.lock().await;
        (guard.conversation.clone(), guard.model.clone())
    };

    let config = CompactionConfig::for_model(&model);
    let result = compact_session(&conversation, config);

    {
        let mut guard = session.runtime_state.lock().await;
        guard.conversation = result.compacted_session;
    }

    Ok(Json(CompactSessionResponse {
        status: "ok".to_string(),
        removed_message_count: result.removed_message_count,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>,
}

pub async fn export_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    let messages = session.runtime_state.lock().await.conversation.messages.clone();
    let format = query.format.unwrap_or_else(|| "json".to_string());

    match format.as_str() {
        "json" => {
            let body = serde_json::to_string_pretty(&messages).unwrap_or_default();
            Ok(axum::response::Response::builder()
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body))
                .unwrap())
        }
        "md" => {
            let mut md = format!("# Session {}\n\n", id);
            for msg in &messages {
                let role = role_label(msg.role);
                md.push_str(&format!("## {role}\n\n"));
                for block in &msg.blocks {
                    match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            md.push_str(text);
                            md.push_str("\n\n");
                        }
                        ContentBlock::Thinking { thinking } if !thinking.is_empty() => {
                            md.push_str(&format!("*Thinking:* {thinking}\n\n"));
                        }
                        _ => {}
                    }
                }
            }
            Ok(axum::response::Response::builder()
                .header("content-type", "text/markdown; charset=utf-8")
                .body(axum::body::Body::from(md))
                .unwrap())
        }
        other => Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_format",
            format!("unsupported format: {other}; use json or md"),
        )),
    }
}

#[derive(Debug, Serialize)]
pub struct ResumeSessionResponse {
    pub status: String,
    pub session_id: String,
}

pub async fn resume_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ResumeSessionResponse>, ApiError> {
    state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "session not found"))?;

    Ok(Json(ResumeSessionResponse {
        status: "ok".to_string(),
        session_id: id,
    }))
}

// ── helpers ─────────────────────────────────────────────────────────

async fn snapshot_for(session: &Arc<SessionData>) -> SessionSnapshot {
    let guard = session.runtime_state.lock().await;
    SessionSnapshot {
        id: session.id.clone(),
        created_at: session.created_at,
        model: guard.model.clone(),
        permission_mode: mode_label(guard.permission_mode).to_string(),
        cwd: session.cwd.clone(),
        message_count: guard.conversation.messages.len(),
    }
}

fn parse_permission_mode(value: Option<&str>) -> Option<PermissionMode> {
    match value? {
        "read-only" | "read_only" => Some(PermissionMode::ReadOnly),
        "workspace-write" | "workspace_write" => Some(PermissionMode::WorkspaceWrite),
        "danger-full-access" | "danger_full_access" => Some(PermissionMode::DangerFullAccess),
        "prompt" => Some(PermissionMode::Prompt),
        "allow" => Some(PermissionMode::Allow),
        _ => None,
    }
}

fn role_label(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn summarize_message(msg: &ConversationMessage) -> String {
    for block in &msg.blocks {
        if let ContentBlock::Text { text } = block {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let max = 200usize;
                return if trimmed.len() <= max {
                    trimmed.to_string()
                } else {
                    let mut s = trimmed.chars().take(max).collect::<String>();
                    s.push('…');
                    s
                };
            }
        }
    }
    String::new()
}

fn estimate_usd(_model: &str, input: u32, output: u32) -> f64 {
    // Conservative placeholder: $3/M input, $15/M output (Sonnet-ish). Real pricing
    // belongs in `runtime::pricing_for_model` once wired.
    let in_cost = f64::from(input) * 3.0 / 1_000_000.0;
    let out_cost = f64::from(output) * 15.0 / 1_000_000.0;
    in_cost + out_cost
}

fn to_sse(event: &ServerEvent) -> Result<Event, serde_json::Error> {
    Ok(Event::default()
        .event(event.event_name())
        .data(serde_json::to_string(event)?))
}
