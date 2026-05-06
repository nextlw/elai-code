use axum::Json;
use serde::Serialize;

// ── models ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub max_tokens: Option<u32>,
    pub supports_thinking: bool,
    pub supports_vision: bool,
}

#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,
}

pub async fn list_models() -> Json<ModelsResponse> {
    let ant_models = api::get_ant_models();

    let models = ant_models
        .iter()
        .map(|m| ModelInfo {
            id: m.alias.clone(),
            name: m.label.clone(),
            max_tokens: m.default_max_tokens,
            supports_thinking: m.always_on_thinking || m.thinking_budget_tokens.is_some(),
            // Vision support heuristic: haiku-class models don't support vision.
            supports_vision: !m.model.contains("haiku"),
        })
        .collect();

    Json(ModelsResponse { models })
}

// ── providers ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub available: bool,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct ProvidersResponse {
    pub providers: Vec<ProviderInfo>,
}

struct ProviderSpec {
    id: &'static str,
    env_var: &'static str,
    description: &'static str,
}

static PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "anthropic",
        env_var: "ANTHROPIC_API_KEY",
        description: "Anthropic Claude models",
    },
    ProviderSpec {
        id: "openai",
        env_var: "OPENAI_API_KEY",
        description: "OpenAI GPT models",
    },
    ProviderSpec {
        id: "xai",
        env_var: "XAI_API_KEY",
        description: "xAI Grok models",
    },
    ProviderSpec {
        id: "ollama",
        env_var: "OLLAMA_BASE_URL",
        description: "Ollama local inference server",
    },
    ProviderSpec {
        id: "lmstudio",
        env_var: "LMSTUDIO_BASE_URL",
        description: "LM Studio local inference server",
    },
];

pub async fn list_providers() -> Json<ProvidersResponse> {
    let providers = PROVIDERS
        .iter()
        .map(|spec| {
            let available = std::env::var(spec.env_var)
                .is_ok_and(|v| !v.trim().is_empty());
            ProviderInfo {
                id: spec.id.to_string(),
                available,
                description: spec.description.to_string(),
            }
        })
        .collect();

    Json(ProvidersResponse { providers })
}
