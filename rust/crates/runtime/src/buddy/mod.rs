//! Companion (Buddy) system — deterministic mascot generated from user identity.
//!
//! Each user gets a unique companion whose appearance is derived from a hash of their
//! user ID (deterministic) and whose name/personality is generated once by the LLM
//! (stored in `~/.elai/companion.json`).
//!
//! # Quick start
//! ```no_run
//! use runtime::buddy::{load_or_hatch, render_companion_header};
//!
//! let companion = load_or_hatch("my-user-id", |prompt| {
//!     // Call your LLM here and return the raw response text.
//!     Ok::<_, String>("{ \"name\": \"Capri\", \"personality\": \"Curious debugger.\" }".to_string())
//! }).unwrap();
//!
//! println!("{}", render_companion_header(&companion));
//! ```

pub mod generator;
pub mod hatch;
pub mod sprites;
pub mod types;

pub use generator::{roll_bones, roll_bones_for};
pub use hatch::{
    load_or_hatch, load_stored_companion, save_pokemon_choice, save_stored_companion,
    update_pokemon_id,
};
pub use sprites::{render_sprite, sprite_for_id};
pub use types::{
    pokemon_name, Companion, CompanionBones, CompanionSoul, Hat, PokemonId, Rarity, StatName,
    StoredCompanion, ALL_EYES, ALL_HATS, ALL_RARITIES, ALL_STAT_NAMES, POKEMON_COUNT,
    POKEMON_NAMES, RARITY_STARS, RARITY_WEIGHTS,
};

/// Renders a compact TUI header line for the companion. The sprite is the
/// embedded ANSI Pokémon art; the summary line names it.
#[must_use]
pub fn render_companion_header(companion: &Companion) -> String {
    let sprite = render_sprite(companion.pokemon_id, companion.shiny);
    let summary = companion.summary_line();
    format!("{sprite}\n{summary}")
}
