use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use svix::webhooks::Webhook;

use crate::{db, AppState};

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Deserialize)]
struct WebhookPayload {
    #[serde(rename = "type")]
    event_type: String,
    data: UserData,
}

#[derive(Debug, Deserialize)]
struct UserData {
    id: String,
    email_addresses: Vec<EmailAddress>,
    first_name: Option<String>,
    last_name: Option<String>,
    image_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmailAddress {
    email_address: String,
}

type ApiError = (StatusCode, Json<ErrorResponse>);

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let wh = Webhook::new(&state.clerk_webhook_secret)
        .map_err(|_| bad_request("invalid webhook secret config"))?;
    wh.verify(&body, &headers)
        .map_err(|_| bad_request("webhook signature invalid"))?;

    let payload: WebhookPayload = serde_json::from_slice(&body)
        .map_err(|_| bad_request("invalid payload"))?;

    let email = payload.data
        .email_addresses
        .first()
        .map(|e| e.email_address.as_str())
        .unwrap_or("");

    let name = match (payload.data.first_name.as_deref(), payload.data.last_name.as_deref()) {
        (Some(f), Some(l)) => Some(format!("{f} {l}")),
        (Some(f), None) => Some(f.to_owned()),
        (None, Some(l)) => Some(l.to_owned()),
        _ => None,
    };

    match payload.event_type.as_str() {
        "user.created" | "user.updated" => {
            db::user::create(
                &state.db,
                &payload.data.id,
                email,
                name.as_deref(),
                payload.data.image_url.as_deref(),
            )
            .await
            .map_err(|e| {
                eprintln!("DB error on user upsert: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "db error".into() }))
            })?;
        }
        "user.deleted" => {
            db::user::delete_by_clerk_id(&state.db, &payload.data.id)
                .await
                .map_err(|e| {
                    eprintln!("DB error on user delete: {e}");
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "db error".into() }))
                })?;
        }
        _ => {}
    }

    Ok(StatusCode::NO_CONTENT)
}

fn bad_request(msg: &str) -> ApiError {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg.to_owned() }))
}
