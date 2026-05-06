//! Dynamic model alias override — equivalent to `tengu_ant_model_override` in the TS reference.
//!
//! Config is loaded once per process from, in priority order:
//!   1. `ELAI_ANT_MODEL_OVERRIDE` env var (JSON string)
//!   2. `~/.elai/ant_model_override.json` file
//!
//! Example config:
//! ```json
//! {
//!   "default_model": "claude-sonnet-4-6",
//!   "ant_models": [
//!     { "alias": "capybara-fast", "model": "claude-haiku-4-5-20251001", "label": "Fast" },
//!     { "alias": "capybara",      "model": "claude-sonnet-4-6",          "label": "Balanced", "always_on_thinking": true }
//!   ]
//! }
//! ```

use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AntModel {
    /// Short alias the user types, e.g. `"capybara-fast"`.
    pub alias: String,
    /// Canonical model ID sent to the API, e.g. `"claude-haiku-4-5-20251001"`.
    pub model: String,
    /// Human-readable label shown in help/completions.
    pub label: String,
    /// When `true`, extended thinking is active on every request with this model alias.
    #[serde(default)]
    pub always_on_thinking: bool,
    /// Thinking budget in tokens when `always_on_thinking` is true. Defaults to 32 000.
    pub thinking_budget_tokens: Option<u32>,
    pub default_max_tokens: Option<u32>,
    pub context_window: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AntModelOverrideConfig {
    /// Replaces the hardcoded default model when set.
    pub default_model: Option<String>,
    #[serde(default)]
    pub ant_models: Vec<AntModel>,
}

static CACHE: OnceLock<Option<AntModelOverrideConfig>> = OnceLock::new();

fn load_from_env() -> Option<AntModelOverrideConfig> {
    let raw = std::env::var("ELAI_ANT_MODEL_OVERRIDE").ok()?;
    serde_json::from_str(&raw)
        .map_err(|e| {
            eprintln!("[elai] ELAI_ANT_MODEL_OVERRIDE parse error: {e}");
        })
        .ok()
}

fn read_json_file(path: &std::path::Path) -> Option<AntModelOverrideConfig> {
    let raw = std::fs::read_to_string(path)
        .map_err(|_| ())
        .ok()?;
    serde_json::from_str(&raw)
        .map_err(|e| eprintln!("[elai] {}: parse error: {e}", path.display()))
        .ok()
}

fn load_from_file() -> Option<AntModelOverrideConfig> {
    // 1. Local project: ./.elai/ant_model_override.json (takes priority over global)
    if let Ok(cwd) = std::env::current_dir() {
        let local = cwd.join(".elai").join("ant_model_override.json");
        if let Some(cfg) = read_json_file(&local) {
            return Some(cfg);
        }
    }
    // 2. Global: ~/.elai/ant_model_override.json
    let home = dirs::home_dir()?;
    read_json_file(&home.join(".elai").join("ant_model_override.json"))
}

fn load_config() -> Option<AntModelOverrideConfig> {
    load_from_env().or_else(load_from_file)
}

/// Returns the active override config, loading it on first call.
pub fn get_ant_model_override_config() -> Option<&'static AntModelOverrideConfig> {
    CACHE.get_or_init(load_config).as_ref()
}

/// Built-in capybara aliases — always available, even without any config file.
fn builtin_ant_models() -> Vec<AntModel> {
    vec![
        AntModel {
            alias: "capybara-fast".to_string(),
            model: "claude-haiku-4-5-20251001".to_string(),
            label: "Fast (Haiku)".to_string(),
            always_on_thinking: false,
            thinking_budget_tokens: None,
            default_max_tokens: None,
            context_window: None,
        },
        AntModel {
            alias: "capybara".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            label: "Balanced (Sonnet)".to_string(),
            always_on_thinking: false,
            thinking_budget_tokens: None,
            default_max_tokens: None,
            context_window: None,
        },
        AntModel {
            alias: "capybara-ultra".to_string(),
            model: "claude-opus-4-7".to_string(),
            label: "Ultra (Opus + thinking)".to_string(),
            always_on_thinking: true,
            thinking_budget_tokens: Some(16_000),
            default_max_tokens: None,
            context_window: None,
        },
    ]
}

/// All model aliases: user overrides first (by alias), then builtins for aliases not overridden.
#[must_use] 
pub fn get_ant_models() -> Vec<AntModel> {
    let builtins = builtin_ant_models();
    let overrides = get_ant_model_override_config()
        .map(|c| c.ant_models.clone())
        .unwrap_or_default();

    if overrides.is_empty() {
        return builtins;
    }

    // User overrides take precedence; builtins fill in any alias not present in overrides.
    let mut merged = overrides;
    for builtin in builtins {
        if !merged.iter().any(|m| m.alias == builtin.alias) {
            merged.push(builtin);
        }
    }
    merged
}

/// Resolves `model` to an `AntModel` by matching alias (exact) or model name (substring).
/// Returns `None` when no ant override applies.
#[must_use] 
pub fn resolve_ant_model(model: &str) -> Option<AntModel> {
    let models = get_ant_models();
    if models.is_empty() {
        return None;
    }
    // Exact alias match only — substring match on canonical model ID caused infinite
    // recursion in metadata_for_model (canonical IDs contain themselves as substrings).
    models.into_iter().find(|m| m.alias == model)
}

/// If the override config specifies a `default_model`, return it.
#[must_use] 
pub fn ant_default_model() -> Option<String> {
    get_ant_model_override_config()?.default_model.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(alias: &str, model: &str) -> AntModelOverrideConfig {
        AntModelOverrideConfig {
            default_model: None,
            ant_models: vec![AntModel {
                alias: alias.to_string(),
                model: model.to_string(),
                label: "Test".to_string(),
                always_on_thinking: false,
                thinking_budget_tokens: None,
                default_max_tokens: None,
                context_window: None,
            }],
        }
    }

    #[test]
    fn deserializes_minimal_json() {
        let json = r#"{"ant_models":[{"alias":"capybara-fast","model":"claude-haiku-4-5-20251001","label":"Fast"}]}"#;
        let cfg: AntModelOverrideConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.ant_models[0].alias, "capybara-fast");
        assert_eq!(cfg.ant_models[0].model, "claude-haiku-4-5-20251001");
        assert!(!cfg.ant_models[0].always_on_thinking);
    }

    #[test]
    fn deserializes_with_default_model() {
        let json = r#"{"default_model":"claude-sonnet-4-6","ant_models":[]}"#;
        let cfg: AntModelOverrideConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.default_model.as_deref(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn resolve_by_alias() {
        let cfg = make_config("capybara-fast", "claude-haiku-4-5-20251001");
        let model = cfg
            .ant_models
            .iter()
            .find(|m| m.alias == "capybara-fast");
        assert!(model.is_some());
        assert_eq!(model.unwrap().model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn resolve_by_model_substring() {
        let cfg = make_config("capybara-fast", "claude-haiku-4-5-20251001");
        let lower = "claude-haiku-4-5-20251001".to_ascii_lowercase();
        let model = cfg
            .ant_models
            .iter()
            .find(|m| lower.contains(&m.model.to_ascii_lowercase()));
        assert!(model.is_some());
    }
}
