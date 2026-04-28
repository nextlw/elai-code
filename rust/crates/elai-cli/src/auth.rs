use std::fmt;
use std::future::Future;
use std::io::{self, BufRead, Read, Write};
use std::net::TcpListener;
use std::process::Command as ProcessCommand;

use api::{base_url_is_anthropic_official, read_base_url, AuthSource, ElaiApiClient};
use runtime::{
    generate_pkce_pair, generate_state, load_auth_method, loopback_redirect_uri,
    parse_oauth_callback_request_target, save_auth_method, clear_auth_method,
    AnthropicOAuthEndpoints, ApiKeyOrigin, AuthMethod, OAuthAuthorizationRequest,
    OAuthCallbackParams, OAuthMode, OAuthTokenExchangeRequest, OAuthTokenSet,
};

use crate::args::LoginArgs;

pub const DEFAULT_OAUTH_CALLBACK_PORT: u16 = 4545;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[allow(dead_code)] // Browser/ConflictingFlags/Cancelled reservados para fluxos futuros (TUI cancel, validações).
pub enum AuthError {
    Io(io::Error),
    Api(api::ApiError),
    Browser(String),
    UnsupportedBaseUrl(String),
    StateMismatch,
    MissingCode,
    ConflictingFlags(String),
    InvalidInput(String),
    Cancelled,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::Io(e) => write!(f, "io error: {e}"),
            AuthError::Api(e) => write!(f, "api error: {e}"),
            AuthError::Browser(msg) => write!(f, "browser error: {msg}"),
            AuthError::UnsupportedBaseUrl(url) => write!(
                f,
                "this login method requires the official Anthropic API endpoint, but ANTHROPIC_BASE_URL is set to {url}"
            ),
            AuthError::StateMismatch => write!(f, "OAuth state mismatch; possible CSRF attack"),
            AuthError::MissingCode => {
                write!(f, "OAuth callback did not include an authorization code")
            }
            AuthError::ConflictingFlags(msg) => write!(f, "conflicting flags: {msg}"),
            AuthError::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            AuthError::Cancelled => write!(f, "login cancelled"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<io::Error> for AuthError {
    fn from(e: io::Error) -> Self {
        AuthError::Io(e)
    }
}

impl From<api::ApiError> for AuthError {
    fn from(e: api::ApiError) -> Self {
        AuthError::Api(e)
    }
}

impl From<runtime::ConfigError> for AuthError {
    fn from(e: runtime::ConfigError) -> Self {
        AuthError::Io(io::Error::other(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Helper: run a tokio future on a one-shot runtime
// ---------------------------------------------------------------------------

fn tokio_block_on<F, T>(fut: F) -> Result<T, AuthError>
where
    F: Future<Output = Result<T, api::ApiError>>,
{
    tokio::runtime::Runtime::new()
        .map_err(AuthError::Io)?
        .block_on(fut)
        .map_err(AuthError::Api)
}

// ---------------------------------------------------------------------------
// Require official Anthropic base URL for OAuth flows
// ---------------------------------------------------------------------------

fn require_anthropic_base_url() -> Result<(), AuthError> {
    if !base_url_is_anthropic_official() {
        return Err(AuthError::UnsupportedBaseUrl(read_base_url()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public dispatch functions
// ---------------------------------------------------------------------------

pub fn dispatch_login(args: &LoginArgs) -> Result<(), AuthError> {
    if args.legacy_elai {
        return legacy_elai_login(args);
    }
    if args.console {
        return login_console(args);
    }
    if args.claudeai {
        return login_claude_ai(args, false);
    }
    if args.sso {
        return login_claude_ai(args, true);
    }
    if args.api_key {
        return login_paste_api_key(args);
    }
    if args.token {
        return login_paste_auth_token(args);
    }
    if args.use_bedrock {
        return toggle_3p(AuthMethod::Bedrock, "CLAUDE_CODE_USE_BEDROCK");
    }
    if args.use_vertex {
        return toggle_3p(AuthMethod::Vertex, "CLAUDE_CODE_USE_VERTEX");
    }
    if args.use_foundry {
        return toggle_3p(AuthMethod::Foundry, "CLAUDE_CODE_USE_FOUNDRY");
    }
    if args.import_claude_code {
        return import_claude_code_login();
    }
    // No method selected — interactive picker placeholder for Layer 4
    Err(AuthError::InvalidInput(
        "no method selected; rerun with --console|--claudeai|--sso|--api-key|--token|--use-bedrock|--use-vertex|--use-foundry|--import-claude-code, or use the TUI".into(),
    ))
}

pub fn dispatch_logout() -> Result<(), AuthError> {
    clear_auth_method().map_err(AuthError::Io)?;
    println!("Cleared saved authentication.");
    Ok(())
}

pub fn dispatch_auth_status(json: bool) -> Result<(), AuthError> {
    let info = collect_auth_info();
    if json {
        let value = auth_info_to_json(&info);
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .unwrap_or_else(|_| "{\"error\":\"serialization failed\"}".into())
        );
    } else {
        print_auth_info_text(&info);
    }
    Ok(())
}

pub fn dispatch_auth_list() -> Result<(), AuthError> {
    println!("Available login methods:");
    println!();
    println!("  console     OAuth via Anthropic Console — creates an API key");
    println!(
        "  claudeai    OAuth via claude.ai — Pro/Max/Team subscriber bearer"
    );
    println!(
        "  sso         claude.ai with login_method=sso (use --email to pre-fill)"
    );
    println!("  api-key     Paste an sk-ant-... key (use --stdin in CI)");
    println!("  token       Paste an ANTHROPIC_AUTH_TOKEN bearer");
    println!(
        "  use-bedrock Switch to AWS Bedrock (uses AWS credential chain)"
    );
    println!("  use-vertex  Switch to Google Vertex AI");
    println!("  use-foundry Switch to Azure Foundry");
    println!("  legacy-elai (deprecated) elai.dev OAuth flow");
    Ok(())
}

// ---------------------------------------------------------------------------
// OAuth flows
// ---------------------------------------------------------------------------

fn login_claude_ai(args: &LoginArgs, sso: bool) -> Result<(), AuthError> {
    require_anthropic_base_url()?;
    let endpoints = AnthropicOAuthEndpoints::production();
    let cfg = endpoints.to_oauth_config(OAuthMode::ClaudeAi);
    let port = DEFAULT_OAUTH_CALLBACK_PORT;
    let pkce = generate_pkce_pair().map_err(AuthError::Io)?;
    let state = generate_state().map_err(AuthError::Io)?;
    let redirect_uri = loopback_redirect_uri(port);
    let mut req = OAuthAuthorizationRequest::from_config(
        &cfg,
        redirect_uri.clone(),
        state.clone(),
        &pkce,
    );
    if let Some(email) = &args.email {
        req = req.with_extra_param("login_hint", email.as_str());
    }
    if sso {
        if args.email.is_none() {
            eprintln!("warning: --sso without --email; IdP discovery may fail if the provider cannot determine the tenant automatically.");
        }
        req = req.with_extra_param("login_method", "sso");
    }
    let url = req.build_url();

    println!("Starting Anthropic OAuth (claude.ai)...");
    println!("Listening for callback on {redirect_uri}");
    if args.no_browser {
        println!("Open this URL manually:\n{url}");
    } else if let Err(e) = open_browser(&url) {
        eprintln!("warning: failed to open browser: {e}");
        println!("Open this URL manually:\n{url}");
    }

    let cb = wait_for_oauth_callback(port)?;
    if let Some(err) = cb.error {
        let desc = cb.error_description.unwrap_or_default();
        return Err(AuthError::Api(api::ApiError::Auth(format!(
            "{err}: {desc}"
        ))));
    }
    let code = cb.code.ok_or(AuthError::MissingCode)?;
    let returned_state = cb.state.unwrap_or_default();
    if returned_state != state {
        return Err(AuthError::StateMismatch);
    }

    let client =
        ElaiApiClient::from_auth(AuthSource::None).with_base_url(read_base_url());
    let exchange_req = OAuthTokenExchangeRequest::from_config(
        &cfg,
        code,
        state,
        pkce.verifier,
        redirect_uri,
    );
    let beta = endpoints.beta_header.clone();
    let tokens = tokio_block_on(async {
        client
            .exchange_oauth_code_with_headers(&cfg, &exchange_req, &[("anthropic-beta", &beta)])
            .await
    })?;

    // Probe roles for subscription label (best-effort).
    let access_token = tokens.access_token.clone();
    let subscription = tokio_block_on(async {
        client.fetch_roles(&endpoints, &access_token).await
    })
    .ok()
    .and_then(|json| {
        json.get("subscription_type")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
    });

    save_auth_method(&AuthMethod::ClaudeAiOAuth {
        token_set: OAuthTokenSet {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
            scopes: tokens.scopes,
        },
        subscription,
    })
    .map_err(AuthError::Io)?;
    println!("Logged in via claude.ai.");
    Ok(())
}

fn login_console(args: &LoginArgs) -> Result<(), AuthError> {
    require_anthropic_base_url()?;
    let endpoints = AnthropicOAuthEndpoints::production();
    let cfg = endpoints.to_oauth_config(OAuthMode::Console);
    let port = DEFAULT_OAUTH_CALLBACK_PORT;
    let pkce = generate_pkce_pair().map_err(AuthError::Io)?;
    let state = generate_state().map_err(AuthError::Io)?;
    let redirect_uri = loopback_redirect_uri(port);
    let mut req = OAuthAuthorizationRequest::from_config(
        &cfg,
        redirect_uri.clone(),
        state.clone(),
        &pkce,
    );
    if let Some(email) = &args.email {
        req = req.with_extra_param("login_hint", email.as_str());
    }
    let url = req.build_url();

    println!("Starting Anthropic OAuth (Console)...");
    println!("Listening for callback on {redirect_uri}");
    if args.no_browser {
        println!("Open this URL manually:\n{url}");
    } else if let Err(e) = open_browser(&url) {
        eprintln!("warning: failed to open browser: {e}");
        println!("Open this URL manually:\n{url}");
    }

    let cb = wait_for_oauth_callback(port)?;
    if let Some(err) = cb.error {
        let desc = cb.error_description.unwrap_or_default();
        return Err(AuthError::Api(api::ApiError::Auth(format!(
            "{err}: {desc}"
        ))));
    }
    let code = cb.code.ok_or(AuthError::MissingCode)?;
    let returned_state = cb.state.unwrap_or_default();
    if returned_state != state {
        return Err(AuthError::StateMismatch);
    }

    let client =
        ElaiApiClient::from_auth(AuthSource::None).with_base_url(read_base_url());
    let exchange_req = OAuthTokenExchangeRequest::from_config(
        &cfg,
        code,
        state,
        pkce.verifier,
        redirect_uri,
    );
    let beta = endpoints.beta_header.clone();
    let tokens = tokio_block_on(async {
        client
            .exchange_oauth_code_with_headers(&cfg, &exchange_req, &[("anthropic-beta", &beta)])
            .await
    })?;

    // Create console API key from the OAuth access token
    let access_token = tokens.access_token.clone();
    let raw_key = tokio_block_on(async {
        client.create_console_api_key(&endpoints, &access_token).await
    })?;

    save_auth_method(&AuthMethod::ConsoleApiKey {
        api_key: raw_key,
        origin: ApiKeyOrigin::ConsoleOAuth,
    })
    .map_err(AuthError::Io)?;
    println!("Logged in via Anthropic Console. API key saved.");
    Ok(())
}

fn login_paste_api_key(args: &LoginArgs) -> Result<(), AuthError> {
    let key = if args.stdin {
        read_secret_from_stdin()?
    } else {
        read_secret_from_tty("Anthropic API key (sk-ant-...): ")?
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(AuthError::InvalidInput("api key is empty".into()));
    }
    if !key.starts_with("sk-ant-") {
        eprintln!("warning: API key does not start with 'sk-ant-'.");
    }
    save_auth_method(&AuthMethod::ConsoleApiKey {
        api_key: key,
        origin: ApiKeyOrigin::Pasted,
    })
    .map_err(AuthError::Io)?;
    println!("API key saved.");
    Ok(())
}

fn login_paste_auth_token(args: &LoginArgs) -> Result<(), AuthError> {
    let token = if args.stdin {
        read_secret_from_stdin()?
    } else {
        read_secret_from_tty("ANTHROPIC_AUTH_TOKEN (Bearer token): ")?
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AuthError::InvalidInput("auth token is empty".into()));
    }
    save_auth_method(&AuthMethod::AnthropicAuthToken { token }).map_err(AuthError::Io)?;
    println!("Auth token saved.");
    Ok(())
}

fn toggle_3p(method: AuthMethod, env_var: &str) -> Result<(), AuthError> {
    save_auth_method(&method).map_err(AuthError::Io)?;
    println!("Switched auth to {method:?}.");
    println!("Add to your shell rc to enable: export {env_var}=1");
    println!("Then restart elai.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Import credentials from an existing Claude Code installation
// ---------------------------------------------------------------------------

/// Attempt to import Claude Code credentials (headless/agent-mode login).
///
/// Search order:
///   1. `~/.claude/.credentials.json`  — Claude Code stores OAuth & API keys here
///   2. macOS Keychain entry "Claude Code-credentials" (best-effort, macOS only)
///   3. `ANTHROPIC_API_KEY` environment variable
fn import_claude_code_login() -> Result<(), AuthError> {
    // 1. Try ~/.claude/.credentials.json
    if let Some(home) = std::env::var_os("HOME") {
        let creds_path = std::path::PathBuf::from(home).join(".claude").join(".credentials.json");
        if let Ok(data) = std::fs::read_to_string(&creds_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                // OAuth token object (claudeAiOAuthToken key)
                if let Some(oauth) = json.get("claudeAiOAuthToken") {
                    let access = oauth.get("accessToken")
                        .or_else(|| oauth.get("access_token"))
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    if !access.is_empty() {
                        let refresh = oauth.get("refreshToken")
                            .or_else(|| oauth.get("refresh_token"))
                            .and_then(|v| v.as_str())
                            .map(ToOwned::to_owned);
                        let expires_at = oauth.get("expiresAt")
                            .or_else(|| oauth.get("expires_at"))
                            .and_then(|v| v.as_u64());
                        let scopes: Vec<String> = oauth.get("scopes")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|s| s.as_str().map(ToOwned::to_owned)).collect())
                            .unwrap_or_default();
                        save_auth_method(&AuthMethod::ClaudeAiOAuth {
                            token_set: OAuthTokenSet {
                                access_token: access,
                                refresh_token: refresh,
                                expires_at,
                                scopes,
                            },
                            subscription: None,
                        })
                        .map_err(AuthError::Io)?;
                        println!("Imported Claude Code OAuth credentials from {}.", creds_path.display());
                        return Ok(());
                    }
                }
                // API key (apiKey key)
                if let Some(key) = json.get("apiKey").and_then(|v| v.as_str()) {
                    if !key.is_empty() {
                        save_auth_method(&AuthMethod::ConsoleApiKey {
                            api_key: key.to_string(),
                            origin: ApiKeyOrigin::Pasted,
                        })
                        .map_err(AuthError::Io)?;
                        println!("Imported Claude Code API key from {}.", creds_path.display());
                        return Ok(());
                    }
                }
            }
        }
    }

    // 2. macOS Keychain (best-effort, silent on non-macOS)
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !raw.is_empty() {
                    // The keychain may store the full JSON or just a token string
                    let token = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                        json.get("accessToken")
                            .or_else(|| json.get("access_token"))
                            .and_then(|v| v.as_str())
                            .map(ToOwned::to_owned)
                            .unwrap_or(raw.clone())
                    } else {
                        raw.clone()
                    };
                    if !token.is_empty() {
                        save_auth_method(&AuthMethod::AnthropicAuthToken { token })
                            .map_err(AuthError::Io)?;
                        println!("Imported Claude Code credentials from macOS Keychain.");
                        return Ok(());
                    }
                }
            }
        }
    }

    // 3. ANTHROPIC_API_KEY env var
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            save_auth_method(&AuthMethod::ConsoleApiKey {
                api_key: key,
                origin: ApiKeyOrigin::Pasted,
            })
            .map_err(AuthError::Io)?;
            println!("Imported credentials from ANTHROPIC_API_KEY environment variable.");
            return Ok(());
        }
    }

    Err(AuthError::InvalidInput(
        "Could not find Claude Code credentials. \
         Ensure Claude Code is installed and logged in, or set ANTHROPIC_API_KEY.".into(),
    ))
}

// ---------------------------------------------------------------------------
// Public helpers for TUI auth picker (no stdin/tty interaction)
// ---------------------------------------------------------------------------

/// Save an Anthropic API key (sk-ant-...) pasted directly by the user in the TUI.
pub fn save_pasted_api_key(value: &str) -> Result<(), AuthError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(AuthError::InvalidInput("api key is empty".into()));
    }
    save_auth_method(&AuthMethod::ConsoleApiKey {
        api_key: v.to_string(),
        origin: ApiKeyOrigin::Pasted,
    })
    .map_err(AuthError::Io)
}

/// Save an ANTHROPIC_AUTH_TOKEN bearer pasted directly by the user in the TUI.
pub fn save_pasted_auth_token(value: &str) -> Result<(), AuthError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(AuthError::InvalidInput("auth token is empty".into()));
    }
    save_auth_method(&AuthMethod::AnthropicAuthToken { token: v.to_string() })
        .map_err(AuthError::Io)
}

/// Save a third-party auth method (Bedrock / Vertex / Foundry) from the TUI.
pub fn save_3p(method: AuthMethod) -> Result<(), AuthError> {
    save_auth_method(&method).map_err(AuthError::Io)
}

/// Make `toggle_3p` accessible from the TUI (uses save_3p internally).
pub fn save_3p_named(env_var: &str) -> Result<(), AuthError> {
    let method = match env_var {
        "CLAUDE_CODE_USE_BEDROCK" => AuthMethod::Bedrock,
        "CLAUDE_CODE_USE_VERTEX" => AuthMethod::Vertex,
        "CLAUDE_CODE_USE_FOUNDRY" => AuthMethod::Foundry,
        _ => return Err(AuthError::InvalidInput(format!("unknown env var: {env_var}"))),
    };
    save_3p(method)
}

// ---------------------------------------------------------------------------
// Legacy elai.dev flow (moved from main.rs)
// ---------------------------------------------------------------------------

fn legacy_elai_login(args: &LoginArgs) -> Result<(), AuthError> {
    let default_oauth = default_oauth_config();
    let oauth = &default_oauth;
    let callback_port = oauth.callback_port.unwrap_or(DEFAULT_OAUTH_CALLBACK_PORT);
    let redirect_uri = loopback_redirect_uri(callback_port);
    let pkce = generate_pkce_pair().map_err(AuthError::Io)?;
    let state = generate_state().map_err(AuthError::Io)?;
    let mut req =
        OAuthAuthorizationRequest::from_config(oauth, redirect_uri.clone(), state.clone(), &pkce);
    if let Some(email) = &args.email {
        req = req.with_extra_param("login_hint", email.as_str());
    }
    let authorize_url = req.build_url();

    println!("Starting Elai OAuth login (legacy)...");
    println!("Listening for callback on {redirect_uri}");
    if args.no_browser {
        println!("Open this URL manually:\n{authorize_url}");
    } else if let Err(error) = open_browser(&authorize_url) {
        eprintln!("warning: failed to open browser automatically: {error}");
        println!("Open this URL manually:\n{authorize_url}");
    }

    let callback = wait_for_oauth_callback(callback_port)?;
    if let Some(error) = callback.error {
        let description = callback
            .error_description
            .unwrap_or_else(|| "authorization failed".to_string());
        return Err(AuthError::Api(api::ApiError::Auth(format!(
            "{error}: {description}"
        ))));
    }
    let code = callback.code.ok_or(AuthError::MissingCode)?;
    let returned_state = callback.state.ok_or(AuthError::MissingCode)?;
    if returned_state != state {
        return Err(AuthError::StateMismatch);
    }

    let client =
        ElaiApiClient::from_auth(AuthSource::None).with_base_url(api::read_base_url());
    let exchange_request = OAuthTokenExchangeRequest::from_config(
        oauth,
        code,
        state,
        pkce.verifier,
        redirect_uri,
    );
    let token_set = tokio_block_on(async {
        client.exchange_oauth_code(oauth, &exchange_request).await
    })?;
    runtime::save_oauth_credentials(&OAuthTokenSet {
        access_token: token_set.access_token,
        refresh_token: token_set.refresh_token,
        expires_at: token_set.expires_at,
        scopes: token_set.scopes,
    })
    .map_err(AuthError::Io)?;
    println!("Elai OAuth login complete.");
    Ok(())
}

fn default_oauth_config() -> runtime::OAuthConfig {
    runtime::OAuthConfig {
        client_id: String::from("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
        authorize_url: String::from("https://platform.elai.dev/oauth/authorize"),
        token_url: String::from("https://platform.elai.dev/v1/oauth/token"),
        callback_port: None,
        manual_redirect_url: None,
        scopes: vec![
            String::from("user:profile"),
            String::from("user:inference"),
            String::from("user:sessions:elai_code"),
        ],
    }
}

// ---------------------------------------------------------------------------
// Browser + callback listener
// ---------------------------------------------------------------------------

pub fn open_browser(url: &str) -> io::Result<()> {
    let commands: Vec<(&str, Vec<&str>)> = if cfg!(target_os = "macos") {
        vec![("open", vec![url])]
    } else if cfg!(target_os = "windows") {
        vec![("cmd", vec!["/C", "start", "", url])]
    } else {
        vec![("xdg-open", vec![url])]
    };
    for (program, prog_args) in commands {
        match ProcessCommand::new(program).args(&prog_args).spawn() {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no supported browser opener command found",
    ))
}

pub fn wait_for_oauth_callback(port: u16) -> Result<OAuthCallbackParams, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(AuthError::Io)?;
    let (mut stream, _) = listener.accept().map_err(AuthError::Io)?;
    let mut buffer = [0_u8; 4096];
    let bytes_read = stream.read(&mut buffer).map_err(AuthError::Io)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let request_line = request.lines().next().ok_or_else(|| {
        AuthError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing callback request line",
        ))
    })?;
    let target = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| {
            AuthError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing callback request target",
            ))
        })?;
    let callback = parse_oauth_callback_request_target(target).map_err(|error| {
        AuthError::Io(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    let body = if callback.error.is_some() {
        "Anthropic OAuth login failed. You can close this window."
    } else {
        "Anthropic OAuth login succeeded. You can close this window."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).map_err(AuthError::Io)?;
    Ok(callback)
}

// ---------------------------------------------------------------------------
// Secret reading
// ---------------------------------------------------------------------------

pub fn read_secret_from_stdin() -> io::Result<String> {
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    // Strip trailing newline
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    Ok(line)
}

pub fn read_secret_from_tty(prompt: &str) -> io::Result<String> {
    #[cfg(unix)]
    {
        // Disable echo via stty, read from /dev/tty, then restore
        let _ = ProcessCommand::new("stty")
            .arg("-echo")
            .stdin(std::process::Stdio::from(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/dev/tty")?,
            ))
            .status();
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")?;
        let mut tty_write = tty.try_clone()?;
        write!(tty_write, "{prompt}")?;
        tty_write.flush()?;
        let mut reader = io::BufReader::new(tty);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        // Restore echo
        let _ = ProcessCommand::new("stty")
            .arg("echo")
            .stdin(std::process::Stdio::from(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/dev/tty")?,
            ))
            .status();
        // Print newline since echo was off
        writeln!(tty_write)?;
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        Ok(line)
    }
    #[cfg(not(unix))]
    {
        // On Windows: no masking available; warn and read from stdin
        eprint!("{prompt}");
        eprintln!("(warning: input is not masked on this platform)");
        read_secret_from_stdin()
    }
}

// ---------------------------------------------------------------------------
// Auth status helpers
// ---------------------------------------------------------------------------

struct AuthInfo {
    method: String,
    source: String,
    subscription: Option<String>,
    expires_at: Option<u64>,
    scopes: Vec<String>,
}

fn collect_auth_info() -> AuthInfo {
    // Priority mirrors resolve_startup_auth_source
    // 1. FD env
    if let Ok(fd_str) = std::env::var("CLAUDE_CODE_API_KEY_FD") {
        if !fd_str.is_empty() {
            return AuthInfo {
                method: "fd".into(),
                source: "CLAUDE_CODE_API_KEY_FD".into(),
                subscription: None,
                expires_at: None,
                scopes: vec![],
            };
        }
    }
    // 2. ANTHROPIC_API_KEY
    if let Ok(v) = std::env::var("ANTHROPIC_API_KEY") {
        if !v.is_empty() {
            return AuthInfo {
                method: "api_key".into(),
                source: "ANTHROPIC_API_KEY".into(),
                subscription: None,
                expires_at: None,
                scopes: vec![],
            };
        }
    }
    // 3. ANTHROPIC_AUTH_TOKEN
    if let Ok(v) = std::env::var("ANTHROPIC_AUTH_TOKEN") {
        if !v.is_empty() {
            return AuthInfo {
                method: "auth_token".into(),
                source: "ANTHROPIC_AUTH_TOKEN".into(),
                subscription: None,
                expires_at: None,
                scopes: vec![],
            };
        }
    }
    // 4. CLAUDE_CODE_OAUTH_TOKEN
    if let Ok(v) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !v.is_empty() {
            return AuthInfo {
                method: "auth_token".into(),
                source: "CLAUDE_CODE_OAUTH_TOKEN".into(),
                subscription: None,
                expires_at: None,
                scopes: vec![],
            };
        }
    }
    // 5. Saved AuthMethod
    if let Ok(Some(method)) = load_auth_method() {
        return auth_info_from_method(&method);
    }
    // None
    AuthInfo {
        method: "none".into(),
        source: "none".into(),
        subscription: None,
        expires_at: None,
        scopes: vec![],
    }
}

fn auth_info_from_method(method: &AuthMethod) -> AuthInfo {
    match method {
        AuthMethod::ClaudeAiOAuth {
            token_set,
            subscription,
        } => AuthInfo {
            method: "claude_ai_oauth".into(),
            source: "credentials.json".into(),
            subscription: subscription.clone(),
            expires_at: token_set.expires_at,
            scopes: token_set.scopes.clone(),
        },
        AuthMethod::ConsoleApiKey { origin, .. } => AuthInfo {
            method: "console_api_key".into(),
            source: format!("credentials.json (origin: {origin:?})"),
            subscription: None,
            expires_at: None,
            scopes: vec![],
        },
        AuthMethod::AnthropicAuthToken { .. } => AuthInfo {
            method: "anthropic_auth_token".into(),
            source: "credentials.json".into(),
            subscription: None,
            expires_at: None,
            scopes: vec![],
        },
        AuthMethod::Bedrock => AuthInfo {
            method: "bedrock".into(),
            source: "credentials.json".into(),
            subscription: None,
            expires_at: None,
            scopes: vec![],
        },
        AuthMethod::Vertex => AuthInfo {
            method: "vertex".into(),
            source: "credentials.json".into(),
            subscription: None,
            expires_at: None,
            scopes: vec![],
        },
        AuthMethod::Foundry => AuthInfo {
            method: "foundry".into(),
            source: "credentials.json".into(),
            subscription: None,
            expires_at: None,
            scopes: vec![],
        },
    }
}

fn auth_info_to_json(info: &AuthInfo) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("method".into(), serde_json::Value::String(info.method.clone()));
    map.insert("source".into(), serde_json::Value::String(info.source.clone()));
    if let Some(sub) = &info.subscription {
        map.insert("subscription".into(), serde_json::Value::String(sub.clone()));
    }
    if let Some(exp) = info.expires_at {
        map.insert("expires_at".into(), serde_json::Value::Number(exp.into()));
    }
    if !info.scopes.is_empty() {
        map.insert(
            "scopes".into(),
            serde_json::Value::Array(
                info.scopes
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(map)
}

fn print_auth_info_text(info: &AuthInfo) {
    println!("Method: {}", info.method);
    println!("Source: {}", info.source);
    if let Some(sub) = &info.subscription {
        println!("Subscription: {sub}");
    }
    if let Some(exp) = info.expires_at {
        // Format as ISO 8601-ish from unix timestamp
        let secs = exp;
        println!("Expires: {secs} (unix timestamp)");
    }
    if !info.scopes.is_empty() {
        println!("Scopes: {}", info.scopes.join(", "));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config_home() -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        std::env::temp_dir().join(format!(
            "auth-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn dispatch_login_with_no_method_returns_invalid_input() {
        let args = LoginArgs::default();
        let err = dispatch_login(&args).expect_err("should fail with no method");
        assert!(
            matches!(err, AuthError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn read_secret_from_stdin_trims_trailing_newline() {
        // We can test the logic directly by feeding a string
        let input = "sk-ant-test123\n";
        let mut cursor = std::io::Cursor::new(input.as_bytes());
        let mut line = String::new();
        use std::io::BufRead;
        cursor.read_line(&mut line).unwrap();
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        assert_eq!(line, "sk-ant-test123");
    }

    #[test]
    fn dispatch_auth_list_prints_all_methods() {
        // Capture stdout isn't easy in tests without a wrapper, but we can
        // verify the function runs without error.
        dispatch_auth_list().expect("auth list should not error");
    }

    #[test]
    fn auth_status_returns_none_when_no_creds() {
        // Use a fresh temp dir with no credentials
        let config_home = temp_config_home();
        std::fs::create_dir_all(&config_home).expect("create temp dir");

        // We need the env lock from runtime — replicate the pattern
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        let _guard = LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let prev = std::env::var_os("ELAI_CONFIG_HOME");
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        let prev_api_key = std::env::var_os("ANTHROPIC_API_KEY");
        let prev_auth_token = std::env::var_os("ANTHROPIC_AUTH_TOKEN");
        let prev_oauth_token = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN");
        let prev_fd = std::env::var_os("CLAUDE_CODE_API_KEY_FD");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_API_KEY_FD");

        // Capture via JSON output
        let info = collect_auth_info();
        let json = auth_info_to_json(&info);
        assert_eq!(
            json.get("method").and_then(|v| v.as_str()),
            Some("none"),
            "expected method=none, got {json}"
        );

        // Restore
        match prev {
            Some(v) => std::env::set_var("ELAI_CONFIG_HOME", v),
            None => std::env::remove_var("ELAI_CONFIG_HOME"),
        }
        match prev_api_key {
            Some(v) => std::env::set_var("ANTHROPIC_API_KEY", v),
            None => std::env::remove_var("ANTHROPIC_API_KEY"),
        }
        match prev_auth_token {
            Some(v) => std::env::set_var("ANTHROPIC_AUTH_TOKEN", v),
            None => std::env::remove_var("ANTHROPIC_AUTH_TOKEN"),
        }
        match prev_oauth_token {
            Some(v) => std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", v),
            None => std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN"),
        }
        match prev_fd {
            Some(v) => std::env::set_var("CLAUDE_CODE_API_KEY_FD", v),
            None => std::env::remove_var("CLAUDE_CODE_API_KEY_FD"),
        }

        std::fs::remove_dir_all(&config_home).ok();
    }
}
