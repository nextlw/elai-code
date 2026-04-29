//! `lauto-bid-demo` — diagnóstico determinístico de BID multi-documento.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use lauto_pricing::bid::{analyze_bid, format_brl, BidDiagnostico};
use lauto_pricing::pricing::LautoTabela;

#[derive(Debug, Parser)]
#[command(
    name = "lauto-bid-demo",
    about = "Demonstração heurística do diagnóstico Nokk para um BID multi-documento.\n\
             Aceita PDFs e/ou diretórios contendo PDFs.",
    version,
)]
struct Cli {
    /// PDFs ou diretórios com PDFs do BID.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Tabela Lauto em JSON (override do default).
    #[arg(long)]
    tabela: Option<PathBuf>,
    /// Caminho para gravar JSON estruturado.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Não imprime saída humana (apenas grava JSON, se solicitado).
    #[arg(long)]
    quiet: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let tabela = match &cli.tabela {
        Some(p) => {
            let raw = fs::read_to_string(p)
                .with_context(|| format!("falha ao ler {}", p.display()))?;
            serde_json::from_str(&raw)
                .with_context(|| format!("tabela inválida em {}", p.display()))?
        }
        None => LautoTabela::default(),
    };

    let pdfs = collect_pdfs(&cli.paths);
    if pdfs.is_empty() {
        eprintln!("nenhum PDF encontrado em {} caminho(s)", cli.paths.len());
        std::process::exit(1);
    }

    let pdf_refs: Vec<&Path> = pdfs.iter().map(PathBuf::as_path).collect();
    let diag = analyze_bid(&pdf_refs, &tabela)?;

    if !cli.quiet {
        println!("{}", render_human(&diag));
    }

    if let Some(out) = cli.out {
        let json = serde_json::to_string_pretty(&diag)?;
        fs::write(&out, json).with_context(|| format!("escrevendo {}", out.display()))?;
        if !cli.quiet {
            println!("\n📝 JSON estruturado: {}", out.display());
        }
    }

    Ok(())
}

fn collect_pdfs(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            if let Ok(rd) = fs::read_dir(p) {
                let mut subset: Vec<PathBuf> = rd
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")))
                    .collect();
                subset.sort();
                out.extend(subset);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) && p.exists() {
            out.push(p.clone());
        } else {
            eprintln!("⚠️  ignorando {} (não é PDF nem diretório)", p.display());
        }
    }
    out
}

// Função puramente de formatação textual; quebrar prejudicaria a leitura do
// layout. Mantemos contínua e suprimimos `too_many_lines` localmente.
#[allow(clippy::too_many_lines)]
fn render_human(d: &BidDiagnostico) -> String {
    let bar = "═".repeat(70);
    let soft = "─".repeat(70);
    let mut out = vec![
        bar.clone(),
        "  📑 LAUTO BID — DIAGNÓSTICO AUTOMÁTICO (DEMO)".to_string(),
        bar.clone(),
        String::new(),
        "🔖 Identificação".to_string(),
    ];
    if let Some(e) = &d.identificacao.edital_numero {
        out.push(format!("   • edital_numero: {e}"));
    }
    if let Some(p) = &d.identificacao.pregao_numero {
        out.push(format!("   • pregao_numero: {p}"));
    }
    if d.identificacao.edital_numero.is_none() && d.identificacao.pregao_numero.is_none() {
        out.push("   • não foi possível extrair identificação".to_string());
    }
    out.push(String::new());

    out.push(format!(
        "📂 Documentos analisados: {}",
        d.documentos_analisados.len()
    ));
    for doc in &d.documentos_analisados {
        out.push(format!(
            "   • {} — {} págs / {} palavras",
            doc.arquivo,
            doc.pages,
            fmt_num(doc.words)
        ));
    }
    out.push(String::new());

    out.push("🚚 Modais".to_string());
    out.push(format!(
        "   • Solicitados: {}",
        if d.modais_solicitados.is_empty() {
            "—".to_string()
        } else {
            d.modais_solicitados.join(", ")
        }
    ));
    out.push(format!(
        "   • Fora de escopo Lauto: {}",
        if d.modais_fora_de_escopo.is_empty() {
            "nenhum".to_string()
        } else {
            d.modais_fora_de_escopo.join(", ")
        }
    ));
    out.push(String::new());

    out.push(format!(
        "⚠️  Cargas especiais detectadas: {}",
        d.cargas_especiais.len()
    ));
    for c in &d.cargas_especiais {
        out.push(format!(
            "   • {} → +{}% (sugestão tabela)",
            c.tipo, c.sugestao_acrescimo_pct
        ));
        for ev in c.evidencias.iter().take(2) {
            out.push(format!("       evidência: \"…{ev}…\""));
        }
    }
    out.push(String::new());

    out.push(format!("🛣  Trechos candidatos: {}", d.trechos.len()));
    for t in d.trechos.iter().take(6) {
        out.push(format!(
            "   • {}  →  {}   [{}]",
            t.origem, t.destino, t.status_atendimento
        ));
    }
    if d.trechos.len() > 6 {
        out.push(format!("   • ... +{} trechos", d.trechos.len() - 6));
    }
    out.push(String::new());

    out.push("📋 Sinais de complexidade".to_string());
    out.push(format!(
        "   • Cláusulas de penalidade/SLA: {} ocorrências",
        d.sinais_complexidade.penalidades_e_sla.len()
    ));
    for line in d.sinais_complexidade.penalidades_e_sla.iter().take(3) {
        out.push(format!("       \"{line}\""));
    }
    if let Some(v) = &d.vigencia_detectada {
        out.push(format!("   • Vigência detectada: {v}"));
    }
    let valores_preview = d
        .valores_detectados_brl
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    out.push(format!(
        "   • Valores monetários no edital: {} (ex: {})",
        d.valores_detectados_brl.len(),
        if valores_preview.is_empty() {
            "—".to_string()
        } else {
            valores_preview
        }
    ));
    out.push(String::new());

    out.push(format!(
        "💰 Preço-base estimado: {}",
        format_brl(d.preco_base_estimado_brl)
    ));
    out.push(String::new());
    out.push(soft.clone());
    out.push(format!("🚦 RECOMENDAÇÃO: {}", d.recomendacao));
    out.push(format!("   {}", d.justificativa));
    out.push(soft.clone());
    out.push(String::new());
    out.push("✉️  DRAFT PARA THIAGO".to_string());
    out.push(soft);
    out.push(d.draft_resposta_para_thiago.clone());
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
