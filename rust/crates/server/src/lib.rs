// Allow pedantic lints that produce excessive warnings for this project
// The app() function legitimately exceeds 266 lines due to its nature.
#![allow(clippy::too_many_lines)]

pub mod auth;
pub mod db;
pub mod permission_bridge;
pub mod session_store;
pub mod routes;
pub mod runtime_bridge;
pub mod state;
pub mod streaming;

use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post, put};
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
        .route(
            "/v1/sessions/{id}/permissions/ws",
            get(routes::sessions::permissions_ws),
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
        .route(
            "/v1/workspace/{session_id}/diff",
            get(routes::workspace::diff),
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
        // mcp routes
        .route(
            "/v1/mcp/servers",
            get(routes::mcp::list_mcp_servers).post(routes::mcp::add_mcp_server),
        )
        .route(
            "/v1/mcp/servers/{name}",
            put(routes::mcp::update_mcp_server).delete(routes::mcp::delete_mcp_server),
        )
        .route(
            "/v1/mcp/servers/{name}/restart",
            post(routes::mcp::restart_mcp_server),
        )
        .route(
            "/v1/mcp/servers/{name}/tools",
            get(routes::mcp::list_mcp_server_tools),
        )
        .route(
            "/v1/mcp/servers/{name}/resources",
            get(routes::mcp::list_mcp_server_resources),
        )
        .route(
            "/v1/mcp/servers/{name}/tools/{tool}/call",
            post(routes::mcp::call_mcp_tool),
        )
        // plugin routes
        .route("/v1/plugins", get(routes::plugins::list_plugins))
        .route(
            "/v1/plugins/{name}",
            post(routes::plugins::install_plugin)
                .put(routes::plugins::update_plugin)
                .delete(routes::plugins::uninstall_plugin),
        )
        // skills routes
        .route("/v1/skills", get(routes::plugins::list_skills))
        .route(
            "/v1/skills/validate",
            get(routes::plugins::validate_skills_route),
        )
        // agents routes
        .route("/v1/agents", get(routes::plugins::list_agents))
        .route(
            "/v1/agents/{name}/run",
            post(routes::plugins::run_agent),
        )
        // hooks routes
        .route(
            "/v1/hooks",
            get(routes::plugins::list_hooks).put(routes::plugins::update_hooks),
        )
        // user-commands routes
        .route(
            "/v1/user-commands",
            get(routes::user_commands::list_user_commands)
                .post(routes::user_commands::create_user_command),
        )
        .route(
            "/v1/user-commands/{name}",
            put(routes::user_commands::update_user_command)
                .delete(routes::user_commands::delete_user_command),
        )
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
        .route(
            "/v1/workspace/{session_id}/git/commit",
            post(routes::git::git_commit),
        )
        .route(
            "/v1/workspace/{session_id}/git/branch/create",
            post(routes::git::git_branch_create),
        )
        .route(
            "/v1/workspace/{session_id}/git/worktree/create",
            post(routes::git::git_worktree_create),
        )
        .route(
            "/v1/workspace/{session_id}/git/pr/create",
            post(routes::git::git_pr_create),
        )
        // tools extended routes
        .route("/v1/tools/{name}", get(routes::tools::get_tool))
        .route(
            "/v1/sessions/{id}/tools/allow",
            post(routes::tools::tools_allow),
        )
        .route(
            "/v1/sessions/{id}/tools/deny",
            post(routes::tools::tools_deny),
        )
        .route("/v1/tools/rate-limit", get(routes::tools::tools_rate_limit))
        // tasks routes
        .route("/v1/tasks", get(routes::tasks::list_tasks))
        .route("/v1/tasks/{id}", get(routes::tasks::get_task))
        .route(
            "/v1/tasks/{id}/output",
            get(routes::tasks::get_task_output),
        )
        .route(
            "/v1/tasks/{id}/cancel",
            post(routes::tasks::cancel_task),
        )
        // cache routes
        .route("/v1/cache/stats", get(routes::cache::cache_stats))
        .route("/v1/cache/clear", post(routes::cache::cache_clear))
        // commands extended routes
        .route(
            "/v1/sessions/{id}/commands/run",
            post(routes::commands::run_command),
        )
        .route(
            "/v1/sessions/{id}/commands/compact",
            post(routes::commands::compact_session),
        )
        .route(
            "/v1/sessions/{id}/commands/export",
            post(routes::commands::export_session),
        )
        .route(
            "/v1/sessions/{id}/commands/resume",
            post(routes::commands::resume_session),
        )
        // session extra routes
        .route(
            "/v1/sessions/{id}/clone",
            post(routes::sessions::clone_session),
        )
        .route(
            "/v1/sessions/{id}/compact",
            post(routes::sessions::compact_session_handler),
        )
        .route(
            "/v1/sessions/{id}/export",
            post(routes::sessions::export_session),
        )
        .route(
            "/v1/sessions/{id}/resume",
            post(routes::sessions::resume_session),
        )
        // auth routes
        .route("/v1/auth/status", get(routes::server_auth::get_auth_status))
        .route("/v1/auth/methods", get(routes::server_auth::list_auth_methods))
        .route("/v1/auth/api-key", post(routes::server_auth::set_api_key))
        .route(
            "/v1/auth/api-key/{provider}",
            delete(routes::server_auth::delete_api_key),
        )
        .route("/v1/auth/oauth/start", post(routes::server_auth::oauth_start))
        .route(
            "/v1/auth/oauth/callback",
            get(routes::server_auth::oauth_callback),
        )
        .route(
            "/v1/auth/oauth/refresh",
            post(routes::server_auth::oauth_refresh),
        )
        .route(
            "/v1/auth/import/claude-code",
            post(routes::server_auth::import_claude_code),
        )
        .route(
            "/v1/auth/import/codex",
            post(routes::server_auth::import_codex),
        )
        // config_full routes
        .route(
            "/v1/config",
            get(routes::config_full::get_config).patch(routes::config_full::patch_config),
        )
        .route("/v1/config/sources", get(routes::config_full::get_config_sources))
        .route(
            "/v1/providers/{id}/test",
            post(routes::config_full::test_provider),
        )
        .route(
            "/v1/budget",
            get(routes::config_full::get_budget).patch(routes::config_full::patch_budget),
        )
        .route(
            "/v1/theme",
            get(routes::config_full::get_theme).patch(routes::config_full::patch_theme),
        )
        .layer(from_fn_with_state(
            state.auth.clone(),
            auth::require_bearer,
        ))
        .with_state(state);

    public.merge(protected).layer(cors)
}
