pub mod auth;
pub mod db;
pub mod permission_bridge;
pub mod routes;
pub mod runtime_bridge;
pub mod state;
pub mod streaming;

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};

pub use state::AppState;

pub fn app(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let public = Router::new()
        .route("/v1/health", get(routes::health::health))
        .with_state(state.clone());

    let protected = Router::new()
        .route("/v1/version", get(routes::health::version))
        .route(
            "/v1/sessions",
            post(routes::sessions::create_session).get(routes::sessions::list_sessions),
        )
        .route(
            "/v1/sessions/{id}",
            get(routes::sessions::get_session)
                .delete(routes::sessions::delete_session)
                .patch(routes::sessions::patch_session),
        )
        .route(
            "/v1/sessions/{id}/messages",
            post(routes::sessions::send_message),
        )
        .route(
            "/v1/sessions/{id}/turns/{turn_id}/cancel",
            post(routes::sessions::cancel_turn),
        )
        .route(
            "/v1/sessions/{id}/events",
            get(routes::sessions::stream_events),
        )
        .route("/v1/sessions/{id}/cost", get(routes::sessions::get_cost))
        .route(
            "/v1/sessions/{id}/permissions/pending",
            get(routes::sessions::list_pending_permissions),
        )
        .route(
            "/v1/permissions/{request_id}/decide",
            post(routes::sessions::decide_permission),
        )
        // workspace routes
        .route(
            "/v1/workspace/{session_id}/read",
            post(routes::workspace::read),
        )
        .route(
            "/v1/workspace/{session_id}/write",
            post(routes::workspace::write),
        )
        .route(
            "/v1/workspace/{session_id}/edit",
            post(routes::workspace::edit),
        )
        .route(
            "/v1/workspace/{session_id}/glob",
            post(routes::workspace::glob),
        )
        .route(
            "/v1/workspace/{session_id}/grep",
            post(routes::workspace::grep),
        )
        .route(
            "/v1/workspace/{session_id}/tree",
            get(routes::workspace::tree),
        )
        // config routes
        .route("/v1/models", get(routes::config::list_models))
        .route("/v1/providers", get(routes::config::list_providers))
        // commands route
        .route("/v1/commands", get(routes::commands::list_commands))
        // tools route
        .route("/v1/tools", get(routes::tools::list_tools))
        // telemetry routes
        .route("/v1/telemetry", get(routes::telemetry::get_telemetry))
        .route("/v1/usage/summary", get(routes::telemetry::get_usage_summary))
        // git routes
        .route(
            "/v1/workspace/{session_id}/git/status",
            get(routes::git::git_status),
        )
        .route(
            "/v1/workspace/{session_id}/git/diff",
            get(routes::git::git_diff),
        )
        .route(
            "/v1/workspace/{session_id}/git/log",
            get(routes::git::git_log),
        )
        .route(
            "/v1/workspace/{session_id}/git/branches",
            get(routes::git::git_branches),
        )
        .layer(from_fn_with_state(
            state.auth.clone(),
            auth::require_bearer,
        ))
        .with_state(state);

    public.merge(protected).layer(cors)
}
