use std::convert::Infallible;

use api::{
    max_tokens_for_model, resolve_model_alias, InputContentBlock, InputMessage, MessageRequest,
    OutputContentBlock, ProviderClient,
};
use async_stream::stream;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::{db, db::user::User, AppState};

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

type ApiError = (StatusCode, Json<ErrorResponse>);

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
}

pub async fn get_messages(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(conv_id): Path<Uuid>,
) -> Result<Json<Vec<db::message::Message>>, ApiError> {
    db::conversation::get(&state.db, conv_id, user.id)
        .await
        .map_err(|_| internal("db error"))?
        .ok_or_else(|| not_found("conversation not found"))?;

    let msgs = db::message::list_by_conversation(&state.db, conv_id)
        .await
        .map_err(|_| internal("db error"))?;

    Ok(Json(msgs))
}

pub async fn send_message(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(conv_id): Path<Uuid>,
    Json(body): Json<SendMessageRequest>,
) -> Result<StatusCode, ApiError> {
    let conv = db::conversation::get(&state.db, conv_id, user.id)
        .await
        .map_err(|_| internal("db error"))?
        .ok_or_else(|| not_found("conversation not found"))?;

    db::message::insert(&state.db, conv_id, "user", &body.content)
        .await
        .map_err(|_| internal("db error"))?;

    if !body.content.is_empty() {
        let title_preview = body.content.chars().take(60).collect::<String>();
        db::conversation::update_title(&state.db, conv_id, &title_preview)
            .await
            .ok();
    }

    let history = db::message::list_by_conversation(&state.db, conv_id)
        .await
        .map_err(|_| internal("db error"))?;

    let system_prompt = build_system_prompt(&user);
    let broadcaster = state.get_or_create_channel(conv_id).await;

    let db_pool = state.db.clone();
    let model = conv
        .model
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| api::suggested_default_model());

    tokio::spawn(async move {
        generate_ai_response(conv_id, history, system_prompt, model, broadcaster, db_pool).await;
    });

    Ok(StatusCode::NO_CONTENT)
}

pub async fn stream_events(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(conv_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    db::conversation::get(&state.db, conv_id, user.id)
        .await
        .map_err(|_| internal("db error"))?
        .ok_or_else(|| not_found("conversation not found"))?;

    let mut receiver = state.get_or_create_channel(conv_id).await.subscribe();

    let s = stream! {
        loop {
            match receiver.recv().await {
                Ok(text) if text == "__done__" => {
                    yield Ok::<Event, Infallible>(Event::default().event("done").data("{}"));
                    break;
                }
                Ok(text) if text.starts_with("__error__:") => {
                    let msg = text.strip_prefix("__error__:").unwrap_or(&text);
                    yield Ok::<Event, Infallible>(
                        Event::default().event("error").data(
                            serde_json::to_string(&serde_json::json!({ "message": msg }))
                                .unwrap_or_default()
                        )
                    );
                    yield Ok::<Event, Infallible>(Event::default().event("done").data("{}"));
                    break;
                }
                Ok(text) => {
                    yield Ok::<Event, Infallible>(
                        Event::default().event("text_delta").data(
                            serde_json::to_string(&serde_json::json!({ "delta": text }))
                                .unwrap_or_default()
                        )
                    );
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(s).keep_alive(KeepAlive::default()))
}

fn build_system_prompt(user: &User) -> String {
    let name = user.name.as_deref().unwrap_or("usuário");
    let occupation = user.occupation.as_deref().unwrap_or("não informada");
    let goals = user.goals.as_deref().unwrap_or("não informado");
    let use_case = user.use_case.as_deref().unwrap_or("geral");
    format!(
        "Você é a Nexa, uma IA de nova geração criada pela Nexcode.\n\
         Usuário: {name}. Ocupação: {occupation}. Objetivo: {goals}. Foco: {use_case}.\n\
         Analise profundamente antes de responder. Responda sempre em português brasileiro, \
         de forma direta e útil."
    )
}

async fn generate_ai_response(
    conv_id: Uuid,
    history: Vec<db::message::Message>,
    system_prompt: String,
    model: String,
    broadcaster: broadcast::Sender<String>,
    db_pool: sqlx::PgPool,
) {
    let client = match ProviderClient::from_model(&model) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(%model, error = %e, "failed to build ProviderClient");
            let _ = broadcaster.send(format!("__error__:Erro ao configurar provider para '{model}': {e}"));
            return;
        }
    };

    let resolved = resolve_model_alias(&model);
    let max_tokens = max_tokens_for_model(&resolved);

    let mut messages: Vec<InputMessage> = Vec::new();
    for m in &history {
        let role = match m.role.as_str() {
            "assistant" => "assistant",
            _ => "user",
        };
        messages.push(InputMessage {
            role: role.to_string(),
            content: vec![InputContentBlock::Text { text: m.content.clone() }],
        });
    }

    let request = MessageRequest {
        model: resolved.clone(),
        max_tokens,
        messages,
        system: Some(system_prompt),
        tools: None,
        tool_choice: None,
        stream: false,
        thinking: None,
        output_config: None,
        reasoning_effort: None,
    };

    match client.send_message(&request).await {
        Ok(response) => {
            let full_text: String = response
                .content
                .iter()
                .filter_map(|block| match block {
                    OutputContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            if full_text.is_empty() {
                tracing::warn!(%conv_id, "AI returned empty response");
                let _ = broadcaster.send("__error__:A IA retornou uma resposta vazia.".to_string());
                return;
            }

            // Envia em blocos para simular streaming (chunks de ~80 chars)
            for chunk in full_text.as_bytes().chunks(80) {
                let text = String::from_utf8_lossy(chunk).into_owned();
                let _ = broadcaster.send(text);
            }

            db::message::insert(&db_pool, conv_id, "assistant", &full_text)
                .await
                .ok();
            db::conversation::touch(&db_pool, conv_id).await.ok();
        }
        Err(e) => {
            tracing::error!(%conv_id, %resolved, error = %e, "AI API call failed");
            let _ = broadcaster.send(format!("__error__:Erro na IA ({resolved}): {e}"));
        }
    }

    let _ = broadcaster.send("__done__".to_owned());
}

fn internal(msg: &str) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: msg.to_owned() }))
}

fn not_found(msg: &str) -> ApiError {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { error: msg.to_owned() }))
}
