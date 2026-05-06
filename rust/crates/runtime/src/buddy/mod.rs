//! Companion (Buddy) system — deterministic mascot generated from user identity.
//!
//! Each user gets a unique companion whose appearance is derived from a hash of their
//! user ID (deterministic) and whose name/personality is generated once by the LLM
//! (stored in `~/.elai/companion.json`).
//!
//! # Coleção e Sistema de Unlock
//!
//! Este sistema também inclui:
//! - **Coleção**: Sistema de persistência de mascotes desbloqueados
//! - **Unlock Events**: Eventos de desbloqueio baseados em milestones de tokens
//! - **Orchestrator**: Coordena o fluxo completo de unlock
//! - **`UnlockIntegration`**: Integração com o runtime principal
//!
//! ## Fluxo de Unlock
//!
//! ```ignore
//! 1. UnlockIntegration.start_task() — inicia tracking de tarefa
//! 2. UnlockIntegration.process_usage() — processa usage da API
//! 3. Ao atingir milestone → PAUSA a tarefa
//! 4. Modelo avalia complexidade → determina raridade
//! 5. Mascote sorteado da raridade
//! 6. Tela de EVENTO DE DESBLOQUEIO exibida
//! 7. Usuário confirma ou recusa
//! 8. Task RESUMIDA
//! ```
//!
//! # Quick start (integração com runtime)
//! ```
//! // UnlockIntegration is available through the buddy module
//! use runtime::buddy::UnlockIntegration;
//!
//! let integration = UnlockIntegration::new();
//! // In real usage, you would call add_tokens() after API calls
//! // The system tracks tokens and triggers unlock events at milestones
//! ```

pub mod collection;
pub mod generator;
pub mod hatch;
pub mod orchestrator;
pub mod sprites;
pub mod types;
pub mod unlock_event;
pub mod unlock_integration;

pub use collection::{
    count_by_rarity, load_or_create_collection, save_collection, rarity_for_pokemon,
    unlock_counts_by_rarity, CollectionEntry, UnlockStatus, UnlockThreshold, UserCollection,
    UNLOCK_THRESHOLDS,
};
pub use generator::{roll_bones, roll_bones_for};
pub use hatch::{
    load_or_hatch, load_stored_companion, save_pokemon_choice, save_stored_companion,
    update_pokemon_id,
};
pub use orchestrator::{UnlockOutcome, UnlockOrchestrator};
pub use sprites::{render_sprite, sprite_for_id};
pub use types::{
    pokemon_name, Companion, CompanionBones, CompanionSoul, Hat, PokemonId, Rarity, StatName,
    StoredCompanion, ALL_EYES, ALL_HATS, ALL_RARITIES, ALL_STAT_NAMES, POKEMON_COUNT,
    POKEMON_NAMES, RARITY_STARS, RARITY_WEIGHTS,
};
pub use unlock_event::{
    ComplexityEvaluation, MilestoneConfig, MilestoneReached, TaskContext, TaskTracker, TaskType,
    UnlockEvent, MILESTONE_CONFIGS, evaluate_task_complexity,
};
pub use unlock_integration::{UnlockIntegration, WithUnlock};

/// Renders a compact TUI header line for the companion. The sprite is the
/// embedded ANSI Pokémon art; the summary line names it.
#[must_use]
pub fn render_companion_header(companion: &Companion) -> String {
    let sprite = render_sprite(companion.pokemon_id, companion.shiny);
    let summary = companion.summary_line();
    format!("{sprite}\n{summary}")
}
