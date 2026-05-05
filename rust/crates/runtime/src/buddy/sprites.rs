//! ANSI sprite art for the 151 Gen-1 Pokémon mascots.
//!
//! Sprites are 256-color half-block art lifted from `nexagent/teste_pokemon`,
//! embedded at compile time via `include_str!`.

use super::types::{PokemonId, POKEMON_COUNT};

/// Indexed by Pokédex id - 1. `SPRITES[24]` is Pikachu.
static SPRITES: [&str; POKEMON_COUNT as usize] = [
    include_str!("../../sprites/001.ans"),
    include_str!("../../sprites/002.ans"),
    include_str!("../../sprites/003.ans"),
    include_str!("../../sprites/004.ans"),
    include_str!("../../sprites/005.ans"),
    include_str!("../../sprites/006.ans"),
    include_str!("../../sprites/007.ans"),
    include_str!("../../sprites/008.ans"),
    include_str!("../../sprites/009.ans"),
    include_str!("../../sprites/010.ans"),
    include_str!("../../sprites/011.ans"),
    include_str!("../../sprites/012.ans"),
    include_str!("../../sprites/013.ans"),
    include_str!("../../sprites/014.ans"),
    include_str!("../../sprites/015.ans"),
    include_str!("../../sprites/016.ans"),
    include_str!("../../sprites/017.ans"),
    include_str!("../../sprites/018.ans"),
    include_str!("../../sprites/019.ans"),
    include_str!("../../sprites/020.ans"),
    include_str!("../../sprites/021.ans"),
    include_str!("../../sprites/022.ans"),
    include_str!("../../sprites/023.ans"),
    include_str!("../../sprites/024.ans"),
    include_str!("../../sprites/025.ans"),
    include_str!("../../sprites/026.ans"),
    include_str!("../../sprites/027.ans"),
    include_str!("../../sprites/028.ans"),
    include_str!("../../sprites/029.ans"),
    include_str!("../../sprites/030.ans"),
    include_str!("../../sprites/031.ans"),
    include_str!("../../sprites/032.ans"),
    include_str!("../../sprites/033.ans"),
    include_str!("../../sprites/034.ans"),
    include_str!("../../sprites/035.ans"),
    include_str!("../../sprites/036.ans"),
    include_str!("../../sprites/037.ans"),
    include_str!("../../sprites/038.ans"),
    include_str!("../../sprites/039.ans"),
    include_str!("../../sprites/040.ans"),
    include_str!("../../sprites/041.ans"),
    include_str!("../../sprites/042.ans"),
    include_str!("../../sprites/043.ans"),
    include_str!("../../sprites/044.ans"),
    include_str!("../../sprites/045.ans"),
    include_str!("../../sprites/046.ans"),
    include_str!("../../sprites/047.ans"),
    include_str!("../../sprites/048.ans"),
    include_str!("../../sprites/049.ans"),
    include_str!("../../sprites/050.ans"),
    include_str!("../../sprites/051.ans"),
    include_str!("../../sprites/052.ans"),
    include_str!("../../sprites/053.ans"),
    include_str!("../../sprites/054.ans"),
    include_str!("../../sprites/055.ans"),
    include_str!("../../sprites/056.ans"),
    include_str!("../../sprites/057.ans"),
    include_str!("../../sprites/058.ans"),
    include_str!("../../sprites/059.ans"),
    include_str!("../../sprites/060.ans"),
    include_str!("../../sprites/061.ans"),
    include_str!("../../sprites/062.ans"),
    include_str!("../../sprites/063.ans"),
    include_str!("../../sprites/064.ans"),
    include_str!("../../sprites/065.ans"),
    include_str!("../../sprites/066.ans"),
    include_str!("../../sprites/067.ans"),
    include_str!("../../sprites/068.ans"),
    include_str!("../../sprites/069.ans"),
    include_str!("../../sprites/070.ans"),
    include_str!("../../sprites/071.ans"),
    include_str!("../../sprites/072.ans"),
    include_str!("../../sprites/073.ans"),
    include_str!("../../sprites/074.ans"),
    include_str!("../../sprites/075.ans"),
    include_str!("../../sprites/076.ans"),
    include_str!("../../sprites/077.ans"),
    include_str!("../../sprites/078.ans"),
    include_str!("../../sprites/079.ans"),
    include_str!("../../sprites/080.ans"),
    include_str!("../../sprites/081.ans"),
    include_str!("../../sprites/082.ans"),
    include_str!("../../sprites/083.ans"),
    include_str!("../../sprites/084.ans"),
    include_str!("../../sprites/085.ans"),
    include_str!("../../sprites/086.ans"),
    include_str!("../../sprites/087.ans"),
    include_str!("../../sprites/088.ans"),
    include_str!("../../sprites/089.ans"),
    include_str!("../../sprites/090.ans"),
    include_str!("../../sprites/091.ans"),
    include_str!("../../sprites/092.ans"),
    include_str!("../../sprites/093.ans"),
    include_str!("../../sprites/094.ans"),
    include_str!("../../sprites/095.ans"),
    include_str!("../../sprites/096.ans"),
    include_str!("../../sprites/097.ans"),
    include_str!("../../sprites/098.ans"),
    include_str!("../../sprites/099.ans"),
    include_str!("../../sprites/100.ans"),
    include_str!("../../sprites/101.ans"),
    include_str!("../../sprites/102.ans"),
    include_str!("../../sprites/103.ans"),
    include_str!("../../sprites/104.ans"),
    include_str!("../../sprites/105.ans"),
    include_str!("../../sprites/106.ans"),
    include_str!("../../sprites/107.ans"),
    include_str!("../../sprites/108.ans"),
    include_str!("../../sprites/109.ans"),
    include_str!("../../sprites/110.ans"),
    include_str!("../../sprites/111.ans"),
    include_str!("../../sprites/112.ans"),
    include_str!("../../sprites/113.ans"),
    include_str!("../../sprites/114.ans"),
    include_str!("../../sprites/115.ans"),
    include_str!("../../sprites/116.ans"),
    include_str!("../../sprites/117.ans"),
    include_str!("../../sprites/118.ans"),
    include_str!("../../sprites/119.ans"),
    include_str!("../../sprites/120.ans"),
    include_str!("../../sprites/121.ans"),
    include_str!("../../sprites/122.ans"),
    include_str!("../../sprites/123.ans"),
    include_str!("../../sprites/124.ans"),
    include_str!("../../sprites/125.ans"),
    include_str!("../../sprites/126.ans"),
    include_str!("../../sprites/127.ans"),
    include_str!("../../sprites/128.ans"),
    include_str!("../../sprites/129.ans"),
    include_str!("../../sprites/130.ans"),
    include_str!("../../sprites/131.ans"),
    include_str!("../../sprites/132.ans"),
    include_str!("../../sprites/133.ans"),
    include_str!("../../sprites/134.ans"),
    include_str!("../../sprites/135.ans"),
    include_str!("../../sprites/136.ans"),
    include_str!("../../sprites/137.ans"),
    include_str!("../../sprites/138.ans"),
    include_str!("../../sprites/139.ans"),
    include_str!("../../sprites/140.ans"),
    include_str!("../../sprites/141.ans"),
    include_str!("../../sprites/142.ans"),
    include_str!("../../sprites/143.ans"),
    include_str!("../../sprites/144.ans"),
    include_str!("../../sprites/145.ans"),
    include_str!("../../sprites/146.ans"),
    include_str!("../../sprites/147.ans"),
    include_str!("../../sprites/148.ans"),
    include_str!("../../sprites/149.ans"),
    include_str!("../../sprites/150.ans"),
    include_str!("../../sprites/151.ans"),
];

/// Returns the ANSI sprite for the given Pokédex id (clamped to 1..=POKEMON_COUNT).
#[must_use]
pub fn sprite_for_id(id: PokemonId) -> &'static str {
    let idx = (id.clamp(1, POKEMON_COUNT) - 1) as usize;
    SPRITES[idx]
}

/// Renders the sprite, prepending a sparkle marker for shiny mascots.
#[must_use]
pub fn render_sprite(id: PokemonId, shiny: bool) -> String {
    let base = sprite_for_id(id);
    if shiny {
        format!("✨ {base}")
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_for_id_returns_non_empty_for_all_ids() {
        for id in 1..=POKEMON_COUNT {
            assert!(!sprite_for_id(id).is_empty(), "sprite #{id} is empty");
        }
    }

    #[test]
    fn sprite_for_id_clamps_out_of_range() {
        assert_eq!(sprite_for_id(0), sprite_for_id(1));
        assert_eq!(sprite_for_id(9999), sprite_for_id(POKEMON_COUNT));
    }

    #[test]
    fn pikachu_sprite_contains_ansi_escape() {
        let s = sprite_for_id(25);
        assert!(s.contains("\x1b["), "expected ANSI escape sequence in Pikachu sprite");
    }
}
