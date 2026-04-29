//! `lauto-unit-econ` — simulador de COGS por documento.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use lauto_pricing::pricing::{self, ModelPrice, PriceTable, DEFAULT_FX};
use lauto_pricing::unit_econ::{analyze_document, DocReport};

#[derive(Debug, Parser)]
#[command(
    name = "lauto-unit-econ",
    about = "Simula COGS de cada PDF (BID/proposta/cotação) para a proposta Nokk × Lauto.\n\
             Substitui o protótipo Python. Modelos e preços ajustáveis via --config.",
    version,
)]
struct Cli {
    /// PDFs a analisar.
    #[arg(required = true)]
    pdfs: Vec<PathBuf>,
    /// Câmbio BRL/USD.
    #[arg(long, default_value_t = DEFAULT_FX)]
    fx: f64,
    /// JSON com override de pricing (mesma forma que `default_pricing`).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Caminho para gravar o relatório agregado em JSON.
    #[arg(long)]
    json: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut pricing_map = pricing::default_pricing();

    if let Some(config_path) = &cli.config {
        let raw = fs::read_to_string(config_path)
            .with_context(|| format!("falha ao ler {}", config_path.display()))?;
        let override_map: HashMap<String, ModelPrice> = serde_json::from_str(&raw)
            .with_context(|| format!("JSON inválido em {}", config_path.display()))?;
        for (k, v) in override_map {
            pricing_map.insert(k, v);
        }
    }

    let table = PriceTable::default();
    let mut reports: Vec<DocReport> = Vec::new();
    for pdf in &cli.pdfs {
        match analyze_document(pdf, &pricing_map, cli.fx, table) {
            Ok(report) => {
                println!("{}", render_human(&report));
                reports.push(report);
            }
            Err(err) => {
                eprintln!("⚠️  {}: {err:#}", pdf.display());
            }
        }
    }

    if let Some(out) = cli.json {
        if reports.is_empty() {
            eprintln!("nenhum relatório a gravar em {}", out.display());
        } else {
            let json = serde_json::to_string_pretty(&reports)?;
            fs::write(&out, json).with_context(|| format!("escrevendo {}", out.display()))?;
            println!("\n📝 relatório JSON: {}", out.display());
        }
    }

    if reports.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

fn render_human(r: &DocReport) -> String {
    let bar = "─".repeat(64);
    let mut out = Vec::new();
    out.push(bar.clone());
    out.push(format!("📄 {}", r.path));
    out.push(bar.clone());
    out.push(format!("  Páginas estimadas:        {}", r.pages));
    out.push(format!("  Palavras:                 {}", fmt_num(r.words)));
    out.push(format!(
        "  Tokens input estimados:   {}",
        fmt_num(r.estimated_input_tokens)
    ));
    let cargas = if r.specials_detected.is_empty() {
        "—".to_string()
    } else {
        r.specials_detected.join(", ")
    };
    out.push(format!("  Cargas especiais:         {cargas}"));
    out.push(format!(
        "  Sinais de complexidade:   {} ocorrências",
        r.complexity_hits
    ));
    out.push(format!(
        "  ▶ Tier classificado:      {}",
        r.tier.to_string().to_uppercase()
    ));
    out.push(format!("  ▶ Modelo recomendado:     {}", r.recommended_model));
    out.push(String::new());
    out.push("  Estágios (USD):".to_string());
    for s in &r.stages {
        out.push(format!(
            "    • {:<14} [{:<22}] in={:>7} out={:>6}  ${:.4}",
            s.name,
            s.model,
            fmt_num(s.input_tokens),
            fmt_num(s.output_tokens),
            s.cost_usd
        ));
    }
    out.push(String::new());
    out.push(format!(
        "  COGS total:               US$ {:.4}  (R$ {:.2})",
        r.total_cogs_usd, r.total_cogs_brl
    ));
    out.push(format!(
        "  Preço lista (tier):       R$ {:.2}",
        r.list_price_brl
    ));
    out.push(format!(
        "  ▶ Margem bruta:           {:.1}%",
        r.gross_margin_pct
    ));
    out.push(bar);
    out.join("\n")
}

fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
