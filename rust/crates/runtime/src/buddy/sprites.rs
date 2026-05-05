//! ASCII art sprites, one per species (3-5 lines, ~20 chars wide).

use super::types::Species;

/// Returns the ASCII sprite for a given species.
#[must_use]
pub fn sprite_for(species: Species) -> &'static str {
    match species {
        Species::Duck => {
            " __\n( o)>\n/ |\\\n"
        }
        Species::Goose => {
            "  _\n /(o)\n/ //\n"
        }
        Species::Blob => {
            " _____\n( o o )\n \\_^_/\n"
        }
        Species::Cat => {
            "/\\_/\\\n( ^.^ )\n > ~ <\n"
        }
        Species::Dragon => {
            " /\\  /\\\n( >X< )\n /~\\/~\\\n"
        }
        Species::Octopus => {
            " _____\n( o o )\n/||||||\\\n"
        }
        Species::Owl => {
            " /\\_/\\\n( O.O )\n |u_u|\n"
        }
        Species::Penguin => {
            "  _\n (o)\n /||\\\n"
        }
        Species::Turtle => {
            " _____\n( o o )\n \\___/\n ~ ~ ~\n"
        }
        Species::Snail => {
            "  __\n_/ o\\\n\\___/\n ~  ~\n"
        }
        Species::Ghost => {
            " ___\n(o_o)\n/   \\\n ~  ~\n"
        }
        Species::Axolotl => {
            " /\\_/\\\n( u.u )\n/|   |\\\n"
        }
        Species::Capybara => {
            " /\\_/\\\n( o.o )\n > ^ <\n"
        }
        Species::Cactus => {
            " _|_\n( o )\n |_|\n"
        }
        Species::Robot => {
            "[=_=]\n|o_o|\n|   |\n"
        }
        Species::Rabbit => {
            "(\\ /)\n( ^_^)\n(\") (\")\n"
        }
        Species::Mushroom => {
            " ___\n(o_o)\n|___|\n"
        }
        Species::Chonk => {
            " _____\n(o . o)\n\\_____/\n"
        }
    }
}

/// Renders the companion sprite with optional "shiny" annotation.
#[must_use]
pub fn render_sprite(species: Species, shiny: bool) -> String {
    let base = sprite_for(species);
    if shiny {
        format!("✨{base}✨")
    } else {
        base.to_string()
    }
}
