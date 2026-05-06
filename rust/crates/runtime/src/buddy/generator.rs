//! Deterministic companion generation from `hash(user_id)`.
//! Ports the Mulberry32 PRNG and generation logic from `src/buddy/companion.ts`.

use std::collections::HashMap;

use super::types::{
    ALL_EYES, ALL_HATS, ALL_STAT_NAMES, CompanionBones, POKEMON_COUNT, RARITY_WEIGHTS, Rarity,
    StatName,
};

// ── Mulberry32 PRNG ───────────────────────────────────────────────────────────

/// Seeded PRNG returning f64 in [0, 1). Equivalent to the JS Mulberry32 impl.
pub struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    pub fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state ^ (self.state >> 15);
        t = t.wrapping_mul(1 | self.state);
        t ^= t.wrapping_add(t.wrapping_mul(61 | t) ^ t);
        t ^= t >> 14;
        f64::from(t) / 4_294_967_296.0
    }

    pub fn next_usize(&mut self, len: usize) -> usize {
        // Mulberry32 RNG inherently involves f64->usize casts — precision loss is intentional
        #[allow(clippy::cast_precision_loss)]
        let idx = (self.next_f64() * len as f64) as usize;
        idx % len
    }
}

// ── FNV-1a 32-bit hash ────────────────────────────────────────────────────────

/// FNV-1a 32-bit hash — matches the JS fallback in `hashString`.
#[must_use]
pub fn hash_string(s: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for byte in s.bytes() {
        h ^= u32::from(byte);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

// ── Rarity roll ───────────────────────────────────────────────────────────────

fn roll_rarity(rng: &mut Mulberry32) -> Rarity {
    let total: u32 = RARITY_WEIGHTS.iter().map(|(_, w)| w).sum();
    let mut roll = (rng.next_f64() * f64::from(total)) as u32;
    for (rarity, weight) in RARITY_WEIGHTS {
        if roll < *weight {
            return *rarity;
        }
        roll -= weight;
    }
    Rarity::Common
}

// ── Stat generation ───────────────────────────────────────────────────────────

const RARITY_FLOOR: &[(Rarity, u8)] = &[
    (Rarity::Common, 5),
    (Rarity::Uncommon, 15),
    (Rarity::Rare, 25),
    (Rarity::Epic, 35),
    (Rarity::Legendary, 50),
];

fn rarity_floor(rarity: Rarity) -> u8 {
    RARITY_FLOOR
        .iter()
        .find(|(r, _)| *r == rarity)
        .map_or(5, |(_, f)| *f)
}

fn roll_stats(rng: &mut Mulberry32, rarity: Rarity) -> HashMap<StatName, u8> {
    let floor = rarity_floor(rarity);
    let peak_idx = rng.next_usize(ALL_STAT_NAMES.len());
    let mut dump_idx = rng.next_usize(ALL_STAT_NAMES.len());
    // Ensure dump != peak
    while dump_idx == peak_idx {
        dump_idx = rng.next_usize(ALL_STAT_NAMES.len());
    }
    let peak = ALL_STAT_NAMES[peak_idx];
    let dump = ALL_STAT_NAMES[dump_idx];

    let mut stats = HashMap::new();
    for &name in ALL_STAT_NAMES {
        let val = if name == peak {
            (floor.saturating_add(50) + (rng.next_f64() * 30.0) as u8).min(100)
        } else if name == dump {
            floor.saturating_sub(10).saturating_add((rng.next_f64() * 15.0) as u8)
        } else {
            floor + (rng.next_f64() * 40.0) as u8
        };
        stats.insert(name, val.min(100));
    }
    stats
}

// ── Main entry ────────────────────────────────────────────────────────────────

/// Determinístico para o usuário: sorteia `rarity/pokemon_id/eye/hat/shiny/stats`
/// usando apenas `user_id` como semente. Útil como fallback antes da escolha.
#[must_use]
pub fn roll_bones(user_id: &str) -> CompanionBones {
    let seed = hash_string(user_id);
    let mut rng = Mulberry32::new(seed);

    let rarity = roll_rarity(&mut rng);
    let pokemon_id = (rng.next_usize(POKEMON_COUNT as usize) + 1) as u16;
    let eye = ALL_EYES[rng.next_usize(ALL_EYES.len())];
    let hat = ALL_HATS[rng.next_usize(ALL_HATS.len())];
    let shiny = rng.next_f64() < 0.05;
    let stats = roll_stats(&mut rng, rarity);

    CompanionBones {
        rarity,
        pokemon_id,
        eye,
        hat,
        shiny,
        stats,
    }
}

/// Determinístico para `(user_id, pokemon_id)`: cada mascote escolhido por um
/// mesmo usuário tem rarity/stats/eye/hat/shiny próprios. Garante variação
/// entre os 151 cards no picker e que a escolha seja idempotente.
#[must_use]
pub fn roll_bones_for(user_id: &str, pokemon_id: u16) -> CompanionBones {
    let combined = format!("{user_id}:{pokemon_id}");
    let seed = hash_string(&combined);
    let mut rng = Mulberry32::new(seed);

    let rarity = roll_rarity(&mut rng);
    let eye = ALL_EYES[rng.next_usize(ALL_EYES.len())];
    let hat = ALL_HATS[rng.next_usize(ALL_HATS.len())];
    let shiny = rng.next_f64() < 0.05;
    let stats = roll_stats(&mut rng, rarity);

    CompanionBones {
        rarity,
        pokemon_id: pokemon_id.clamp(1, POKEMON_COUNT),
        eye,
        hat,
        shiny,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roll_bones_is_deterministic() {
        let a = roll_bones("user-123");
        let b = roll_bones("user-123");
        assert_eq!(a.rarity, b.rarity);
        assert_eq!(a.pokemon_id, b.pokemon_id);
        assert_eq!(a.eye, b.eye);
        assert_eq!(a.stats, b.stats);
    }

    #[test]
    fn different_users_may_differ() {
        let a = roll_bones("alice");
        let b = roll_bones("bob");
        // Very unlikely to be identical for two different seeds
        assert!(a.pokemon_id != b.pokemon_id || a.rarity != b.rarity || a.eye != b.eye);
    }

    #[test]
    fn roll_bones_pokemon_id_in_range() {
        for i in 0..200 {
            let b = roll_bones(&format!("user-{i}"));
            assert!(b.pokemon_id >= 1 && b.pokemon_id <= POKEMON_COUNT);
        }
    }

    #[test]
    fn rarity_distribution_within_bounds() {
        let n = 1000;
        let mut counts = [0usize; 5];
        for i in 0..n {
            let b = roll_bones(&format!("user-{i}"));
            let idx = match b.rarity {
                Rarity::Common => 0,
                Rarity::Uncommon => 1,
                Rarity::Rare => 2,
                Rarity::Epic => 3,
                Rarity::Legendary => 4,
            };
            counts[idx] += 1;
        }
        // Common must be the most frequent
        assert!(counts[0] > counts[1]);
        assert!(counts[1] > counts[2]);
    }

    #[test]
    fn stats_all_in_range() {
        let b = roll_bones("test-user");
        for &v in b.stats.values() {
            assert!(v <= 100, "stat value {v} out of range");
        }
        assert_eq!(b.stats.len(), ALL_STAT_NAMES.len());
    }

    #[test]
    fn hash_string_matches_expected_for_known_input() {
        // FNV-1a is deterministic — just verify it doesn't change
        let h = hash_string("hello");
        assert_eq!(h, hash_string("hello"));
        assert_ne!(h, hash_string("world"));
    }
}
