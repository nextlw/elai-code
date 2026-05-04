//! Configuração do auto-dream. Defaults vêm do leaked Claude Code.
//!
//! Override via env: `ELAI_AUTO_DREAM_MIN_HOURS`, `ELAI_AUTO_DREAM_MIN_SESSIONS`,
//! `ELAI_AUTO_DREAM_DISABLED=1`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoDreamConfig {
    pub min_hours: u32,
    pub min_sessions: u32,
    pub enabled: bool,
}

impl Default for AutoDreamConfig {
    fn default() -> Self {
        Self {
            min_hours: 24,
            min_sessions: 5,
            enabled: true,
        }
    }
}

impl AutoDreamConfig {
    /// Carrega config aplicando overrides de env.
    #[must_use]
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("ELAI_AUTO_DREAM_MIN_HOURS") {
            if let Ok(n) = v.parse::<u32>() {
                if n > 0 {
                    cfg.min_hours = n;
                }
            }
        }
        if let Ok(v) = std::env::var("ELAI_AUTO_DREAM_MIN_SESSIONS") {
            if let Ok(n) = v.parse::<u32>() {
                if n > 0 {
                    cfg.min_sessions = n;
                }
            }
        }
        if std::env::var("ELAI_AUTO_DREAM_DISABLED")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        {
            cfg.enabled = false;
        }
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_leaked_thresholds() {
        let c = AutoDreamConfig::default();
        assert_eq!(c.min_hours, 24);
        assert_eq!(c.min_sessions, 5);
        assert!(c.enabled);
    }
}
