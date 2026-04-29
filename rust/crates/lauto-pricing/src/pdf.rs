//! Extração de texto e contagem de páginas via `pdftotext`/`pdfinfo`.
//!
//! Mantemos a mesma dependência externa que o pipeline de discovery
//! original. Em produção, este módulo é substituído por Jina Reader.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

/// Texto extraído + número de páginas estimado.
#[derive(Debug, Clone)]
pub struct PdfContent {
    pub text: String,
    pub pages: u32,
}

/// Lê o PDF combinando `pdftotext` (com fallback sem `-layout`) e `pdfinfo`.
///
/// Retorna `Ok` mesmo quando o PDF não tem camada de texto — `text` fica
/// vazio nesse caso, sem panic. O caller decide o que fazer.
pub fn read_pdf(path: &Path) -> Result<PdfContent> {
    if !path.exists() {
        anyhow::bail!("arquivo não encontrado: {}", path.display());
    }

    let pages = count_pages(path).unwrap_or(1);
    let text = run_pdftotext(path, true)
        .or_else(|_| run_pdftotext(path, false))
        .unwrap_or_default();

    Ok(PdfContent { text, pages })
}

fn run_pdftotext(path: &Path, layout: bool) -> Result<String> {
    let mut cmd = Command::new("pdftotext");
    if layout {
        cmd.arg("-layout");
    }
    cmd.arg(path).arg("-");

    let output = cmd
        .output()
        .with_context(|| "falha ao executar pdftotext (instale poppler)")?;

    if !output.status.success() {
        anyhow::bail!(
            "pdftotext retornou status {} para {}",
            output.status,
            path.display()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[allow(clippy::cast_lossless)] // contagem positiva pequena, sem perda real
fn count_pages(path: &Path) -> Result<u32> {
    let output = Command::new("pdfinfo")
        .arg(path)
        .output()
        .context("falha ao executar pdfinfo (instale poppler)")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Pages:") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                return Ok(n.max(1));
            }
        }
    }
    Ok(1)
}

/// Conta tokens-palavra (Unicode-friendly) sem alocar `Vec` intermediário.
#[must_use]
pub fn count_words(text: &str) -> usize {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .count()
}

/// Estima tokens de input usando o fator empírico para PT-BR (1.35 tok/palavra).
#[must_use]
pub fn estimate_input_tokens(words: usize) -> u64 {
    (words as f64 * 1.35).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_words_basic() {
        assert_eq!(count_words("Olá mundo, isto é Rust!"), 5);
    }

    #[test]
    fn count_words_unicode() {
        assert_eq!(count_words("inflamável combustível"), 2);
    }

    #[test]
    fn count_words_empty() {
        assert_eq!(count_words(""), 0);
    }

    #[test]
    fn tokens_match_python_baseline() {
        // 4318 palavras → 5829 tokens (mesmo valor do simulador Python original).
        assert_eq!(estimate_input_tokens(4318), 5829);
    }
}
