//! Global configuration persistence at `~/.elai/config.json`.
//!
//! Single source of truth para preferências do usuário (model, permission\_mode,
//! auto-update, telemetry, etc). Setup wizard escreve aqui na primeira run.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Feature toggles agrupadas. Cada flag é independente e ortogonal —
/// agrupar em sub-struct evita o lint `clippy::struct_excessive_bools`
/// (limite de 3 bools por struct) e dá um namespace semântico claro.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureFlags {
    #[serde(default = "yes")]
    pub auto_update: bool,
    #[serde(default = "yes")]
    pub telemetry: bool,
    #[serde(default = "yes")]
    pub indexing: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            auto_update: true,
            telemetry: true,
            indexing: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalConfig {
    #[serde(default)]
    pub setup_complete: bool,
    #[serde(default = "default_model")]
    pub default_model: String,
    #[serde(default = "default_perm_mode")]
    pub default_permission_mode: String,
    #[serde(default)]
    pub features: FeatureFlags,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            setup_complete: false,
            default_model: default_model(),
            default_permission_mode: default_perm_mode(),
            features: FeatureFlags::default(),
        }
    }
}

fn default_model() -> String {
    "claude-opus-4-7".to_string()
}
fn default_perm_mode() -> String {
    "danger-full-access".to_string()
}
fn yes() -> bool {
    true
}

#[must_use]
pub fn config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".elai").join("config.json"))
}

pub fn load() -> std::io::Result<GlobalConfig> {
    let Some(path) = config_path() else {
        return Ok(GlobalConfig::default());
    };
    if !path.is_file() {
        return Ok(GlobalConfig::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    serde_json::from_str(&raw)
        .map_err(|e| std::io::Error::other(format!("parse {}: {e}", path.display())))
}

pub fn save(cfg: &GlobalConfig) -> std::io::Result<()> {
    let Some(path) = config_path() else {
        return Err(std::io::Error::other(
            "no HOME/USERPROFILE for config path",
        ));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

#[must_use]
pub fn is_setup_complete() -> bool {
    load().map_or(false, |c| c.setup_complete)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serialize env-mutation tests to avoid data races across threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.prev {
                Some(p) => std::env::set_var(self.key, p),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn with_home(td: &TempDir, f: impl FnOnce()) {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _restore = EnvRestore {
            key: "HOME",
            prev: std::env::var_os("HOME"),
        };
        std::env::set_var("HOME", td.path());
        f();
    }

    #[test]
    fn defaults_used_when_no_file() {
        let cfg = GlobalConfig::default();
        assert_eq!(cfg.default_model, "claude-opus-4-7");
        assert!(!cfg.setup_complete);
    }

    #[test]
    fn save_then_load_round_trip() {
        let td = TempDir::new().unwrap();
        with_home(&td, || {
            let cfg = GlobalConfig {
                setup_complete: true,
                default_model: "test-model".into(),
                ..GlobalConfig::default()
            };
            save(&cfg).unwrap();
            let loaded = load().unwrap();
            assert_eq!(loaded.default_model, "test-model");
            assert!(loaded.setup_complete);
        });
    }

    #[test]
    fn is_setup_complete_returns_false_when_no_file() {
        let td = TempDir::new().unwrap();
        with_home(&td, || {
            assert!(!is_setup_complete());
        });
    }

    #[test]
    fn is_setup_complete_returns_true_after_save() {
        let td = TempDir::new().unwrap();
        with_home(&td, || {
            let cfg = GlobalConfig {
                setup_complete: true,
                ..GlobalConfig::default()
            };
            save(&cfg).unwrap();
            assert!(is_setup_complete());
        });
    }
}
