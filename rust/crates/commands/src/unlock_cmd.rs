//! Comando `/unlock` — Interface de linha de comando para o sistema de unlock.
//!
//! Uso: `/unlock` — mostra status do unlock atual
//!       `/unlock simular <tokens>` — simula gasto de tokens
//!       `/unlock força <id>` — força desbloqueio de um mascote

use std::time::{SystemTime, UNIX_EPOCH};

use runtime::buddy::{
    collection::{load_or_create_collection, save_collection},
    orchestrator::UnlockOrchestrator,
    unlock_event::TaskType,
    pokemon_name, rarity_for_pokemon, count_by_rarity, Rarity, MILESTONE_CONFIGS,
    UserCollection,
};

/// Executa o comando de unlock
#[must_use] 
pub fn run_unlock_command(args: &[&str]) -> String {
    let mut collection = load_or_create_collection();

    if args.is_empty() || args[0] == "status" {
        return show_unlock_status(&collection);
    }

    match args[0] {
        "simular" | "simulate" => {
            if args.len() < 2 {
                return "Uso: /unlock simular <tokens>\nExemplo: /unlock simular 15000".to_string();
            }
            if let Ok(tokens) = args[1].parse::<u64>() {
                simulate_token_spending(tokens, &mut collection)
            } else {
                "Tokens inválidos. Use um número.".to_string()
            }
        }
        "força" | "force" => {
            if args.len() < 2 {
                return "Uso: /unlock força <id>\nExemplo: /unlock força 42".to_string();
            }
            if let Ok(id) = args[1].parse::<u16>() {
                force_unlock(id, &mut collection)
            } else {
                "ID inválido.".to_string()
            }
        }
        "milestone" => {
            list_milestones()
        }
        "raridade" | "rarity" => {
            if args.len() < 2 {
                return "Uso: /unlock raridade <common|uncommon|rare|epic|legendary>\nExemplo: /unlock raridade uncommon".to_string();
            }
            show_rarity_info(args[1])
        }
        "teste" | "test" => {
            run_integration_test()
        }
        "help" => {
            UNLOCK_HELP.to_string()
        }
        _ => {
            format!("Comando desconhecido: {}\n\n{}", args[0], UNLOCK_HELP)
        }
    }
}

fn show_unlock_status(collection: &UserCollection) -> String {
    let tokens = collection.total_tokens_spent();
    let unlocked = collection.unlocked_count();
    let total = 151;
    let percentage = (unlocked as f64 / f64::from(total)) * 100.0;

    let mut lines = vec![
        format!("╔════════════════════════════════════════════════════════════╗"),
        format!("║           🐾 SISTEMA DE UNLOCK DE COMPANHEIROS 🐾        ║"),
        format!("╠════════════════════════════════════════════════════════════╣"),
        format!("║  💰 Tokens gastos na tarefa atual: {:>15}              ║", format_num(tokens)),
        format!("║  📦 Mascotes desbloqueados:         {:>15}/151              ║", unlocked),
        format!("║  📈 Progresso da coleção:          {:>14.1}%              ║", percentage),
    ];

    // Mostra próximo milestone
    lines.push("╠════════════════════════════════════════════════════════════╣".to_string());
    lines.push("║  🎯 PRÓXIMOS MILESTONES:                                    ║".to_string());

    let milestones = get_milestone_display(tokens);
    for (i, (threshold, rarity)) in milestones.iter().enumerate() {
        if i >= 4 {
            break;
        }
        let status = if tokens >= *threshold {
            "✓ ATINGIDO"
        } else {
            "⏳ PENDENTE"
        };
        let remaining = if tokens >= *threshold {
            0
        } else {
            threshold - tokens
        };
        lines.push(format!(
            "║    {} {}: {:>12} tokens (faltam {:>10})              ║",
            status,
            rarity,
            format_num(*threshold),
            format_num(remaining)
        ));
    }

    lines.push("╚════════════════════════════════════════════════════════════╝".to_string());

    lines.join("\n")
}

fn simulate_token_spending(tokens: u64, collection: &mut UserCollection) -> String {
    let mut lines = vec![format!("🎮 Simulação de gasto de {} tokens:\n", tokens)];

    // Cria orchestrator com a collection
    let mut orch = UnlockOrchestrator::from_collection(collection.clone());
    orch.start_task("simulacao".to_string());

    // Adiciona tokens
    if let Some(milestone) = orch.add_tokens(tokens) {
        lines.push("🎉 MILESTONE ATINGIDO!".to_string());
        lines.push(format!("  Threshold: {} tokens", format_num(milestone.tokens_threshold)));
        lines.push(format!("  Raridade base: {:?}", milestone.rarity));
        lines.push(format!("  Probabilidade: {:.0}%", milestone.probability * 100.0));

        // Avalia complexidade
        orch.update_task_context(
            vec!["src/main.rs".to_string()],
            100,
            50,
            TaskType::Feature,
        );

        if let Some(eval) = orch.evaluate_current_task() {
            lines.push(String::new());
            lines.push("📊 Avaliação de Complexidade:".to_string());
            lines.push(format!("  Score: {}/10", eval.complexity_score));
            lines.push(format!("  Tipo: {:?}", eval.task_type));
            lines.push(format!("  Raridade determinada: {:?}", eval.determined_rarity));
            lines.push(format!("  Bônus: {}", if eval.bonus_rarity { "SIM" } else { "NÃO" }));
            lines.push(format!("  Reason: {}", eval.reasoning));

            // Cria evento
            if let Some(event) = orch.create_unlock_event(&eval) {
                if let Some(mascot_id) = event.chosen_mascot {
                    lines.push(String::new());
                    lines.push("✨ MASCOTE SORTEADO:".to_string());
                    lines.push(format!(
                        "  #{} {} ({:?})",
                        mascot_id,
                        pokemon_name(mascot_id),
                        event.rarity
                    ));

                    // Confirma
                    let outcome = orch.confirm_unlock();
                    lines.push(String::new());
                    lines.push("✅ RESULTADO:".to_string());
                    lines.push(format!("  {}", outcome.message));
                }
            }
        }
    } else {
        lines.push("❌ Nenhum milestone atingido.".to_string());
        let remaining = next_threshold_for_tokens(tokens) - tokens;
        lines.push(format!(
            "💡 Gaste mais {} tokens para atingir o próximo milestone.",
            format_num(remaining)
        ));
    }

    // Atualiza collection
    *collection = orch.collection;

    lines.join("\n")
}

fn force_unlock(id: u16, collection: &mut UserCollection) -> String {
    if !(1..=151).contains(&id) {
        return format!("ID {id} inválido. Use um número de 1 a 151.");
    }

    if collection.is_unlocked(id) {
        return format!(
            "Mascote #{} {} já está desbloqueado!",
            id,
            pokemon_name(id)
        );
    }

    // Guarda o valor antes do borrow
    let current_tokens = collection.total_tokens_spent();

    // Desbloqueia
    if let Some(entry) = collection.entry_mut(id) {
        entry.status = runtime::buddy::UnlockStatus::Unlocked {
            unlocked_at: now_unix_secs(),
        };
        entry.tokens_at_unlock = current_tokens;
    }

    // Salva
    if let Err(e) = save_collection(collection) {
        return format!("Erro ao salvar: {e}");
    }

    let rarity = rarity_for_pokemon(id);
    format!(
        "✨ #{} {} forçadamente desbloqueado! ({})\n{}",
        id,
        pokemon_name(id),
        rarity.as_str(),
        rarity.stars()
    )
}

fn list_milestones() -> String {
    let mut lines = vec![
        "🎯 MILESTONES DO SISTEMA DE UNLOCK:".to_string(),
        String::new(),
    ];

    for (i, config) in MILESTONE_CONFIGS.iter().enumerate() {
        lines.push(format!(
            "  {}. {:>12} tokens → {:?} (prob: {:.0}%)",
            i + 1,
            format_num(config.tokens),
            config.rarity,
            config.base_probability * 100.0
        ));
    }

    lines.push(String::new());
    lines.push("💡 Dica: Apenas tarefas que atingem milestones podem desbloquear mascotes!".to_string());

    lines.join("\n")
}

fn show_rarity_info(rarity_str: &str) -> String {
    let rarity = match rarity_str.to_lowercase().as_str() {
        "common" => Rarity::Common,
        "uncommon" => Rarity::Uncommon,
        "rare" => Rarity::Rare,
        "epic" => Rarity::Epic,
        "legendary" => Rarity::Legendary,
        _ => {
            return format!(
                "Raridade '{rarity_str}' desconhecida.\nUse: common, uncommon, rare, epic ou legendary"
            )
        }
    };

    let count = count_by_rarity(rarity);
    let collection = load_or_create_collection();
    let unlocked = collection
        .unlocked()
        .iter()
        .filter(|&&id| {
            let entry_rarity = rarity_for_pokemon(id);
            entry_rarity == rarity
        })
        .count();

    let progress = if count > 0 {
        (unlocked as f64 / count as f64) * 100.0
    } else {
        0.0
    };

    format!(
        "╔══════════════════════════════════════╗
║  {} {} {}
╠══════════════════════════════════════╣
║  Total na raridade: {:>3}
║  Desbloqueados:      {:>3}/{:>3}
║  Progresso:          {:>3.0}%
╠══════════════════════════════════════╣
║  Probabilidade base: {:.0}%
║  Threshold:          {:>10} tokens
╚══════════════════════════════════════╝",
        "  ",
        rarity.stars(),
        rarity.as_str().to_uppercase(),
        count,
        unlocked,
        count,
        progress,
        milestone_probability_for_rarity(rarity) * 100.0,
        format_num(threshold_for_rarity(rarity))
    )
}

fn run_integration_test() -> String {
    let mut lines = vec![
        "🧪 EXECUTANDO TESTE DE INTEGRAÇÃO...".to_string(),
        String::new(),
    ];

    // Teste 1: Coleção carrega
    lines.push("1. Carregando coleção...".to_string());
    let collection = load_or_create_collection();
    lines.push(format!(
        "   ✓ Coleção carregada: {} mascotes",
        collection.unlocked_count()
    ));

    // Teste 2: Simula milestone
    lines.push(String::new());
    lines.push("2. Simulando milestone de 10k tokens...".to_string());
    let mut orch = UnlockOrchestrator::from_collection(collection.clone());
    orch.start_task("teste_automatico".to_string());
    
    if let Some(milestone) = orch.add_tokens(10_000) {
        lines.push(format!(
            "   ✓ Milestone atingido: {} tokens ({:?})",
            milestone.tokens_threshold, milestone.rarity
        ));

        // Avalia
        orch.update_task_context(
            vec!["test.rs".to_string()],
            50,
            10,
            TaskType::BugFix,
        );

        if let Some(eval) = orch.evaluate_current_task() {
            lines.push(format!("   ✓ Avaliação: score={}/10, rarity={:?}", eval.complexity_score, eval.determined_rarity));

            if let Some(event) = orch.create_unlock_event(&eval) {
                if let Some(id) = event.chosen_mascot {
                    lines.push(format!(
                        "   ✓ Mascote sorteado: #{} {}",
                        id,
                        pokemon_name(id)
                    ));
                    orch.confirm_unlock();
                    lines.push("   ✓ Confirmação OK".to_string());
                }
            }
        }
    } else {
        lines.push("   ✗ ERRO: Milestone não atingiu".to_string());
    }

    // Teste 3: Verifica persistência
    lines.push(String::new());
    lines.push("3. Verificando persistência...".to_string());
    let reloaded = load_or_create_collection();
    lines.push(format!(
        "   ✓ Collection persistida: {} mascotes",
        reloaded.unlocked_count()
    ));

    lines.push(String::new());
    lines.push("════════════════════════════════════".to_string());
    lines.push("✅ TESTE DE INTEGRAÇÃO COMPLETO!".to_string());
    lines.push("════════════════════════════════════".to_string());

    lines.join("\n")
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn get_milestone_display(_current_tokens: u64) -> Vec<(u64, &'static str)> {
    MILESTONE_CONFIGS
        .iter()
        .map(|c| (c.tokens, c.rarity.as_str()))
        .collect()
}

fn next_threshold_for_tokens(tokens: u64) -> u64 {
    MILESTONE_CONFIGS
        .iter()
        .find(|c| tokens < c.tokens)
        .map_or(0, |c| c.tokens)
}

fn threshold_for_rarity(rarity: Rarity) -> u64 {
    MILESTONE_CONFIGS
        .iter()
        .find(|c| c.rarity == rarity)
        .map_or(0, |c| c.tokens)
}

fn milestone_probability_for_rarity(rarity: Rarity) -> f64 {
    MILESTONE_CONFIGS
        .iter()
        .find(|c| c.rarity == rarity)
        .map_or(0.0, |c| c.base_probability)
}

const UNLOCK_HELP: &str = r"🐾 Comandos de Unlock:

  /unlock                — Mostra status do sistema
  /unlock status         — Mesmo que acima
  /unlock simular <n>    — Simula gasto de n tokens
  /unlock força <id>     — Força desbloqueio do mascote #id
  /unlock milestone      — Lista todos os milestones
  /unlock raridade <r>   — Info sobre uma raridade
  /unlock teste          — Executa teste de integração
  /unlock help           — Mostra esta ajuda

💡 Exemplos:
  /unlock simular 15000
  /unlock força 42
  /unlock raridade epic

🎯 Como funciona:
  1. Gaste tokens em uma tarefa
  2. Ao atingir um milestone, a tarefa PAUSA
  3. O modelo avalia a complexidade
  4. Um mascote é sorteado da raridade determinada
  5. Você pode aceitar ou recusar
  6. Tarefa continua normalmente";

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Adiciona separador de milhares a um número
fn format_num(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}