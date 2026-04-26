//! Skill model, parser, loader, validator, and prompt builder.
//!
//! Skills are SKILL.md files discovered under `.elai/skills/`, `.codex/skills/`,
//! and their home-directory equivalents. Each file has a YAML frontmatter block
//! followed by markdown instruction text injected into the system prompt.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ─── Value types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum FrontmatterValue {
    Str(String),
    Int(i32),
    Float(f32),
    Bool(bool),
    List(Vec<String>),
}

// ─── Structs ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub version: String,
    pub priority: i32,
    pub budget_multiplier: f32,
    pub force_provider: Option<String>,
    pub incompatible_with: Vec<String>,
    pub requires_tools: Vec<String>,
    pub allow_fallback: bool,
    pub max_output_tokens: Option<u32>,
    pub timeout_ms: Option<u64>,
}

impl Default for SkillMetadata {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            version: "1.0.0".to_string(),
            priority: 50,
            budget_multiplier: 1.0,
            force_provider: None,
            incompatible_with: Vec::new(),
            requires_tools: Vec::new(),
            allow_fallback: true,
            max_output_tokens: None,
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub metadata: SkillMetadata,
    pub body: String,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SkillValidation {
    pub valid: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SkillPromptResult {
    pub sections: Vec<String>,
    pub budget_multiplier: f32,
    pub force_provider: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub timeout_ms: Option<u64>,
}

// ─── Frontmatter parser ───────────────────────────────────────────────────────

fn unquote_str(value: &str) -> &str {
    let v = value.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"'))
            || (v.starts_with('\'') && v.ends_with('\'')))
    {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

fn parse_scalar(value: &str) -> FrontmatterValue {
    let v = unquote_str(value);
    if v == "true" {
        return FrontmatterValue::Bool(true);
    }
    if v == "false" {
        return FrontmatterValue::Bool(false);
    }
    if let Ok(i) = v.parse::<i32>() {
        return FrontmatterValue::Int(i);
    }
    if let Ok(f) = v.parse::<f32>() {
        return FrontmatterValue::Float(f);
    }
    FrontmatterValue::Str(v.to_string())
}

fn parse_frontmatter(content: &str) -> (HashMap<String, FrontmatterValue>, String) {
    let lines: Vec<&str> = content.lines().collect();

    if lines.first().map(|s| s.trim()) != Some("---") {
        return (HashMap::new(), content.to_string());
    }

    let mut map = HashMap::new();
    let mut end_index = None;
    let mut pending_list_key: Option<String> = None;
    let mut pending_list: Vec<String> = Vec::new();
    let mut i = 1;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed == "---" {
            if let Some(key) = pending_list_key.take() {
                map.insert(key, FrontmatterValue::List(pending_list.clone()));
                pending_list.clear();
            }
            end_index = Some(i);
            break;
        }

        if let Some(ref key) = pending_list_key.clone() {
            if let Some(item) = trimmed.strip_prefix("- ") {
                pending_list.push(unquote_str(item.trim()).to_string());
                i += 1;
                continue;
            } else if trimmed.is_empty() || !trimmed.starts_with('-') {
                map.insert(key.clone(), FrontmatterValue::List(pending_list.clone()));
                pending_list.clear();
                pending_list_key = None;
                if trimmed.is_empty() {
                    i += 1;
                    continue;
                }
                // fall through to reparse as key:value
            }
        }

        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim().to_string();
            let value = trimmed[colon_pos + 1..].trim();
            if value.is_empty() {
                pending_list_key = Some(key);
                pending_list.clear();
            } else {
                map.insert(key, parse_scalar(value));
            }
        }

        i += 1;
    }

    if let Some(key) = pending_list_key.take() {
        if !pending_list.is_empty() {
            map.insert(key, FrontmatterValue::List(pending_list));
        }
    }

    let body = if let Some(idx) = end_index {
        lines[idx + 1..].join("\n")
    } else {
        content.to_string()
    };

    (map, body)
}

fn map_to_metadata(map: &HashMap<String, FrontmatterValue>, path: &Path) -> SkillMetadata {
    let mut meta = SkillMetadata::default();

    if let Some(FrontmatterValue::Str(v)) = map.get("name") {
        meta.name = v.clone();
    }
    if meta.name.is_empty() {
        // Fall back to directory/file name
        if let Some(stem) = path.parent().and_then(|p| p.file_name()) {
            meta.name = stem.to_string_lossy().into_owned();
        } else if let Some(stem) = path.file_stem() {
            meta.name = stem.to_string_lossy().into_owned();
        }
    }

    if let Some(FrontmatterValue::Str(v)) = map.get("description") {
        meta.description = v.clone();
    }
    if let Some(FrontmatterValue::Str(v)) = map.get("version") {
        meta.version = v.clone();
    }
    if let Some(v) = map.get("priority") {
        meta.priority = match v {
            FrontmatterValue::Int(i) => *i,
            FrontmatterValue::Float(f) => *f as i32,
            _ => 50,
        };
    }
    if let Some(v) = map.get("budget_multiplier") {
        meta.budget_multiplier = match v {
            FrontmatterValue::Float(f) => *f,
            FrontmatterValue::Int(i) => *i as f32,
            _ => 1.0,
        };
    }
    if let Some(FrontmatterValue::Str(v)) = map.get("force_provider") {
        meta.force_provider = Some(v.clone());
    }
    if let Some(FrontmatterValue::List(v)) = map.get("incompatible_with") {
        meta.incompatible_with = v.clone();
    }
    if let Some(FrontmatterValue::List(v)) = map.get("requires_tools") {
        meta.requires_tools = v.clone();
    }
    if let Some(FrontmatterValue::Bool(v)) = map.get("allow_fallback") {
        meta.allow_fallback = *v;
    }
    if let Some(v) = map.get("max_output_tokens") {
        meta.max_output_tokens = match v {
            FrontmatterValue::Int(i) if *i > 0 => Some(*i as u32),
            _ => None,
        };
    }
    if let Some(v) = map.get("timeout_ms") {
        meta.timeout_ms = match v {
            FrontmatterValue::Int(i) if *i > 0 => Some(*i as u64),
            _ => None,
        };
    }

    meta
}

// ─── Loader ───────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SkillLoadError {
    Io(std::io::Error),
    MissingName(PathBuf),
}

impl std::fmt::Display for SkillLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::MissingName(p) => write!(f, "skill at {} has no name", p.display()),
        }
    }
}

impl From<std::io::Error> for SkillLoadError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub fn load_skill(path: &Path) -> Result<Skill, SkillLoadError> {
    let contents = std::fs::read_to_string(path)?;
    let (map, body) = parse_frontmatter(&contents);
    let metadata = map_to_metadata(&map, path);
    if metadata.name.is_empty() {
        return Err(SkillLoadError::MissingName(path.to_path_buf()));
    }
    Ok(Skill {
        metadata,
        body: body.trim().to_string(),
        file_path: path.to_path_buf(),
    })
}

fn skill_dirs_from_roots(roots: &[PathBuf]) -> Vec<Skill> {
    let mut skills = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dir_skills = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let skill_md = if path.is_dir() {
                path.join("SKILL.md")
            } else if path.file_name().map(|n| n == "SKILL.md").unwrap_or(false) {
                path.clone()
            } else {
                continue;
            };
            if skill_md.is_file() {
                match load_skill(&skill_md) {
                    Ok(s) => dir_skills.push(s),
                    Err(e) => {
                        eprintln!("[skills] skipping {}: {e}", skill_md.display());
                    }
                }
            }
        }
        dir_skills.sort_by(|a, b| b.metadata.priority.cmp(&a.metadata.priority));
        skills.extend(dir_skills);
    }
    skills
}

fn discover_skill_roots(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let push = |dirs: &mut Vec<PathBuf>, p: PathBuf| {
        if p.is_dir() && !dirs.iter().any(|d| *d == p) {
            dirs.push(p);
        }
    };

    for ancestor in cwd.ancestors() {
        push(&mut dirs, ancestor.join(".elai").join("skills"));
        push(&mut dirs, ancestor.join(".codex").join("skills"));
    }

    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        push(&mut dirs, PathBuf::from(codex_home).join("skills"));
    }

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        push(&mut dirs, home.join(".elai").join("skills"));
        push(&mut dirs, home.join(".codex").join("skills"));
    }

    dirs
}

/// Loads all skills from discovered directories under `cwd`.
/// Skills are deduplicated by name (first occurrence wins), then sorted by
/// priority descending.
pub fn load_all_skills(cwd: &Path) -> Vec<Skill> {
    let roots = discover_skill_roots(cwd);
    let raw = skill_dirs_from_roots(&roots);

    let mut seen = std::collections::HashSet::new();
    let mut skills: Vec<Skill> = Vec::new();
    for skill in raw {
        let name = skill.metadata.name.clone();
        if seen.insert(name) {
            skills.push(skill);
        }
    }

    skills.sort_by(|a, b| b.metadata.priority.cmp(&a.metadata.priority));
    skills
}

// ─── Validation ───────────────────────────────────────────────────────────────

pub fn validate_skills(skills: &[Skill]) -> SkillValidation {
    let mut errors = Vec::new();

    // Incompatibility check (bidirectional)
    for skill in skills {
        for incompatible in &skill.metadata.incompatible_with {
            let conflict = skills
                .iter()
                .any(|other| &other.metadata.name == incompatible);
            if conflict {
                errors.push(format!(
                    "Skill '{}' is incompatible with '{incompatible}'",
                    skill.metadata.name
                ));
            }
        }
    }

    // Provider conflict check
    let providers: Vec<&str> = skills
        .iter()
        .filter_map(|s| s.metadata.force_provider.as_deref())
        .collect();
    if providers.len() > 1 {
        let first = providers[0];
        for other in &providers[1..] {
            if *other != first {
                errors.push(format!(
                    "Skills force conflicting providers: '{first}' vs '{other}'"
                ));
            }
        }
    }

    SkillValidation {
        valid: errors.is_empty(),
        errors,
    }
}

// ─── Prompt builder ───────────────────────────────────────────────────────────

pub fn build_skill_prompt_sections(skills: &[Skill]) -> SkillPromptResult {
    let mut sorted: Vec<&Skill> = skills.iter().collect();
    sorted.sort_by(|a, b| b.metadata.priority.cmp(&a.metadata.priority));

    let mut sections = Vec::new();
    let header = format!(
        "# Active Skills\nThe following {} skill(s) are loaded.",
        sorted.len()
    );
    sections.push(header);

    for skill in &sorted {
        let m = &skill.metadata;
        let mut section = format!(
            "## ACTIVE SKILL: {} (v{})\nPriority: {} | Budget: {}x",
            m.name, m.version, m.priority, m.budget_multiplier
        );
        if !m.description.is_empty() {
            section.push('\n');
            section.push_str(&m.description);
        }
        if !skill.body.is_empty() {
            section.push_str("\n\n");
            section.push_str(&skill.body);
        }
        sections.push(section);
    }

    let budget_multiplier = sorted
        .iter()
        .map(|s| s.metadata.budget_multiplier)
        .fold(1.0_f32, |acc, x| acc * x);

    let force_provider = sorted
        .iter()
        .find_map(|s| s.metadata.force_provider.clone());

    let max_output_tokens = sorted
        .iter()
        .filter_map(|s| s.metadata.max_output_tokens)
        .min();

    let timeout_ms = sorted
        .iter()
        .filter_map(|s| s.metadata.timeout_ms)
        .min();

    SkillPromptResult {
        sections,
        budget_multiplier,
        force_provider,
        max_output_tokens,
        timeout_ms,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("skill-test-{nanos}"))
    }

    fn make_skill_md(dir: &Path, name: &str, content: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn test_parse_full_frontmatter() {
        let content = "---\nname: my-skill\ndescription: Does things\nversion: 2.0.0\npriority: 80\nbudget_multiplier: 1.5\nforce_provider: sonnet\nallow_fallback: false\n---\nThe body.\n";
        let (map, body) = parse_frontmatter(content);
        assert_eq!(map.get("name").map(|v| if let FrontmatterValue::Str(s) = v { s.as_str() } else { "" }), Some("my-skill"));
        assert_eq!(map.get("priority").map(|v| if let FrontmatterValue::Int(i) = v { *i } else { 0 }), Some(80));
        assert_eq!(map.get("allow_fallback").map(|v| if let FrontmatterValue::Bool(b) = v { *b } else { true }), Some(false));
        assert!(body.contains("The body."));
    }

    #[test]
    fn test_parse_minimal_frontmatter() {
        let content = "---\nname: minimal\n---\nInstructions here.";
        let (map, body) = parse_frontmatter(content);
        assert!(matches!(map.get("name"), Some(FrontmatterValue::Str(_))));
        assert!(!body.is_empty());
        // Defaults applied via map_to_metadata
        let meta = map_to_metadata(&map, Path::new("/skills/minimal/SKILL.md"));
        assert_eq!(meta.priority, 50);
        assert!((meta.budget_multiplier - 1.0).abs() < f32::EPSILON);
        assert!(meta.allow_fallback);
    }

    #[test]
    fn test_parse_array_fields() {
        let content = "---\nname: test\nincompatible_with:\n- skill-a\n- skill-b\nrequires_tools:\n- bash\n---\n";
        let (map, _) = parse_frontmatter(content);
        let incompat = map.get("incompatible_with");
        assert!(matches!(incompat, Some(FrontmatterValue::List(v)) if v.len() == 2));
    }

    #[test]
    fn test_parse_no_frontmatter() {
        let content = "Just plain content.";
        let (map, body) = parse_frontmatter(content);
        assert!(map.is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_load_skill_from_file() {
        let root = tmp();
        let skill_path = root.join("SKILL.md");
        fs::create_dir_all(&root).unwrap();
        fs::write(&skill_path, "---\nname: my-skill\ndescription: Test\n---\nDo stuff.").unwrap();
        let skill = load_skill(&skill_path).unwrap();
        assert_eq!(skill.metadata.name, "my-skill");
        assert_eq!(skill.body, "Do stuff.");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn test_load_all_skills_from_dir() {
        let root = tmp();
        let skills_dir = root.join(".elai").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        make_skill_md(&skills_dir, "skill-high", "---\nname: skill-high\npriority: 90\n---\nHigh.");
        make_skill_md(&skills_dir, "skill-low", "---\nname: skill-low\npriority: 10\n---\nLow.");

        let skills = load_all_skills(&root);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].metadata.name, "skill-high");
        assert_eq!(skills[1].metadata.name, "skill-low");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn test_empty_skills_dir() {
        let root = tmp();
        let skills_dir = root.join(".elai").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let skills = load_all_skills(&root);
        assert!(skills.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn test_deduplication_by_name() {
        let root = tmp();
        let skills_dir = root.join(".elai").join("skills");
        let other_dir = root.join(".codex").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::create_dir_all(&other_dir).unwrap();
        make_skill_md(&skills_dir, "dupe", "---\nname: dupe\n---\nFirst.");
        make_skill_md(&other_dir, "dupe", "---\nname: dupe\n---\nSecond.");
        let skills = load_all_skills(&root);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].body, "First.");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn test_validate_incompatible_skills() {
        let mut s1 = make_test_skill("alpha", 50);
        s1.metadata.incompatible_with = vec!["beta".to_string()];
        let s2 = make_test_skill("beta", 50);
        let result = validate_skills(&[s1, s2]);
        assert!(!result.valid);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_validate_provider_conflict() {
        let mut s1 = make_test_skill("a", 50);
        s1.metadata.force_provider = Some("opus".to_string());
        let mut s2 = make_test_skill("b", 50);
        s2.metadata.force_provider = Some("haiku".to_string());
        let result = validate_skills(&[s1, s2]);
        assert!(!result.valid);
    }

    #[test]
    fn test_validate_valid_set() {
        let s1 = make_test_skill("a", 50);
        let s2 = make_test_skill("b", 50);
        let result = validate_skills(&[s1, s2]);
        assert!(result.valid);
    }

    #[test]
    fn test_build_prompt_sections() {
        let mut s = make_test_skill("my-skill", 80);
        s.metadata.budget_multiplier = 2.0;
        s.body = "Do X.".to_string();
        let result = build_skill_prompt_sections(&[s]);
        assert!(result.sections[0].contains("Active Skills"));
        assert!(result.sections[1].contains("my-skill"));
        assert!(result.sections[1].contains("Do X."));
        assert!((result.budget_multiplier - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_budget_multiplier_product() {
        let mut s1 = make_test_skill("a", 50);
        s1.metadata.budget_multiplier = 2.0;
        let mut s2 = make_test_skill("b", 50);
        s2.metadata.budget_multiplier = 3.0;
        let result = build_skill_prompt_sections(&[s1, s2]);
        assert!((result.budget_multiplier - 6.0).abs() < f32::EPSILON);
    }

    fn make_test_skill(name: &str, priority: i32) -> Skill {
        Skill {
            metadata: SkillMetadata {
                name: name.to_string(),
                priority,
                ..SkillMetadata::default()
            },
            body: String::new(),
            file_path: PathBuf::from(format!("/skills/{name}/SKILL.md")),
        }
    }
}
