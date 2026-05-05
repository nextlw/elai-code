//! Companion types — species, rarity, stats, and the Companion struct.
//! Ported from `src/buddy/types.ts` in the TS reference.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Species ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Species {
    Duck,
    Goose,
    Blob,
    Cat,
    Dragon,
    Octopus,
    Owl,
    Penguin,
    Turtle,
    Snail,
    Ghost,
    Axolotl,
    Capybara,
    Cactus,
    Robot,
    Rabbit,
    Mushroom,
    Chonk,
}

pub const ALL_SPECIES: &[Species] = &[
    Species::Duck,
    Species::Goose,
    Species::Blob,
    Species::Cat,
    Species::Dragon,
    Species::Octopus,
    Species::Owl,
    Species::Penguin,
    Species::Turtle,
    Species::Snail,
    Species::Ghost,
    Species::Axolotl,
    Species::Capybara,
    Species::Cactus,
    Species::Robot,
    Species::Rabbit,
    Species::Mushroom,
    Species::Chonk,
];

impl Species {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duck => "duck",
            Self::Goose => "goose",
            Self::Blob => "blob",
            Self::Cat => "cat",
            Self::Dragon => "dragon",
            Self::Octopus => "octopus",
            Self::Owl => "owl",
            Self::Penguin => "penguin",
            Self::Turtle => "turtle",
            Self::Snail => "snail",
            Self::Ghost => "ghost",
            Self::Axolotl => "axolotl",
            Self::Capybara => "capybara",
            Self::Cactus => "cactus",
            Self::Robot => "robot",
            Self::Rabbit => "rabbit",
            Self::Mushroom => "mushroom",
            Self::Chonk => "chonk",
        }
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionBones {
    pub rarity: Rarity,
    pub species: Species,
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

/// What actually persists.  Bones are always regenerated from `hash(user_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCompanion {
    pub soul: CompanionSoul,
    /// Unix timestamp (seconds) when the companion was hatched.
    pub hatched_at: u64,
}

/// Fully resolved companion (bones + soul).
#[derive(Debug, Clone)]
pub struct Companion {
    pub rarity: Rarity,
    pub species: Species,
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
            species: bones.species,
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
        format!(
            "{} {} [{}] {}",
            self.name,
            self.species.as_str(),
            self.rarity.as_str(),
            self.rarity.stars()
        )
    }
}
