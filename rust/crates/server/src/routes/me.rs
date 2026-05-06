use axum::{extract::State, http::StatusCode, Extension, Json};
use serde::Serialize;

use crate::{db, db::user::User, AppState};

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

type ApiError = (StatusCode, Json<ErrorResponse>);

pub async fn get_me(Extension(user): Extension<User>) -> Json<User> {
    Json(user)
}

pub async fn patch_me(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(patch): Json<db::user::UpdateProfile>,
) -> Result<Json<User>, ApiError> {
    let updated = db::user::update_profile(&state.db, user.id, patch)
        .await
        .map_err(|e| {
            eprintln!("DB error updating profile: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "db error".into() }))
        })?;
    Ok(Json(updated))
}
