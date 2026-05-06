//! Pipeline de seleção de tools — 5 estágios por turno.
//!
//! O pipeline opera sobre `PipelineTool` (somente `name` + `index_in_original`),
//! pois o crate `runtime` não pode depender de `api` (dependência circular).
//! O chamador (`filter_tool_specs` em `elai-cli`) mapeia `ToolDefinition → PipelineTool`,
//! executa o pipeline e usa os nomes resultantes para reconstruir a lista filtrada.
//!
//! Ordem dos estágios:
//! 1. Context gating  — remove tools desabilitadas no catálogo
//! 2. Skill overrides — force-include (`requires_tools`) e force-exclude (`incompatible_with`)
//! 3. User filter     — --allowedTools (gate duro, sempre vence)
//! 4. Priority ranking — ordena por priority do catálogo (desc)
//! 5. Budget cap      — top-N respeitando `mcp_share` e `per_server_max`

use crate::tool_catalog::ToolCatalog;
use crate::skills::Skill;

/// Representação minimalista de uma tool no pipeline.
#[derive(Debug, Clone)]
pub struct PipelineTool {
    /// Nome canônico da tool.
    pub name: String,
}

/// Representa uma ferramenta que foi removida do snapshot com a razão.
#[derive(Debug, Clone)]
pub struct RejectedTool {
    pub id: String,
    pub reason: RejectionReason,
}

/// Motivo pelo qual uma tool foi removida do snapshot do turno.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    /// `enabled: false` no TOML ou gating de contexto.
    Disabled,
    /// Incompatível com a skill ativa (`incompatible_with`).
    SkillIncompatible(String),
    /// Não incluída pelo filtro de usuário (`--allowedTools`).
    UserFilter,
    /// Cortada pelo budget cap (top-N).
    BudgetCap,
    // TODO: ContextGate(String) — ENV/file gating, implementar em fase futura.
}

/// Resultado do pipeline: snapshot congelado + lista de rejeitadas.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Nomes das tools aceitas, em ordem de prioridade (desc).
    pub tool_names: Vec<String>,
    pub rejected: Vec<RejectedTool>,
}

/// Configuração de budget de tools para o estágio 5 do pipeline de seleção.
///
/// Renomeado para `ToolBudgetConfig` para evitar conflito com `runtime::ToolBudgetConfig`
/// (budget de tokens/custo USD do módulo `budget`).
#[derive(Debug, Clone)]
pub struct ToolBudgetConfig {
    /// Número máximo de tools no snapshot final.
    pub global_max: usize,
    /// Máximo de tools por servidor MCP.
    pub per_server_max: usize,
    /// Fração do `global_max` reservada para tools MCP.
    pub mcp_share: f32,
}

impl Default for ToolBudgetConfig {
    fn default() -> Self {
        Self {
            global_max: 24,
            per_server_max: 12,
            mcp_share: 0.6,
        }
    }
}

/// Conjunto de IDs que são imunes ao budget cap (`requires_tools` da skill ativa).
#[derive(Debug, Default, Clone)]
struct ImmuneSet {
    ids: Vec<String>,
}

impl ImmuneSet {
    fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|s| s == id)
    }
}

// ─── Estágios ────────────────────────────────────────────────────────────────

/// Estágio 1: remove tools com `enabled: false` no catálogo.
///
/// TODO: implementar `ContextGate` para ENV/file gating em fase futura.
fn stage_context_gate(
    kept: &mut Vec<PipelineTool>,
    rejected: &mut Vec<RejectedTool>,
    catalog: &ToolCatalog,
) {
    kept.retain(|tool| {
        if catalog.enabled(&tool.name) {
            true
        } else {
            rejected.push(RejectedTool {
                id: tool.name.clone(),
                reason: RejectionReason::Disabled,
            });
            false
        }
    });
}

/// Estágio 2: aplica overrides da skill ativa.
///
/// - `incompatible_with` → exclui a tool (mesmo que o user filter depois a permitisse)
/// - `requires_tools` → marca a tool como imune ao budget cap
fn stage_skill_overrides(
    kept: &mut Vec<PipelineTool>,
    rejected: &mut Vec<RejectedTool>,
    active_skill: Option<&Skill>,
    immune: &mut ImmuneSet,
) {
    let Some(skill) = active_skill else {
        return;
    };
    let incompatible = &skill.metadata.incompatible_with;
    let requires = &skill.metadata.requires_tools;

    // Marcar imunes antes de qualquer exclusão.
    for req in requires {
        if !immune.ids.contains(req) {
            immune.ids.push(req.clone());
        }
    }

    let skill_name = skill.metadata.name.clone();
    kept.retain(|tool| {
        if incompatible.iter().any(|id| id == &tool.name) {
            rejected.push(RejectedTool {
                id: tool.name.clone(),
                reason: RejectionReason::SkillIncompatible(skill_name.clone()),
            });
            false
        } else {
            true
        }
    });
}

/// Padrão de matching para filtro de tools.
///
/// Separado de `tools::MatcherPattern` para evitar dependência circular.
/// O chamador converte `tools::MatcherPattern` → `FilterPattern` antes de chamar o pipeline.
#[derive(Debug, Clone)]
pub enum FilterPattern {
    Exact(String),
    Prefix(String),
}

impl FilterPattern {
    #[must_use] 
    pub fn matches(&self, name: &str) -> bool {
        match self {
            Self::Exact(s) => s == name,
            Self::Prefix(p) => name.starts_with(p.as_str()),
        }
    }
}

/// Estágio 3: filtra pelas patterns de `--allowedTools` (gate duro, sempre vence).
///
/// O user filter é um gate absoluto: nenhuma tool passa sem match, nem as marcadas
/// como imunes pela skill. Imunidade (`requires_tools`) só protege contra o budget cap
/// no estágio 5 — o usuário sempre tem a última palavra.
fn stage_user_filter(
    kept: &mut Vec<PipelineTool>,
    rejected: &mut Vec<RejectedTool>,
    allowed: Option<&[FilterPattern]>,
) {
    let Some(patterns) = allowed else {
        return; // sem filtro — tudo passa
    };
    kept.retain(|tool| {
        if patterns.iter().any(|p| p.matches(&tool.name)) {
            true
        } else {
            rejected.push(RejectedTool {
                id: tool.name.clone(),
                reason: RejectionReason::UserFilter,
            });
            false
        }
    });
}

/// Estágio 4: ordena por priority do catálogo (descrescente).
///
/// Tools em `requires_tools` da skill ativa recebem +100 (`skill_boost`).
fn stage_priority_ranking(
    kept: &mut [PipelineTool],
    catalog: &ToolCatalog,
    active_skill: Option<&Skill>,
) {
    let requires: &[String] = active_skill
        .map_or(&[], |s| s.metadata.requires_tools.as_slice());

    kept.sort_by(|a, b| {
        let priority_a = effective_priority(&a.name, catalog, requires);
        let priority_b = effective_priority(&b.name, catalog, requires);
        priority_b.cmp(&priority_a)
    });
}

fn effective_priority(name: &str, catalog: &ToolCatalog, requires: &[String]) -> i32 {
    let base = catalog.priority_for(name).unwrap_or(50);
    let boost = if requires.iter().any(|r| r == name) { 100 } else { 0 };
    base + boost
}

/// Estágio 5: aplica o budget cap — top-N respeitando `mcp_share` e `per_server_max`.
///
/// Tools imunes (`requires_tools`) entram antes do corte e nunca são descartadas.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn stage_budget_cap(
    kept: &mut Vec<PipelineTool>,
    rejected: &mut Vec<RejectedTool>,
    immune: &ImmuneSet,
    budget: &ToolBudgetConfig,
) {
    let mcp_slot = (budget.global_max as f32 * budget.mcp_share).floor() as usize;

    // Separar imunes de discricionárias.
    let (immune_tools, discretionary): (Vec<_>, Vec<_>) =
        kept.drain(..).partition(|t| immune.contains(&t.name));

    let (mcp_disc, non_mcp_disc): (Vec<_>, Vec<_>) = discretionary
        .into_iter()
        .partition(|t| t.name.starts_with("mcp__"));

    // Aplicar per_server_max: agrupar por servidor (segundo segmento de mcp__server__).
    let mut mcp_kept: Vec<PipelineTool> = Vec::new();
    let mut mcp_rejected_tools: Vec<PipelineTool> = Vec::new();
    {
        use std::collections::HashMap;
        // Manter ordem de chegada por servidor (já priorizada pelo estágio 4).
        let mut server_count: HashMap<String, usize> = HashMap::new();
        for tool in mcp_disc {
            let server = mcp_server_prefix(&tool.name);
            let count = server_count.entry(server).or_insert(0);
            if *count < budget.per_server_max {
                *count += 1;
                mcp_kept.push(tool);
            } else {
                mcp_rejected_tools.push(tool);
            }
        }
    }

    // Aplicar global mcp_slot: top-K por ordem da lista (já priorizada).
    if mcp_kept.len() > mcp_slot {
        let cut = mcp_kept.split_off(mcp_slot);
        mcp_rejected_tools.extend(cut);
    }

    // Non-MCP: slots restantes = global_max - slots usados por MCP.
    // Slots MCP não usados se tornam disponíveis para non-MCP.
    let actual_mcp_used = mcp_kept.len();
    let effective_non_mcp_slot = budget.global_max.saturating_sub(actual_mcp_used);
    let (non_mcp_kept, non_mcp_cut) = if non_mcp_disc.len() > effective_non_mcp_slot {
        let mut v = non_mcp_disc;
        let cut = v.split_off(effective_non_mcp_slot);
        (v, cut)
    } else {
        (non_mcp_disc, vec![])
    };

    // Imunes entram incondicionalmente; contam no global mas não são cortadas.
    kept.extend(immune_tools);
    kept.extend(mcp_kept);
    kept.extend(non_mcp_kept);

    for tool in mcp_rejected_tools.into_iter().chain(non_mcp_cut) {
        rejected.push(RejectedTool {
            id: tool.name,
            reason: RejectionReason::BudgetCap,
        });
    }
}

/// Extrai o prefixo de servidor de um nome MCP qualificado (`mcp__server__tool` → `server`).
fn mcp_server_prefix(name: &str) -> String {
    // Formato: "mcp__<server>__<tool>"
    name.split("__").nth(1).unwrap_or("").to_string()
}

// ─── Pipeline principal ───────────────────────────────────────────────────────

/// Executa os 5 estágios do pipeline. Deve ser chamado uma vez por turno.
///
/// `all_tools` é a lista completa de tools disponíveis (como nomes).
/// Retorna `PipelineResult` com os nomes aceitos em ordem de prioridade.
#[must_use] 
pub fn run_pipeline(
    all_tools: Vec<PipelineTool>,
    catalog: &ToolCatalog,
    allowed: Option<&[FilterPattern]>,
    active_skill: Option<&Skill>,
    budget: &ToolBudgetConfig,
) -> PipelineResult {
    let mut kept = all_tools;
    let mut rejected: Vec<RejectedTool> = Vec::new();
    let mut immune = ImmuneSet::default();

    // [1] Context gating
    stage_context_gate(&mut kept, &mut rejected, catalog);

    // [2] Skill overrides (popula immune antes do user filter)
    stage_skill_overrides(&mut kept, &mut rejected, active_skill, &mut immune);

    // [3] User filter — gate duro, sempre vence (imunes não têm exceção aqui)
    stage_user_filter(&mut kept, &mut rejected, allowed);

    // [4] Priority ranking
    stage_priority_ranking(&mut kept, catalog, active_skill);

    // [5] Budget cap
    stage_budget_cap(&mut kept, &mut rejected, &immune, budget);

    PipelineResult {
        tool_names: kept.into_iter().map(|t| t.name).collect(),
        rejected,
    }
}

// ─── Testes ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillMetadata;

    fn make_tool(name: &str) -> PipelineTool {
        PipelineTool { name: name.to_string() }
    }

    fn make_skill(requires: &[&str], incompatible: &[&str]) -> Skill {
        use std::path::PathBuf;
        Skill {
            metadata: SkillMetadata {
                name: "test_skill".to_string(),
                requires_tools: requires.iter().map(std::string::ToString::to_string).collect(),
                incompatible_with: incompatible.iter().map(std::string::ToString::to_string).collect(),
                ..SkillMetadata::default()
            },
            body: String::new(),
            file_path: PathBuf::from("test_skill.md"),
        }
    }

    #[test]
    fn user_filter_wins_over_skill_requires() {
        // skill requires_tools: ["read_file"]
        // allowed: ["edit_file"]
        // → read_file deve ser rejeitada com reason UserFilter
        let tools = vec![make_tool("read_file"), make_tool("edit_file")];
        let skill = make_skill(&["read_file"], &[]);
        let catalog = ToolCatalog::default();
        let allowed = vec![FilterPattern::Exact("edit_file".to_string())];
        let result = run_pipeline(tools, &catalog, Some(&allowed), Some(&skill), &ToolBudgetConfig::default());

        // read_file deve estar rejeitada com UserFilter
        let read_rejected = result
            .rejected
            .iter()
            .find(|r| r.id == "read_file")
            .expect("read_file deveria ter sido rejeitada");
        assert_eq!(read_rejected.reason, RejectionReason::UserFilter);

        // edit_file deve estar nas kept
        assert!(result.tool_names.contains(&"edit_file".to_string()));
        // read_file não deve estar nas kept
        assert!(!result.tool_names.contains(&"read_file".to_string()));
    }

    #[test]
    fn incompatible_with_excludes_even_if_user_allowed() {
        // skill incompatible_with: ["execute_bash"]
        // allowed: None (tudo permitido)
        // → execute_bash deve ser rejeitada com SkillIncompatible
        let tools = vec![make_tool("execute_bash"), make_tool("read_file")];
        let skill = make_skill(&[], &["execute_bash"]);
        let catalog = ToolCatalog::default();
        let result = run_pipeline(tools, &catalog, None, Some(&skill), &ToolBudgetConfig::default());

        let bash_rejected = result
            .rejected
            .iter()
            .find(|r| r.id == "execute_bash")
            .expect("execute_bash deveria ter sido rejeitada");
        assert!(matches!(
            bash_rejected.reason,
            RejectionReason::SkillIncompatible(_)
        ));
        assert!(!result.tool_names.contains(&"execute_bash".to_string()));
    }

    #[test]
    fn budget_cap_respects_mcp_share() {
        // 30 MCP tools + 5 builtin
        // ToolBudgetConfig { global_max: 20, per_server_max: 20, mcp_share: 0.6 }
        // → ≤12 MCP + ≤8 non-MCP = ≤20 total
        let mut tools: Vec<PipelineTool> = Vec::new();
        for i in 0..30 {
            tools.push(make_tool(&format!("mcp__server_a__tool_{i}")));
        }
        for i in 0..5 {
            tools.push(make_tool(&format!("builtin_{i}")));
        }

        let catalog = ToolCatalog::default();
        let budget = ToolBudgetConfig {
            global_max: 20,
            per_server_max: 20,
            mcp_share: 0.6,
        };
        let result = run_pipeline(tools, &catalog, None, None, &budget);

        let mcp_count = result.tool_names.iter().filter(|n| n.starts_with("mcp__")).count();
        let non_mcp_count = result.tool_names.iter().filter(|n| !n.starts_with("mcp__")).count();

        assert!(mcp_count <= 12, "MCP count {mcp_count} exceeds limit 12");
        assert!(non_mcp_count <= 8, "Non-MCP count {non_mcp_count} exceeds limit 8");
        assert!(result.tool_names.len() <= 20, "Total {} exceeds global_max 20", result.tool_names.len());
    }

    #[test]
    fn disabled_tool_is_rejected() {
        use crate::tool_catalog::loader::ToolOverride;
        let mut catalog = ToolCatalog::default();
        catalog.overrides.push(ToolOverride {
            id: "bash".to_string(),
            enabled: Some(false),
            priority: None,
            category: None,
            embedding_hints: None,
            rate_limit: None,
        });
        let tools = vec![make_tool("bash"), make_tool("read_file")];
        let result = run_pipeline(tools, &catalog, None, None, &ToolBudgetConfig::default());

        let bash_rejected = result
            .rejected
            .iter()
            .find(|r| r.id == "bash")
            .expect("bash deveria ter sido rejeitada");
        assert_eq!(bash_rejected.reason, RejectionReason::Disabled);
        assert!(!result.tool_names.contains(&"bash".to_string()));
        assert!(result.tool_names.contains(&"read_file".to_string()));
    }

    #[test]
    fn per_server_max_limits_mcp_tools() {
        let tools: Vec<PipelineTool> = (0..10)
            .map(|i| make_tool(&format!("mcp__github__tool_{i}")))
            .collect();
        let catalog = ToolCatalog::default();
        let budget = ToolBudgetConfig {
            global_max: 24,
            per_server_max: 3,
            mcp_share: 0.9,
        };
        let result = run_pipeline(tools, &catalog, None, None, &budget);
        let github_count = result.tool_names.iter().filter(|n| n.starts_with("mcp__github__")).count();
        assert!(github_count <= 3, "github tool count {github_count} exceeds per_server_max 3");
    }
}
