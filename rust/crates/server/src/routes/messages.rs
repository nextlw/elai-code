use std::convert::Infallible;

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
    let model = conv.model.clone().unwrap_or_else(|| "go:kimi-k2.6".to_owned());
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
                    // keep the stream open — client reconnects for next message otherwise
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
    let api_key = std::env::var("XAI_API_KEY").unwrap_or_default();
    let base_url = std::env::var("XAI_BASE_URL")
        .unwrap_or_else(|_| "https://api.x.ai/v1".to_string());

    if api_key.is_empty() {
        let mock = "Olá! Configure XAI_API_KEY para respostas reais.";
        let _ = broadcaster.send(mock.to_owned());
        db::message::insert(&db_pool, conv_id, "assistant", mock).await.ok();
        let _ = broadcaster.send("__done__".to_owned());
        return;
    }

    let mut oai_messages = vec![serde_json::json!({ "role": "system", "content": system_prompt })];
    for m in &history {
        if m.role == "user" || m.role == "assistant" {
            oai_messages.push(serde_json::json!({ "role": m.role, "content": m.content }));
        }
    }

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 64_000,
        "stream": true,
        "messages": oai_messages
    });

    let client = reqwest::Client::new();
    let response = match client
        .post(format!("{base_url}/chat/completions"))
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            eprintln!("AI API error {}", r.status());
            let _ = broadcaster.send("__done__".to_owned());
            return;
        }
        Err(e) => {
            eprintln!("AI request failed: {e}");
            let _ = broadcaster.send("__done__".to_owned());
            return;
        }
    };

    let mut full_text = String::new();
    let mut raw_buf: Vec<u8> = Vec::new();
    let mut response = response;

    'outer: while let Ok(Some(chunk)) = response.chunk().await {
        raw_buf.extend_from_slice(&chunk);
        while let Some(pos) = raw_buf.iter().position(|&b| b == b'\n') {
            let line_bytes = raw_buf.drain(..=pos).collect::<Vec<u8>>();
            let line = match String::from_utf8(line_bytes) {
                Ok(s) => s,
                Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
            };
            let line = line.trim();
            let Some(data) = line.strip_prefix("data: ") else { continue };
            if data == "[DONE]" { break 'outer; }
            let Ok(val) = serde_json::from_str::<serde_json::Value>(data) else { continue };
            if let Some(text) = val["choices"][0]["delta"]["content"].as_str() {
                if !text.is_empty() {
                    full_text.push_str(text);
                    let _ = broadcaster.send(text.to_owned());
                }
            }
        }
    }

    db::message::insert(&db_pool, conv_id, "assistant", &full_text).await.ok();
    db::conversation::touch(&db_pool, conv_id).await.ok();
    let _ = broadcaster.send("__done__".to_owned());
}

fn internal(msg: &str) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: msg.to_owned() }))
}

fn not_found(msg: &str) -> ApiError {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { error: msg.to_owned() }))
}
