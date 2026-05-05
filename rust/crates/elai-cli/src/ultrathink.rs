//! Detecção da palavra-chave **ultrathink** nas mensagens de usuário (TUI / headless).

/// `true` se `message` contém a substring **`ultrathink`** ignorando casing ASCII
/// (`UltraThink`, `ULTRATHINK`, etc.).
#[must_use]
pub(crate) fn message_contains_ultrathink_keyword(message: &str) -> bool {
    message.to_ascii_lowercase().contains("ultrathink")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mixed_case() {
        assert!(message_contains_ultrathink_keyword("Use Ultrathink here"));
        assert!(message_contains_ultrathink_keyword("ULTRATHINK"));
        assert!(message_contains_ultrathink_keyword("superultrathink")); // substring, matches legacy behaviour
    }

    #[test]
    fn no_false_empty() {
        assert!(!message_contains_ultrathink_keyword(""));
        assert!(!message_contains_ultrathink_keyword("thinking only"));
    }
}
