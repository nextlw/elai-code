//! Pure-string progress bar (mythos-router pattern). Sem ANSI control codes,
//! sem `\r` — apenas Unicode estático para ser renderizado como qualquer
//! linha de chat/log dentro do TUI.

/// Retorna `[██████████░░░░░░░░░░] 50%` ou similar.
/// `pct` é clamped para [0.0, 100.0]. `width` é a largura visual em chars.
#[must_use]
#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
pub fn progress_bar(pct: f32, width: usize) -> String {
    let pct = pct.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!(
        "[{}{}] {}%",
        "█".repeat(filled),
        "░".repeat(empty),
        pct.round() as u32
    )
}

/// Variante com label e contadores: `Indexing [████░░░░░░] 23/100 (23%)`.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn progress_bar_labeled(label: &str, current: usize, total: usize, width: usize) -> String {
    let pct = if total == 0 {
        0.0_f32
    } else {
        (current as f32 / total as f32) * 100.0
    };
    let bar = progress_bar(pct, width);
    format!("{label} {bar} {current}/{total}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bar_for_zero_percent() {
        let bar = progress_bar(0.0, 10);
        assert!(bar.contains("░░░░░░░░░░"));
        assert!(bar.contains("0%"));
    }

    #[test]
    fn full_bar_for_hundred_percent() {
        let bar = progress_bar(100.0, 10);
        assert!(bar.contains("██████████"));
        assert!(bar.contains("100%"));
    }

    #[test]
    fn half_bar_for_fifty_percent() {
        let bar = progress_bar(50.0, 10);
        assert!(bar.contains("█████"));
        assert!(bar.contains("░░░░░"));
    }

    #[test]
    fn clamps_negative_to_zero() {
        let bar = progress_bar(-50.0, 10);
        assert!(bar.contains("0%"));
    }

    #[test]
    fn clamps_over_to_hundred() {
        let bar = progress_bar(150.0, 10);
        assert!(bar.contains("100%"));
    }

    #[test]
    fn labeled_format() {
        let bar = progress_bar_labeled("Indexing", 30, 100, 10);
        assert!(bar.starts_with("Indexing"));
        assert!(bar.contains("30/100"));
    }
}
