use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// ─── Types ────────────────────────────────────────────────────────────────────

/// A file path reference extracted from an instruction file (ELAI.md, CLAUDE.md, etc.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub path: PathBuf,
    pub source_file: PathBuf,
    pub line_number: usize,
}

/// Result of comparing filesystem vs. memory entries
#[derive(Debug, Default)]
pub struct VerifyReport {
    pub verified: Vec<PathBuf>,
    pub missing: Vec<PathBuf>,
    pub untracked: Vec<PathBuf>,
    pub drift: Vec<PathBuf>,
    pub files_scanned: usize,
    pub memory_entries: usize,
}


// ─── walk_project ─────────────────────────────────────────────────────────────

/// Recursively walk the project tree, returning relative paths for all files.
/// Delegates to `code_index::walker` for the actual traversal.
pub fn walk_project(root: &Path) -> io::Result<Vec<PathBuf>> {
    let abs_paths = code_index::walker::walk_project(root);
    let rel_paths = abs_paths
        .into_iter()
        .map(|p| {
            p.strip_prefix(root)
                .map(PathBuf::from)
                .unwrap_or(p)
        })
        .collect();
    Ok(rel_paths)
}

// ─── parse_memory_entries ────────────────────────────────────────────────────

const RECOGNIZED_EXTENSIONS: &[&str] = &[
    ".rs", ".ts", ".toml", ".md", ".json", ".yaml", ".yml", ".py", ".go", ".sh",
];

/// Paths that are always local/machine-specific and must never be flagged as
/// missing — even when the instruction file mentions them in prose.
const LOCAL_ONLY_PATHS: &[&str] = &[
    ".elai.json",
    ".elai/settings.local.json",
    ".elai/settings.local.toml",
    ".claude/settings.local.json",
];

/// Extract file path references from the given instruction file contents.
/// `instruction_files` is a slice of (path, content) pairs.
pub fn parse_memory_entries(instruction_files: &[(PathBuf, String)]) -> Vec<MemoryEntry> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut entries = Vec::new();

    for (source_file, content) in instruction_files {
        for (line_idx, line) in content.lines().enumerate() {
            let line_number = line_idx + 1;
            let candidates = extract_path_candidates(line);
            for path_str in candidates {
                // Skip machine-local paths — they are never committed and
                // reporting them as "missing" is always a false positive.
                if LOCAL_ONLY_PATHS.iter().any(|p| path_str == *p) {
                    continue;
                }
                let pb = PathBuf::from(&path_str);
                if seen.contains(&pb) {
                    continue;
                }
                seen.insert(pb.clone());
                entries.push(MemoryEntry {
                    path: pb,
                    source_file: source_file.clone(),
                    line_number,
                });
            }
        }
    }

    entries
}

fn extract_path_candidates(line: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    // Extract from backtick spans: `src/foo.rs`
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        if let Some(end) = after.find('`') {
            let candidate = &after[..end];
            if looks_like_path(candidate) {
                candidates.push(candidate.to_string());
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }

    // Extract after keyword prefixes: CREATE:, MODIFY:, DELETE:, READ:, file:
    for prefix in &["CREATE:", "MODIFY:", "DELETE:", "READ:", "file:"] {
        if let Some(pos) = line.find(prefix) {
            let after = line[pos + prefix.len()..].trim_start();
            let word: String = after
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ',' && *c != ')' && *c != ']')
                .collect();
            if looks_like_path(&word) {
                candidates.push(word);
            }
        }
    }

    // Extract bare paths: tokens with recognized extension (may or may not contain '/')
    for token in line.split_whitespace() {
        let token = token
            .trim_matches(|c: char| c == '`' || c == '\'' || c == '"' || c == ',' || c == ')' || c == '(');
        if looks_like_path(token) {
            candidates.push(token.to_string());
        }
    }

    // Deduplicate while preserving order
    let mut seen = HashSet::new();
    candidates.retain(|c| seen.insert(c.clone()));
    candidates
}

fn looks_like_path(s: &str) -> bool {
    if s.is_empty() || s.len() < 3 {
        return false;
    }
    let has_ext = RECOGNIZED_EXTENSIONS.iter().any(|ext| s.ends_with(ext));
    has_ext
}

// ─── diff_entries ────────────────────────────────────────────────────────────

/// Compare filesystem files vs. memory entries and produce a report.
pub fn diff_entries(_root: &Path, files: &[PathBuf], memory: &[MemoryEntry]) -> VerifyReport {
    let file_set: HashSet<&PathBuf> = files.iter().collect();
    let mem_paths: Vec<&PathBuf> = memory.iter().map(|e| &e.path).collect();
    let mem_set: HashSet<&PathBuf> = mem_paths.iter().copied().collect();

    // Collect DELETE-mentioned paths from memory entries
    // (we need the original source file content for this, but we store drift
    //  based on files that exist on disk but appear in a DELETE context)
    // Simplified: drift = files that exist on disk AND appear in memory entries
    // whose source line mentioned "DELETE"
    // We don't have line content here, so we keep a separate pass in run_verify.
    // For now, drift is empty (populated by run_verify).

    let verified: Vec<PathBuf> = mem_paths
        .iter()
        .filter(|p| file_set.contains(*p))
        .map(|p| (*p).clone())
        .collect();

    let missing: Vec<PathBuf> = mem_paths
        .iter()
        .filter(|p| !file_set.contains(*p))
        .map(|p| (*p).clone())
        .collect();

    let untracked: Vec<PathBuf> = files
        .iter()
        .filter(|p| !mem_set.contains(p))
        .cloned()
        .collect();

    VerifyReport {
        files_scanned: files.len(),
        memory_entries: memory.len(),
        verified,
        missing,
        untracked,
        drift: Vec::new(),
    }
}

// ─── render_verify_report ────────────────────────────────────────────────────

const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_RESET: &str = "\x1b[0m";

/// Render a colored report string for terminal output.
pub fn render_verify_report(report: &VerifyReport, _root: &Path) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "{ANSI_BOLD}Verify — Codebase \u{00d7} Memory Sync{ANSI_RESET}\n"
    ));
    out.push_str(&format!(
        "  Scanned {} files | Memory has {} entries\n",
        report.files_scanned, report.memory_entries
    ));

    // File references section
    let has_refs = !report.verified.is_empty()
        || !report.missing.is_empty()
        || !report.drift.is_empty();

    if has_refs {
        out.push('\n');
        out.push_str("File References in Memory:\n");

        for path in &report.verified {
            out.push_str(&format!(
                "  {ANSI_GREEN}\u{2713}{ANSI_RESET}  {}\n",
                path.display()
            ));
        }
        for path in &report.drift {
            out.push_str(&format!(
                "  {ANSI_YELLOW}?{ANSI_RESET}  {} — drift\n",
                path.display()
            ));
        }
        for path in &report.missing {
            out.push_str(&format!(
                "  {ANSI_RED}\u{2717}{ANSI_RESET}  {} — missing from filesystem\n",
                path.display()
            ));
        }
    } else if report.memory_entries == 0 {
        out.push('\n');
        out.push_str("File References in Memory:\n");
        out.push_str(&format!(
            "  {ANSI_DIM}No file paths found in instruction files.{ANSI_RESET}\n"
        ));
    }

    // Untracked section
    if !report.untracked.is_empty() {
        out.push('\n');
        out.push_str(&format!(
            "Untracked (not in memory): {} files\n",
            report.untracked.len()
        ));
        const MAX_SHOW: usize = 10;
        for path in report.untracked.iter().take(MAX_SHOW) {
            out.push_str(&format!(
                "  {ANSI_DIM}{}{ANSI_RESET}\n",
                path.display()
            ));
        }
        if report.untracked.len() > MAX_SHOW {
            out.push_str(&format!(
                "  {ANSI_DIM}... and {} more{ANSI_RESET}\n",
                report.untracked.len() - MAX_SHOW
            ));
        }
    }

    // Summary
    out.push('\n');
    out.push_str(&format!(
        "Summary: {green}{v} verified{reset} | {yellow}{d} drift{reset} | {red}{m} missing{reset} | {dim}{u} untracked{reset}\n",
        green = ANSI_GREEN,
        yellow = ANSI_YELLOW,
        red = ANSI_RED,
        dim = ANSI_DIM,
        reset = ANSI_RESET,
        v = report.verified.len(),
        d = report.drift.len(),
        m = report.missing.len(),
        u = report.untracked.len(),
    ));

    out
}

/// Plain-text report for TUI embedding — no ANSI codes, no untracked file
/// list (just a count). Fits neatly inside a `ChatEntry::SystemNote`.
pub fn render_verify_report_tui(report: &VerifyReport) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "Verify — Codebase × Memory Sync\n  {} files escaneados | {} entradas na memória\n",
        report.files_scanned, report.memory_entries
    ));

    let has_refs = !report.verified.is_empty()
        || !report.missing.is_empty()
        || !report.drift.is_empty();

    if has_refs || report.memory_entries > 0 {
        out.push_str("\nReferências de Arquivo:\n");
        for path in &report.verified {
            out.push_str(&format!("  ✓  {}\n", path.display()));
        }
        for path in &report.drift {
            out.push_str(&format!("  ~  {} — drift\n", path.display()));
        }
        for path in &report.missing {
            out.push_str(&format!("  ✗  {} — não encontrado\n", path.display()));
        }
        if !has_refs {
            out.push_str("  (nenhuma referência de arquivo nas instruções)\n");
        }
    }

    out.push_str(&format!(
        "\nResumo: {} verificados | {} drift | {} ausentes | {} não rastreados\n",
        report.verified.len(),
        report.drift.len(),
        report.missing.len(),
        report.untracked.len(),
    ));

    if !report.missing.is_empty() {
        out.push_str("\nDica: os arquivos ausentes foram removidos ou renomeados.\n");
        out.push_str("      Atualize ELAI.md para refletir o estado atual.\n");
    }

    out
}

// ─── run_verify ──────────────────────────────────────────────────────────────

/// Runs the verify flow and returns `(report, formatted_terminal_string)`.
pub fn run_verify_inner(
    cwd: &Path,
) -> Result<(VerifyReport, String), Box<dyn std::error::Error>> {
    // Discover instruction files
    let candidate_names = [
        "ELAI.md",
        "CLAUDE.md",
        ".elai/ELAI.md",
        ".elai/instructions.md",
        ".elai/memory.md",
    ];

    let instruction_files: Vec<(PathBuf, String)> = candidate_names
        .iter()
        .map(|name| cwd.join(name))
        .filter(|p| p.is_file())
        .filter_map(|p| {
            let content = fs::read_to_string(&p).ok()?;
            Some((p, content))
        })
        .collect();

    let files = walk_project(cwd)?;
    let memory = parse_memory_entries(&instruction_files);

    let mut report = diff_entries(cwd, &files, &memory);

    // Populate drift: files that exist on disk but memory mentions "DELETE <path>"
    let file_set: HashSet<&PathBuf> = files.iter().collect();
    for (_, content) in &instruction_files {
        for line in content.lines() {
            if let Some(pos) = line.find("DELETE ") {
                let after = line[pos + 7..].trim_start();
                let word: String = after
                    .chars()
                    .take_while(|c| !c.is_whitespace() && *c != ',' && *c != ')')
                    .collect();
                if looks_like_path(&word) {
                    let pb = PathBuf::from(&word);
                    if file_set.contains(&pb) && !report.drift.contains(&pb) {
                        report.drift.push(pb);
                    }
                }
            }
        }
    }

    let rendered = render_verify_report(&report, cwd);
    Ok((report, rendered))
}

/// Orchestrates the full verify flow and returns the terminal-formatted report.
pub fn run_verify(cwd: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let (_, rendered) = run_verify_inner(cwd)?;
    Ok(rendered)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(suffix: &str) -> Self {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("elai_verify_test_{suffix}_{ts}"));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn test_walk_project_respects_gitignore() {
        let dir = TempDir::new("gitignore");
        let root = dir.path();

        // Create .gitignore
        fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        // Create directory structure
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("debug.log"), "log content").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("target/debug/binary"), "bin").unwrap();

        let files = walk_project(root).unwrap();
        let names: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();

        assert!(
            names.iter().any(|n| n.contains("main.rs")),
            "main.rs should be present, got: {:?}", names
        );
        assert!(
            !names.iter().any(|n| n.contains("debug.log")),
            "debug.log should be excluded by .gitignore"
        );
        assert!(
            !names.iter().any(|n| n.contains("target")),
            "target/ should be excluded by hardcoded ignores"
        );
    }

    #[test]
    fn test_parse_memory_entries_extracts_paths() {
        let content = "
# Project

This project uses `src/main.rs` as the entry point.
See also Cargo.toml for dependencies.
MODIFY: src/lib.rs — add new function
";
        let source = PathBuf::from("ELAI.md");
        let files = vec![(source.clone(), content.to_string())];
        let entries = parse_memory_entries(&files);

        let paths: Vec<String> = entries.iter().map(|e| e.path.to_string_lossy().to_string()).collect();

        assert!(
            paths.iter().any(|p| p == "src/main.rs"),
            "should extract src/main.rs from backticks, got: {:?}", paths
        );
        assert!(
            paths.iter().any(|p| p == "Cargo.toml"),
            "should extract Cargo.toml, got: {:?}", paths
        );
        assert!(
            paths.iter().any(|p| p == "src/lib.rs"),
            "should extract src/lib.rs from MODIFY: prefix, got: {:?}", paths
        );

        // Deduplication: src/main.rs should appear only once
        let count = paths.iter().filter(|p| *p == "src/main.rs").count();
        assert_eq!(count, 1, "src/main.rs should appear only once");
    }

    #[test]
    fn test_diff_entries_categorizes_correctly() {
        let root = PathBuf::from("/tmp/test");
        let files = vec![
            PathBuf::from("a.rs"),
            PathBuf::from("b.rs"),
            PathBuf::from("c.rs"),
        ];
        let source = PathBuf::from("ELAI.md");
        let memory = vec![
            MemoryEntry { path: PathBuf::from("a.rs"), source_file: source.clone(), line_number: 1 },
            MemoryEntry { path: PathBuf::from("b.rs"), source_file: source.clone(), line_number: 2 },
            MemoryEntry { path: PathBuf::from("d.rs"), source_file: source.clone(), line_number: 3 },
        ];

        let report = diff_entries(&root, &files, &memory);

        let verified_paths: Vec<&str> = report.verified.iter().map(|p| p.to_str().unwrap()).collect();
        assert!(verified_paths.contains(&"a.rs"), "a.rs should be verified");
        assert!(verified_paths.contains(&"b.rs"), "b.rs should be verified");

        let untracked_paths: Vec<&str> = report.untracked.iter().map(|p| p.to_str().unwrap()).collect();
        assert!(untracked_paths.contains(&"c.rs"), "c.rs should be untracked");

        let missing_paths: Vec<&str> = report.missing.iter().map(|p| p.to_str().unwrap()).collect();
        assert!(missing_paths.contains(&"d.rs"), "d.rs should be missing");
    }

    #[test]
    fn test_ignore_rules_parse() {
        use code_index::walker::IgnoreRules;

        let dir = TempDir::new("ignore_rules");
        let root = dir.path();

        fs::write(
            root.join(".gitignore"),
            "*.log\nbuild/\n!important.log\ntarget\n",
        ).unwrap();

        let rules = IgnoreRules::load(root);

        assert!(
            rules.should_ignore(&PathBuf::from("debug.log"), "debug.log"),
            "*.log pattern should match debug.log"
        );
        assert!(
            !rules.should_ignore(&PathBuf::from("important.log"), "important.log"),
            "!important.log negation should not ignore important.log"
        );
        assert!(
            rules.should_ignore(&PathBuf::from("build"), "build"),
            "build/ pattern should ignore build dir"
        );
        assert!(
            rules.should_ignore(&PathBuf::from("target"), "target"),
            "hardcoded target should always be ignored"
        );
        assert!(
            !rules.should_ignore(&PathBuf::from("src/main.rs"), "main.rs"),
            "regular .rs file should not be ignored"
        );
    }

    #[test]
    fn test_render_verify_report_format() {
        let report = VerifyReport {
            verified: vec![PathBuf::from("src/main.rs"), PathBuf::from("Cargo.toml")],
            missing: vec![PathBuf::from("src/deleted.rs")],
            untracked: vec![PathBuf::from("src/new.rs")],
            drift: vec![],
            files_scanned: 10,
            memory_entries: 3,
        };

        let root = PathBuf::from("/tmp");
        let output = render_verify_report(&report, &root);

        assert!(output.contains("verified"), "output should mention 'verified'");
        assert!(output.contains("missing"), "output should mention 'missing'");
        assert!(output.contains("2 verified"), "should show count 2 verified");
        assert!(output.contains("1 missing"), "should show count 1 missing");
        assert!(output.contains("src/main.rs"), "should list src/main.rs");
        assert!(output.contains("src/deleted.rs"), "should list deleted file");
        assert!(output.contains("Scanned 10 files"), "should show scanned count");
        assert!(output.contains("Memory has 3 entries"), "should show memory count");
    }

    #[test]
    fn test_walk_project_depth_limit() {
        let dir = TempDir::new("depth");
        let root = dir.path();

        // Create 20 levels of nesting
        let mut deep = root.to_path_buf();
        for i in 0..20 {
            deep = deep.join(format!("level{i}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep_file.rs"), "deep").unwrap();

        // Create a file at depth 14 (should be included)
        let mut shallow = root.to_path_buf();
        for i in 0..14 {
            shallow = shallow.join(format!("level{i}"));
        }
        fs::write(shallow.join("shallow_file.rs"), "shallow").unwrap();

        let files = walk_project(root).unwrap();
        let names: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();

        // deep_file.rs is at depth 21 (20 dirs + file), beyond MAX_DEPTH=15
        assert!(
            !names.iter().any(|n| n.contains("deep_file.rs")),
            "file at depth >15 should not be included"
        );
        // shallow_file.rs at depth 14 should be present
        assert!(
            names.iter().any(|n| n.contains("shallow_file.rs")),
            "file at depth 14 should be included, got: {:?}", names
        );
    }
}
