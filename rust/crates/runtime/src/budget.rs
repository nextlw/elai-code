use std::io;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::usage::{UsageTracker, pricing_for_model};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_tokens: Option<u64>,
    pub max_turns: Option<u32>,
    pub max_cost_usd: Option<f64>,
    pub warn_at_pct: f32,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_tokens: None,
            max_turns: None,
            max_cost_usd: None,
            warn_at_pct: 80.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BudgetStatus {
    Ok,
    Warning { pct: f32, dimension: &'static str },
    Exhausted { reason: String },
    Disabled,
}

#[derive(Debug, Clone, Copy)]
pub struct BudgetUsagePct {
    pub tokens_pct: f32,
    pub turns_pct: f32,
    pub cost_pct: f32,
    pub highest_pct: f32,
    pub current_cost_usd: f64,
    pub total_tokens: u64,
}

pub struct BudgetTracker {
    config: BudgetConfig,
    enabled: bool,
}

impl BudgetTracker {
    #[must_use] 
    pub fn new(config: BudgetConfig) -> Self {
        Self { config, enabled: true }
    }

    #[must_use] 
    pub fn disabled() -> Self {
        Self { config: BudgetConfig::default(), enabled: false }
    }

    #[must_use] 
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[must_use] 
    pub fn config(&self) -> &BudgetConfig {
        &self.config
    }

    pub fn update_config(&mut self, config: BudgetConfig) {
        self.config = config;
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    #[must_use] 
    pub fn check(&self, usage: &UsageTracker, model: &str) -> BudgetStatus {
        if !self.enabled {
            return BudgetStatus::Disabled;
        }

        let pct = self.usage_pct(usage, model);

        // Check exhaustion first
        if let Some(max) = self.config.max_tokens {
            if pct.total_tokens >= max {
                return BudgetStatus::Exhausted {
                    reason: format!(
                        "Tokens: {}/{} (100%)",
                        pct.total_tokens, max
                    ),
                };
            }
        }
        if let Some(max) = self.config.max_turns {
            if usage.turns() >= max {
                return BudgetStatus::Exhausted {
                    reason: format!(
                        "Turns: {}/{} (100%)",
                        usage.turns(), max
                    ),
                };
            }
        }
        if let Some(max) = self.config.max_cost_usd {
            if pct.current_cost_usd >= max {
                return BudgetStatus::Exhausted {
                    reason: format!(
                        "Cost: ${:.4}/$ {:.4} (100%)",
                        pct.current_cost_usd, max
                    ),
                };
            }
        }

        // Check warnings
        let warn = self.config.warn_at_pct;
        if pct.tokens_pct >= warn {
            return BudgetStatus::Warning { pct: pct.tokens_pct, dimension: "tokens" };
        }
        if pct.turns_pct >= warn {
            return BudgetStatus::Warning { pct: pct.turns_pct, dimension: "turns" };
        }
        if pct.cost_pct >= warn {
            return BudgetStatus::Warning { pct: pct.cost_pct, dimension: "cost" };
        }

        BudgetStatus::Ok
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    #[must_use] 
    pub fn usage_pct(&self, usage: &UsageTracker, model: &str) -> BudgetUsagePct {
        let cumulative = usage.cumulative_usage();
        let total_tokens = u64::from(cumulative.total_tokens());

        let current_cost_usd = if let Some(pricing) = pricing_for_model(model) {
            cumulative.estimate_cost_usd_with_pricing(pricing).total_cost_usd()
        } else {
            cumulative.estimate_cost_usd().total_cost_usd()
        };

        let tokens_pct = self.config.max_tokens
            .map_or(0.0, |m| (total_tokens as f32 / m as f32) * 100.0);

        let turns_pct = self.config.max_turns
            .map_or(0.0, |m| (usage.turns() as f32 / m as f32) * 100.0);

        let cost_pct = self.config.max_cost_usd
            .map_or(0.0, |m| (current_cost_usd / m) as f32 * 100.0);

        let highest_pct = tokens_pct.max(turns_pct).max(cost_pct);

        BudgetUsagePct {
            tokens_pct,
            turns_pct,
            cost_pct,
            highest_pct,
            current_cost_usd,
            total_tokens,
        }
    }
}

#[must_use] 
pub fn budget_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".elai").join("budget.json")
}

#[must_use] 
pub fn load_budget_config(cwd: &Path) -> Option<BudgetConfig> {
    let path = budget_config_path(cwd);
    let json = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&json).ok()
}

pub fn save_budget_config(cwd: &Path, config: &BudgetConfig) -> io::Result<()> {
    let path = budget_config_path(cwd);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(io::Error::other)?;
    std::fs::write(&path, json)
}

pub fn remove_budget_config(cwd: &Path) -> io::Result<()> {
    let path = budget_config_path(cwd);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{UsageTracker, TokenUsage};

    fn make_tracker_with_usage(tokens: u32, turns: u32) -> UsageTracker {
        let mut tracker = UsageTracker::new();
        for _ in 0..turns {
            tracker.record(TokenUsage {
                input_tokens: tokens / turns,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            });
        }
        tracker
    }

    #[test]
    fn disabled_always_returns_disabled() {
        let tracker = make_tracker_with_usage(100_000, 5);
        let budget = BudgetTracker::disabled();
        assert_eq!(budget.check(&tracker, "claude-sonnet-4-6"), BudgetStatus::Disabled);
    }

    #[test]
    fn ok_when_under_limits() {
        let tracker = make_tracker_with_usage(100_000, 5);
        let budget = BudgetTracker::new(BudgetConfig {
            max_tokens: Some(500_000),
            max_turns: Some(50),
            max_cost_usd: Some(10.0),
            warn_at_pct: 80.0,
        });
        assert_eq!(budget.check(&tracker, "claude-sonnet-4-6"), BudgetStatus::Ok);
    }

    #[test]
    fn warning_when_near_token_limit() {
        let tracker = make_tracker_with_usage(450_000, 1);
        let budget = BudgetTracker::new(BudgetConfig {
            max_tokens: Some(500_000),
            max_turns: None,
            max_cost_usd: None,
            warn_at_pct: 80.0,
        });
        assert!(matches!(
            budget.check(&tracker, "claude-sonnet-4-6"),
            BudgetStatus::Warning { dimension: "tokens", .. }
        ));
    }

    #[test]
    fn exhausted_when_turns_exceeded() {
        let tracker = make_tracker_with_usage(1000, 10);
        let budget = BudgetTracker::new(BudgetConfig {
            max_tokens: None,
            max_turns: Some(10),
            max_cost_usd: None,
            warn_at_pct: 80.0,
        });
        assert!(matches!(
            budget.check(&tracker, "claude-sonnet-4-6"),
            BudgetStatus::Exhausted { .. }
        ));
    }

    #[test]
    fn budget_json_roundtrip() {
        let config = BudgetConfig {
            max_tokens: Some(500_000),
            max_turns: Some(100),
            max_cost_usd: Some(5.0),
            warn_at_pct: 75.0,
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: BudgetConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.max_tokens, config.max_tokens);
        assert_eq!(decoded.max_turns, config.max_turns);
        assert_eq!(decoded.max_cost_usd, config.max_cost_usd);
        assert!((decoded.warn_at_pct - config.warn_at_pct).abs() < f32::EPSILON);
    }
}
