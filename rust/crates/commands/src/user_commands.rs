//! Discovery e registry de slash commands customizados pelo usuário.
//!
//! Locais:
//!   - `<cwd>/.elai/commands/<name>.md`  → comandos do projeto
//!   - `~/.elai/commands/<name>.md`      → comandos globais
//!
//! Cada `.md` é um command. Frontmatter YAML opcional define metadados;
//! body é o template de prompt com substituições.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Escopo de origem de um user command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCommandScope {
    Project,
    Global,
}

/// Um slash command definido pelo usuário via arquivo `.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub body_template: String,
    pub source_path: PathBuf,
    pub scope: UserCommandScope,
}

/// Registry de user commands descobertos em disco.
#[derive(Debug, Default)]
pub struct UserCommandRegistry {
    by_name: HashMap<String, UserCommand>,
}

impl UserCommandRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Carrega project + global. Project tem precedência (override silencioso de global com mesmo nome).
    pub fn discover(cwd: &Path) -> std::io::Result<Self> {
        let mut reg = Self::new();
        // Global primeiro (project sobrescreve depois)
        if let Some(home) = home_dir() {
            reg.load_dir(
                &home.join(".elai").join("commands"),
                UserCommandScope::Global,
            )?;
        }
        reg.load_dir(
            &cwd.join(".elai").join("commands"),
            UserCommandScope::Project,
        )?;
        Ok(reg)
    }

    fn load_dir(&mut self, dir: &Path, scope: UserCommandScope) -> std::io::Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let name = match path.file_stem().and_then(|s| s.to_str()) {
                Some(n) if is_valid_name(n) => n.to_string(),
                _ => continue,
            };
            let content = std::fs::read_to_string(&path)?;
            if let Some(cmd) = parse_user_command(&name, &content, &path, scope) {
                self.by_name.insert(name, cmd);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&UserCommand> {
        self.by_name.get(name)
    }

    pub fn all(&self) -> impl Iterator<Item = &UserCommand> {
        self.by_name.values()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.by_name.keys().cloned().collect()
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.by_name.len()
    }
}

/// Parsing simples de frontmatter YAML-like (sem dep extra).
/// Aceita formato:
/// ```text
/// ---
/// description: foo
/// argument-hint: "[file]"
/// ---
/// body...
/// ```
#[must_use]
pub fn parse_user_command(
    name: &str,
    content: &str,
    source_path: &Path,
    scope: UserCommandScope,
) -> Option<UserCommand> {
    let (frontmatter, body) = split_frontmatter(content);
    let mut description = format!("Custom command: {name}");
    let mut argument_hint: Option<String> = None;
    if let Some(fm) = frontmatter {
        for line in fm.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
            match key {
                "description"
                    if !value.is_empty() => {
                        description = value;
                    }
                "argument-hint" | "argumentHint"
                    if !value.is_empty() => {
                        argument_hint = Some(value);
                    }
                _ => {}
            }
        }
    }
    Some(UserCommand {
        name: name.to_string(),
        description,
        argument_hint,
        body_template: body.to_string(),
        source_path: source_path.to_path_buf(),
        scope,
    })
}

fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let trimmed = content.trim_start_matches('\u{feff}'); // BOM
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let frontmatter = &rest[..end];
            let body = &rest[end + 5..];
            return (Some(frontmatter), body);
        }
        if let Some(end) = rest.find("\n---") {
            let frontmatter = &rest[..end];
            let body_start = end + 4;
            let body = if body_start < rest.len() {
                &rest[body_start..]
            } else {
                ""
            };
            return (Some(frontmatter), body);
        }
    }
    (None, trimmed)
}

fn is_valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Expande o template substituindo placeholders.
/// Suporte a: `$ARGUMENTS` (tudo), `$1`..$9 (posicional), `$CWD`, `$DATE`.
#[must_use]
pub fn expand_template(template: &str, args: &str, cwd: &Path) -> String {
    let positional: Vec<&str> = args.split_whitespace().collect();
    let mut out = String::with_capacity(template.len() + args.len());
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let next = chars[i + 1];
            // $1..$9
            if next.is_ascii_digit() && next != '0' {
                let idx = (next as u8 - b'0') as usize - 1;
                if idx < positional.len() {
                    out.push_str(positional[idx]);
                }
                i += 2;
                continue;
            }
            // Named substitutions — check longest first to avoid prefix collisions
            let rest: String = chars[i + 1..].iter().collect();
            let mut matched = false;
            for (key, make_value) in named_substitutions(args, cwd) {
                if rest.starts_with(key) {
                    out.push_str(&make_value);
                    i += 1 + key.len();
                    matched = true;
                    break;
                }
            }
            if !matched {
                // literal $
                out.push('$');
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn named_substitutions<'a>(
    args: &'a str,
    cwd: &'a Path,
) -> [(&'static str, String); 3] {
    [
        ("ARGUMENTS", args.to_string()),
        ("CWD", cwd.to_string_lossy().into_owned()),
        ("DATE", date_iso8601()),
    ]
}

/// Retorna a data atual no formato ISO 8601 (YYYY-MM-DD).
/// Usa apenas a stdlib — sem dependência de chrono.
fn date_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Algoritmo de Fliegel & Van Flandern (adaptado para u64)
    let days = (secs / 86_400).cast_signed(); // dias desde 1970-01-01
    // Converter Julian Day Number para Y-M-D
    let jd = days + 2_440_588; // 2440588 = JDN de 1970-01-01
    let l = jd + 68_569;
    let n = (4 * l) / 146_097;
    let l = l - (146_097 * n + 3) / 4;
    let i = (4000 * (l + 1)) / 1_461_001;
    let l = l - (1461 * i) / 4 + 31;
    let j = (80 * l) / 2447;
    let day = l - (2447 * j) / 80;
    let l = j / 11;
    let month = j + 2 - 12 * l;
    let year = 100 * (n - 49) + i + l;
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("user-commands-{label}-{nanos}"))
    }

    #[test]
    fn parse_command_with_full_frontmatter() {
        let content = "---\ndescription: My command\nargument-hint: [file]\n---\nbody $ARGUMENTS";
        let path = std::path::Path::new("/fake/my-cmd.md");
        let cmd =
            parse_user_command("my-cmd", content, path, UserCommandScope::Project).unwrap();
        assert_eq!(cmd.name, "my-cmd");
        assert_eq!(cmd.description, "My command");
        assert_eq!(cmd.argument_hint.as_deref(), Some("[file]"));
        assert_eq!(cmd.body_template, "body $ARGUMENTS");
        assert_eq!(cmd.scope, UserCommandScope::Project);
    }

    #[test]
    fn parse_command_without_frontmatter() {
        let content = "just the body text";
        let path = std::path::Path::new("/fake/simple.md");
        let cmd =
            parse_user_command("simple", content, path, UserCommandScope::Global).unwrap();
        assert_eq!(cmd.description, "Custom command: simple");
        assert!(cmd.argument_hint.is_none());
        assert_eq!(cmd.body_template, "just the body text");
    }

    #[test]
    fn parse_command_with_partial_frontmatter() {
        let content = "---\ndescription: Partial\n---\nbody here";
        let path = std::path::Path::new("/fake/partial.md");
        let cmd =
            parse_user_command("partial", content, path, UserCommandScope::Project).unwrap();
        assert_eq!(cmd.description, "Partial");
        assert!(cmd.argument_hint.is_none());
        assert_eq!(cmd.body_template, "body here");
    }

    #[test]
    fn expand_template_replaces_arguments() {
        let cwd = std::path::Path::new("/some/dir");
        let result = expand_template("hi $ARGUMENTS", "world", cwd);
        assert_eq!(result, "hi world");
    }

    #[test]
    fn expand_template_replaces_positional() {
        let cwd = std::path::Path::new("/some/dir");
        let result = expand_template("first=$1 second=$2", "a b", cwd);
        assert_eq!(result, "first=a second=b");
    }

    #[test]
    fn expand_template_replaces_cwd() {
        let cwd = std::path::Path::new("/my/project");
        let result = expand_template("in $CWD", "", cwd);
        assert_eq!(result, "in /my/project");
    }

    #[test]
    fn expand_template_handles_no_match_gracefully() {
        let cwd = std::path::Path::new("/some/dir");
        let result = expand_template("$X", "", cwd);
        assert_eq!(result, "$X");
    }

    #[test]
    fn discover_loads_md_files_from_project_dir() {
        let root = temp_dir("discover-project");
        let cmd_dir = root.join(".elai").join("commands");
        fs::create_dir_all(&cmd_dir).unwrap();
        fs::write(
            cmd_dir.join("foo.md"),
            "---\ndescription: Foo command\n---\ndo foo $ARGUMENTS",
        )
        .unwrap();

        let reg = UserCommandRegistry::discover(&root).unwrap();
        let cmd = reg.get("foo").expect("foo command should be found");
        assert_eq!(cmd.name, "foo");
        assert_eq!(cmd.description, "Foo command");
        assert_eq!(cmd.scope, UserCommandScope::Project);
    }

    #[test]
    fn discover_project_overrides_global() {
        use std::env;

        let global_home = temp_dir("override-global");
        let global_cmd_dir = global_home.join(".elai").join("commands");
        fs::create_dir_all(&global_cmd_dir).unwrap();
        fs::write(
            global_cmd_dir.join("shared.md"),
            "---\ndescription: Global version\n---\nglobal body",
        )
        .unwrap();

        let project_root = temp_dir("override-project");
        let project_cmd_dir = project_root.join(".elai").join("commands");
        fs::create_dir_all(&project_cmd_dir).unwrap();
        fs::write(
            project_cmd_dir.join("shared.md"),
            "---\ndescription: Project version\n---\nproject body",
        )
        .unwrap();

        // Override HOME so global discovery points to our fake home
        let old_home = env::var_os("HOME");
        env::set_var("HOME", &global_home);

        let reg = UserCommandRegistry::discover(&project_root).unwrap();

        match &old_home {
            Some(v) => env::set_var("HOME", v),
            None => env::remove_var("HOME"),
        }

        let cmd = reg.get("shared").expect("shared command should exist");
        assert_eq!(cmd.scope, UserCommandScope::Project);
        assert_eq!(cmd.description, "Project version");
    }

    #[test]
    fn is_valid_name_rejects_uppercase_and_dashes_at_start() {
        assert!(is_valid_name("valid-name"));
        assert!(is_valid_name("foo123"));
        assert!(is_valid_name("foo_bar"));
        assert!(!is_valid_name("UpperCase"));
        assert!(!is_valid_name("-starts-with-dash"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("1starts-with-digit"));
    }

    #[test]
    fn date_iso8601_format_is_correct() {
        let date = date_iso8601();
        assert_eq!(date.len(), 10, "date should be YYYY-MM-DD");
        let parts: Vec<&str> = date.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].len(), 4);
        assert_eq!(parts[1].len(), 2);
        assert_eq!(parts[2].len(), 2);
    }
}
