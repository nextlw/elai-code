use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runtime::{
    load_oauth_credentials, save_oauth_credentials, OAuthConfig, OAuthRefreshRequest,
    OAuthTokenExchangeRequest,
};
use serde::Deserialize;

use crate::error::ApiError;

use super::{Provider, ProviderFuture};
use crate::sse::SseParser;
use crate::types::{MessageRequest, MessageResponse, StreamEvent};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const REQUEST_ID_HEADER: &str = "request-id";
const ALT_REQUEST_ID_HEADER: &str = "x-request-id";
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_millis(200);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(2);
const DEFAULT_MAX_RETRIES: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoofMode {
    None,
    /// Mascarar como Claude Code 2.1.x (billing header + Stainless headers + system relocation + PascalCase MCP).
    ClaudeCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    None,
    ApiKey(String),
    BearerToken(String),
    ApiKeyAndBearer {
        api_key: String,
        bearer_token: String,
    },
}

impl AuthSource {
    pub fn from_env() -> Result<Self, ApiError> {
        let api_key = read_env_non_empty("ANTHROPIC_API_KEY")?;
        let auth_token = read_env_non_empty("ANTHROPIC_AUTH_TOKEN")?;
        match (api_key, auth_token) {
            (Some(api_key), Some(bearer_token)) => Ok(Self::ApiKeyAndBearer {
                api_key,
                bearer_token,
            }),
            (Some(api_key), None) => Ok(Self::ApiKey(api_key)),
            (None, Some(bearer_token)) => Ok(Self::BearerToken(bearer_token)),
            (None, None) => Err(ApiError::missing_credentials(
                "Elai",
                &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
            )),
        }
    }

    #[must_use]
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(api_key) | Self::ApiKeyAndBearer { api_key, .. } => Some(api_key),
            Self::None | Self::BearerToken(_) => None,
        }
    }

    #[must_use]
    pub fn bearer_token(&self) -> Option<&str> {
        match self {
            Self::BearerToken(token)
            | Self::ApiKeyAndBearer {
                bearer_token: token,
                ..
            } => Some(token),
            Self::None | Self::ApiKey(_) => None,
        }
    }

    #[must_use]
    pub fn masked_authorization_header(&self) -> &'static str {
        if self.bearer_token().is_some() {
            "Bearer [REDACTED]"
        } else {
            "<absent>"
        }
    }

    pub fn apply(&self, mut request_builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = self.api_key() {
            request_builder = request_builder.header("x-api-key", api_key);
        }
        if let Some(token) = self.bearer_token() {
            request_builder = request_builder.bearer_auth(token);
        }
        request_builder
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OAuthTokenSet {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

impl From<OAuthTokenSet> for AuthSource {
    fn from(value: OAuthTokenSet) -> Self {
        Self::BearerToken(value.access_token)
    }
}

impl From<&runtime::AuthMethod> for AuthSource {
    fn from(method: &runtime::AuthMethod) -> Self {
        match method {
            runtime::AuthMethod::ClaudeAiOAuth { token_set, .. } => {
                Self::BearerToken(token_set.access_token.clone())
            }
            runtime::AuthMethod::ConsoleApiKey { api_key, .. } => {
                Self::ApiKey(api_key.clone())
            }
            runtime::AuthMethod::AnthropicAuthToken { token } => {
                Self::BearerToken(token.clone())
            }
            // 3P: HTTP layer não envia auth — provider client (Bedrock/Vertex/Foundry) usa creds próprias.
            // OpenAiApiKey: o ElaiApiClient (Anthropic) ignora — quem usa essa
            // credencial é o `OpenAiCompatProvider`. Se o usuário tentar
            // chamar um modelo Anthropic só com `OpenAiApiKey` salvo, a
            // autenticação falha cedo (esperado).
            runtime::AuthMethod::Bedrock
            | runtime::AuthMethod::Vertex
            | runtime::AuthMethod::Foundry
            | runtime::AuthMethod::OpenAiApiKey { .. } => Self::None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ElaiApiClient {
    http: reqwest::Client,
    auth: AuthSource,
    base_url: String,
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    spoof_mode: SpoofMode,
    cli_version: String,
}

impl ElaiApiClient {
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth: AuthSource::ApiKey(api_key.into()),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            spoof_mode: SpoofMode::None,
            cli_version: super::claude_code_spoof::CLAUDE_CODE_VERSION_FALLBACK.to_string(),
        }
    }

    #[must_use]
    pub fn from_auth(auth: AuthSource) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff: DEFAULT_INITIAL_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            spoof_mode: SpoofMode::None,
            cli_version: super::claude_code_spoof::CLAUDE_CODE_VERSION_FALLBACK.to_string(),
        }
    }

    pub fn from_env() -> Result<Self, ApiError> {
        // Usa `resolve_startup_auth_source` em vez do legado `from_env_or_saved`
        // para enxergar o `AuthMethod::ClaudeAiOAuth` salvo em `credentials.json`
        // pela chave nova `auth` (além das env vars e da chave legada `oauth`).
        // A closure retorna `None`: refresh transparente de token expirado exige
        // `OAuthConfig`, que vive no crate `runtime`/CLI; aqui só conseguimos
        // usar tokens válidos. Token expirado → erro pedindo novo login.
        let auth = resolve_startup_auth_source(|| Ok(None))?;
        let spoof = detect_spoof_mode_for_auth(&auth);
        Ok(Self::from_auth(auth).with_base_url(read_base_url()).with_spoof_mode(spoof))
    }

    #[must_use]
    pub fn with_auth_source(mut self, auth: AuthSource) -> Self {
        self.auth = auth;
        self
    }

    #[must_use]
    pub fn with_auth_token(mut self, auth_token: Option<String>) -> Self {
        match (
            self.auth.api_key().map(ToOwned::to_owned),
            auth_token.filter(|token| !token.is_empty()),
        ) {
            (Some(api_key), Some(bearer_token)) => {
                self.auth = AuthSource::ApiKeyAndBearer {
                    api_key,
                    bearer_token,
                };
            }
            (Some(api_key), None) => {
                self.auth = AuthSource::ApiKey(api_key);
            }
            (None, Some(bearer_token)) => {
                self.auth = AuthSource::BearerToken(bearer_token);
            }
            (None, None) => {
                self.auth = AuthSource::None;
            }
        }
        self
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    #[must_use]
    pub fn with_spoof_mode(mut self, mode: SpoofMode) -> Self {
        self.spoof_mode = mode;
        self
    }

    #[must_use]
    pub fn with_cli_version(mut self, version: impl Into<String>) -> Self {
        self.cli_version = version.into();
        self
    }

    #[must_use]
    pub fn spoof_mode(&self) -> SpoofMode {
        self.spoof_mode
    }

    #[must_use]
    pub fn with_retry_policy(
        mut self,
        max_retries: u32,
        initial_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        self.max_retries = max_retries;
        self.initial_backoff = initial_backoff;
        self.max_backoff = max_backoff;
        self
    }

    #[must_use]
    pub fn auth_source(&self) -> &AuthSource {
        &self.auth
    }

    pub async fn send_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageResponse, ApiError> {
        let request = MessageRequest {
            stream: false,
            ..request.clone()
        };
        let (response, modified_tools) = self.send_with_retry(&request).await?;
        let request_id = request_id_from_headers(response.headers());
        let mut body: serde_json::Value = response.json().await.map_err(ApiError::from)?;
        if !modified_tools.is_empty() {
            super::claude_code_spoof::apply_response_transform(&mut body, &modified_tools);
        }
        let mut parsed: MessageResponse = serde_json::from_value(body)
            .map_err(|e| ApiError::Auth(format!("parse message response: {e}")))?;
        if parsed.request_id.is_none() {
            parsed.request_id = request_id;
        }
        Ok(parsed)
    }

    pub async fn stream_message(
        &self,
        request: &MessageRequest,
    ) -> Result<MessageStream, ApiError> {
        let (response, modified_tools) = self
            .send_with_retry(&request.clone().with_streaming())
            .await?;
        Ok(MessageStream {
            request_id: request_id_from_headers(response.headers()),
            response,
            parser: SseParser::new(),
            pending: VecDeque::new(),
            done: false,
            modified_tools,
        })
    }

    pub async fn exchange_oauth_code(
        &self,
        config: &OAuthConfig,
        request: &OAuthTokenExchangeRequest,
    ) -> Result<OAuthTokenSet, ApiError> {
        self.exchange_oauth_code_with_headers(config, request, &[]).await
    }

    pub async fn exchange_oauth_code_with_headers(
        &self,
        config: &OAuthConfig,
        request: &OAuthTokenExchangeRequest,
        extra_headers: &[(&str, &str)],
    ) -> Result<OAuthTokenSet, ApiError> {
        let mut builder = self
            .http
            .post(&config.token_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .form(&request.form_params());
        for (k, v) in extra_headers {
            builder = builder.header(*k, *v);
        }
        let response = builder.send().await.map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        response
            .json::<OAuthTokenSet>()
            .await
            .map_err(ApiError::from)
    }

    pub async fn refresh_oauth_token(
        &self,
        config: &OAuthConfig,
        request: &OAuthRefreshRequest,
    ) -> Result<OAuthTokenSet, ApiError> {
        self.refresh_oauth_token_with_headers(config, request, &[]).await
    }

    pub async fn refresh_oauth_token_with_headers(
        &self,
        config: &OAuthConfig,
        request: &OAuthRefreshRequest,
        extra_headers: &[(&str, &str)],
    ) -> Result<OAuthTokenSet, ApiError> {
        let mut builder = self
            .http
            .post(&config.token_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .form(&request.form_params());
        for (k, v) in extra_headers {
            builder = builder.header(*k, *v);
        }
        let response = builder.send().await.map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        response
            .json::<OAuthTokenSet>()
            .await
            .map_err(ApiError::from)
    }

    pub async fn create_console_api_key(
        &self,
        endpoints: &runtime::AnthropicOAuthEndpoints,
        access_token: &str,
    ) -> Result<String, ApiError> {
        let response = self
            .http
            .post(&endpoints.api_key_url)
            .bearer_auth(access_token)
            .header("anthropic-beta", &endpoints.beta_header)
            .header("content-type", "application/json")
            .send()
            .await
            .map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        let body: serde_json::Value = response.json().await.map_err(ApiError::from)?;
        body.get("raw_key")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| ApiError::Auth("create_api_key response missing raw_key".to_string()))
    }

    pub async fn fetch_roles(
        &self,
        endpoints: &runtime::AnthropicOAuthEndpoints,
        access_token: &str,
    ) -> Result<serde_json::Value, ApiError> {
        let response = self
            .http
            .get(&endpoints.roles_url)
            .bearer_auth(access_token)
            .header("anthropic-beta", &endpoints.beta_header)
            .send()
            .await
            .map_err(ApiError::from)?;
        let response = expect_success(response).await?;
        response.json::<serde_json::Value>().await.map_err(ApiError::from)
    }

    async fn send_with_retry(
        &self,
        request: &MessageRequest,
    ) -> Result<(reqwest::Response, Vec<String>), ApiError> {
        let mut attempts = 0;
        let mut last_error: Option<ApiError>;

        loop {
            attempts += 1;
            match self.send_raw_request(request).await {
                Ok((response, modified_tools)) => match expect_success(response).await {
                    Ok(response) => return Ok((response, modified_tools)),
                    Err(error) if error.is_retryable() && attempts <= self.max_retries + 1 => {
                        last_error = Some(error);
                    }
                    Err(error) => return Err(error),
                },
                Err(error) if error.is_retryable() && attempts <= self.max_retries + 1 => {
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }

            if attempts > self.max_retries {
                break;
            }

            tokio::time::sleep(self.backoff_for_attempt(attempts)?).await;
        }

        Err(ApiError::RetriesExhausted {
            attempts,
            last_error: Box::new(last_error.expect("retry loop must capture an error")),
        })
    }

    async fn send_raw_request(
        &self,
        request: &MessageRequest,
    ) -> Result<(reqwest::Response, Vec<String>), ApiError> {
        let request_url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let mut body = serde_json::to_value(request)
            .map_err(|e| ApiError::Auth(format!("serialize request body: {e}")))?;

        let modified_tools = if self.spoof_mode == SpoofMode::ClaudeCode {
            super::claude_code_spoof::apply_request_transform(&mut body, &self.cli_version)
        } else {
            Vec::new()
        };

        let mut builder = self
            .http
            .post(&request_url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");
        builder = self.auth.apply(builder);

        if self.spoof_mode == SpoofMode::ClaudeCode {
            for (k, v) in super::claude_code_spoof::stainless_headers(&self.cli_version) {
                builder = builder.header(k, v);
            }
            builder = builder.header(
                "anthropic-beta",
                super::claude_code_spoof::extra_oauth_betas().join(","),
            );
            builder = builder.query(&[("beta", "true")]);
        }

        builder = builder.json(&body);
        let response = builder.send().await.map_err(ApiError::from)?;
        Ok((response, modified_tools))
    }

    fn backoff_for_attempt(&self, attempt: u32) -> Result<Duration, ApiError> {
        let Some(multiplier) = 1_u32.checked_shl(attempt.saturating_sub(1)) else {
            return Err(ApiError::BackoffOverflow {
                attempt,
                base_delay: self.initial_backoff,
            });
        };
        Ok(self
            .initial_backoff
            .checked_mul(multiplier)
            .map_or(self.max_backoff, |delay| delay.min(self.max_backoff)))
    }
}

impl AuthSource {
    pub fn from_env_or_saved() -> Result<Self, ApiError> {
        if let Some(api_key) = read_env_non_empty("ANTHROPIC_API_KEY")? {
            return match read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
                Some(bearer_token) => Ok(Self::ApiKeyAndBearer {
                    api_key,
                    bearer_token,
                }),
                None => Ok(Self::ApiKey(api_key)),
            };
        }
        if let Some(bearer_token) = read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
            return Ok(Self::BearerToken(bearer_token));
        }
        match load_saved_oauth_token() {
            Ok(Some(token_set)) if oauth_token_is_expired(&token_set) => {
                if token_set.refresh_token.is_some() {
                    Err(ApiError::Auth(
                        "saved OAuth token is expired; load runtime OAuth config to refresh it"
                            .to_string(),
                    ))
                } else {
                    Err(ApiError::ExpiredOAuthToken)
                }
            }
            Ok(Some(token_set)) => Ok(Self::BearerToken(token_set.access_token)),
            Ok(None) => Err(ApiError::missing_credentials(
                "Elai",
                &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
            )),
            Err(error) => Err(error),
        }
    }
}

#[must_use]
pub fn oauth_token_is_expired(token_set: &OAuthTokenSet) -> bool {
    token_set
        .expires_at
        .is_some_and(|expires_at| expires_at <= now_unix_timestamp())
}

pub fn resolve_saved_oauth_token(config: &OAuthConfig) -> Result<Option<OAuthTokenSet>, ApiError> {
    let Some(token_set) = load_saved_oauth_token()? else {
        return Ok(None);
    };
    resolve_saved_oauth_token_set(config, token_set).map(Some)
}

pub fn has_auth_from_env_or_saved() -> Result<bool, ApiError> {
    Ok(read_env_non_empty("ANTHROPIC_API_KEY")?.is_some()
        || read_env_non_empty("ANTHROPIC_AUTH_TOKEN")?.is_some()
        || read_env_non_empty("CLAUDE_CODE_OAUTH_TOKEN")?.is_some()
        || load_saved_oauth_token()?.is_some()
        || runtime::load_auth_method().map_err(ApiError::from)?.is_some())
}

pub fn resolve_startup_auth_source<F>(load_oauth_config: F) -> Result<AuthSource, ApiError>
where
    F: FnOnce() -> Result<Option<OAuthConfig>, ApiError>,
{
    // 1. FD via CLAUDE_CODE_API_KEY_FD
    if let Some(key) = read_api_key_from_fd_env()? {
        return Ok(AuthSource::ApiKey(key));
    }
    // 2. apiKeyHelper — pulado por enquanto (settings.json wiring fica para a CLI layer);
    // TODO: wire apiKeyHelper from settings.json in CLI layer

    // 3. ANTHROPIC_API_KEY env (+ opcional ANTHROPIC_AUTH_TOKEN)
    if let Some(api_key) = read_env_non_empty("ANTHROPIC_API_KEY")? {
        return match read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
            Some(bearer) => Ok(AuthSource::ApiKeyAndBearer { api_key, bearer_token: bearer }),
            None => Ok(AuthSource::ApiKey(api_key)),
        };
    }
    // 4. ANTHROPIC_AUTH_TOKEN ou CLAUDE_CODE_OAUTH_TOKEN
    if let Some(token) = read_env_non_empty("ANTHROPIC_AUTH_TOKEN")? {
        return Ok(AuthSource::BearerToken(token));
    }
    if let Some(token) = read_env_non_empty("CLAUDE_CODE_OAUTH_TOKEN")? {
        return Ok(AuthSource::BearerToken(token));
    }
    // 5. AuthMethod salvo (refresh transparente)
    if let Some(method) = runtime::load_auth_method().map_err(ApiError::from)? {
        return resolve_from_method(method, load_oauth_config);
    }
    // 6. Fallback legado: oauth-key em credentials.json (mantido por uma versão)
    if let Some(token_set) = load_saved_oauth_token()? {
        if !oauth_token_is_expired(&token_set) {
            return Ok(AuthSource::BearerToken(token_set.access_token));
        }
        if token_set.refresh_token.is_none() {
            return Err(ApiError::ExpiredOAuthToken);
        }
        let Some(config) = load_oauth_config()? else {
            return Err(ApiError::Auth(
                "saved OAuth token is expired; runtime OAuth config is missing".to_string(),
            ));
        };
        return Ok(AuthSource::from(resolve_saved_oauth_token_set(&config, token_set)?));
    }
    // 7. erro
    Err(ApiError::missing_credentials("Elai", &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]))
}

fn resolve_from_method<F>(
    method: runtime::AuthMethod,
    load_oauth_config: F,
) -> Result<AuthSource, ApiError>
where
    F: FnOnce() -> Result<Option<OAuthConfig>, ApiError>,
{
    match method {
        runtime::AuthMethod::ConsoleApiKey { api_key, .. } => Ok(AuthSource::ApiKey(api_key)),
        runtime::AuthMethod::AnthropicAuthToken { token } => Ok(AuthSource::BearerToken(token)),
        runtime::AuthMethod::Bedrock
        | runtime::AuthMethod::Vertex
        | runtime::AuthMethod::Foundry
        | runtime::AuthMethod::OpenAiApiKey { .. } => Ok(AuthSource::None),
        runtime::AuthMethod::ClaudeAiOAuth {
            token_set,
            subscription,
        } => {
            let local_token_set = OAuthTokenSet {
                access_token: token_set.access_token.clone(),
                refresh_token: token_set.refresh_token.clone(),
                expires_at: token_set.expires_at,
                scopes: token_set.scopes.clone(),
            };
            if !oauth_token_is_expired(&local_token_set) {
                return Ok(AuthSource::BearerToken(local_token_set.access_token));
            }
            if local_token_set.refresh_token.is_none() {
                return Err(ApiError::ExpiredOAuthToken);
            }
            let Some(config) = load_oauth_config()? else {
                return Err(ApiError::Auth(
                    "saved OAuth token is expired; runtime OAuth config is missing".to_string(),
                ));
            };
            let refreshed = resolve_saved_oauth_token_set(&config, local_token_set)?;
            // Persist refreshed token back as AuthMethod
            runtime::save_auth_method(&runtime::AuthMethod::ClaudeAiOAuth {
                token_set: runtime::OAuthTokenSet {
                    access_token: refreshed.access_token.clone(),
                    refresh_token: refreshed.refresh_token.clone(),
                    expires_at: refreshed.expires_at,
                    scopes: refreshed.scopes.clone(),
                },
                subscription,
            })
            .map_err(ApiError::from)?;
            Ok(AuthSource::BearerToken(refreshed.access_token))
        }
    }
}

fn resolve_saved_oauth_token_set(
    config: &OAuthConfig,
    token_set: OAuthTokenSet,
) -> Result<OAuthTokenSet, ApiError> {
    if !oauth_token_is_expired(&token_set) {
        return Ok(token_set);
    }
    let Some(refresh_token) = token_set.refresh_token.clone() else {
        return Err(ApiError::ExpiredOAuthToken);
    };
    let client = ElaiApiClient::from_auth(AuthSource::None).with_base_url(read_base_url());
    let refreshed = client_runtime_block_on(async {
        client
            .refresh_oauth_token(
                config,
                &OAuthRefreshRequest::from_config(
                    config,
                    refresh_token,
                    Some(token_set.scopes.clone()),
                ),
            )
            .await
    })?;
    let resolved = OAuthTokenSet {
        access_token: refreshed.access_token,
        refresh_token: refreshed.refresh_token.or(token_set.refresh_token),
        expires_at: refreshed.expires_at,
        scopes: refreshed.scopes,
    };
    save_oauth_credentials(&runtime::OAuthTokenSet {
        access_token: resolved.access_token.clone(),
        refresh_token: resolved.refresh_token.clone(),
        expires_at: resolved.expires_at,
        scopes: resolved.scopes.clone(),
    })
    .map_err(ApiError::from)?;
    Ok(resolved)
}

fn client_runtime_block_on<F, T>(future: F) -> Result<T, ApiError>
where
    F: std::future::Future<Output = Result<T, ApiError>>,
{
    tokio::runtime::Runtime::new()
        .map_err(ApiError::from)?
        .block_on(future)
}

fn load_saved_oauth_token() -> Result<Option<OAuthTokenSet>, ApiError> {
    let token_set = load_oauth_credentials().map_err(ApiError::from)?;
    Ok(token_set.map(|token_set| OAuthTokenSet {
        access_token: token_set.access_token,
        refresh_token: token_set.refresh_token,
        expires_at: token_set.expires_at,
        scopes: token_set.scopes,
    }))
}

fn now_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn read_env_non_empty(key: &str) -> Result<Option<String>, ApiError> {
    match std::env::var(key) {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(ApiError::from(error)),
    }
}

/// Read an API key from the file descriptor named in `CLAUDE_CODE_API_KEY_FD`.
/// On Unix: reads via `/dev/fd/{fd}` (follows the Claude Code convention). On Windows: returns None.
#[cfg(unix)]
fn read_api_key_from_fd_env() -> Result<Option<String>, ApiError> {
    let fd_str = match std::env::var("CLAUDE_CODE_API_KEY_FD") {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };
    let fd: i32 = match fd_str.parse() {
        Ok(fd) => fd,
        Err(_) => return Ok(None),
    };
    let path = format!("/dev/fd/{fd}");
    let mut file = std::fs::File::open(&path).map_err(ApiError::from)?;
    let mut contents = String::new();
    use std::io::Read as _;
    file.read_to_string(&mut contents).map_err(ApiError::from)?;
    let key = contents.trim().to_string();
    if key.is_empty() {
        return Ok(None);
    }
    Ok(Some(key))
}

#[cfg(not(unix))]
fn read_api_key_from_fd_env() -> Result<Option<String>, ApiError> {
    Ok(None)
}

#[cfg(test)]
fn read_api_key() -> Result<String, ApiError> {
    let auth = AuthSource::from_env_or_saved()?;
    auth.api_key()
        .or_else(|| auth.bearer_token())
        .map(ToOwned::to_owned)
        .ok_or(ApiError::missing_credentials(
            "Elai",
            &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
        ))
}

#[cfg(test)]
fn read_auth_token() -> Option<String> {
    read_env_non_empty("ANTHROPIC_AUTH_TOKEN")
        .ok()
        .and_then(std::convert::identity)
}

/// Detect whether to auto-activate spoof mode based on the resolved auth source.
///
/// Spoof mode is also activatable manually via [`ElaiApiClient::with_spoof_mode`].
fn detect_spoof_mode_for_auth(auth: &AuthSource) -> SpoofMode {
    let is_bearer = matches!(auth, AuthSource::BearerToken(_) | AuthSource::ApiKeyAndBearer { .. });
    if !is_bearer {
        return SpoofMode::None;
    }
    match runtime::load_auth_method() {
        Ok(Some(runtime::AuthMethod::ClaudeAiOAuth { .. }))
        | Ok(Some(runtime::AuthMethod::AnthropicAuthToken { .. })) => SpoofMode::ClaudeCode,
        _ => SpoofMode::None,
    }
}

#[must_use]
pub fn read_base_url() -> String {
    std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string())
}

#[must_use]
pub fn base_url_is_anthropic_official() -> bool {
    let url = read_base_url();
    url == DEFAULT_BASE_URL
        || url == "https://api.anthropic.com"
        || url == "https://api.anthropic.com/"
}

fn request_id_from_headers(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .or_else(|| headers.get(ALT_REQUEST_ID_HEADER))
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

impl Provider for ElaiApiClient {
    type Stream = MessageStream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse> {
        Box::pin(async move { self.send_message(request).await })
    }

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream> {
        Box::pin(async move { self.stream_message(request).await })
    }
}

#[derive(Debug)]
pub struct MessageStream {
    request_id: Option<String>,
    response: reqwest::Response,
    parser: SseParser,
    pending: VecDeque<StreamEvent>,
    done: bool,
    modified_tools: Vec<String>,
}

impl MessageStream {
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(self.maybe_reverse_spoof(event)));
            }

            if self.done {
                let remaining = self.parser.finish()?;
                self.pending.extend(remaining);
                if let Some(event) = self.pending.pop_front() {
                    return Ok(Some(self.maybe_reverse_spoof(event)));
                }
                return Ok(None);
            }

            match self.response.chunk().await? {
                Some(chunk) => {
                    self.pending.extend(self.parser.push(&chunk)?);
                }
                None => {
                    self.done = true;
                }
            }
        }
    }

    fn maybe_reverse_spoof(&self, event: StreamEvent) -> StreamEvent {
        if self.modified_tools.is_empty() {
            return event;
        }
        let mut event_value = match serde_json::to_value(&event) {
            Ok(v) => v,
            Err(_) => return event,
        };
        super::claude_code_spoof::reverse_pascalcase_mcp_in_streaming_event(
            &mut event_value,
            &self.modified_tools,
        );
        match serde_json::from_value::<StreamEvent>(event_value) {
            Ok(transformed) => transformed,
            Err(_) => event,
        }
    }
}

async fn expect_success(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_else(|_| String::new());
    let parsed_error = serde_json::from_str::<ApiErrorEnvelope>(&body).ok();
    let retryable = is_retryable_status(status);

    Err(ApiError::Api {
        status,
        error_type: parsed_error
            .as_ref()
            .map(|error| error.error.error_type.clone()),
        message: parsed_error
            .as_ref()
            .map(|error| error.error.message.clone()),
        body,
        retryable,
    })
}

const fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429 | 500 | 502 | 503 | 504)
}

#[derive(Debug, Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::{ALT_REQUEST_ID_HEADER, REQUEST_ID_HEADER};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use runtime::{clear_oauth_credentials, save_oauth_credentials, OAuthConfig};

    use super::{
        now_unix_timestamp, oauth_token_is_expired, resolve_saved_oauth_token,
        resolve_startup_auth_source, AuthSource, ElaiApiClient, OAuthTokenSet,
    };
    use crate::types::{ContentBlockDelta, MessageRequest};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn temp_config_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "api-oauth-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn cleanup_temp_config_home(config_home: &std::path::Path) {
        match std::fs::remove_dir_all(config_home) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cleanup temp dir: {error}"),
        }
    }

    fn sample_oauth_config(token_url: String) -> OAuthConfig {
        OAuthConfig {
            client_id: "runtime-client".to_string(),
            authorize_url: "https://console.test/oauth/authorize".to_string(),
            token_url,
            callback_port: Some(4545),
            manual_redirect_url: Some("https://console.test/oauth/callback".to_string()),
            scopes: vec!["org:read".to_string(), "user:write".to_string()],
        }
    }

    fn spawn_token_server(response_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).expect("read request");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        format!("http://{address}/oauth/token")
    }

    #[test]
    fn read_api_key_requires_presence() {
        let _guard = env_lock();
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ELAI_CONFIG_HOME");
        let error = super::read_api_key().expect_err("missing key should error");
        assert!(matches!(
            error,
            crate::error::ApiError::MissingCredentials { .. }
        ));
    }

    #[test]
    fn read_api_key_requires_non_empty_value() {
        let _guard = env_lock();
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "");
        std::env::remove_var("ANTHROPIC_API_KEY");
        let error = super::read_api_key().expect_err("empty key should error");
        assert!(matches!(
            error,
            crate::error::ApiError::MissingCredentials { .. }
        ));
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    }

    #[test]
    fn read_api_key_prefers_api_key_env() {
        let _guard = env_lock();
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");
        std::env::set_var("ANTHROPIC_API_KEY", "legacy-key");
        assert_eq!(
            super::read_api_key().expect("api key should load"),
            "legacy-key"
        );
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn read_auth_token_reads_auth_token_env() {
        let _guard = env_lock();
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");
        assert_eq!(super::read_auth_token().as_deref(), Some("auth-token"));
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    }

    #[test]
    fn oauth_token_maps_to_bearer_auth_source() {
        let auth = AuthSource::from(OAuthTokenSet {
            access_token: "access-token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(123),
            scopes: vec!["scope:a".to_string()],
        });
        assert_eq!(auth.bearer_token(), Some("access-token"));
        assert_eq!(auth.api_key(), None);
    }

    #[test]
    fn auth_source_from_env_combines_api_key_and_bearer_token() {
        let _guard = env_lock();
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "auth-token");
        std::env::set_var("ANTHROPIC_API_KEY", "legacy-key");
        let auth = AuthSource::from_env().expect("env auth");
        assert_eq!(auth.api_key(), Some("legacy-key"));
        assert_eq!(auth.bearer_token(), Some("auth-token"));
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    #[test]
    fn auth_source_from_saved_oauth_when_env_absent() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "saved-access-token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_unix_timestamp() + 300),
            scopes: vec!["scope:a".to_string()],
        })
        .expect("save oauth credentials");

        let auth = AuthSource::from_env_or_saved().expect("saved auth");
        assert_eq!(auth.bearer_token(), Some("saved-access-token"));

        clear_oauth_credentials().expect("clear credentials");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[test]
    fn oauth_token_expiry_uses_expires_at_timestamp() {
        assert!(oauth_token_is_expired(&OAuthTokenSet {
            access_token: "access-token".to_string(),
            refresh_token: None,
            expires_at: Some(1),
            scopes: Vec::new(),
        }));
        assert!(!oauth_token_is_expired(&OAuthTokenSet {
            access_token: "access-token".to_string(),
            refresh_token: None,
            expires_at: Some(now_unix_timestamp() + 60),
            scopes: Vec::new(),
        }));
    }

    #[test]
    fn resolve_saved_oauth_token_refreshes_expired_credentials() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "expired-access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(1),
            scopes: vec!["scope:a".to_string()],
        })
        .expect("save expired oauth credentials");

        let token_url = spawn_token_server(
            "{\"access_token\":\"refreshed-token\",\"refresh_token\":\"fresh-refresh\",\"expires_at\":9999999999,\"scopes\":[\"scope:a\"]}",
        );
        let resolved = resolve_saved_oauth_token(&sample_oauth_config(token_url))
            .expect("resolve refreshed token")
            .expect("token set present");
        assert_eq!(resolved.access_token, "refreshed-token");
        let stored = runtime::load_oauth_credentials()
            .expect("load stored credentials")
            .expect("stored token set");
        assert_eq!(stored.access_token, "refreshed-token");

        clear_oauth_credentials().expect("clear credentials");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[test]
    fn resolve_startup_auth_source_uses_saved_oauth_without_loading_config() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "saved-access-token".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_unix_timestamp() + 300),
            scopes: vec!["scope:a".to_string()],
        })
        .expect("save oauth credentials");

        let auth = resolve_startup_auth_source(|| panic!("config should not be loaded"))
            .expect("startup auth");
        assert_eq!(auth.bearer_token(), Some("saved-access-token"));

        clear_oauth_credentials().expect("clear credentials");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[test]
    fn resolve_startup_auth_source_errors_when_refreshable_token_lacks_config() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "expired-access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(1),
            scopes: vec!["scope:a".to_string()],
        })
        .expect("save expired oauth credentials");

        let error =
            resolve_startup_auth_source(|| Ok(None)).expect_err("missing config should error");
        assert!(
            matches!(error, crate::error::ApiError::Auth(message) if message.contains("runtime OAuth config is missing"))
        );

        let stored = runtime::load_oauth_credentials()
            .expect("load stored credentials")
            .expect("stored token set");
        assert_eq!(stored.access_token, "expired-access-token");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-token"));

        clear_oauth_credentials().expect("clear credentials");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[test]
    fn resolve_saved_oauth_token_preserves_refresh_token_when_refresh_response_omits_it() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("ANTHROPIC_API_KEY");
        save_oauth_credentials(&runtime::OAuthTokenSet {
            access_token: "expired-access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            expires_at: Some(1),
            scopes: vec!["scope:a".to_string()],
        })
        .expect("save expired oauth credentials");

        let token_url = spawn_token_server(
            "{\"access_token\":\"refreshed-token\",\"expires_at\":9999999999,\"scopes\":[\"scope:a\"]}",
        );
        let resolved = resolve_saved_oauth_token(&sample_oauth_config(token_url))
            .expect("resolve refreshed token")
            .expect("token set present");
        assert_eq!(resolved.access_token, "refreshed-token");
        assert_eq!(resolved.refresh_token.as_deref(), Some("refresh-token"));
        let stored = runtime::load_oauth_credentials()
            .expect("load stored credentials")
            .expect("stored token set");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-token"));

        clear_oauth_credentials().expect("clear credentials");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[test]
    fn message_request_stream_helper_sets_stream_true() {
        let request = MessageRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 64,
            messages: vec![],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
        };

        assert!(request.with_streaming().stream);
    }

    #[test]
    fn backoff_doubles_until_maximum() {
        let client = ElaiApiClient::new("test-key").with_retry_policy(
            3,
            Duration::from_millis(10),
            Duration::from_millis(25),
        );
        assert_eq!(
            client.backoff_for_attempt(1).expect("attempt 1"),
            Duration::from_millis(10)
        );
        assert_eq!(
            client.backoff_for_attempt(2).expect("attempt 2"),
            Duration::from_millis(20)
        );
        assert_eq!(
            client.backoff_for_attempt(3).expect("attempt 3"),
            Duration::from_millis(25)
        );
    }

    #[test]
    fn retryable_statuses_are_detected() {
        assert!(super::is_retryable_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(super::is_retryable_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(!super::is_retryable_status(
            reqwest::StatusCode::UNAUTHORIZED
        ));
    }

    #[test]
    fn tool_delta_variant_round_trips() {
        let delta = ContentBlockDelta::InputJsonDelta {
            partial_json: "{\"city\":\"Paris\"}".to_string(),
        };
        let encoded = serde_json::to_string(&delta).expect("delta should serialize");
        let decoded: ContentBlockDelta =
            serde_json::from_str(&encoded).expect("delta should deserialize");
        assert_eq!(decoded, delta);
    }

    #[test]
    fn request_id_uses_primary_or_fallback_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(REQUEST_ID_HEADER, "req_primary".parse().expect("header"));
        assert_eq!(
            super::request_id_from_headers(&headers).as_deref(),
            Some("req_primary")
        );

        headers.clear();
        headers.insert(
            ALT_REQUEST_ID_HEADER,
            "req_fallback".parse().expect("header"),
        );
        assert_eq!(
            super::request_id_from_headers(&headers).as_deref(),
            Some("req_fallback")
        );
    }

    #[test]
    fn auth_source_applies_headers() {
        let auth = AuthSource::ApiKeyAndBearer {
            api_key: "test-key".to_string(),
            bearer_token: "proxy-token".to_string(),
        };
        let request = auth
            .apply(reqwest::Client::new().post("https://example.test"))
            .build()
            .expect("request build");
        let headers = request.headers();
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("test-key")
        );
        assert_eq!(
            headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer proxy-token")
        );
    }

    // --- New tests for Layer 2 auth ---

    #[test]
    fn auth_source_from_claude_ai_oauth_method_returns_bearer() {
        let method = runtime::AuthMethod::ClaudeAiOAuth {
            token_set: runtime::OAuthTokenSet {
                access_token: "bearer-from-oauth".to_string(),
                refresh_token: Some("refresh".to_string()),
                expires_at: Some(9_999_999_999),
                scopes: vec!["user:profile".to_string()],
            },
            subscription: None,
        };
        let auth = AuthSource::from(&method);
        assert_eq!(auth.bearer_token(), Some("bearer-from-oauth"));
        assert_eq!(auth.api_key(), None);
    }

    #[test]
    fn auth_source_from_console_api_key_method_returns_api_key() {
        let method = runtime::AuthMethod::ConsoleApiKey {
            api_key: "sk-ant-console-key".to_string(),
            origin: runtime::ApiKeyOrigin::ConsoleOAuth,
        };
        let auth = AuthSource::from(&method);
        assert_eq!(auth.api_key(), Some("sk-ant-console-key"));
        assert_eq!(auth.bearer_token(), None);
    }

    #[test]
    fn auth_source_from_3p_method_returns_none() {
        for method in [
            runtime::AuthMethod::Bedrock,
            runtime::AuthMethod::Vertex,
            runtime::AuthMethod::Foundry,
        ] {
            let auth = AuthSource::from(&method);
            assert_eq!(auth, AuthSource::None, "expected None for {method:?}");
        }
    }

    #[test]
    fn resolve_startup_prefers_anthropic_api_key_env_over_saved_method() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::set_var("ANTHROPIC_API_KEY", "env-api-key");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_API_KEY_FD");
        runtime::save_auth_method(&runtime::AuthMethod::ConsoleApiKey {
            api_key: "saved-key".to_string(),
            origin: runtime::ApiKeyOrigin::ConsoleOAuth,
        })
        .expect("save auth method");

        let auth = resolve_startup_auth_source(|| Ok(None)).expect("startup auth");
        assert_eq!(auth.api_key(), Some("env-api-key"));

        runtime::clear_auth_method().expect("clear");
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[test]
    fn resolve_startup_uses_saved_console_api_key_when_no_env() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_API_KEY_FD");
        runtime::save_auth_method(&runtime::AuthMethod::ConsoleApiKey {
            api_key: "sk-ant-saved".to_string(),
            origin: runtime::ApiKeyOrigin::ConsoleOAuth,
        })
        .expect("save auth method");

        let auth = resolve_startup_auth_source(|| Ok(None)).expect("startup auth");
        assert_eq!(auth.api_key(), Some("sk-ant-saved"));

        runtime::clear_auth_method().expect("clear");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[test]
    fn resolve_startup_refreshes_expired_claude_ai_oauth_method() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        std::env::remove_var("CLAUDE_CODE_API_KEY_FD");

        runtime::save_auth_method(&runtime::AuthMethod::ClaudeAiOAuth {
            token_set: runtime::OAuthTokenSet {
                access_token: "expired-access-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                expires_at: Some(1),
                scopes: vec!["user:profile".to_string()],
            },
            subscription: None,
        })
        .expect("save expired auth method");

        let token_url = spawn_token_server(
            "{\"access_token\":\"new-access-token\",\"refresh_token\":\"new-refresh\",\"expires_at\":9999999999,\"scopes\":[\"user:profile\"]}",
        );
        let config = sample_oauth_config(token_url);

        let auth = resolve_startup_auth_source(|| Ok(Some(config))).expect("startup auth");
        assert_eq!(auth.bearer_token(), Some("new-access-token"));

        // Verify persisted
        let stored = runtime::load_auth_method()
            .expect("load auth method")
            .expect("auth method present");
        match stored {
            runtime::AuthMethod::ClaudeAiOAuth { token_set, .. } => {
                assert_eq!(token_set.access_token, "new-access-token");
            }
            other => panic!("expected ClaudeAiOAuth, got {other:?}"),
        }

        runtime::clear_auth_method().expect("clear");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    #[tokio::test]
    async fn exchange_with_extra_headers_includes_them() {
        use runtime::{OAuthTokenExchangeRequest, OAuthConfig};
        use std::sync::Arc;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("local addr");
        let captured_headers = Arc::new(std::sync::Mutex::new(String::new()));
        let captured_clone = Arc::clone(&captured_headers);

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = vec![0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            *captured_clone.lock().unwrap() = String::from_utf8_lossy(&buffer[..bytes_read]).into_owned();
            let body = "{\"access_token\":\"tok\",\"refresh_token\":null,\"expires_at\":9999999999,\"scopes\":[]}";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write response");
        });

        let config = OAuthConfig {
            client_id: "test-client".to_string(),
            authorize_url: "https://example.test/auth".to_string(),
            token_url: format!("http://{address}/token"),
            callback_port: None,
            manual_redirect_url: None,
            scopes: vec![],
        };
        let exchange_req = OAuthTokenExchangeRequest {
            grant_type: "authorization_code",
            code: "code123".to_string(),
            redirect_uri: "http://localhost/callback".to_string(),
            client_id: "test-client".to_string(),
            code_verifier: "verifier".to_string(),
            state: "state".to_string(),
        };

        let client = ElaiApiClient::from_auth(AuthSource::None);
        client
            .exchange_oauth_code_with_headers(&config, &exchange_req, &[("anthropic-beta", "oauth-2025-04-20")])
            .await
            .expect("exchange with headers");

        let headers = captured_headers.lock().unwrap().clone();
        assert!(
            headers.to_lowercase().contains("anthropic-beta: oauth-2025-04-20"),
            "expected anthropic-beta header in request: {headers}"
        );
    }

    #[tokio::test]
    async fn create_console_api_key_returns_raw_key() {
        use runtime::AnthropicOAuthEndpoints;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("local addr");

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).expect("read request");
            let body = "{\"raw_key\":\"sk-ant-test\"}";
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write response");
        });

        let mut endpoints = AnthropicOAuthEndpoints::production();
        endpoints.api_key_url = format!("http://{address}/create_api_key");

        let client = ElaiApiClient::from_auth(AuthSource::None);
        let key = client
            .create_console_api_key(&endpoints, "bearer-token")
            .await
            .expect("create api key");
        assert_eq!(key, "sk-ant-test");
    }

    #[cfg(unix)]
    #[test]
    fn read_api_key_from_fd_env_returns_some_when_fd_set() {
        let _guard = env_lock();
        use std::os::unix::io::AsRawFd;

        // Write the key to a temp file, then pass its fd number via the env var.
        let dir = std::env::temp_dir().join(format!(
            "api-fd-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let key_path = dir.join("api_key.txt");
        std::fs::write(&key_path, "sk-ant-fd-test\n").expect("write key file");

        // Open the file for reading; keep the handle alive until after the call.
        let file = std::fs::File::open(&key_path).expect("open key file");
        let raw_fd = file.as_raw_fd();

        std::env::set_var("CLAUDE_CODE_API_KEY_FD", raw_fd.to_string());
        let result = super::read_api_key_from_fd_env().expect("read fd env");
        std::env::remove_var("CLAUDE_CODE_API_KEY_FD");

        // Clean up
        drop(file);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(result.as_deref(), Some("sk-ant-fd-test"));
    }

    #[test]
    fn base_url_is_anthropic_official_detects_default() {
        let _guard = env_lock();
        std::env::remove_var("ANTHROPIC_BASE_URL");
        assert!(super::base_url_is_anthropic_official());

        std::env::set_var("ANTHROPIC_BASE_URL", "https://api.anthropic.com");
        assert!(super::base_url_is_anthropic_official());

        std::env::set_var("ANTHROPIC_BASE_URL", "https://api.anthropic.com/");
        assert!(super::base_url_is_anthropic_official());

        std::env::set_var("ANTHROPIC_BASE_URL", "https://custom.proxy.example.com");
        assert!(!super::base_url_is_anthropic_official());

        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    // --- SpoofMode tests ---

    #[test]
    fn spoof_mode_defaults_to_none() {
        let client = ElaiApiClient::new("test-key");
        assert_eq!(client.spoof_mode(), super::SpoofMode::None);
    }

    #[test]
    fn with_spoof_mode_claude_code_sets_field() {
        let client = ElaiApiClient::new("test-key").with_spoof_mode(super::SpoofMode::ClaudeCode);
        assert_eq!(client.spoof_mode(), super::SpoofMode::ClaudeCode);
    }

    #[test]
    fn detect_spoof_mode_returns_none_for_api_key_auth() {
        let auth = AuthSource::ApiKey("sk-ant-test".to_string());
        let mode = super::detect_spoof_mode_for_auth(&auth);
        assert_eq!(mode, super::SpoofMode::None);
    }

    #[test]
    fn detect_spoof_mode_returns_claude_code_when_bearer_and_saved_method() {
        let _guard = env_lock();
        let config_home = temp_config_home();
        std::env::set_var("ELAI_CONFIG_HOME", &config_home);
        runtime::save_auth_method(&runtime::AuthMethod::ClaudeAiOAuth {
            token_set: runtime::OAuthTokenSet {
                access_token: "bearer-tok".to_string(),
                refresh_token: None,
                expires_at: Some(9_999_999_999),
                scopes: vec![],
            },
            subscription: None,
        })
        .expect("save auth method");

        let auth = AuthSource::BearerToken("bearer-tok".to_string());
        let mode = super::detect_spoof_mode_for_auth(&auth);
        assert_eq!(mode, super::SpoofMode::ClaudeCode);

        runtime::clear_auth_method().expect("clear");
        std::env::remove_var("ELAI_CONFIG_HOME");
        cleanup_temp_config_home(&config_home);
    }

    fn spawn_messages_server<F>(handler: F) -> String
    where
        F: FnOnce(String) -> String + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let address = listener.local_addr().expect("local addr");
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buffer = vec![0_u8; 8192];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request_text = String::from_utf8_lossy(&buffer[..bytes_read]).into_owned();
            let response = handler(request_text);
            stream.write_all(response.as_bytes()).expect("write response");
        });
        format!("http://{address}")
    }

    fn fake_message_response_json() -> &'static str {
        "{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"model\":\"claude-opus-4-6\",\"stop_reason\":\"end_turn\",\"stop_sequence\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}"
    }

    #[tokio::test]
    async fn send_request_with_spoof_includes_query_param_beta_true() {
        let base_url = spawn_messages_server(|request_text| {
            let body = fake_message_response_json();
            // Verify URL contains ?beta=true
            assert!(
                request_text.contains("?beta=true") || request_text.contains("beta=true"),
                "expected beta=true in URL: {request_text}"
            );
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            )
        });

        let request = MessageRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 64,
            messages: vec![crate::types::InputMessage::user_text("hello")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
        };

        let client = ElaiApiClient::from_auth(AuthSource::ApiKey("sk-test".to_string()))
            .with_base_url(&base_url)
            .with_spoof_mode(super::SpoofMode::ClaudeCode);

        // send_message will fail to parse the response but query param check happens server-side
        let _ = client.send_message(&request).await;
    }

    #[tokio::test]
    async fn send_request_with_spoof_includes_stainless_headers() {
        let base_url = spawn_messages_server(|request_text| {
            assert!(
                request_text.to_lowercase().contains("x-stainless-package-version"),
                "expected x-stainless-package-version header: {request_text}"
            );
            let body = fake_message_response_json();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            )
        });

        let request = MessageRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 64,
            messages: vec![crate::types::InputMessage::user_text("hello")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
        };

        let client = ElaiApiClient::from_auth(AuthSource::ApiKey("sk-test".to_string()))
            .with_base_url(&base_url)
            .with_spoof_mode(super::SpoofMode::ClaudeCode);

        let _ = client.send_message(&request).await;
    }

    #[tokio::test]
    async fn send_request_without_spoof_does_not_add_beta_query() {
        let base_url = spawn_messages_server(|request_text| {
            assert!(
                !request_text.contains("beta=true"),
                "unexpected beta=true in URL: {request_text}"
            );
            let body = fake_message_response_json();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            )
        });

        let request = MessageRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 64,
            messages: vec![crate::types::InputMessage::user_text("hello")],
            system: None,
            tools: None,
            tool_choice: None,
            stream: false,
        };

        let client = ElaiApiClient::from_auth(AuthSource::ApiKey("sk-test".to_string()))
            .with_base_url(&base_url)
            .with_spoof_mode(super::SpoofMode::None);

        let _ = client.send_message(&request).await;
    }

    #[tokio::test]
    async fn send_message_reverses_pascalcase_in_response() {
        // The server returns mcp_Bash in tool_use; the client with spoof mode should reverse it to mcp_bash.
        let body = "{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"mcp_Bash\",\"input\":{}}],\"model\":\"claude-opus-4-6\",\"stop_reason\":\"tool_use\",\"stop_sequence\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}";
        let body_owned = body.to_string();
        let base_url = spawn_messages_server(move |_request_text| {
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body_owned.len(),
                body_owned
            )
        });

        let request = MessageRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 64,
            messages: vec![crate::types::InputMessage::user_text("hello")],
            system: None,
            tools: Some(vec![crate::types::ToolDefinition {
                name: "mcp_bash".to_string(),
                description: None,
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            }]),
            tool_choice: None,
            stream: false,
        };

        let client = ElaiApiClient::from_auth(AuthSource::ApiKey("sk-test".to_string()))
            .with_base_url(&base_url)
            .with_spoof_mode(super::SpoofMode::ClaudeCode);

        let response = client.send_message(&request).await.expect("send message");
        match &response.content[0] {
            crate::types::OutputContentBlock::ToolUse { name, .. } => {
                assert_eq!(name, "mcp_bash", "expected reversed PascalCase name mcp_bash, got {name}");
            }
            other => panic!("expected ToolUse content block, got {other:?}"),
        }
    }
}
