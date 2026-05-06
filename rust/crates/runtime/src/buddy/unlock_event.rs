//! Sistema de Evento de Desbloqueio de Companheiro
//!
//! ## Fluxo Completo
//!
//! 1. Usuário inicia tarefa → `TaskContext` criado
//! 2. Tokens gastos são trackeados na tarefa
//! 3. Quando tokens >= milestone threshold, task é PAUSADA
//! 4. Modelo avalia COMPLEXIDADE da tarefa:
//!    - Linhas de código alteradas
//!    - Arquivos modificados
//!    - Tipos de operações (debug, feature, refactor)
//!    - Tempo estimado
//! 5. Baseado na avaliação → determina RARIDADE
//! 6. Sorteia mascote da raridade
//! 7. Mostra EVENTO DE DESBLOQUEIO na tela
//! 8. Usuário confirma ou cancela
//! 9. Task RESUMIDA com novo companheiro
//!
//! ## Thresholds por Complexidade
//!
//! | Tokens Gastos | Raridade Avaliada | Probabilidade |
//! |--------------|-------------------|--------------|
//! | 0 - 10k      | Common            | 80%          |
//! | 10k - 50k    | Common            | 70%          |
//! | 50k - 150k   | Uncommon          | 60%          |
//! | 150k - 500k  | Rare              | 50%          |
//! | 500k - 1M    | Epic              | 40%          |
//! | 1M+          | Legendary         | 30%          |



use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::types::{PokemonId, Rarity};

// ── Task Progress ──────────────────────────────────────────────────────────────

/// Contexto de progresso de uma tarefa específica
#[derive(Debug, Clone)]
pub struct TaskContext {
    pub task_id: String,
    pub tokens_spent: u64,
    pub files_modified: Vec<String>,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub task_type: TaskType,
    /// Raridade atual - determinada após avaliação
    pub evaluated_rarity: Option<Rarity>,
    /// Se evento de unlock está pendente
    pub unlock_pending: bool,
    /// Timestamp de início
    pub started_at: u64,
}

/// Tipo de tarefa para avaliação de complexidade
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskType {
    /// Correção de bug simples
    BugFix,
    /// Feature pequena/média
    Feature,
    /// Refactoring
    Refactor,
    /// Investigação/debugging
    Investigation,
    /// Tarefa complexa multi-arquivo
    Complex,
    /// Documentação
    Documentation,
    /// Configuração/setup
    Setup,
    /// Outra
    Other,
}

impl TaskType {
    #[must_use] 
    pub fn base_rarity(&self) -> Rarity {
        // Group TaskTypes by their resulting rarity
        // Different variants may map to the same rarity — that's intentional for metrics
        match self {
            TaskType::BugFix | TaskType::Documentation | TaskType::Setup | TaskType::Other => Rarity::Common,
            TaskType::Feature | TaskType::Refactor => Rarity::Uncommon,
            TaskType::Investigation => Rarity::Rare,
            TaskType::Complex => Rarity::Epic,
        }
    }
}

/// Avaliação de complexidade feita pelo modelo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityEvaluation {
    /// Score 1-10 de complexidade geral
    pub complexity_score: u8,
    /// Tipo de tarefa
    pub task_type: TaskType,
    /// Razão da avaliação
    pub reasoning: String,
    /// Raridade determinada
    pub determined_rarity: Rarity,
    /// Bônus por achievements especiais
    pub bonus_rarity: bool,
}

/// Milestone de tokens atingido
#[derive(Debug, Clone, Copy)]
pub struct MilestoneReached {
    pub tokens_threshold: u64,
    pub rarity: Rarity,
    pub probability: f64,
    pub reached_at: u64,
}

// ── Milestones ────────────────────────────────────────────────────────────────

/// Thresholds de milestones e probabilidades
#[derive(Debug, Clone, Copy)]
pub struct MilestoneConfig {
    pub tokens: u64,
    pub rarity: Rarity,
    pub base_probability: f64,
}

pub const MILESTONE_CONFIGS: &[MilestoneConfig] = &[
    MilestoneConfig { tokens: 10_000, rarity: Rarity::Common, base_probability: 0.80 },
    MilestoneConfig { tokens: 50_000, rarity: Rarity::Common, base_probability: 0.70 },
    MilestoneConfig { tokens: 150_000, rarity: Rarity::Uncommon, base_probability: 0.60 },
    MilestoneConfig { tokens: 500_000, rarity: Rarity::Rare, base_probability: 0.50 },
    MilestoneConfig { tokens: 1_000_000, rarity: Rarity::Epic, base_probability: 0.40 },
    MilestoneConfig { tokens: 5_000_000, rarity: Rarity::Legendary, base_probability: 0.30 },
];

impl MilestoneConfig {
    #[must_use] 
    pub fn probability_with_complexity(&self, complexity: u8) -> f64 {
        // Maior complexidade = maior chance
        let complexity_bonus = (f64::from(complexity) - 1.0) / 9.0 * 0.2; // +0-20%
        (self.base_probability + complexity_bonus).min(1.0)
    }
}

// ── Unlock Event ──────────────────────────────────────────────────────────────

/// Evento de desbloqueio pendente
#[derive(Debug, Clone)]
pub struct UnlockEvent {
    pub task_id: String,
    pub rarity: Rarity,
    pub evaluation: ComplexityEvaluation,
    pub available_mascots: Vec<PokemonId>,
    pub chosen_mascot: Option<PokemonId>,
    pub created_at: u64,
}

impl UnlockEvent {
    #[must_use] 
    pub fn new(
        task_id: String,
        evaluation: ComplexityEvaluation,
        collection: &UserCollection,
    ) -> Self {
        let available = collection
            .locked()
            .into_iter()
            .filter(|id| {
                let rarity = rarity_for_pokemon(*id);
                rarity == evaluation.determined_rarity
            })
            .collect();

        Self {
            task_id,
            rarity: evaluation.determined_rarity,
            evaluation,
            available_mascots: available,
            chosen_mascot: None,
            created_at: now_unix_secs(),
        }
    }

    /// Sorteia mascote aleatório da raridade disponível
    pub fn roll_mascot(&mut self) -> Option<PokemonId> {
        if self.available_mascots.is_empty() {
            None
        } else {
            let idx = fastrand::usize(0..self.available_mascots.len());
            let id = self.available_mascots[idx];
            self.chosen_mascot = Some(id);
            Some(id)
        }
    }
}

// ── Global Task Tracker ─────────────────────────────────────────────────────

/// Estado global do tracker de tarefas
#[derive(Debug, Clone)]
pub struct TaskTracker {
    /// Tarefa atual em progresso
    current_task: Option<TaskContext>,
    /// Eventos de unlock pendentes
    pending_unlock: Option<UnlockEvent>,
    /// Histórico de milestones atingidos
    milestone_history: Vec<MilestoneReached>,
}

impl TaskTracker {
    #[must_use] 
    pub fn new() -> Self {
        Self {
            current_task: None,
            pending_unlock: None,
            milestone_history: Vec::new(),
        }
    }

    /// Inicia uma nova tarefa
    pub fn start_task(&mut self, task_id: String) {
        self.current_task = Some(TaskContext {
            task_id,
            tokens_spent: 0,
            files_modified: Vec::new(),
            lines_added: 0,
            lines_removed: 0,
            task_type: TaskType::Other,
            evaluated_rarity: None,
            unlock_pending: false,
            started_at: now_unix_secs(),
        });
    }

    /// Adiciona tokens gastos na tarefa atual
    /// Retorna Some(UnlockEvent) se desbloqueou milestone
    pub fn add_tokens(&mut self, tokens: u64) -> Option<MilestoneReached> {
        let task = self.current_task.as_mut()?;
        task.tokens_spent += tokens;

        // Verifica milestones
        for config in MILESTONE_CONFIGS {
            if task.tokens_spent >= config.tokens {
                // Verifica se já atingiu este milestone
                let already_reached = self.milestone_history.iter().any(|m| {
                    m.tokens_threshold == config.tokens
                });

                if !already_reached {
                    let milestone = MilestoneReached {
                        tokens_threshold: config.tokens,
                        rarity: config.rarity,
                        probability: config.base_probability,
                        reached_at: now_unix_secs(),
                    };
                    self.milestone_history.push(milestone);
                    return Some(milestone);
                }
            }
        }
        None
    }

    /// Atualiza contexto da tarefa com mais detalhes
    pub fn update_task_context(
        &mut self,
        files: Vec<String>,
        lines_added: u64,
        lines_removed: u64,
        task_type: TaskType,
    ) {
        if let Some(task) = &mut self.current_task {
            task.files_modified = files;
            task.lines_added = lines_added;
            task.lines_removed = lines_removed;
            task.task_type = task_type;
        }
    }

    /// Marca que desbloqueio está pendente
    pub fn set_unlock_pending(&mut self, pending: bool) {
        if let Some(task) = &mut self.current_task {
            task.unlock_pending = pending;
        }
    }

    /// Retorna tarefa atual
    #[must_use] 
    pub fn current_task(&self) -> Option<&TaskContext> {
        self.current_task.as_ref()
    }

    /// Verifica se há unlock pendente
    #[must_use] 
    pub fn has_pending_unlock(&self) -> bool {
        self.pending_unlock.is_some()
    }

    /// Define evento de unlock
    pub fn set_unlock_event(&mut self, event: UnlockEvent) {
        self.pending_unlock = Some(event);
        if let Some(task) = &mut self.current_task {
            task.unlock_pending = true;
        }
    }

    /// Retorna e consome evento de unlock
    pub fn take_unlock_event(&mut self) -> Option<UnlockEvent> {
        self.pending_unlock.take()
    }

    /// Encerra tarefa atual
    pub fn end_task(&mut self) -> Option<TaskContext> {
        self.current_task.take()
    }

    /// Retorna histórico de milestones
    #[must_use] 
    pub fn milestone_history(&self) -> &[MilestoneReached] {
        &self.milestone_history
    }
}

impl Default for TaskTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rarity by Task Complexity ───────────────────────────────────────────────

/// Avalia complexidade e determina raridade base
#[must_use] 
pub fn evaluate_task_complexity(
    task: &TaskContext,
    lines_changed: u64,
    files_count: usize,
    has_tests: bool,
    is_recursive: bool,
) -> ComplexityEvaluation {
    let mut complexity_score: u8 = 1;
    let mut bonus_rarity = false;
    let mut reasoning_parts = Vec::new();

    // Baseado no task_type
    complexity_score += match task.task_type {
        TaskType::BugFix | TaskType::Setup | TaskType::Other => 2,
        TaskType::Documentation => 1,
        TaskType::Refactor => 3,
        TaskType::Feature => 4,
        TaskType::Investigation => 5,
        TaskType::Complex => 7,
    };
    reasoning_parts.push(format!("task_type={:?}", task.task_type));

    // Baseado em linhas alteradas
    if lines_changed > 1000 {
        complexity_score += 3;
        reasoning_parts.push("1000+ linhas alteradas".to_string());
    } else if lines_changed > 500 {
        complexity_score += 2;
        reasoning_parts.push("500+ linhas alteradas".to_string());
    } else if lines_changed > 100 {
        complexity_score += 1;
        reasoning_parts.push("100+ linhas alteradas".to_string());
    }

    // Baseado em arquivos
    if files_count > 20 {
        complexity_score += 3;
        reasoning_parts.push("20+ arquivos".to_string());
    } else if files_count > 10 {
        complexity_score += 2;
        reasoning_parts.push("10+ arquivos".to_string());
    } else if files_count > 5 {
        complexity_score += 1;
        reasoning_parts.push("5+ arquivos".to_string());
    }

    // Bônus especiais
    if has_tests {
        complexity_score += 1;
        reasoning_parts.push("com testes".to_string());
    }
    if is_recursive {
        bonus_rarity = true;
        reasoning_parts.push("recursão detectada".to_string());
    }
    if task.task_type == TaskType::Complex {
        bonus_rarity = true;
        reasoning_parts.push("tarefa complexa".to_string());
    }

    // Clamp complexity
    complexity_score = complexity_score.min(10);

    // Determina raridade final
    let base_rarity = task.task_type.base_rarity();
    let rarity = if bonus_rarity {
        upgrade_rarity(base_rarity)
    } else {
        base_rarity
    };

    ComplexityEvaluation {
        complexity_score,
        task_type: task.task_type,
        reasoning: reasoning_parts.join(", "),
        determined_rarity: rarity,
        bonus_rarity,
    }
}

/// Faz upgrade de raridade (max is Legendary)
fn upgrade_rarity(rarity: Rarity) -> Rarity {
    match rarity {
        Rarity::Common => Rarity::Uncommon,
        Rarity::Uncommon => Rarity::Rare,
        Rarity::Rare => Rarity::Epic,
        Rarity::Epic | Rarity::Legendary => Rarity::Legendary,
    }
}

// ── Helper para Collection ─────────────────────────────────────────────────

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Determina raridade por `PokemonId`
#[must_use] 
#[expect(clippy::match_same_arms)]
pub fn rarity_for_pokemon(id: PokemonId) -> Rarity {
    // Ranges intentionally overlap for Common to bias toward more Common mascotes
    match id {
        1..=30 => Rarity::Common,
        31..=60 => Rarity::Uncommon,
        61..=80 => Rarity::Rare,
        81..=90 => Rarity::Epic,
        91..=140 => Rarity::Common,
        141..=151 => Rarity::Legendary,
        _ => Rarity::Common,
    }
}

// ── Re-export do collection.rs para compatibilidade ─────────────────────────

pub use super::collection::{
    CollectionEntry, UnlockStatus, UserCollection, load_or_create_collection, save_collection,
};
