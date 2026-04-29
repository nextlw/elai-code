use std::fs;
use std::path::{Path, PathBuf};

// ─── Binary extensions to skip ────────────────────────────────────────────────

const BINARY_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".zip", ".tar", ".gz", ".lock",
];

// ─── IgnoreRules ──────────────────────────────────────────────────────────────

enum IgnorePattern {
    Exact(String),
    Extension(String),
    PathPrefix(String),
    Negation(String),
}

/// Rules for ignoring files and directories during project walking.
pub struct IgnoreRules {
    patterns: Vec<IgnorePattern>,
}

impl IgnoreRules {
    /// Load ignore rules from `.gitignore` in `root`, plus hardcoded defaults.
    #[must_use]
    pub fn load(root: &Path) -> Self {
        let hardcoded = [
            ".git",
            "target",
            "node_modules",
            ".DS_Store",
            ".omc",
            ".elai/sessions",
        ];

        let mut patterns: Vec<IgnorePattern> = hardcoded
            .iter()
            .map(|s| IgnorePattern::Exact((*s).to_string()))
            .collect();

        if let Ok(content) = fs::read_to_string(root.join(".gitignore")) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some(rest) = line.strip_prefix('!') {
                    patterns.push(IgnorePattern::Negation(rest.to_string()));
                } else if line.starts_with("*.") {
                    let ext = line[1..].to_string(); // e.g. ".log"
                    patterns.push(IgnorePattern::Extension(ext));
                } else if line.ends_with('/') {
                    let prefix = line.trim_end_matches('/').to_string();
                    patterns.push(IgnorePattern::PathPrefix(prefix));
                } else {
                    patterns.push(IgnorePattern::Exact(line.to_string()));
                }
            }
        }

        Self { patterns }
    }

    /// Returns `true` if the entry should be excluded from the walk.
    #[must_use]
    pub fn should_ignore(&self, rel: &Path, name: &str) -> bool {
        let rel_str = rel.to_string_lossy();
        let mut ignored = false;

        for pattern in &self.patterns {
            match pattern {
                IgnorePattern::Exact(s) => {
                    if name == s || rel_str == *s {
                        ignored = true;
                    }
                }
                IgnorePattern::Extension(ext) => {
                    if name.ends_with(ext.as_str()) {
                        ignored = true;
                    }
                }
                IgnorePattern::PathPrefix(prefix) => {
                    if name == prefix
                        || rel_str.starts_with(&format!("{prefix}/"))
                        || rel_str == *prefix
                    {
                        ignored = true;
                    }
                }
                IgnorePattern::Negation(s) => {
                    if name == s || rel_str == *s {
                        ignored = false;
                    }
                }
            }
        }

        ignored
    }
}

// ─── WalkOptions ──────────────────────────────────────────────────────────────

/// Options controlling the behaviour of [`walk_project_with`].
pub struct WalkOptions {
    /// Files larger than this size (in bytes) are skipped. Default: 1 MiB.
    pub max_file_bytes: u64,
    /// Whether to follow symbolic links. Default: `false`.
    pub follow_symlinks: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024, // 1 MiB
            follow_symlinks: false,
        }
    }
}

// ─── walk_project_with / walk_project ─────────────────────────────────────────

const MAX_DEPTH: usize = 15;

/// Recursively walk the project tree, returning **absolute** paths for all
/// files that pass ignore rules, size limits, and binary-extension filters.
#[must_use]
pub fn walk_project_with(root: &Path, opts: &WalkOptions) -> Vec<PathBuf> {
    let rules = IgnoreRules::load(root);
    let mut files = Vec::new();
    walk_inner(root, root, &rules, opts, 0, &mut files);
    files
}

/// Shortcut for [`walk_project_with`] with [`WalkOptions::default()`].
#[must_use]
pub fn walk_project(root: &Path) -> Vec<PathBuf> {
    walk_project_with(root, &WalkOptions::default())
}

fn walk_inner(
    dir: &Path,
    root: &Path,
    rules: &IgnoreRules,
    opts: &WalkOptions,
    depth: usize,
    out: &mut Vec<PathBuf>,
) {
    if depth >= MAX_DEPTH {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else { return };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_path_buf();

        if rules.should_ignore(&rel, name) {
            continue;
        }

        // Resolve symlinks according to opts
        let meta = if opts.follow_symlinks {
            fs::metadata(&path).ok()
        } else {
            fs::symlink_metadata(&path).ok()
        };

        let Some(meta) = meta else { continue };

        // Skip symlinks when not following them
        if !opts.follow_symlinks && meta.file_type().is_symlink() {
            continue;
        }

        if meta.is_dir() {
            walk_inner(&path, root, rules, opts, depth + 1, out);
        } else if meta.is_file() {
            // Skip oversized files
            if meta.len() > opts.max_file_bytes {
                continue;
            }
            // Skip binary extensions
            if has_binary_extension(name) {
                continue;
            }
            out.push(path);
        }
    }
}

fn has_binary_extension(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    BINARY_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::TempDir;

    #[test]
    fn walk_respects_gitignore() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Write .gitignore
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        // Directories that should be ignored
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/binary"), "bin").unwrap();
        fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        fs::write(root.join("node_modules/foo/index.js"), "{}").unwrap();

        // Files that should appear
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();

        let files = walk_project(root);
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.to_str())
            .map(String::from)
            .collect();

        assert!(
            names.iter().any(|n| n.ends_with("src/main.rs")),
            "src/main.rs should be present, got: {names:?}"
        );
        assert!(
            names.iter().any(|n| n.ends_with("Cargo.toml")),
            "Cargo.toml should be present"
        );
        assert!(
            !names.iter().any(|n| n.contains(".git/")),
            ".git contents should be excluded"
        );
        assert!(
            !names.iter().any(|n| n.contains("target/")),
            "target/ should be excluded by hardcoded ignores"
        );
        assert!(
            !names.iter().any(|n| n.contains("node_modules/")),
            "node_modules/ should be excluded"
        );
    }

    #[test]
    fn walk_respects_max_file_bytes() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Create a file larger than 1 MiB
        let large_path = root.join("large.txt");
        {
            let mut f = fs::File::create(&large_path).unwrap();
            let buf = vec![b'a'; 2 * 1024 * 1024]; // 2 MiB
            f.write_all(&buf).unwrap();
        }

        // Small file
        fs::write(root.join("small.txt"), "hello").unwrap();

        let opts = WalkOptions {
            max_file_bytes: 1024 * 1024, // 1 MiB limit
            follow_symlinks: false,
        };
        let files = walk_project_with(root, &opts);
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.to_str())
            .map(String::from)
            .collect();

        assert!(
            names.iter().any(|n| n.ends_with("small.txt")),
            "small.txt should be included"
        );
        assert!(
            !names.iter().any(|n| n.ends_with("large.txt")),
            "large.txt (2 MiB) should be excluded when max is 1 MiB"
        );
    }

    #[test]
    fn walk_skips_binary_extensions() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        fs::write(root.join("image.png"), b"\x89PNG").unwrap();
        fs::write(root.join("Cargo.lock"), "# lock file").unwrap();
        fs::write(root.join("main.rs"), "fn main() {}").unwrap();

        let files = walk_project(root);
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| p.to_str())
            .map(String::from)
            .collect();

        assert!(
            !names.iter().any(|n| n.ends_with("image.png")),
            ".png should be skipped"
        );
        assert!(
            !names.iter().any(|n| n.ends_with("Cargo.lock")),
            ".lock should be skipped"
        );
        assert!(
            names.iter().any(|n| n.ends_with("main.rs")),
            "main.rs should be included"
        );
    }
}
