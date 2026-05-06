use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use server::auth::{generate_token, jwks};
use server::{db, AppState};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "elai-server", version)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:3000")]
    listen: SocketAddr,

    /// Allowed CORS origin (currently informational; default config permits localhost).
    #[arg(long)]
    cors_origin: Option<String>,

    /// Path to a file holding the bearer token. Created with a fresh token if missing.
    #[arg(long)]
    token_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    load_env();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args = Args::parse();
    let listen = std::env::var("PORT")
        .ok()
        .and_then(|port| format!("0.0.0.0:{port}").parse::<SocketAddr>().ok())
        .unwrap_or(args.listen);
    let token_path = args
        .token_file
        .unwrap_or_else(default_token_path);
    let token = ensure_token(&token_path)?;

    tracing::info!(path = %token_path.display(), "auth token persisted");
    tracing::info!(addr = %listen, "starting elai-server");
    if let Some(origin) = &args.cors_origin {
        tracing::info!(origin = %origin, "CORS origin (informational)");
    }

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mcp_manager = {
        let loader = runtime::ConfigLoader::default_for(&cwd);
        match loader.load() {
            Ok(config) => runtime::McpServerManager::from_runtime_config(&config),
            Err(_) => runtime::McpServerManager::from_servers(&std::collections::BTreeMap::new()),
        }
    };

    let database_url = std::env::var("DATABASE_URL").context(
        "DATABASE_URL must be set; copy rust/crates/server/.env.example to rust/crates/server/.env or export it",
    )?;
    let clerk_jwks_url = std::env::var("CLERK_JWKS_URL").context(
        "CLERK_JWKS_URL must be set; copy rust/crates/server/.env.example to rust/crates/server/.env or export it",
    )?;
    let clerk_webhook_secret = std::env::var("CLERK_WEBHOOK_SECRET").unwrap_or_default();
    let model = std::env::var("ELAI_MODEL").unwrap_or_else(|_| "go:kimi-k2.6".to_string());
    let ai_base_url = std::env::var("AI_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());

    tracing::info!("connecting to PostgreSQL");
    let db_pool = db::connect(&database_url)
        .await
        .context("failed to connect to PostgreSQL")?;
    tracing::info!("PostgreSQL connected");

    tracing::info!(url = %clerk_jwks_url, "fetching Clerk JWKS");
    let jwk_set = jwks::fetch(&clerk_jwks_url)
        .await
        .context("failed to fetch Clerk JWKS")?;
    tracing::info!(keys = jwk_set.keys.len(), "Clerk JWKS loaded");

    if std::env::var("AI_API_KEY").map_or(true, |key| key.is_empty()) {
        tracing::warn!("AI_API_KEY not set; SaaS chat responses will use the mock fallback");
    }
    tracing::info!(model = %model, endpoint = %ai_base_url, "AI backend configured");

    let state = AppState::new(token, mcp_manager, db_pool, jwk_set, clerk_webhook_secret);
    let app = server::app(state);

    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("failed to bind {listen}"))?;
    axum::serve(listener, app)
        .await
        .context("server error")?;
    Ok(())
}

fn load_env() {
    dotenvy::dotenv().ok();
    dotenvy::from_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".env")).ok();
}

fn default_token_path() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(".elai").join("server-token")
}

fn ensure_token(path: &PathBuf) -> Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create token dir {}", parent.display()))?;
    }
    let token = generate_token();
    std::fs::write(path, &token)
        .with_context(|| format!("failed to write token to {}", path.display()))?;
    Ok(token)
}
