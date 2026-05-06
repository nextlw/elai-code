//! Sistema de Orchestration de Companheiro
//!
//! Conecta o tracker de tarefas, eventos de unlock, avaliação de complexidade,
//! e a coleção para criar o fluxo completo de desbloqueio.
//!
//! ## Fluxo de Integração
//!
//! ```ignore
//! 1. TaskManager.start_task() — inicia tracking
//! 2. TaskManager.add_tokens() — adiciona gastos
//! 3. TaskManager.evaluate_and_trigger() — avalia complexidade
//! 4. UnlockOrchestrator.create_event() — cria evento de unlock
//! 5. UnlockOrchestrator.roll_and_reveal() — sorteia mascote
//! 6. Collection.add_unlocked() — adiciona à coleção
//! ```

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use super::types::{PokemonId, POKEMON_COUNT, Rarity};

pub use super::collection::{
    CollectionEntry, UnlockStatus, UserCollection,
    load_or_create_collection, save_collection, rarity_for_pokemon,
};
pub use super::unlock_event::{
    ComplexityEvaluation, MilestoneConfig, MilestoneReached, TaskContext, TaskTracker,
    TaskType, UnlockEvent, MILESTONE_CONFIGS,
};

/// Resultado da operação de unlock
#[derive(Debug, Clone)]
pub struct UnlockOutcome {
    /// Se foi bem sucedido
    pub success: bool,
    /// Mascote desbloqueado
    pub mascot_id: Option<PokemonId>,
    /// Raridade
    pub rarity: Rarity,
    /// Avaliação de complexidade
    pub evaluation: Option<ComplexityEvaluation>,
    /// Mensagem para exibir
    pub message: String,
    /// Se o evento foi confirmado pelo usuário
    pub user_confirmed: Option<bool>,
}

/// Orchestrador de unlock que conecta todos os componentes
pub struct UnlockOrchestrator {
    pub tracker: TaskTracker,
    pub collection: UserCollection,
    /// Eventos pendentes
    pending_events: Vec<UnlockEvent>,
}

impl UnlockOrchestrator {
    /// Cria novo orchestrador
    pub fn new() -> Self {
        Self {
            tracker: TaskTracker::new(),
            collection: load_or_create_collection(),
            pending_events: Vec::new(),
        }
    }

    /// Carrega de collections persistence
    pub fn from_collection(collection: UserCollection) -> Self {
        Self {
            tracker: TaskTracker::new(),
            collection,
            pending_events: Vec::new(),
        }
    }

    /// Inicia uma nova tarefa para tracking
    pub fn start_task(&mut self, task_id: String) {
        self.tracker.start_task(task_id);
    }

    /// Adiciona tokens gastos e verifica milestones
    /// Retorna Some(MilestoneReached) se um milestone foi atingido
    pub fn add_tokens(&mut self, tokens: u64) -> Option<MilestoneReached> {
        self.tracker.add_tokens(tokens)
    }

    /// Atualiza contexto da tarefa para avaliação posterior
    pub fn update_task_context(
        &mut self,
        files: Vec<String>,
        lines_added: u64,
        lines_removed: u64,
        task_type: TaskType,
    ) {
        self.tracker.update_task_context(files, lines_added, lines_removed, task_type);
    }

    /// Avalia a complexidade da tarefa atual
    pub fn evaluate_current_task(&self) -> Option<ComplexityEvaluation> {
        let task = self.tracker.current_task()?;
        let total_lines = task.lines_added + task.lines_removed;
        
        // Simula avaliação (em produção, isso seria feito pelo modelo LLM)
        Some(evaluate_task_complexity(
            task,
            total_lines,
            task.files_modified.len(),
            false, // has_tests
            false, // is_recursive
        ))
    }

    /// Cria um evento de unlock baseado na avaliação
    pub fn create_unlock_event(&mut self, evaluation: &ComplexityEvaluation) -> Option<UnlockEvent> {
        let task_id = self.tracker.current_task()
            .map(|t| t.task_id.clone())
            .unwrap_or_else(|| format!("task_{}", now_unix_secs()));

        let mut event = UnlockEvent::new(task_id, evaluation.clone(), &self.collection);
        
        // Tenta sortear mascote
        event.roll_mascot();
        
        // Se tem mascote escolhido, salva o evento
        if event.chosen_mascot.is_some() {
            self.pending_events.push(event.clone());
            Some(event)
        } else {
            None
        }
    }

    /// Confirma o evento de unlock mais recente
    pub fn confirm_unlock(&mut self) -> UnlockOutcome {
        if let Some(event) = self.pending_events.pop() {
            if let Some(mascot_id) = event.chosen_mascot {
                // Adiciona à coleção
                let tokens_now = self.collection.total_tokens_spent();
                if let Some(entry) = self.collection.entry_mut(mascot_id) {
                    entry.status = UnlockStatus::Unlocked {
                        unlocked_at: now_unix_secs(),
                    };
                    entry.tokens_at_unlock = tokens_now;
                }

                // Salva coleção
                if let Err(e) = save_collection(&self.collection) {
                    return UnlockOutcome {
                        success: false,
                        mascot_id: None,
                        rarity: event.rarity,
                        evaluation: Some(event.evaluation),
                        message: format!("Erro ao salvar coleção: {}", e),
                        user_confirmed: Some(true),
                    };
                }

                return UnlockOutcome {
                    success: true,
                    mascot_id: Some(mascot_id),
                    rarity: event.rarity,
                    evaluation: Some(event.evaluation),
                    message: format!(
                        "✨ #{} {} desbloqueado! ({})",
                        mascot_id,
                        pokemon_name(mascot_id),
                        event.rarity.as_str()
                    ),
                    user_confirmed: Some(true),
                };
            }
        }

        UnlockOutcome {
            success: false,
            mascot_id: None,
            rarity: Rarity::Common,
            evaluation: None,
            message: "Nenhum evento pendente".to_string(),
            user_confirmed: None,
        }
    }

    /// Cancela o evento de unlock mais recente
    pub fn cancel_unlock(&mut self) -> UnlockOutcome {
        if let Some(event) = self.pending_events.pop() {
            return UnlockOutcome {
                success: true,
                mascot_id: None,
                rarity: event.rarity,
                evaluation: Some(event.evaluation),
                message: "Evento recusado. Tente alcançar mais milestones!".to_string(),
                user_confirmed: Some(false),
            };
        }

        UnlockOutcome {
            success: false,
            mascot_id: None,
            rarity: Rarity::Common,
            evaluation: None,
            message: "Nenhum evento pendente para cancelar".to_string(),
            user_confirmed: None,
        }
    }

    /// Retorna evento pendente atual
    pub fn current_pending_event(&self) -> Option<&UnlockEvent> {
        self.pending_events.last()
    }

    /// Verifica se há eventos pendentes
    pub fn has_pending_event(&self) -> bool {
        !self.pending_events.is_empty()
    }

    /// Termina a tarefa atual
    pub fn end_task(&mut self) -> Option<TaskContext> {
        self.tracker.end_task()
    }

    /// Salva o estado atual da coleção
    pub fn save_collection(&self) -> std::io::Result<()> {
        save_collection(&self.collection)
    }

    /// Retorna relatório da coleção
    pub fn collection_report(&self) -> String {
        self.collection.collection_report()
    }
}

impl Default for UnlockOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────────

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Determina raridade por PokemonId
fn pokemon_name(id: PokemonId) -> &'static str {
    super::types::pokemon_name(id)
}

/// Avalia complexidade da tarefa (versão simplificada)
fn evaluate_task_complexity(
    task: &TaskContext,
    lines_changed: u64,
    files_count: usize,
    has_tests: bool,
    is_recursive: bool,
) -> ComplexityEvaluation {
    use super::unlock_event::evaluate_task_complexity;
    
    evaluate_task_complexity(task, lines_changed, files_count, has_tests, is_recursive)
}

/// Score de probabilidade baseado na raridade
fn probability_for_rarity(rarity: Rarity) -> f64 {
    match rarity {
        Rarity::Common => 0.80,
        Rarity::Uncommon => 0.60,
        Rarity::Rare => 0.40,
        Rarity::Epic => 0.25,
        Rarity::Legendary => 0.10,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_flow() {
        let mut orch = UnlockOrchestrator::new();
        
        // Inicia tarefa
        orch.start_task("test_task_1".to_string());
        
        // Adiciona tokens
        let milestone = orch.add_tokens(10_000);
        assert!(milestone.is_some());
        
        // Atualiza contexto
        orch.update_task_context(
            vec!["src/main.rs".to_string()],
            100,
            50,
            TaskType::Feature,
        );
        
        // Avalia
        let eval = orch.evaluate_current_task();
        assert!(eval.is_some());
        let eval = eval.unwrap();
        println!("Complexity score: {}", eval.complexity_score);
        println!("Determined rarity: {:?}", eval.determined_rarity);
        
        // Cria evento
        let event = orch.create_unlock_event(&eval);
        assert!(event.is_some());
        
        // Confirma
        let outcome = orch.confirm_unlock();
        assert!(outcome.success);
        println!("{}", outcome.message);
    }

    #[test]
    fn test_milestone_detection() {
        let mut orch = UnlockOrchestrator::new();
        orch.start_task("milestone_test".to_string());
        
        // Não deve trigger até 10k
        assert!(orch.add_tokens(5_000).is_none());
        
        // 10k milestone
        let milestone = orch.add_tokens(5_000);
        assert!(milestone.is_some());
        assert_eq!(milestone.unwrap().tokens_threshold, 10_000);
        
        // Não deve trigger de novo o mesmo milestone
        assert!(orch.add_tokens(1).is_none());
    }
}
