//! Comando `/collection` — exibe e gerencia a coleção de mascotes.
//!
//! Uso: `/collection` — mostra relatório da coleção
//!       `/collection show <id>` — mostra detalhes de um mascote
//!       `/collection active <id>` — define mascote ativo

use runtime::buddy::{
    collection::{load_or_create_collection, save_collection, UserCollection},
    count_by_rarity, pokemon_name, rarity_for_pokemon, sprite_for_id, PokemonId, Rarity,
};

/// Formata número com separador de milhar (`1234567` → `"1,234,567"`).
fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// Executa o comando de coleção
pub fn run_collection_command(args: &[&str]) -> String {
    let mut collection = load_or_create_collection();

    if args.is_empty() || args[0] == "status" || args[0] == "" {
        return collection.collection_report();
    }

    match args[0] {
        "show" => {
            if args.len() < 2 {
                return "Uso: /collection show <id>\nExemplo: /collection show 25".to_string();
            }
            if let Ok(id) = args[1].parse::<u16>() {
                show_pokemon_detail(&collection, id)
            } else {
                "ID inválido. Use um número de 1 a 151.".to_string()
            }
        }
        "active" => {
            if args.len() < 2 {
                return "Uso: /collection active <id>\nExemplo: /collection active 25".to_string();
            }
            if let Ok(id) = args[1].parse::<u16>() {
                if collection.set_active(id) {
                    let _ = save_collection(&collection);
                    format!(
                        "✅ Mascote ativo definido: #{} {}",
                        id,
                        pokemon_name(id)
                    )
                } else {
                    format!(
                        "❌ Mascote #{} ainda não foi desbloqueado.\nDesbloqueie-o gastando mais tokens!",
                        id
                    )
                }
            } else {
                "ID inválido.".to_string()
            }
        }
        "list" => {
            if args.len() < 2 || args[1] == "unlocked" {
                list_unlocked(&collection)
            } else if args[1] == "locked" {
                list_locked(&collection)
            } else if args[1] == "starters" {
                list_starters(&collection)
            } else {
                "Uso: /collection list [unlocked|locked|starters]".to_string()
            }
        }
        "progress" => {
            show_progress_detail(&collection)
        }
        "help" => {
            COLLECTION_HELP.to_string()
        }
        _ => {
            format!("Comando desconhecido: {}\n\n{}", args[0], COLLECTION_HELP)
        }
    }
}

fn show_pokemon_detail(collection: &UserCollection, id: PokemonId) -> String {
    let entry = match collection.entry(id) {
        Some(e) => e,
        None => return format!("Mascote #{} não existe.", id),
    };

    let name = pokemon_name(id);
    let rarity = rarity_for_pokemon(id);
    let is_unlocked = entry.status.is_unlocked();

    let status_str = match entry.status {
        runtime::buddy::collection::UnlockStatus::Starter => "🌟 Starter",
        runtime::buddy::collection::UnlockStatus::Unlocked { .. } => "🔓 Desbloqueado",
        runtime::buddy::collection::UnlockStatus::Locked => "🔒 Bloqueado",
    };

    let mut lines = vec![
        format!("╔══════════════════════════════════════╗"),
        format!("║  #{:03} {}  {}", id, name, rarity.stars()),
        format!("╠══════════════════════════════════════╣"),
        format!("║  Status: {}", status_str),
        format!("║  Raridade: {} ({})", rarity.as_str(), rarity.stars()),
    ];

    if is_unlocked {
        lines.push(format!(
            "║  Shiny: {}",
            if entry.shiny_unlocked {
                "✨ Desbloqueado"
            } else {
                "🔒 Bloqueado (100k tokens extra)"
            }
        ));
        lines.push(format!(
            "║  Golden: {}",
            if entry.golden_unlocked {
                "🌟 Desbloqueado"
            } else {
                "🔒 Complete a raridade"
            }
        ));
        if entry.tokens_at_unlock > 0 {
            lines.push(format!("║  Desbloqueado aos: {} tokens", format_thousands(entry.tokens_at_unlock)));
        }
    } else {
        lines.push(format!(
            "║  Requisito: {} tokens para desbloquear",
            next_threshold_for_rarity(&collection, rarity)
        ));
    }

    lines.push(format!("╚══════════════════════════════════════╝"));

    lines.join("\n")
}

fn list_unlocked(collection: &UserCollection) -> String {
    let unlocked = collection.unlocked();
    let total = unlocked.len();

    let mut lines = vec![format!("📦 Mascotes desbloqueados ({}/{}):\n", total, 151)];

    for (i, &id) in unlocked.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            lines.push(String::new());
        }
        let rarity = rarity_for_pokemon(id);
        lines.push(format!("  #{:03} {} {}", id, pokemon_name(id), rarity.stars()));
    }

    lines.join("\n")
}

fn list_locked(collection: &UserCollection) -> String {
    let locked = collection.locked();

    let mut lines = vec![format!("🔒 Mascotes bloqueados ({}):\n", locked.len())];

    // Agrupa por raridade
    let mut by_rarity: std::collections::HashMap<Rarity, Vec<PokemonId>> =
        std::collections::HashMap::new();
    for &id in &locked {
        let rarity = rarity_for_pokemon(id);
        by_rarity.entry(rarity).or_default().push(id);
    }

    for rarity in [
        Rarity::Legendary,
        Rarity::Epic,
        Rarity::Rare,
        Rarity::Uncommon,
        Rarity::Common,
    ] {
        if let Some(ids) = by_rarity.get(&rarity) {
            lines.push(format!(
                "\n{} {}:",
                rarity.stars(),
                rarity.as_str().to_uppercase()
            ));
            // Mostra os primeiros 10
            for &id in ids.iter().take(10) {
                lines.push(format!("  #{:03} {}", id, pokemon_name(id)));
            }
            if ids.len() > 10 {
                lines.push(format!("  ... e mais {} bloqueados", ids.len() - 10));
            }
        }
    }

    lines.join("\n")
}

fn list_starters(collection: &UserCollection) -> String {
    let starters = collection.starters();

    if starters.is_empty() {
        return "Nenhum starter selecionado.".to_string();
    }

    let mut lines = vec!["🌟 Seus mascotes iniciais:\n".to_string()];

    for &id in &starters {
        lines.push(format!("  #{:03} {}", id, pokemon_name(id)));
    }

    lines.join("\n")
}

fn show_progress_detail(collection: &UserCollection) -> String {
    let tokens = collection.total_tokens_spent();
    let next = collection.next_unlock_threshold();

    let mut lines = vec![
        "📊 Progresso detalhado:".to_string(),
        format!("  💰 Total de tokens gastos: {}", format_thousands(tokens)),
        String::new(),
    ];

    if let Some(n) = next {
        let needed = n.tokens_required - tokens;
        let progress = (tokens as f64 / n.tokens_required as f64 * 100.0).min(100.0);
        lines.push(format!("  🔓 Próximo desbloqueio: {} ({})", n.rarity.as_str(), n.rarity.stars()));
        lines.push(format!("  📈 Progresso: {:.1}%", progress));
        lines.push(format!("  ⏳ Faltam: {} tokens", format_thousands(needed)));
    } else {
        lines.push("  🏆 Você desbloqueou todos os mascotes! Parabéns!".to_string());
    }

    lines.push(String::new());
    lines.push("  Resumo por raridade:".to_string());

    for rarity in [
        Rarity::Legendary,
        Rarity::Epic,
        Rarity::Rare,
        Rarity::Uncommon,
        Rarity::Common,
    ] {
        let total = count_by_rarity(rarity);
        let unlocked = count_by_rarity_in_unlocked(&collection, rarity);
        let pct = if total > 0 {
            (unlocked as f64 / total as f64 * 100.0)
        } else {
            0.0
        };

        lines.push(format!(
            "    {} {}: {}/{} ({:.0}%)",
            rarity.stars(),
            rarity.as_str(),
            unlocked,
            total,
            pct
        ));
    }

    lines.join("\n")
}

fn next_threshold_for_rarity(collection: &UserCollection, _rarity: Rarity) -> u64 {
    collection
        .next_unlock_threshold()
        .map(|t| t.tokens_required)
        .unwrap_or(0)
}

fn count_by_rarity_in_unlocked(collection: &UserCollection, rarity: Rarity) -> usize {
    collection
        .unlocked_by_rarity()
        .get(&rarity)
        .copied()
        .unwrap_or(0)
}

const COLLECTION_HELP: &str = r#"📦 Comandos de Coleção:

  /collection          — Mostra relatório geral da coleção
  /collection status   — Mesmo que acima
  /collection progress — Detalhes do progresso
  /collection list     — Lista mascotes desbloqueados
  /collection show <id> — Mostra detalhes do mascote #<id>
  /collection active <id> — Define mascote ativo como #<id>
  /collection help    — Mostra esta ajuda

💡 Dicas:
  • Gaste tokens para desbloquear novos mascotes!
  • Cada raridade tem requisitos diferentes:
    - Common:      a partir de 10k tokens
    - Uncommon:    a partir de 50k tokens
    - Rare:        a partir de 150k tokens
    - Epic:        a partir de 500k tokens
    - Legendary:   a partir de 1M tokens
  • Complete uma raridade para desbloquear versão Golden!"#;
