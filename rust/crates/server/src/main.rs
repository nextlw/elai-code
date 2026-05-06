use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use server::auth::generate_token;
use server::AppState;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(name = "elai-server", version)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8456")]
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
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args = Args::parse();
    let token_path = args
        .token_file
        .unwrap_or_else(default_token_path);
    let token = ensure_token(&token_path)?;

    tracing::info!(path = %token_path.display(), "auth token persisted");
    tracing::info!(addr = %args.listen, "starting elai-server");
    if let Some(origin) = &args.cors_origin {
        tracing::info!(origin = %origin, "CORS origin (informational)");
    }

    let state = AppState::new(token);
    let app = server::app(state);

    let listener = TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("failed to bind {}", args.listen))?;
    axum::serve(listener, app)
        .await
        .context("server error")?;
    Ok(())
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
