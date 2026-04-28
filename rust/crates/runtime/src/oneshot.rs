//! One-shot LLM helpers (non-interactive).
//!
//! Used to generate an `ELAI.md` grounded in project facts extracted by static
//! analysis, without maintaining a conversation session.
//!
//! The `runtime` crate does not call the LLM directly — that lives in `api`.
//! Instead, callers inject a `sender` closure that performs the actual request.
//! This keeps the module free of circular dependencies and easy to test.

// ─── OneshotError ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum OneshotError {
    NoAuth(String),
    Api(String),
    Timeout,
}

impl std::fmt::Display for OneshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAuth(m) => write!(f, "no auth available: {m}"),
            Self::Api(m) => write!(f, "api error: {m}"),
            Self::Timeout => write!(f, "timeout"),
        }
    }
}

impl std::error::Error for OneshotError {}

// ─── generate_elai_md_with ────────────────────────────────────────────────────

/// Generate `ELAI.md` content by calling `sender(model, prompt)`.
///
/// `sender` receives the model name and the full prompt string and must return
/// either the LLM response text or an error message.  This design keeps
/// `runtime` free of a direct dependency on the `api` crate.
///
/// # Errors
/// Returns [`OneshotError::Api`] if `sender` returns an error.
pub fn generate_elai_md_with<F>(
    facts_json: &str,
    model: &str,
    sender: F,
) -> Result<String, OneshotError>
where
    F: FnOnce(&str, &str) -> Result<String, String>,
{
    let prompt = build_elai_md_prompt(facts_json);
    sender(model, &prompt).map_err(OneshotError::Api)
}

// ─── build_elai_md_prompt ─────────────────────────────────────────────────────

/// Build the grounded prompt that instructs the LLM to produce `ELAI.md`.
#[must_use]
pub fn build_elai_md_prompt(facts_json: &str) -> String {
    format!(
        r#"Você é um assistente que documenta repositórios.

A partir dos FATOS estruturais abaixo (extraídos por análise estática), gere um arquivo ELAI.md em Markdown com as seções:

1. **Visão geral** — 2-3 linhas resumindo o que o projeto faz (deduzir de README, frameworks, símbolos).
2. **Estrutura** — diretórios principais e o que cada um contém.
3. **Convenções** — linguagens dominantes, frameworks, padrões observados nos top símbolos.
4. **Comandos** — comandos típicos para o stack detectado (build/test/lint/run).
5. **Riscos** — apontamentos óbvios (ex: "código sem testes em X", "muitas TODO em Y") se evidentes nos fatos.

Restrições:
- Use APENAS informação que esteja nos fatos. Não invente arquivos ou funções.
- Saída deve ser SOMENTE o conteúdo do ELAI.md, sem cercas de código nem prefácio.
- Em português técnico.

FATOS (JSON):
{facts_json}
"#
    )
}

// ─── render_static_elai_md ────────────────────────────────────────────────────

/// Render a static `ELAI.md` from `facts_json` without calling the LLM.
///
/// Used as a fallback when the LLM is unavailable (offline mode, missing auth).
/// Parses `facts_json` leniently — invalid JSON produces a minimal but valid
/// document instead of panicking.
#[must_use]
pub fn render_static_elai_md(facts_json: &str) -> String {
    use std::fmt::Write as _;

    let v: serde_json::Value =
        serde_json::from_str(facts_json).unwrap_or(serde_json::Value::Null);

    let mut out = String::from("# ELAI.md\n\n");
    out.push_str(
        "> Gerado automaticamente pelo `elai init` (modo offline — LLM indisponível).\n\n",
    );

    // Vision / overview from README excerpt
    out.push_str("## Visão geral\n\n");
    if let Some(readme) = v.get("readme_excerpt").and_then(|x| x.as_str()) {
        let excerpt = readme.lines().take(8).collect::<Vec<_>>().join("\n");
        out.push_str(&excerpt);
        out.push_str("\n\n");
    } else {
        out.push_str("Repositório sem README detectado. Adicione contexto manualmente.\n\n");
    }

    // Directory structure
    out.push_str("## Estrutura\n\n");
    if let Some(dirs) = v.get("dirs_summary").and_then(|x| x.as_array()) {
        for d in dirs.iter().take(15) {
            if let (Some(dir), Some(files)) = (
                d.get("dir").and_then(serde_json::Value::as_str),
                d.get("files").and_then(serde_json::Value::as_u64),
            ) {
                let _ = writeln!(out, "- `{dir}` ({files} arquivos)");
            }
        }
    }
    out.push('\n');

    // Conventions
    out.push_str("## Convenções\n\n");
    if let Some(by_lang) = v.get("by_lang").and_then(|x| x.as_object()) {
        let mut entries: Vec<(&str, u64)> = by_lang
            .iter()
            .filter_map(|(k, v)| v.as_u64().map(|n| (k.as_str(), n)))
            .collect();
        entries.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        for (lang, n) in entries.iter().take(8) {
            let _ = writeln!(out, "- {lang}: {n} arquivos");
        }
    }
    if let Some(fwks) = v.get("frameworks").and_then(|x| x.as_array()) {
        if !fwks.is_empty() {
            out.push_str("\n**Frameworks detectados:** ");
            let names: Vec<&str> = fwks.iter().filter_map(serde_json::Value::as_str).collect();
            out.push_str(&names.join(", "));
            out.push('\n');
        }
    }

    // Commands placeholder
    out.push_str("\n## Comandos\n\n_Adicione comandos típicos do stack._\n\n");

    // Top symbols
    out.push_str("## Top símbolos (extração estática)\n\n");
    if let Some(syms) = v.get("top_symbols").and_then(|x| x.as_array()) {
        for s in syms.iter().take(20) {
            if let (Some(sym), Some(path), Some(line)) = (
                s.get("symbol").and_then(serde_json::Value::as_str),
                s.get("rel_path").and_then(serde_json::Value::as_str),
                s.get("line_start").and_then(serde_json::Value::as_u64),
            ) {
                let _ = writeln!(out, "- `{sym}` — `{path}:{line}`");
            }
        }
    }

    out
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_elai_md_prompt_includes_facts_json() {
        let facts = r#"{"total_files":3}"#;
        let prompt = build_elai_md_prompt(facts);
        assert!(
            prompt.contains(facts),
            "prompt must embed the facts JSON verbatim"
        );
        assert!(
            prompt.contains("ELAI.md"),
            "prompt must mention ELAI.md"
        );
    }

    #[test]
    fn render_static_elai_md_includes_dirs_section() {
        let facts = serde_json::json!({
            "total_files": 5,
            "by_lang": {"rust": 4, "toml": 1},
            "frameworks": ["rust-cargo"],
            "dirs_summary": [
                {"dir": "src", "files": 4},
                {"dir": ".", "files": 1}
            ],
            "top_symbols": [],
            "readme_excerpt": null
        })
        .to_string();

        let md = render_static_elai_md(&facts);
        assert!(md.contains("## Estrutura"), "must have Estrutura section");
        assert!(md.contains("`src`"), "must list src dir");
        assert!(md.contains("## Convenções"), "must have Convenções section");
        assert!(md.contains("rust-cargo"), "must list detected framework");
    }

    #[test]
    fn render_static_elai_md_handles_invalid_json() {
        // Must not panic on malformed input
        let md = render_static_elai_md("{not json");
        assert!(
            md.contains("# ELAI.md"),
            "should still produce a valid heading"
        );
        assert!(
            md.contains("Adicione contexto manualmente"),
            "should fall back to no-readme message"
        );
    }
}
