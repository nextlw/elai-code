use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{db, db::conversation::Conversation, db::user::User, AppState};

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

type ApiError = (StatusCode, Json<ErrorResponse>);

#[derive(Deserialize)]
pub struct CreateConversationRequest {
    pub title: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateConversationRequest>,
) -> Result<(StatusCode, Json<Conversation>), ApiError> {
    let model = db::config::get_active_model(&state.db)
        .await
        .unwrap_or_else(|_| "go:kimi-k2.6".to_owned());

    let conv = db::conversation::create(&state.db, user.id, body.title.as_deref(), Some(&model))
        .await
        .map_err(|e| {
            eprintln!("DB error creating conversation: {e}");
            internal("db error")
        })?;

    Ok((StatusCode::CREATED, Json(conv)))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<Conversation>>, ApiError> {
    let convs = db::conversation::list_by_user(&state.db, user.id)
        .await
        .map_err(|_| internal("db error"))?;
    Ok(Json(convs))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    db::conversation::delete(&state.db, id, user.id)
        .await
        .map_err(|_| internal("db error"))?;
    Ok(StatusCode::NO_CONTENT)
}

fn internal(msg: &str) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: msg.to_owned() }))
}
