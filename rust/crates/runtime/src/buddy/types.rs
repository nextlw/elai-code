//! Companion types — Pokémon mascot, rarity, stats, and the Companion struct.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Pokémon mascot ────────────────────────────────────────────────────────────

/// Pokédex number, 1..=`POKEMON_COUNT`.
pub type PokemonId = u16;

/// Total number of available Pokémon sprites (Gen 1).
pub const POKEMON_COUNT: u16 = 151;

/// Original mascot names — `MASCOT_NAMES[id - 1]` for ids 1..=151. These are
/// fantasy names invented for this project; they intentionally do not reuse
/// any real franchise names. Order follows the sprite numbering.
pub const POKEMON_NAMES: [&str; POKEMON_COUNT as usize] = [
    "Sprigleaf", "Vineroot", "Florajaw", "Emberlit", "Cinderclaw", "Pyrowing",
    "Aquapup", "Tidefin", "Hydrocast", "Cribble", "Husklet", "Lepidor",
    "Stingnit", "Cocara", "Beestrike", "Twigwing", "Skybeak", "Galetail",
    "Whiskit", "Gnawler", "Beaklet", "Talonbird", "Hissten", "Coilfang",
    "Sparkmouse", "Voltrok", "Dunelet", "Sandshield", "Spikette", "Spinora",
    "Spinqueen", "Spikobu", "Spinoran", "Spinking", "Moonlet", "Moonbel",
    "Pyrokit", "Pyralis", "Bopple", "Boppair", "Echoflit", "Sonarwing",
    "Stinkroot", "Bloomweed", "Reekvine", "Sporekit", "Sporix", "Pollenfly",
    "Mothira", "Wormi", "Tribur", "Coinkit", "Goldpaw", "Headuck",
    "Dazedrake", "Furor", "Furyape", "Pyrhound", "Inferdog", "Swimrl",
    "Frogup", "Frogist", "Trancekid", "Telekith", "Mindara", "Brawnki",
    "Brawnix", "Champro", "Belldrip", "Bellweep", "Tongrip", "Stingjel",
    "Jellfang", "Boulkid", "Boulroll", "Boulker", "Sparflame", "Pyrosteed",
    "Drowzy", "Slumshell", "Magbolt", "Maglinx", "Verduck", "Twostrut",
    "Tristrut", "Sealoo", "Tuskling", "Sluddoo", "Toxooze", "Clammy",
    "Shellfort", "Wispgha", "Ectorum", "Specrum", "Stonecoil", "Sleeptap",
    "Hypnoze", "Pincherk", "Pincherd", "Sparkball", "Voltsphere", "Eggsix",
    "Treenut", "Bonehel", "Bonewar", "Kickbox", "Punchbox", "Tonglik",
    "Sphering", "Twinsmog", "Rockram", "Rockdrill", "Eggnursie", "Vinedom",
    "Kangamum", "Curlsea", "Spinhorse", "Goldfin", "Goldspear", "Starlit",
    "Starlux", "Mimikai", "Bladewing", "Frostkiss", "Voltbuzz", "Magflare",
    "Pinclash", "Bullrage", "Splashy", "Hydrool", "Marindel", "Goopette",
    "Furli", "Aquafur", "Voltfur", "Pyrofur", "Polyhex", "Spirashell",
    "Spirafort", "Trilobit", "Trilobat", "Saurwing", "Snoozer", "Frostavi",
    "Voltavi", "Pyravi", "Wyrmlet", "Wyrmair", "Wyrmking", "Psyclone",
    "Auralin",
];

/// Returns the canonical Pokémon name for the given id (clamped to 1..=`POKEMON_COUNT`).
#[must_use]
pub fn pokemon_name(id: PokemonId) -> &'static str {
    let idx = (id.clamp(1, POKEMON_COUNT) - 1) as usize;
    POKEMON_NAMES[idx]
}

// ── Rarity ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

pub const ALL_RARITIES: &[Rarity] = &[
    Rarity::Common,
    Rarity::Uncommon,
    Rarity::Rare,
    Rarity::Epic,
    Rarity::Legendary,
];

pub const RARITY_WEIGHTS: &[(Rarity, u32)] = &[
    (Rarity::Common, 60),
    (Rarity::Uncommon, 25),
    (Rarity::Rare, 10),
    (Rarity::Epic, 4),
    (Rarity::Legendary, 1),
];

pub const RARITY_STARS: &[(Rarity, &str)] = &[
    (Rarity::Common, "★"),
    (Rarity::Uncommon, "★★"),
    (Rarity::Rare, "★★★"),
    (Rarity::Epic, "★★★★"),
    (Rarity::Legendary, "★★★★★"),
];

impl Rarity {
    #[must_use]
    pub fn stars(self) -> &'static str {
        RARITY_STARS
            .iter()
            .find(|(r, _)| *r == self)
            .map_or("★", |(_, s)| s)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Uncommon => "uncommon",
            Self::Rare => "rare",
            Self::Epic => "epic",
            Self::Legendary => "legendary",
        }
    }
}

// ── Eyes & Hats ───────────────────────────────────────────────────────────────

pub const ALL_EYES: &[char] = &['·', '✦', '×', '◉', '@', '°'];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hat {
    None,
    Crown,
    Tophat,
    Propeller,
    Halo,
    Wizard,
    Beanie,
    Tinyduck,
}

pub const ALL_HATS: &[Hat] = &[
    Hat::None,
    Hat::Crown,
    Hat::Tophat,
    Hat::Propeller,
    Hat::Halo,
    Hat::Wizard,
    Hat::Beanie,
    Hat::Tinyduck,
];

// ── Stats ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StatName {
    Debugging,
    Patience,
    Chaos,
    Wisdom,
    Snark,
}

pub const ALL_STAT_NAMES: &[StatName] = &[
    StatName::Debugging,
    StatName::Patience,
    StatName::Chaos,
    StatName::Wisdom,
    StatName::Snark,
];

impl StatName {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Debugging => "DEBUGGING",
            Self::Patience => "PATIENCE",
            Self::Chaos => "CHAOS",
            Self::Wisdom => "WISDOM",
            Self::Snark => "SNARK",
        }
    }
}

// ── Core structs ──────────────────────────────────────────────────────────────

/// Deterministic parts — regenerated from `hash(user_id)` on every load.
/// `pokemon_id` may be overridden by an explicit user choice persisted in
/// `StoredCompanion::pokemon_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionBones {
    pub rarity: Rarity,
    pub pokemon_id: PokemonId,
    pub eye: char,
    pub hat: Hat,
    pub shiny: bool,
    pub stats: HashMap<StatName, u8>,
}

/// LLM-generated soul — persisted in `~/.elai/companion.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanionSoul {
    pub name: String,
    pub personality: String,
}

/// What actually persists. Bones (other than `pokemon_id`) are always regenerated
/// from `hash(user_id)`. `pokemon_id` records the user's explicit pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCompanion {
    pub soul: CompanionSoul,
    /// Unix timestamp (seconds) when the companion was hatched.
    pub hatched_at: u64,
    /// User-chosen Pokédex id. Absent for legacy records — triggers the picker
    /// on next launch.
    #[serde(default)]
    pub pokemon_id: Option<PokemonId>,
}

/// Fully resolved companion (bones + soul).
#[derive(Debug, Clone)]
pub struct Companion {
    pub rarity: Rarity,
    pub pokemon_id: PokemonId,
    pub eye: char,
    pub hat: Hat,
    pub shiny: bool,
    pub stats: HashMap<StatName, u8>,
    pub name: String,
    pub personality: String,
    pub hatched_at: u64,
}

impl Companion {
    #[must_use]
    pub fn from_parts(bones: CompanionBones, soul: CompanionSoul, hatched_at: u64) -> Self {
        Self {
            rarity: bones.rarity,
            pokemon_id: bones.pokemon_id,
            eye: bones.eye,
            hat: bones.hat,
            shiny: bones.shiny,
            stats: bones.stats,
            name: soul.name,
            personality: soul.personality,
            hatched_at,
        }
    }

    /// One-line summary displayed in the TUI header.
    #[must_use]
    pub fn summary_line(&self) -> String {
        let shiny_mark = if self.shiny { "✨ " } else { "" };
        format!(
            "{shiny}{name} · {species} #{id:03} [{rarity}] {stars}",
            shiny = shiny_mark,
            name = self.name,
            species = pokemon_name(self.pokemon_id),
            id = self.pokemon_id,
            rarity = self.rarity.as_str(),
            stars = self.rarity.stars(),
        )
    }
}
