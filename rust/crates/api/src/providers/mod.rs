use std::future::Future;
use std::pin::Pin;

use crate::error::ApiError;
use crate::types::{MessageRequest, MessageResponse};

pub mod claude_code_spoof;
pub mod elai_provider;
pub mod openai_compat;

pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ApiError>> + Send + 'a>>;

pub trait Provider {
    type Stream;

    fn send_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, MessageResponse>;

    fn stream_message<'a>(
        &'a self,
        request: &'a MessageRequest,
    ) -> ProviderFuture<'a, Self::Stream>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    ElaiApi,
    Xai,
    OpenAi,
    /// Ollama local server (OpenAI-compatible API at `:11434/v1`).
    Ollama,
    /// LM Studio local server (OpenAI-compatible API at `:1234/v1`).
    LmStudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub provider: ProviderKind,
    pub auth_env: &'static str,
    pub base_url_env: &'static str,
    pub default_base_url: &'static str,
}

const MODEL_REGISTRY: &[(&str, ProviderMetadata)] = &[
    (
        "opus",
        ProviderMetadata {
            provider: ProviderKind::ElaiApi,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: elai_provider::DEFAULT_BASE_URL,
        },
    ),
    (
        "sonnet",
        ProviderMetadata {
            provider: ProviderKind::ElaiApi,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: elai_provider::DEFAULT_BASE_URL,
        },
    ),
    (
        "haiku",
        ProviderMetadata {
            provider: ProviderKind::ElaiApi,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: elai_provider::DEFAULT_BASE_URL,
        },
    ),
    (
        "claude-opus-4-6",
        ProviderMetadata {
            provider: ProviderKind::ElaiApi,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: elai_provider::DEFAULT_BASE_URL,
        },
    ),
    (
        "claude-sonnet-4-6",
        ProviderMetadata {
            provider: ProviderKind::ElaiApi,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: elai_provider::DEFAULT_BASE_URL,
        },
    ),
    (
        "claude-haiku-4-5-20251001",
        ProviderMetadata {
            provider: ProviderKind::ElaiApi,
            auth_env: "ANTHROPIC_API_KEY",
            base_url_env: "ANTHROPIC_BASE_URL",
            default_base_url: elai_provider::DEFAULT_BASE_URL,
        },
    ),
    (
        "grok",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-3",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-mini",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-3-mini",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
    (
        "grok-2",
        ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        },
    ),
];

/// Reconhece um prefixo `<provider>:<model>` que força o roteamento para um
/// provider local (Ollama / LM Studio). Devolve o `ProviderKind` e o nome do
/// modelo sem o prefixo. Usado tanto pelo `resolve_model_alias` quanto pelo
/// `detect_provider_kind` para evitar ambiguidade — modelos locais costumam
/// ter nomes genéricos (`llama3`, `qwen`) que poderiam colidir com OpenAI/xAI.
#[must_use]
pub fn parse_local_provider_prefix(model: &str) -> Option<(ProviderKind, String)> {
    let trimmed = model.trim();
    let (prefix, rest) = trimmed.split_once(':')?;
    let kind = match prefix.to_ascii_lowercase().as_str() {
        "ollama" => ProviderKind::Ollama,
        "lmstudio" | "lm-studio" | "lm_studio" => ProviderKind::LmStudio,
        _ => return None,
    };
    let bare = rest.trim();
    if bare.is_empty() {
        return None;
    }
    Some((kind, bare.to_string()))
}

#[must_use]
pub fn resolve_model_alias(model: &str) -> String {
    if let Some((_, bare)) = parse_local_provider_prefix(model) {
        return bare;
    }
    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();
    MODEL_REGISTRY
        .iter()
        .find_map(|(alias, metadata)| {
            (*alias == lower).then_some(match metadata.provider {
                ProviderKind::ElaiApi => match *alias {
                    "opus" => "claude-opus-4-6",
                    "sonnet" => "claude-sonnet-4-6",
                    "haiku" => "claude-haiku-4-5-20251001",
                    _ => trimmed,
                },
                ProviderKind::Xai => match *alias {
                    "grok" | "grok-3" => "grok-3",
                    "grok-mini" | "grok-3-mini" => "grok-3-mini",
                    "grok-2" => "grok-2",
                    _ => trimmed,
                },
                ProviderKind::OpenAi | ProviderKind::Ollama | ProviderKind::LmStudio => trimmed,
            })
        })
        .map_or_else(|| trimmed.to_string(), ToOwned::to_owned)
}

#[must_use]
pub fn metadata_for_model(model: &str) -> Option<ProviderMetadata> {
    // Prefixo explícito `ollama:NAME` / `lmstudio:NAME` curto-circuita a
    // detecção heurística — não tentamos adivinhar pelo nome bare do modelo.
    if let Some((kind, _)) = parse_local_provider_prefix(model) {
        return Some(match kind {
            ProviderKind::Ollama => ProviderMetadata {
                provider: ProviderKind::Ollama,
                auth_env: "OLLAMA_API_KEY",
                base_url_env: "OLLAMA_BASE_URL",
                default_base_url: openai_compat::DEFAULT_OLLAMA_BASE_URL,
            },
            ProviderKind::LmStudio => ProviderMetadata {
                provider: ProviderKind::LmStudio,
                auth_env: "LMSTUDIO_API_KEY",
                base_url_env: "LMSTUDIO_BASE_URL",
                default_base_url: openai_compat::DEFAULT_LM_STUDIO_BASE_URL,
            },
            _ => unreachable!("parse_local_provider_prefix only returns Ollama or LmStudio"),
        });
    }
    let canonical = resolve_model_alias(model);
    let lower = canonical.to_ascii_lowercase();
    if let Some((_, metadata)) = MODEL_REGISTRY.iter().find(|(alias, _)| *alias == lower) {
        return Some(*metadata);
    }
    if lower.starts_with("grok") {
        return Some(ProviderMetadata {
            provider: ProviderKind::Xai,
            auth_env: "XAI_API_KEY",
            base_url_env: "XAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_XAI_BASE_URL,
        });
    }
    if looks_like_openai_chat_model(&lower) {
        return Some(ProviderMetadata {
            provider: ProviderKind::OpenAi,
            auth_env: "OPENAI_API_KEY",
            base_url_env: "OPENAI_BASE_URL",
            default_base_url: openai_compat::DEFAULT_OPENAI_BASE_URL,
        });
    }
    None
}

fn looks_like_openai_chat_model(lower: &str) -> bool {
    lower.starts_with("gpt-")
        || lower.starts_with("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.starts_with("ft:")
        || lower.starts_with("chatgpt-")
}

#[must_use]
pub fn detect_provider_kind(model: &str) -> ProviderKind {
    if let Some(metadata) = metadata_for_model(model) {
        return metadata.provider;
    }
    if elai_provider::has_auth_from_env_or_saved().unwrap_or(false) {
        return ProviderKind::ElaiApi;
    }
    if openai_compat::has_openai_credentials() {
        return ProviderKind::OpenAi;
    }
    if openai_compat::has_api_key("XAI_API_KEY") {
        return ProviderKind::Xai;
    }
    // Local providers como último recurso: se o usuário setou explicitamente
    // o base_url de um provider local, assume que está rodando ele.
    if std::env::var_os("OLLAMA_BASE_URL").is_some() {
        return ProviderKind::Ollama;
    }
    if std::env::var_os("LMSTUDIO_BASE_URL").is_some() {
        return ProviderKind::LmStudio;
    }
    ProviderKind::ElaiApi
}

/// OpenAI Chat Completions rejects requests when `max_tokens` exceeds the model's completion limit
/// (e.g. `gpt-4o` currently allows at most 16384 completion tokens).
const OPENAI_DEFAULT_MAX_COMPLETION_TOKENS: u32 = 16_384;

#[must_use]
pub fn max_tokens_for_model(model: &str) -> u32 {
    // Provider local definido por prefixo: limite default conservador (4k)
    // que pode ser sobrescrito por `ELAI_LOCAL_MAX_COMPLETION_TOKENS` —
    // modelos pequenos rodando local frequentemente não suportam saídas
    // longas.
    if let Some((kind, _)) = parse_local_provider_prefix(model) {
        if matches!(kind, ProviderKind::Ollama | ProviderKind::LmStudio) {
            return local_max_completion_tokens();
        }
    }

    let canonical = resolve_model_alias(model);
    let lower = canonical.to_ascii_lowercase();

    if looks_like_openai_chat_model(&lower) {
        return openai_max_completion_tokens(&lower);
    }

    if lower.starts_with("grok") {
        return 64_000;
    }

    if canonical.contains("opus") {
        32_000
    } else {
        64_000
    }
}

fn local_max_completion_tokens() -> u32 {
    if let Ok(raw) = std::env::var("ELAI_LOCAL_MAX_COMPLETION_TOKENS") {
        if let Ok(parsed) = raw.trim().parse::<u32>() {
            if (1..=128_000).contains(&parsed) {
                return parsed;
            }
        }
    }
    4_096
}

fn openai_max_completion_tokens(lower: &str) -> u32 {
    if let Ok(raw) = std::env::var("ELAI_OPENAI_MAX_COMPLETION_TOKENS") {
        if let Ok(parsed) = raw.trim().parse::<u32>() {
            if (1..=128_000).contains(&parsed) {
                return parsed;
            }
        }
    }
    // Newer GPT-5 family models may advertise higher output limits; cap sanely until we track per-model tables.
    if lower.contains("gpt-5") {
        return 32_768;
    }
    OPENAI_DEFAULT_MAX_COMPLETION_TOKENS
}

/// Default model id for the CLI when the user did not pass `--model`, based on which API keys exist.
#[must_use]
pub fn suggested_default_model() -> String {
    // Priority: Anthropic → OpenAI → xAI → fallback
    if elai_provider::has_auth_from_env_or_saved().unwrap_or(false) {
        return "claude-haiku-4-5-20251001".to_string();
    }
    if openai_compat::has_openai_credentials() {
        return std::env::var("ELAI_DEFAULT_OPENAI_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
    }
    if openai_compat::has_api_key("XAI_API_KEY") {
        return "grok-3".to_string();
    }
    // Providers locais — preferimos Ollama se ambos estiverem indicados via env.
    if std::env::var_os("OLLAMA_BASE_URL").is_some() {
        return std::env::var("ELAI_DEFAULT_OLLAMA_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "ollama:llama3.1".to_string());
    }
    if std::env::var_os("LMSTUDIO_BASE_URL").is_some() {
        return std::env::var("ELAI_DEFAULT_LMSTUDIO_MODEL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "lmstudio:local".to_string());
    }
    // No key detected yet — neutral fallback; wizard will correct this.
    "claude-haiku-4-5-20251001".to_string()
}

// ── Thinking / Effort Awareness ──────────────────────────────
// These functions decide when to request extended thinking from the API,
// mirroring the logic from `thinking.ts` in Claude Code and the Mythos
// Router's `anthropic.ts`.  Local providers (Ollama, LM Studio) and
// OpenAI-compat providers that don't support thinking always get `None`.

use crate::types::{EffortLevel, OutputConfig, ThinkingConfig};

/// Returns `true` when the model supports extended thinking at all.
/// Claude 4+ (including Haiku 4.5) on first-party.  Local and OpenAI
/// providers do not.
#[must_use]
pub fn model_supports_thinking(model: &str) -> bool {
    // Local providers never support Anthropic-style thinking.
    if parse_local_provider_prefix(model).is_some() {
        return false;
    }
    let canonical = resolve_model_alias(model);
    let lower = canonical.to_ascii_lowercase();
    // Non-Anthropic models.
    if lower.starts_with("gpt")
        || lower.starts_with("grok")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.starts_with("deepseek")
        || lower.starts_with("chatgpt")
        || lower.starts_with("ft:")
    {
        return false;
    }
    // Claude 3.x does not support thinking.
    if lower.contains("claude-3-") {
        return false;
    }
    // Everything else from Anthropic (claude-4+, haiku-4-5, etc.) does.
    true
}

/// Returns `true` when the model supports *adaptive* thinking (type=adaptive).
/// Only Claude 4-6 / 4-7 sonnet & opus qualify today.
#[must_use]
pub fn model_supports_adaptive_thinking(model: &str) -> bool {
    if !model_supports_thinking(model) {
        return false;
    }
    let canonical = resolve_model_alias(model);
    let lower = canonical.to_ascii_lowercase();
    lower.contains("opus-4-6")
        || lower.contains("sonnet-4-6")
        || lower.contains("opus-4-7")
        || lower.contains("sonnet-4-7")
        || lower.contains("opus-4.6")
        || lower.contains("sonnet-4.6")
        || lower.contains("opus-4.7")
        || lower.contains("sonnet-4.7")
}

/// Default thinking budget for models that support thinking but NOT adaptive.
/// Overridable via `MAX_THINKING_TOKENS` env var.
#[must_use]
pub fn thinking_budget_for_model(model: &str) -> u32 {
    if let Ok(raw) = std::env::var("MAX_THINKING_TOKENS") {
        if let Ok(v) = raw.trim().parse::<u32>() {
            if v > 0 {
                return v;
            }
        }
    }
    let canonical = resolve_model_alias(model);
    let lower = canonical.to_ascii_lowercase();
    if lower.contains("opus") {
        32_000
    } else if lower.contains("sonnet") {
        16_000
    } else {
        10_000
    }
}

/// Build the appropriate `ThinkingConfig` for a model.
/// Returns `None` for models that don't support thinking.
#[must_use]
pub fn default_thinking_config(model: &str) -> Option<ThinkingConfig> {
    if !model_supports_thinking(model) {
        return None;
    }
    // Check if user explicitly disabled thinking.
    if let Ok(val) = std::env::var("MAX_THINKING_TOKENS") {
        if val.trim() == "0" {
            return Some(ThinkingConfig::Disabled);
        }
    }
    if model_supports_adaptive_thinking(model) {
        Some(ThinkingConfig::Adaptive)
    } else {
        Some(ThinkingConfig::Enabled {
            budget_tokens: thinking_budget_for_model(model),
        })
    }
}

/// Build the appropriate `OutputConfig` from an optional CLI/env effort override.
/// Returns `None` when no override is set (API uses its own default).
#[must_use]
pub fn resolve_output_config(effort_override: Option<EffortLevel>) -> Option<OutputConfig> {
    effort_override.map(|effort| OutputConfig { effort })
}


#[cfg(test)]
mod tests {
    use super::{
        detect_provider_kind, max_tokens_for_model, parse_local_provider_prefix,
        resolve_model_alias, ProviderKind,
    };

    #[test]
    fn resolves_grok_aliases() {
        assert_eq!(resolve_model_alias("grok"), "grok-3");
        assert_eq!(resolve_model_alias("grok-mini"), "grok-3-mini");
        assert_eq!(resolve_model_alias("grok-2"), "grok-2");
    }

    #[test]
    fn detects_provider_from_model_name_first() {
        assert_eq!(detect_provider_kind("grok"), ProviderKind::Xai);
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-6"),
            ProviderKind::ElaiApi
        );
        assert_eq!(detect_provider_kind("gpt-4o"), ProviderKind::OpenAi);
    }

    #[test]
    fn keeps_existing_max_token_heuristic() {
        assert_eq!(max_tokens_for_model("opus"), 32_000);
        assert_eq!(max_tokens_for_model("grok-3"), 64_000);
        assert_eq!(max_tokens_for_model("gpt-4o"), 16_384);
    }

    #[test]
    fn parses_ollama_and_lmstudio_prefixes() {
        let (kind, name) = parse_local_provider_prefix("ollama:llama3.1").unwrap();
        assert_eq!(kind, ProviderKind::Ollama);
        assert_eq!(name, "llama3.1");

        let (kind, name) = parse_local_provider_prefix("lmstudio:qwen2.5-coder").unwrap();
        assert_eq!(kind, ProviderKind::LmStudio);
        assert_eq!(name, "qwen2.5-coder");

        // Variantes case-insensitive e separador alternativo do LM Studio.
        assert_eq!(
            parse_local_provider_prefix("LM-STUDIO:foo").map(|(k, _)| k),
            Some(ProviderKind::LmStudio)
        );

        // Sem prefixo conhecido → None.
        assert!(parse_local_provider_prefix("gpt-4o").is_none());
        assert!(parse_local_provider_prefix("claude-opus-4-6").is_none());

        // Prefixo sem nome de modelo → None (evita aceitar "ollama:" sozinho).
        assert!(parse_local_provider_prefix("ollama:").is_none());
    }

    #[test]
    fn local_prefixes_route_to_local_providers_without_env() {
        // Mesmo sem `OLLAMA_BASE_URL` setado, prefixo explícito vence.
        assert_eq!(detect_provider_kind("ollama:llama3"), ProviderKind::Ollama);
        assert_eq!(
            detect_provider_kind("lmstudio:qwen"),
            ProviderKind::LmStudio
        );
    }

    #[test]
    fn resolve_model_alias_strips_local_prefix() {
        assert_eq!(resolve_model_alias("ollama:llama3.1"), "llama3.1");
        assert_eq!(
            resolve_model_alias("lmstudio:qwen2.5-coder"),
            "qwen2.5-coder"
        );
    }

    #[test]
    fn local_models_use_conservative_token_limit() {
        assert_eq!(max_tokens_for_model("ollama:llama3"), 4_096);
        assert_eq!(max_tokens_for_model("lmstudio:qwen"), 4_096);
    }
}
