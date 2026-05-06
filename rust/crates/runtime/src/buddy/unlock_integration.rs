//! Integração do Sistema de Unlock com o Runtime
//!
//! Este módulo conecta o UnlockOrchestrator ao ConversationRuntime,
//! disparando eventos de desbloqueio quando milestones são atingidos.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use super::types::{PokemonId, Rarity};
use super::collection::{load_or_create_collection, save_collection, UserCollection, UnlockStatus, CollectionEntry};
use super::orchestrator::{UnlockOrchestrator, UnlockOutcome};
use super::unlock_event::{TaskContext, TaskType, MilestoneReached, ComplexityEvaluation, MILESTONE_CONFIGS};

/// Estado global do sistema de unlock
/// Usado pelo runtime para trackear tokens e disparar eventos
pub struct UnlockIntegration {
    /// Orchestrator ativo
    orchestrator: UnlockOrchestrator,
    /// Callbacks para notificação
    notify_fn: Option<Arc<dyn Fn(String) + Send + Sync>>,
    /// Habilitado?
    enabled: bool,
}

impl UnlockIntegration {
    /// Cria nova integração
    pub fn new() -> Self {
        Self {
            orchestrator: UnlockOrchestrator::new(),
            notify_fn: None,
            enabled: true,
        }
    }

    /// Cria com collection customizada
    pub fn with_collection(collection: UserCollection) -> Self {
        Self {
            orchestrator: UnlockOrchestrator::from_collection(collection),
            notify_fn: None,
            enabled: true,
        }
    }

    /// Seta callback de notificação
    pub fn with_notify(mut self, f: impl Fn(String) + Send + Sync + 'static) -> Self {
        self.notify_fn = Some(Arc::new(f));
        self
    }

    /// Desabilita o sistema
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Habilita o sistema
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    fn notify(&self, msg: String) {
        if let Some(f) = &self.notify_fn {
            f(msg);
        }
    }

    /// Inicia uma nova tarefa para tracking
    pub fn start_task(&mut self, task_id: String) {
        if self.enabled {
            self.orchestrator.start_task(task_id);
            self.notify(format!("[unlock] Task iniciada: tracking tokens"));
        }
    }

    /// Atualiza contexto da tarefa (deve ser chamado periodicamente)
    pub fn update_task_context(
        &mut self,
        files: Vec<String>,
        lines_added: u64,
        lines_removed: u64,
        task_type: TaskType,
    ) {
        if self.enabled {
            self.orchestrator.update_task_context(files, lines_added, lines_removed, task_type);
        }
    }

    /// Adiciona tokens gastos e verifica se milestone foi atingido
    /// Retorna Some(MilestoneReached) se um milestone foi atingido
    pub fn add_tokens(&mut self, total_tokens: u64) -> Option<MilestoneReached> {
        if !self.enabled {
            return None;
        }

        let milestone = self.orchestrator.add_tokens(total_tokens);
        
        if let Some(m) = milestone {
            self.notify(format!(
                "[unlock] 🎯 Milestone atingido: {} tokens ({:?})",
                m.tokens_threshold,
                m.rarity
            ));
        }
        
        milestone
    }

    /// Verifica se há desbloqueio pendente
    pub fn has_pending_unlock(&self) -> bool {
        self.orchestrator.has_pending_event()
    }

    /// Retorna o próximo mascote sorteado (se pendente)
    pub fn pending_mascot(&self) -> Option<(PokemonId, Rarity)> {
        self.orchestrator
            .current_pending_event()
            .and_then(|e| e.chosen_mascot.map(|id| (id, e.rarity)))
    }

    /// Confirma o unlock pendente
    pub fn confirm_unlock(&mut self) -> UnlockOutcome {
        let outcome = self.orchestrator.confirm_unlock();
        if outcome.success {
            if let Some(id) = outcome.mascot_id {
                self.notify(format!(
                    "[unlock] ✨ #{} {} adicionado à coleção!",
                    id,
                    super::types::pokemon_name(id)
                ));
            }
        }
        outcome
    }

    /// Cancela o unlock pendente
    pub fn cancel_unlock(&mut self) -> UnlockOutcome {
        let outcome = self.orchestrator.cancel_unlock();
        self.notify(format!(
            "[unlock] Unlock cancelado pelo usuário: {}",
            outcome.message
        ));
        outcome
    }

    /// Encerra a tarefa atual
    pub fn end_task(&mut self) {
        if let Some(task) = self.orchestrator.end_task() {
            self.notify(format!(
                "[unlock] Task finalizada: {} tokens gastos",
                task.tokens_spent
            ));
        }
    }

    /// Retorna relatório da coleção
    pub fn collection_report(&self) -> String {
        self.orchestrator.collection_report()
    }

    /// Retorna o orchestrator para uso interno
    pub fn orchestrator(&self) -> &UnlockOrchestrator {
        &self.orchestrator
    }

    /// Retorna mutable do orchestrator
    pub fn orchestrator_mut(&mut self) -> &mut UnlockOrchestrator {
        &mut self.orchestrator
    }

    /// Avalia a complexidade da tarefa atual
    pub fn evaluate_current_task(&self) -> Option<ComplexityEvaluation> {
        self.orchestrator.evaluate_current_task()
    }
}

impl Default for UnlockIntegration {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helper para adicionar ao UsageTracker ────────────────────────────────────

use super::super::usage::TokenUsage;

impl UnlockIntegration {
    /// Processa usage de uma chamada de API
    /// Verifica se milestone foi atingido e retorna resultado
    pub fn process_usage(&mut self, usage: &TokenUsage) -> Option<MilestoneReached> {
        let total = usage.total_tokens() as u64;
        self.add_tokens(total)
    }

    /// Adiciona tokens de forma cumulativa (para tracking real)
    pub fn add_cumulative_tokens(&mut self, tokens: u64) -> Option<MilestoneReached> {
        if !self.enabled {
            return None;
        }

        // Adiciona via método público (campo é privado).
        let _newly_unlocked = self.orchestrator.collection.register_token_spent(tokens);
        let total = self.orchestrator.collection.total_tokens_spent();

        // Verifica milestones
        for config in MILESTONE_CONFIGS {
            if total >= config.tokens {
                // Verifica se já desbloqueou alguém dessa raridade recentemente
                // Para simplificar, apenas trigger o evento
                let milestone = MilestoneReached {
                    tokens_threshold: config.tokens,
                    rarity: config.rarity,
                    probability: config.base_probability,
                    reached_at: now_unix_secs(),
                };
                
                self.notify(format!(
                    "[unlock] 🎯 Milestone {} tokens atingido!",
                    config.tokens
                ));
                
                return Some(milestone);
            }
        }
        
        None
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ── Trait para fácil integração ───────────────────────────────────────────────

/// Trait para adicionar funcionalidade de unlock a qualquer struct
pub trait WithUnlock {
    fn unlock_integration(&self) -> Option<&UnlockIntegration>;
    fn unlock_integration_mut(&mut self) -> Option<&mut UnlockIntegration>;
}

impl WithUnlock for UnlockIntegration {
    fn unlock_integration(&self) -> Option<&UnlockIntegration> {
        Some(self)
    }
    
    fn unlock_integration_mut(&mut self) -> Option<&mut UnlockIntegration> {
        Some(self)
    }
}
