pub mod loader;
pub mod pipeline;
pub mod rate_limit;
pub use loader::ToolCatalog;
pub use pipeline::{
    run_pipeline, FilterPattern, PipelineResult, PipelineTool, RejectedTool, RejectionReason,
    ToolBudgetConfig,
};
pub use rate_limit::{check_rate_limit, init_rate_limiter, RateLimit, RateLimiter};

use std::sync::Mutex;

/// Snapshot congelado do turno atual.
///
/// Evita dessync se um MCP server respawnar mid-turn: o pipeline roda uma vez no início
/// do turno e o resultado é frozen até o próximo user message.
pub struct TurnToolSnapshot {
    pub result: PipelineResult,
}

static LAST_TURN_SNAPSHOT: Mutex<Option<TurnToolSnapshot>> = Mutex::new(None);

/// Persiste o resultado do pipeline como snapshot do turno atual.
pub fn set_turn_snapshot(result: PipelineResult) {
    *LAST_TURN_SNAPSHOT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(TurnToolSnapshot { result });
}

/// Retorna a lista de tools rejeitadas no último turno, ou vazio se não houver snapshot.
pub fn last_rejected() -> Vec<RejectedTool> {
    LAST_TURN_SNAPSHOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .map(|s| s.result.rejected.clone())
        .unwrap_or_default()
}
