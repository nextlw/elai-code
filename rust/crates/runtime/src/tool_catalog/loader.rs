use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct ToolOverride {
    pub id: String,
    pub priority: Option<i32>,
    pub category: Option<String>,
    pub embedding_hints: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ToolCatalogFile {
    #[serde(default, rename = "tool")]
    pub overrides: Vec<ToolOverride>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Default)]
pub struct ToolCatalog {
    pub overrides: Vec<ToolOverride>,
    pub aliases: BTreeMap<String, String>,
}

impl ToolCatalog {
    /// Discovers and merges all `tools.toml` files found in:
    /// `$HOME/.elai/tools/` → ancestors → cwd (cwd has highest precedence).
    ///
    /// Mirrors the discovery logic in `discover_skill_roots` from `skills.rs`.
    pub fn load(cwd: &Path) -> Self {
        let roots = discover_tool_roots(cwd);
        let mut merged = ToolCatalog::default();
        // Roots are ordered cwd-first; load in reverse so cwd overrides $HOME.
        for root in roots.iter().rev() {
            let path = root.join("tools.toml");
            if let Ok(content) = std::fs::read_to_string(&path) {
                match toml::from_str::<ToolCatalogFile>(&content) {
                    Ok(file) => {
                        for ov in file.overrides {
                            if let Some(existing) =
                                merged.overrides.iter_mut().find(|o| o.id == ov.id)
                            {
                                // Merge: Some fields override existing.
                                if ov.priority.is_some() {
                                    existing.priority = ov.priority;
                                }
                                if ov.category.is_some() {
                                    existing.category = ov.category.clone();
                                }
                                if ov.embedding_hints.is_some() {
                                    existing.embedding_hints = ov.embedding_hints.clone();
                                }
                                if ov.enabled.is_some() {
                                    existing.enabled = ov.enabled;
                                }
                            } else {
                                merged.overrides.push(ov);
                            }
                        }
                        merged.aliases.extend(file.aliases);
                    }
                    Err(e) => {
                        eprintln!("[tool_catalog] skipping {}: {e}", path.display());
                    }
                }
            }
        }
        merged
    }

    pub fn priority_for(&self, id: &str) -> Option<i32> {
        self.overrides.iter().find(|o| o.id == id)?.priority
    }

    pub fn enabled(&self, id: &str) -> bool {
        self.overrides
            .iter()
            .find(|o| o.id == id)
            .and_then(|o| o.enabled)
            .unwrap_or(true)
    }

    /// Resolve an alias to its canonical name.
    /// Returns the input unchanged if no alias is defined.
    pub fn resolve_alias<'a>(&'a self, name: &'a str) -> &'a str {
        self.aliases.get(name).map(String::as_str).unwrap_or(name)
    }
}

/// Returns candidate directories for `tools.toml` files, from cwd ancestors
/// down to `$HOME/.elai/tools/`. Mirrors `discover_skill_roots` in `skills.rs`.
fn discover_tool_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    let push = |v: &mut Vec<PathBuf>, p: PathBuf| {
        if p.is_dir() && !v.iter().any(|d| *d == p) {
            v.push(p);
        }
    };

    for ancestor in cwd.ancestors() {
        push(&mut roots, ancestor.join(".elai").join("tools"));
    }

    if let Some(home) = std::env::var_os("HOME") {
        push(&mut roots, PathBuf::from(home).join(".elai").join("tools"));
    }

    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_returns_default_when_no_files_exist() {
        let dir = std::env::temp_dir().join("tool_catalog_empty_test");
        let _ = fs::create_dir_all(&dir);
        let catalog = ToolCatalog::load(&dir);
        assert!(catalog.overrides.is_empty());
        assert!(catalog.aliases.is_empty());
    }

    #[test]
    fn resolve_alias_returns_original_when_not_found() {
        let catalog = ToolCatalog::default();
        assert_eq!(catalog.resolve_alias("read"), "read");
    }

    #[test]
    fn resolve_alias_returns_canonical() {
        let mut catalog = ToolCatalog::default();
        catalog
            .aliases
            .insert("r".to_string(), "read_file".to_string());
        assert_eq!(catalog.resolve_alias("r"), "read_file");
    }

    #[test]
    fn enabled_defaults_to_true() {
        let catalog = ToolCatalog::default();
        assert!(catalog.enabled("nonexistent_tool"));
    }

    #[test]
    fn priority_for_returns_none_when_not_found() {
        let catalog = ToolCatalog::default();
        assert!(catalog.priority_for("nonexistent_tool").is_none());
    }
}
