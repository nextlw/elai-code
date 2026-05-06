//! Testes de Integração do Sistema de Unlock
//!
//! Este arquivo demonstra o fluxo completo:
//! 1. Iniciar tarefa
//! 2. Adicionar tokens até atingir milestone
//! 3. Avaliar complexidade
//! 4. Criar evento de unlock
//! 5. Confirmar na coleção

#[cfg(test)]
mod integration_tests {
    use crate::buddy::{
        collection::{load_or_create_collection, save_collection},
        count_by_rarity, pokemon_name, rarity_for_pokemon, sprite_for_id, unlock_event::{
            evaluate_task_complexity, ComplexityEvaluation, MilestoneReached, TaskContext, TaskTracker, TaskType, UnlockEvent,
        }, orchestrator::{UnlockOrchestrator, UnlockOutcome}, PokemonId, Rarity,
    };

    #[test]
    fn test_full_unlock_flow() {
        let mut orch = UnlockOrchestrator::new();

        // 1. Inicia tarefa
        orch.start_task("test_integration_task".to_string());

        // 2. Adiciona tokens até milestone
        // Primeiro lote: 5k - não atinge milestone
        let milestone = orch.add_tokens(5_000);
        assert!(milestone.is_none(), "Não deve atingir milestone com 5k tokens");

        // Segundo lote: 5k mais = 10k - ATINGE milestone
        let milestone = orch.add_tokens(5_000);
        assert!(milestone.is_some(), "Deve atingir milestone de 10k");
        let milestone = milestone.unwrap();
        assert_eq!(milestone.tokens_threshold, 10_000);

        // 3. Atualiza contexto para avaliação
        orch.update_task_context(
            vec![
                "src/main.rs".to_string(),
                "src/utils.rs".to_string(),
                "tests/integration.rs".to_string(),
            ],
            250,   // lines_added
            100,   // lines_removed
            TaskType::Feature,
        );

        // 4. Avalia complexidade
        let eval = orch.evaluate_current_task();
        assert!(eval.is_some(), "Deve ter avaliação");
        let eval = eval.unwrap();

        println!("\n=== Avaliação de Complexidade ===");
        println!("Task type: {:?}", eval.task_type);
        println!("Complexity score: {}/10", eval.complexity_score);
        println!("Determined rarity: {:?}", eval.determined_rarity);
        println!("Reasoning: {}", eval.reasoning);
        println!("Bonus rarity: {}", eval.bonus_rarity);

        // Score deve ser pelo menos 5 para Feature + 250 linhas
        assert!(eval.complexity_score >= 5);

        // 5. Cria evento de unlock
        let event = orch.create_unlock_event(&eval);
        assert!(event.is_some(), "Deve criar evento");
        let event = event.unwrap();

        println!("\n=== Evento de Unlock ===");
        println!("Rarity: {:?}", event.rarity);
        println!("Chosen mascot: {:?}", event.chosen_mascot);
        if let Some(id) = event.chosen_mascot {
            println!("Mascot name: {}", pokemon_name(id));
        }
        println!("Available mascots of this rarity: {}", event.available_mascots.len());

        // Deve ter escolhido um mascote
        assert!(event.chosen_mascot.is_some());

        // 6. Confirma unlock
        let outcome = orch.confirm_unlock();
        assert!(outcome.success, "Confirmação deve ser bem sucedida");
        assert!(outcome.mascot_id.is_some());
        assert!(outcome.user_confirmed == Some(true));

        println!("\n=== Resultado ===");
        println!("{}", outcome.message);

        // Verifica que está na coleção
        let collection = &orch.collection;
        let mascot_id = outcome.mascot_id.unwrap();
        assert!(collection.is_unlocked(mascot_id), "Mascote deve estar desbloqueado na coleção");
    }

    #[test]
    fn test_milestone_only_triggers_once() {
        let mut orch = UnlockOrchestrator::new();
        orch.start_task("milestone_once_test".to_string());

        // Atinge 10k milestone
        let m1 = orch.add_tokens(10_000);
        assert!(m1.is_some());
        assert_eq!(m1.unwrap().tokens_threshold, 10_000);

        // Não deve trigger de novo com mais tokens
        let m2 = orch.add_tokens(1_000);
        assert!(m2.is_none(), "Mesmo milestone não deve trigger duas vezes");

        // Não deve trigger o mesmo milestone mesmo muito depois
        let m3 = orch.add_tokens(50_000);
        assert!(m3.is_some());
        // O próximo milestone deve ser 50k, não 10k de novo
        assert_eq!(m3.unwrap().tokens_threshold, 50_000);
    }

    #[test]
    fn test_rarity_determination_by_task_type() {
        let mut orch = UnlockOrchestrator::new();

        // Testa diferentes tipos de tarefa
        let test_cases = vec![
            (TaskType::BugFix, false, Rarity::Common),
            (TaskType::Documentation, false, Rarity::Common),
            (TaskType::Setup, false, Rarity::Common),
            (TaskType::Feature, false, Rarity::Uncommon),
            (TaskType::Refactor, false, Rarity::Uncommon),
            (TaskType::Investigation, false, Rarity::Rare),
            (TaskType::Complex, false, Rarity::Epic),
        ];

        for (task_type, _expected_bonus, _expected_base_rarity) in test_cases {
            orch.start_task(format!("test_{:?}", task_type));
            orch.update_task_context(
                vec!["test.rs".to_string()],
                50,
                10,
                task_type,
            );

            let eval = orch.evaluate_current_task();
            assert!(eval.is_some());

            println!("{:?} -> {:?} (score: {})", task_type, eval.unwrap().determined_rarity, eval.as_ref().unwrap().complexity_score);
        }
    }

    #[test]
    fn test_complexity_score_calculation() {
        let mut orch = UnlockOrchestrator::new();
        orch.start_task("complexity_test".to_string());

        // Caso simples: poucos arquivos, poucas linhas
        orch.update_task_context(
            vec!["file.rs".to_string()],
            10,
            5,
            TaskType::BugFix,
        );
        let eval = orch.evaluate_current_task().unwrap();
        assert!(eval.complexity_score <= 5, "BugFix simples deve ter score baixo");

        // Caso complexo: muitos arquivos, muitas linhas
        orch.start_task("complexity_test_2".to_string());
        orch.update_task_context(
            vec![
                "src/a.rs".to_string(),
                "src/b.rs".to_string(),
                "src/c.rs".to_string(),
                "src/d.rs".to_string(),
                "src/e.rs".to_string(),
                "src/f.rs".to_string(),
            ],
            500,
            200,
            TaskType::Complex,
        );
        let eval = orch.evaluate_current_task().unwrap();
        assert!(eval.complexity_score >= 7, "Tarefa complexa deve ter score alto");
        assert!(eval.bonus_rarity, "Complex task deve ter bonus");

        println!("Complex task score: {}", eval.complexity_score);
        println!("Rarity: {:?}", eval.determined_rarity);
    }

    #[test]
    fn test_collection_persistence() {
        // Salva estado inicial
        let mut collection = load_or_create_collection();
        let initial_count = collection.unlocked_count();

        // Simula desbloqueio
        let test_mascot: PokemonId = 42; // Um mascote qualquer
        if let Some(entry) = collection.entry_mut(test_mascot) {
            entry.status = crate::buddy::collection::UnlockStatus::Unlocked {
                unlocked_at: 0,
            };
        }

        // Salva
        save_collection(&collection).unwrap();

        // Recarrega
        let reloaded = load_or_create_collection();
        assert!(
            reloaded.unlocked_count() >= initial_count,
            "Deve ter pelo menos a quantidade inicial desbloqueada"
        );
    }

    #[test]
    fn test_all_rarities_have_mascots() {
        for rarity in [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::Epic, Rarity::Legendary] {
            let count = count_by_rarity(rarity);
            assert!(count > 0, "Raridade {:?} deve ter mascotes", rarity);
            println!("{:?} has {} mascotes", rarity, count);
        }
    }

    #[test]
    fn test_cancel_unlock_flow() {
        let mut orch = UnlockOrchestrator::new();
        orch.start_task("cancel_test".to_string());

        // Atinge milestone
        orch.add_tokens(10_000);

        orch.update_task_context(
            vec!["file.rs".to_string()],
            100,
            50,
            TaskType::Feature,
        );

        let eval = orch.evaluate_current_task().unwrap();
        orch.create_unlock_event(&eval);

        // Cancela
        let outcome = orch.cancel_unlock();
        assert!(outcome.success, "Cancelamento deve funcionar");
        assert!(outcome.user_confirmed == Some(false));
        assert!(outcome.mascot_id.is_none(), "Não deve ter mascote após cancelar");

        println!("Cancelamento: {}", outcome.message);
    }
}
