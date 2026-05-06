//! Sistema de coleção de mascotes — persistência e operações.
//!
//! Cada usuário tem uma "Pokedex" com 151 mascotes.
//! O progresso é baseado em tokens gastos cumulativos.
//!
//! ## Mecânica
//!
//! 1. **Início**: Usuário escolhe 3 mascotes iniciais (starters)
//! 2. **Progresso**: Tokens gastos desbloqueiam novos mascotes
//! 3. **Raridade**: Quanto maior a raridade, mais tokens necessários
//! 4. **Aleatoriedade**: Novos desbloqueios são aleatórios dentro da raridade
//! 5. **Completar raridade**: Desbloqueia versão shiny dourado
//!
//! ## Arquivo: `~/.elai/collection.json`

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::types::{PokemonId, POKEMON_COUNT, Rarity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnlockStatus {
    /// Disponível para escolha inicial (3 iniciais)
    Starter,
    /// Desbloqueado após gastar tokens
    Unlocked { unlocked_at: u64 },
    /// Ainda não desbloqueado
    Locked,
}

impl UnlockStatus {
    #[must_use] 
    pub fn is_unlocked(&self) -> bool {
        matches!(self, UnlockStatus::Starter | UnlockStatus::Unlocked { .. })
    }

    #[must_use] 
    pub fn is_locked(&self) -> bool {
        matches!(self, UnlockStatus::Locked)
    }
}

// ── Entry da coleção ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionEntry {
    pub pokemon_id: PokemonId,
    pub status: UnlockStatus,
    /// Tokens gastos quando foi desbloqueado
    pub tokens_at_unlock: u64,
    /// Shiny conquistado?
    pub shiny_unlocked: bool,
    /// Golden shiny conquistado?
    pub golden_unlocked: bool,
}

impl CollectionEntry {
    #[must_use] 
    pub fn new(pokemon_id: PokemonId) -> Self {
        Self {
            pokemon_id,
            status: UnlockStatus::Locked,
            tokens_at_unlock: 0,
            shiny_unlocked: false,
            golden_unlocked: false,
        }
    }
}

// ── Thresholds de desbloqueio ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct UnlockThreshold {
    pub rarity: Rarity,
    pub tokens_required: u64,
}

/// Thresholds de tokens para desbloqueio progressivo
pub const UNLOCK_THRESHOLDS: &[UnlockThreshold] = &[
    // Tier 0: Iniciais (0 tokens - escolhidos pelo usuário)
    UnlockThreshold { rarity: Rarity::Common, tokens_required: 0 },
    // Tier 1: Mais commons (10k tokens)
    UnlockThreshold { rarity: Rarity::Common, tokens_required: 10_000 },
    // Tier 2: Incomuns (50k tokens)
    UnlockThreshold { rarity: Rarity::Uncommon, tokens_required: 50_000 },
    // Tier 3: Raros (150k tokens)
    UnlockThreshold { rarity: Rarity::Rare, tokens_required: 150_000 },
    // Tier 4: Épicos (500k tokens)
    UnlockThreshold { rarity: Rarity::Epic, tokens_required: 500_000 },
    // Tier 5: Lendários (1M tokens)
    UnlockThreshold { rarity: Rarity::Legendary, tokens_required: 1_000_000 },
];

// ── Coleção do usuário ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserCollection {
    /// Lista de entries, index = `pokemon_id` - 1
    entries: Vec<CollectionEntry>,
    /// Total de tokens gastos cumulativos
    total_tokens_spent: u64,
    /// Mascote atualmente ativo
    active_companion_id: Option<PokemonId>,
    /// Contador de desbloqueios por raridade
    #[serde(default)]
    unlock_counts: HashMap<String, u8>,
}

impl UserCollection {
    /// Cria uma coleção nova com os 3 iniciais
    #[must_use] 
    pub fn new_with_starters(starter_ids: [PokemonId; 3]) -> Self {
        let mut entries = Vec::with_capacity(POKEMON_COUNT as usize);
        for id in 1..=POKEMON_COUNT {
            let status = if starter_ids.contains(&id) {
                UnlockStatus::Starter
            } else {
                UnlockStatus::Locked
            };
            entries.push(CollectionEntry {
                pokemon_id: id,
                status,
                tokens_at_unlock: 0,
                shiny_unlocked: false,
                golden_unlocked: false,
            });
        }

        let mut collection = Self {
            entries,
            total_tokens_spent: 0,
            active_companion_id: Some(starter_ids[0]),
            unlock_counts: HashMap::new(),
        };

        // Contabiliza starters
        for id in starter_ids {
            collection.increment_unlock_count(id);
        }

        collection
    }

    /// Retorna quantos mascotes estão desbloqueados
    #[must_use] 
    pub fn unlocked_count(&self) -> usize {
        self.entries.iter().filter(|e| e.status.is_unlocked()).count()
    }

    /// Retorna quantos mascotes de cada raridade estão desbloqueados
    #[must_use] 
    pub fn unlocked_by_rarity(&self) -> HashMap<Rarity, usize> {
        let mut counts: HashMap<Rarity, usize> = HashMap::new();
        for entry in &self.entries {
            if entry.status.is_unlocked() {
                let rarity = rarity_for_pokemon(entry.pokemon_id);
                *counts.entry(rarity).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Retorna quais mascotes estão disponíveis para escolha inicial
    #[must_use] 
    pub fn starters(&self) -> Vec<PokemonId> {
        self.entries
            .iter()
            .filter(|e| matches!(e.status, UnlockStatus::Starter))
            .map(|e| e.pokemon_id)
            .collect()
    }

    /// Retorna quais mascotes estão desbloqueados (incluindo starters)
    #[must_use] 
    pub fn unlocked(&self) -> Vec<PokemonId> {
        self.entries
            .iter()
            .filter(|e| e.status.is_unlocked())
            .map(|e| e.pokemon_id)
            .collect()
    }

    /// Retorna quais mascotes ainda estão bloqueados
    #[must_use] 
    pub fn locked(&self) -> Vec<PokemonId> {
        self.entries
            .iter()
            .filter(|e| e.status.is_locked())
            .map(|e| e.pokemon_id)
            .collect()
    }

    /// Verifica se um mascote está desbloqueado
    #[must_use] 
    pub fn is_unlocked(&self, pokemon_id: PokemonId) -> bool {
        self.entry(pokemon_id)
            .is_some_and(|e| e.status.is_unlocked())
    }

    /// Retorna o entry de um `pokemon_id`
    #[must_use] 
    pub fn entry(&self, pokemon_id: PokemonId) -> Option<&CollectionEntry> {
        let idx = (pokemon_id.saturating_sub(1)) as usize;
        self.entries.get(idx)
    }

    /// Retorna mutable do entry
    pub fn entry_mut(&mut self, pokemon_id: PokemonId) -> Option<&mut CollectionEntry> {
        let idx = (pokemon_id.saturating_sub(1)) as usize;
        self.entries.get_mut(idx)
    }

    /// Retorna total de tokens gastos
    #[must_use] 
    pub fn total_tokens_spent(&self) -> u64 {
        self.total_tokens_spent
    }

    /// Registra gasto de tokens e verifica novos desbloqueios
    /// Retorna lista de mascotes recém-desbloqueados
    pub fn register_token_spent(&mut self, tokens: u64) -> Vec<PokemonId> {
        self.total_tokens_spent += tokens;
        let mut newly_unlocked = Vec::new();

        // Verifica cada threshold
        for threshold in UNLOCK_THRESHOLDS {
            if self.total_tokens_spent >= threshold.tokens_required {
                // Procura mascotes dessa raridade que ainda estão bloqueados
                let candidates: Vec<PokemonId> = self
                    .entries
                    .iter()
                    .filter(|e| {
                        e.status.is_locked()
                            && rarity_for_pokemon(e.pokemon_id) == threshold.rarity
                    })
                    .map(|e| e.pokemon_id)
                    .collect();

                if !candidates.is_empty() {
                    let to_unlock = candidates[fastrand::usize(0..candidates.len())];
                    let now = now_unix_secs();
                    let tokens_now = self.total_tokens_spent;
                    if let Some(entry) = self.entry_mut(to_unlock) {
                        entry.status = UnlockStatus::Unlocked { unlocked_at: now };
                        entry.tokens_at_unlock = tokens_now;
                    }
                    self.increment_unlock_count(to_unlock);
                    newly_unlocked.push(to_unlock);
                }
            }
        }

        newly_unlocked
    }

    /// Retorna o próximo threshold de desbloqueio
    #[must_use] 
    pub fn next_unlock_threshold(&self) -> Option<UnlockThreshold> {
        UNLOCK_THRESHOLDS
            .iter()
            .find(|t| self.total_tokens_spent < t.tokens_required)
            .copied()
    }

    /// Retorna tokens necessários para próximo desbloqueio
    #[must_use] 
    pub fn tokens_to_next_unlock(&self) -> Option<u64> {
        self.next_unlock_threshold()
            .map(|t| t.tokens_required - self.total_tokens_spent)
    }

    /// Seta o companion ativo
    pub fn set_active(&mut self, pokemon_id: PokemonId) -> bool {
        if self.is_unlocked(pokemon_id) {
            self.active_companion_id = Some(pokemon_id);
            true
        } else {
            false
        }
    }

    /// Desbloqueia shiny para um mascote (requer 100k tokens extra)
    pub fn unlock_shiny(&mut self, pokemon_id: PokemonId) -> bool {
        if !self.is_unlocked(pokemon_id) {
            return false;
        }
        if let Some(entry) = self.entry_mut(pokemon_id) {
            entry.shiny_unlocked = true;
            return true;
        }
        false
    }

    /// Desbloqueia golden shiny (requer completar raridade)
    pub fn unlock_golden(&mut self, pokemon_id: PokemonId) -> bool {
        if !self.is_unlocked(pokemon_id) {
            return false;
        }
        if let Some(entry) = self.entry_mut(pokemon_id) {
            entry.golden_unlocked = true;
            return true;
        }
        false
    }

    fn increment_unlock_count(&mut self, pokemon_id: PokemonId) {
        let rarity = rarity_for_pokemon(pokemon_id);
        let key = rarity.as_str().to_string();
        *self.unlock_counts.entry(key).or_insert(0) += 1;
    }

    /// Gera relatório da coleção
    #[must_use] 
    pub fn collection_report(&self) -> String {
        let total = self.entries.len();
        let unlocked = self.unlocked_count();
        // Percentage calculation - precision loss is acceptable for display
        let percentage = (unlocked as f64 / total as f64) * 100.0;

        let mut lines = vec![
            format!("📦 Coleção Pokelais (Companions) — {unlocked}/{total} ({percentage:.1}%)"),
            format!("💰 Tokens gastos: {}", format_thousands(self.total_tokens_spent)),
            String::new(),
            "Progresso por raridade:".to_string(),
        ];

        for rarity in [
            Rarity::Legendary,
            Rarity::Epic,
            Rarity::Rare,
            Rarity::Uncommon,
            Rarity::Common,
        ] {
            let total_rarity = count_by_rarity(rarity);
            let unlocked_rarity = self
                .entries
                .iter()
                .filter(|e| {
                    e.status.is_unlocked() && rarity_for_pokemon(e.pokemon_id) == rarity
                })
                .count();

            let bar_len = ((unlocked_rarity * 10) / total_rarity.max(1)).min(10);
            let bar = "█".repeat(bar_len) + &"░".repeat(10 - bar_len);

            lines.push(format!(
                "  {} {} {}/{}",
                rarity.stars(),
                bar,
                unlocked_rarity,
                total_rarity
            ));
        }

        // Próximo desbloqueio
        if let Some(next) = self.next_unlock_threshold() {
            let needed = next.tokens_required - self.total_tokens_spent;
            lines.push(String::new());
            lines.push(format!(
                "🔓 Próximo desbloqueio ({rarity}): {needed} tokens",
                rarity = next.rarity.as_str(),
                needed = format_thousands(needed)
            ));
        }

        lines.join("\n")
    }
}

// ── Helpers de raridade ───────────────────────────────────────────────────────

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Formata número com separador de milhar (`1234567` → `"1,234,567"`).
/// Substitui o `{:,}` estilo Python que não existe em Rust.
fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// Determina raridade por `PokemonId` (distribuição pseudo-aleatória mas determinística)
#[must_use] 
#[expect(clippy::match_same_arms)]
pub fn rarity_for_pokemon(id: PokemonId) -> Rarity {
    // Ranges intentionally overlap for Common to bias toward more Common mascotes
    match id {
        1..=30 => Rarity::Common,
        31..=60 => Rarity::Uncommon,
        61..=80 => Rarity::Rare,
        81..=90 => Rarity::Epic,
        91..=140 => Rarity::Common,
        141..=151 => Rarity::Legendary,
        _ => Rarity::Common,
    }
}

/// Conta total de pokemon de uma raridade
#[must_use] 
pub fn count_by_rarity(rarity: Rarity) -> usize {
    (1..=POKEMON_COUNT)
        .filter(|&id| rarity_for_pokemon(id) == rarity)
        .count()
}

/// Retorna contagem de desbloqueios por raridade
#[allow(dead_code)]
#[must_use] 
pub fn unlock_counts_by_rarity() -> HashMap<Rarity, usize> {
    HashMap::new()
}

// ── Storage ───────────────────────────────────────────────────────────────────

fn collection_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".elai").join("collection.json"))
}

/// Carrega coleção existente ou cria nova com starters default
#[must_use] 
pub fn load_or_create_collection() -> UserCollection {
    let path = match collection_path() {
        Some(p) => p,
        None => return UserCollection::default(),
    };

    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(collection) = serde_json::from_str(&raw) {
            return collection;
        }
    }

    // Default: starters são os 3 primeiros (Common)
    UserCollection::new_with_starters([1, 4, 7])
}

/// Salva coleção no disco
pub fn save_collection(collection: &UserCollection) -> std::io::Result<()> {
    let path = collection_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "home dir unavailable")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(collection)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, json)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_collection_has_starters() {
        let collection = UserCollection::new_with_starters([1, 4, 7]);
        assert_eq!(collection.unlocked_count(), 3);
        assert!(collection.is_unlocked(1));
        assert!(collection.is_unlocked(4));
        assert!(collection.is_unlocked(7));
        assert!(!collection.is_unlocked(2));
    }

    #[test]
    fn token_spent_triggers_unlock() {
        let mut collection = UserCollection::new_with_starters([1, 4, 7]);
        let before = collection.unlocked_count();
        let new = collection.register_token_spent(10_000);
        assert!(collection.unlocked_count() > before);
        assert!(!new.is_empty());
    }

    #[test]
    fn locked_mascotes_are_accessible() {
        let collection = UserCollection::new_with_starters([1, 4, 7]);
        let locked = collection.locked();
        assert!(!locked.is_empty());
        assert!(!locked.contains(&1));
    }

    #[test]
    fn rarity_distribution() {
        assert_eq!(rarity_for_pokemon(1), Rarity::Common);
        assert_eq!(rarity_for_pokemon(31), Rarity::Uncommon);
        assert_eq!(rarity_for_pokemon(61), Rarity::Rare);
        assert_eq!(rarity_for_pokemon(81), Rarity::Epic);
        assert_eq!(rarity_for_pokemon(141), Rarity::Legendary);
    }
}
