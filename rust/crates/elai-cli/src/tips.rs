//! Dicas exibidas no TUI quando o chat ainda está vazio.
//!
//! O conteúdo é curado pelos desenvolvedores em [`assets/tips.toml`] e embutido
//! no binário em compile-time via [`include_str!`]. Para adicionar uma dica
//! nova, edite o TOML e recompile — não há override pelo usuário final.

use std::time::{SystemTime, UNIX_EPOCH};

const TIPS_TOML: &str = include_str!("../assets/tips.toml");

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Tip {
    pub title: String,
    pub body: String,
}

#[derive(Debug, serde::Deserialize)]
struct TipsFile {
    tip: Vec<Tip>,
}

/// Carrega as dicas embutidas. Falha em compile-equivalent se o TOML for
/// inválido — o erro só aparece em desenvolvimento, antes do release.
pub fn load_tips() -> Vec<Tip> {
    let parsed: TipsFile = toml::from_str(TIPS_TOML)
        .expect("tips.toml inválido — o ELAI binário não foi compilado corretamente");
    parsed
        .tip
        .into_iter()
        .map(|t| Tip {
            title: t.title.trim().to_string(),
            body: t.body.trim().to_string(),
        })
        .filter(|t| !t.title.is_empty() && !t.body.is_empty())
        .collect()
}

/// Permutação Fisher–Yates dos índices `0..n`. Determinística para `n == 0`
/// (vazio) e `n == 1` (um elemento). Para `n > 1`, embaralha com seed derivada
/// do relógio do sistema — diferente a cada execução do TUI.
pub fn shuffle_indices(n: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    if n < 2 {
        return order;
    }
    let mut state = seed();
    #[allow(clippy::cast_possible_truncation)]
    for i in (1..n).rev() {
        state = lcg_next(state);
        let j = (state as usize) % (i + 1);
        order.swap(i, j);
    }
    order
}

#[allow(clippy::cast_possible_truncation)]
fn seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0xDEAD_BEEF_CAFE_BABE, |d| {
            (d.as_nanos() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        })
        | 1
}

/// LCG simples (Numerical Recipes). Suficiente para escolher a ordem das
/// dicas — não é cripto.
fn lcg_next(state: u64) -> u64 {
    state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_tips_returns_non_empty_entries() {
        let tips = load_tips();
        assert!(!tips.is_empty(), "esperado pelo menos uma dica");
        for t in &tips {
            assert!(!t.title.is_empty(), "title vazio em alguma dica");
            assert!(!t.body.is_empty(), "body vazio em alguma dica");
        }
    }

    #[test]
    fn shuffle_indices_is_a_permutation() {
        for n in [0usize, 1, 2, 5, 15] {
            let order = shuffle_indices(n);
            assert_eq!(order.len(), n);
            let mut sorted = order.clone();
            sorted.sort_unstable();
            let expected: Vec<usize> = (0..n).collect();
            assert_eq!(sorted, expected, "shuffle não é permutação para n={n}");
        }
    }
}
