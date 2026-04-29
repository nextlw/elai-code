//! Estimativa de COGS por documento — equivalente Rust do simulador Python.

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::patterns;
use crate::pdf::{self, PdfContent};
use crate::pricing::{ModelPrice, PriceTable};
use crate::tier::{self, Tier};

/// Custo de um estágio individual do pipeline de inferência.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageCost {
    pub name: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Relatório completo de unit economics para um documento.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocReport {
    pub path: String,
    pub pages: u32,
    pub words: u64,
    pub estimated_input_tokens: u64,
    pub specials_detected: Vec<String>,
    pub complexity_hits: u32,
    pub tier: Tier,
    pub recommended_model: String,
    pub stages: Vec<StageCost>,
    pub total_cogs_usd: f64,
    pub total_cogs_brl: f64,
    pub list_price_brl: f64,
    pub gross_margin_pct: f64,
}

/// Calcula custo de um estágio. Retorna erro se o modelo não estiver no
/// `pricing` (em vez de panic).
pub fn cost_for<S: BuildHasher>(
    name: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    pricing: &HashMap<String, ModelPrice, S>,
) -> Result<StageCost> {
    let p = pricing
        .get(model)
        .with_context(|| format!("modelo '{model}' ausente no pricing"))?;
    let in_cost = (input_tokens as f64 / 1_000_000.0) * p.input;
    let out_cost = (output_tokens as f64 / 1_000_000.0) * p.output;
    let cost_usd = round6(in_cost + out_cost);
    Ok(StageCost {
        name: name.to_string(),
        model: model.to_string(),
        input_tokens,
        output_tokens,
        cost_usd,
    })
}

/// Detecta categorias de carga especial. Usa apenas presença, não conta
/// frequência — a contagem fica para `count_complexity_hits`.
fn detect_specials(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (label, raw_patterns) in patterns::cargas_especiais() {
        for raw in *raw_patterns {
            // Compilação on-demand é aceitável aqui: <30 padrões totais por
            // documento; o ganho de cache não compensa a complexidade.
            if let Ok(re) = Regex::new(raw) {
                if re.is_match(text) {
                    found.push((*label).to_string());
                    break;
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

fn count_complexity_hits(text: &str) -> u32 {
    let mut hits: u32 = 0;
    for raw in patterns::complexity_patterns() {
        if let Ok(re) = Regex::new(raw) {
            hits = hits.saturating_add(u32::try_from(re.find_iter(text).count()).unwrap_or(u32::MAX));
        }
    }
    hits
}

/// Roteia o pipeline conforme o tier classificado e popula os estágios.
fn simulate_pipeline<S: BuildHasher>(
    report: &mut DocReport,
    pricing: &HashMap<String, ModelPrice, S>,
) -> Result<()> {
    let in_toks = report.estimated_input_tokens;

    let mut stages = vec![cost_for("jina_reader", "jina_reader", in_toks, 0, pricing)?];

    let recommended = match report.tier {
        Tier::Light => {
            let out = (in_toks as f64 * 0.25) as u64;
            stages.push(cost_for("extract+answer", "gpt_4o_mini", in_toks, out, pricing)?);
            "gpt_4o_mini".to_string()
        }
        Tier::Standard => {
            let cached_ratio = 0.50;
            let live_input = ((in_toks as f64) * (1.0 - cached_ratio)) as u64;
            let map_out = (in_toks as f64 * 0.10) as u64;
            let red_out = (live_input as f64 * 0.30) as u64;
            stages.push(cost_for("extract", "haiku_3_5", in_toks, map_out, pricing)?);
            stages.push(cost_for("reason", "sonnet_3_7", live_input, red_out, pricing)?);
            "haiku_3_5+sonnet_3_7".to_string()
        }
        Tier::Heavy => {
            let cached_ratio = 0.65;
            let live_input = ((in_toks as f64) * (1.0 - cached_ratio)) as u64;
            let map_out = (in_toks as f64 * 0.08) as u64;
            let red_out = (live_input as f64 * 0.35) as u64;
            stages.push(cost_for("map_extract", "haiku_3_5", in_toks, map_out, pricing)?);
            stages.push(cost_for(
                "reduce_reason",
                "sonnet_3_7",
                live_input,
                red_out,
                pricing,
            )?);
            "haiku_map+sonnet_reduce".to_string()
        }
    };

    let total_usd = round4(stages.iter().map(|s| s.cost_usd).sum::<f64>());
    report.stages = stages;
    report.total_cogs_usd = total_usd;
    report.recommended_model = recommended;
    Ok(())
}

/// Pipeline principal por documento.
pub fn analyze_document<S: BuildHasher>(
    path: &Path,
    pricing: &HashMap<String, ModelPrice, S>,
    fx: f64,
    table: PriceTable,
) -> Result<DocReport> {
    let PdfContent { text, pages } = pdf::read_pdf(path)?;
    let words = pdf::count_words(&text);
    let in_toks = pdf::estimate_input_tokens(words);
    let specials = detect_specials(&text);
    let complexity_hits = count_complexity_hits(&text);
    let tier = tier::classify(pages, specials.len(), complexity_hits);

    let list_price = match tier {
        Tier::Light => table.bid_light,
        Tier::Standard => table.bid_standard,
        Tier::Heavy => table.bid_heavy,
    };

    let mut report = DocReport {
        path: path.display().to_string(),
        pages,
        words: words as u64,
        estimated_input_tokens: in_toks,
        specials_detected: specials,
        complexity_hits,
        tier,
        recommended_model: String::new(),
        stages: Vec::new(),
        total_cogs_usd: 0.0,
        total_cogs_brl: 0.0,
        list_price_brl: list_price,
        gross_margin_pct: 0.0,
    };

    simulate_pipeline(&mut report, pricing)?;
    report.total_cogs_brl = round2(report.total_cogs_usd * fx);
    if report.list_price_brl > 0.0 {
        report.gross_margin_pct =
            round2((report.list_price_brl - report.total_cogs_brl) / report.list_price_brl * 100.0);
    }
    Ok(report)
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}
fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricing_default() -> HashMap<String, ModelPrice> {
        crate::pricing::default_pricing()
    }

    #[test]
    fn cost_for_known_model() {
        let pricing = pricing_default();
        let stage = cost_for("test", "gpt_4o_mini", 1_000_000, 1_000_000, &pricing)
            .expect("modelo conhecido deve calcular custo");
        // 0.15 + 0.60 = 0.75 USD para 1M input + 1M output
        assert!((stage.cost_usd - 0.75).abs() < 1e-9);
    }

    #[test]
    fn cost_for_unknown_model_errors() {
        let pricing = pricing_default();
        let err = cost_for("test", "modelo_inexistente", 100, 0, &pricing).unwrap_err();
        assert!(err.to_string().contains("modelo_inexistente"));
    }

    #[test]
    fn detect_specials_inflamavel_combustivel() {
        let text = "transporte de combustível e produto inflamável";
        let s = detect_specials(text);
        assert!(s.contains(&"inflamavel".to_string()));
    }

    #[test]
    fn complexity_counts_multiple() {
        // "edital" + "BID" + "trecho" + "contrato" → ≥4 hits
        let text = "este edital de BID descreve o trecho do contrato";
        assert!(count_complexity_hits(text) >= 4);
    }
}
