//! Classificador de tier (Light / Standard / Heavy).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Tier de complexidade — escolhe o roteamento de modelos no pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Light,
    Standard,
    Heavy,
}

impl Tier {
    /// Nome canônico em minúsculas (para JSON e logs).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Standard => "standard",
            Self::Heavy => "heavy",
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classifica via heurística: páginas + nº de cargas especiais + sinais de
/// complexidade contratual. Mantém os mesmos thresholds do protótipo Python
/// — qualquer mudança aqui afeta o pricing publicado em §9.2.
#[must_use]
pub fn classify(pages: u32, specials: usize, complexity_hits: u32) -> Tier {
    if pages <= 10 && specials == 0 && complexity_hits < 8 {
        Tier::Light
    } else if pages <= 40 && specials <= 1 && complexity_hits < 25 {
        Tier::Standard
    } else {
        Tier::Heavy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_doc_no_specials_is_light() {
        assert_eq!(classify(3, 0, 1), Tier::Light);
    }

    #[test]
    fn medium_with_one_special_is_standard() {
        assert_eq!(classify(25, 1, 18), Tier::Standard);
    }

    #[test]
    fn many_pages_is_heavy() {
        assert_eq!(classify(70, 3, 45), Tier::Heavy);
    }

    #[test]
    fn high_complexity_promotes_to_heavy() {
        // 5 págs, 2 cargas, mas 30 sinais → Heavy.
        assert_eq!(classify(5, 2, 30), Tier::Heavy);
    }

    #[test]
    fn matches_python_baseline_etp_marinha() {
        // Caso real ETP Marinha PE 90008/2025: 23 págs, 5 cargas, 51 hits → HEAVY.
        assert_eq!(classify(23, 5, 51), Tier::Heavy);
    }
}
