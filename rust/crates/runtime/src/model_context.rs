//! Approximate **total context window** (input budget) per model id for compaction gating.
//! Values are conservative; set `ELAI_CONTEXT_TOKENS` to override globally (numeric).

#[must_use]
pub fn input_context_tokens_for_model(model: &str) -> u32 {
    if let Some(v) = env_context_override() {
        return v;
    }

    // Ant model override: respects explicit `context_window` from config.
    if let Some(window) = ant_context_window(model) {
        return window;
    }

    let trimmed = model.trim();
    let lower = trimmed.to_ascii_lowercase();

    if let Some(prefix) = local_provider_prefix(trimmed) {
        if is_local_provider_prefix(prefix) {
            return local_context_tokens();
        }
    }

    // Anthropic Claude (product ids and short aliases).
    if lower.contains("claude")
        || lower.contains("opus")
        || lower.contains("sonnet")
        || lower.contains("haiku")
    {
        return 200_000;
    }

    // OpenAI Chat Completions family (and Codex-style ids).
    if lower.starts_with("gpt-")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
        || lower.starts_with("chatgpt-")
    {
        // Newer long-context GPTs — keep a single conservative bucket until we track per-id tables.
        if lower.contains("gpt-5") || lower.contains("gpt-4.1") {
            return 400_000;
        }
        return 128_000;
    }

    if lower.contains("grok") {
        return 131_072;
    }

    // OpenCode router models and similar (deepseek, qwen, glm, kimi, …).
    if lower.contains("kimi")
        || lower.contains("glm")
        || lower.contains("deepseek")
        || lower.contains("qwen")
        || lower.contains("minimax")
        || lower.contains("mimo")
        || lower.contains("big-pickle")
        || lower.contains("hy3")
        || lower.contains("ling-")
        || lower.contains("trinity")
        || lower.contains("nemotron")
    {
        return 128_000;
    }

    // Unknown id: prefer compacting earlier rather than overshooting a small window.
    32_000
}

/// Estimated session size at which [`crate::compact::should_compact`](super::compact::should_compact)
/// should start considering compaction for this model (same units as `estimate_message_tokens`).
#[must_use]
pub fn compaction_trigger_estimated_tokens_for_model(model: &str) -> usize {
    let ctx = u64::from(input_context_tokens_for_model(model));
    // Leave headroom for system prompt, tool definitions, completion, and the next user turn.
    let trigger = ctx * 65 / 100;
    trigger.max(4_096) as usize
}

fn env_context_override() -> Option<u32> {
    std::env::var("ELAI_CONTEXT_TOKENS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|&n| n >= 1_024)
}

fn local_context_tokens() -> u32 {
    std::env::var("ELAI_LOCAL_CONTEXT_TOKENS")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|&n| n >= 512)
        .unwrap_or(8_192)
}

fn local_provider_prefix(model: &str) -> Option<&str> {
    let (prefix, rest) = model.split_once(':')?;
    if rest.trim().is_empty() {
        return None;
    }
    Some(prefix)
}

fn is_local_provider_prefix(prefix: &str) -> bool {
    matches!(
        prefix.to_ascii_lowercase().as_str(),
        "ollama" | "lmstudio" | "lm-studio" | "lm_studio" | "go" | "opencode-go"
            | "opencode_go" | "zen" | "opencode-zen" | "opencode_zen"
    )
}

/// Reads `context_window` from the ant model override config without depending on the `api` crate.
/// Mirrors the lookup in `api::providers::ant_models` — same sources, same priority order.
fn ant_context_window(model: &str) -> Option<u32> {
    use std::sync::OnceLock;
    static CACHE: OnceLock<Option<Vec<(String, String, Option<u32>)>>> = OnceLock::new();

    let entries = CACHE.get_or_init(|| {
        let raw = std::env::var("ELAI_ANT_MODEL_OVERRIDE").ok().or_else(|| {
            let home = dirs::home_dir()?;
            std::fs::read_to_string(home.join(".elai").join("ant_model_override.json")).ok()
        })?;

        #[derive(serde::Deserialize)]
        struct M {
            alias: String,
            model: String,
            context_window: Option<u32>,
        }
        #[derive(serde::Deserialize)]
        struct C {
            #[serde(default)]
            ant_models: Vec<M>,
        }

        let cfg: C = serde_json::from_str(&raw).ok()?;
        Some(
            cfg.ant_models
                .into_iter()
                .map(|m| (m.alias, m.model, m.context_window))
                .collect(),
        )
    });

    let entries = entries.as_deref()?;
    let lower = model.to_ascii_lowercase();
    entries
        .iter()
        .find(|(alias, model_id, _)| {
            alias == model || lower.contains(&model_id.to_ascii_lowercase())
        })
        .and_then(|(_, _, window)| *window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_models_use_smaller_window_than_claude() {
        assert!(
            input_context_tokens_for_model("ollama:llama3") < input_context_tokens_for_model("claude-sonnet-4-6")
        );
    }

    #[test]
    fn trigger_scales_with_window() {
        let t_small = compaction_trigger_estimated_tokens_for_model("ollama:qwen");
        let t_large = compaction_trigger_estimated_tokens_for_model("claude-opus-4-6");
        assert!(t_small < t_large);
    }
}
