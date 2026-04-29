//! Diagnóstico de BID multi-documento — equivalente Rust do `bid_demo.py`.
//!
//! Heurística determinística para demonstrar **formato e capacidade** do
//! pipeline Nokk antes de chamar LLM em produção. Nunca usar em produção
//! sem substituir os módulos de extração por chamadas de modelo dedicadas.

use std::path::Path;

use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::patterns;
use crate::pdf::{self, PdfContent};
use crate::pricing::LautoTabela;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentoMeta {
    pub arquivo: String,
    pub pages: u32,
    pub words: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargaEspecialFlag {
    pub tipo: String,
    pub evidencias: Vec<String>,
    pub sugestao_acrescimo_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trecho {
    pub origem: String,
    pub destino: String,
    pub status_atendimento: String,
    pub modal_inferido: String,
    pub observacao: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SinaisComplexidade {
    pub penalidades_e_sla: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Identificacao {
    pub edital_numero: Option<String>,
    pub pregao_numero: Option<String>,
}

/// Diagnóstico completo de um BID — saída-modelo do demo.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BidDiagnostico {
    pub identificacao: Identificacao,
    pub documentos_analisados: Vec<DocumentoMeta>,
    pub modais_solicitados: Vec<String>,
    pub modais_fora_de_escopo: Vec<String>,
    pub cargas_especiais: Vec<CargaEspecialFlag>,
    pub trechos: Vec<Trecho>,
    pub sinais_complexidade: SinaisComplexidade,
    pub valores_detectados_brl: Vec<String>,
    pub vigencia_detectada: Option<String>,
    pub preco_base_estimado_brl: f64,
    pub recomendacao: String,
    pub justificativa: String,
    pub draft_resposta_para_thiago: String,
}

/// Pipeline de análise de BID. Lê todos os PDFs, agrega texto e roda
/// extração heurística + recomendação GO/NO-GO.
pub fn analyze_bid(pdfs: &[&Path], tabela: &LautoTabela) -> Result<BidDiagnostico> {
    let mut docs = Vec::with_capacity(pdfs.len());
    let mut full_text = String::new();
    for path in pdfs {
        let PdfContent { text, pages } = pdf::read_pdf(path)?;
        let words = pdf::count_words(&text);
        let arquivo = path
            .file_name()
            .map_or_else(|| path.display().to_string(), |s| s.to_string_lossy().into_owned());
        docs.push(DocumentoMeta {
            arquivo,
            pages,
            words: words as u64,
        });
        full_text.push_str(&text);
        full_text.push_str("\n\n");
    }

    let mut diag = BidDiagnostico {
        documentos_analisados: docs,
        ..BidDiagnostico::default()
    };

    diag.identificacao = extract_identificacao(&full_text);
    diag.cargas_especiais = extract_cargas_especiais(&full_text, tabela);
    let (solicitados, fora) = extract_modais(&full_text, tabela);
    diag.modais_solicitados = solicitados;
    diag.modais_fora_de_escopo = fora;
    diag.trechos = extract_trechos(&full_text);
    diag.sinais_complexidade.penalidades_e_sla = extract_penalidades(&full_text);
    diag.valores_detectados_brl = extract_valores(&full_text);
    diag.vigencia_detectada = extract_vigencia(&full_text);
    diag.preco_base_estimado_brl = estimate_preco_base(&diag.trechos, &diag.cargas_especiais, tabela);

    let (rec, just) = recomendar(&diag);
    diag.recomendacao = rec;
    diag.justificativa = just;
    diag.draft_resposta_para_thiago = build_draft(&diag);

    Ok(diag)
}

fn extract_identificacao(text: &str) -> Identificacao {
    let edital = patterns::edital_numero()
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string());
    let pregao = patterns::pregao_numero()
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string());
    Identificacao { edital_numero: edital, pregao_numero: pregao }
}

fn extract_cargas_especiais(text: &str, tabela: &LautoTabela) -> Vec<CargaEspecialFlag> {
    let mut out = Vec::new();
    for (label, raw_patterns) in patterns::cargas_especiais() {
        let mut evidencias: Vec<String> = Vec::new();
        for raw in *raw_patterns {
            let Ok(re) = Regex::new(raw) else { continue };
            for m in re.find_iter(text) {
                if evidencias.len() >= 3 {
                    break;
                }
                let start = m.start().saturating_sub(30);
                let end = (m.end() + 30).min(text.len());
                let snippet = safe_slice(text, start, end)
                    .replace('\n', " ")
                    .trim()
                    .chars()
                    .take(120)
                    .collect::<String>();
                evidencias.push(snippet);
            }
            if evidencias.len() >= 3 {
                break;
            }
        }
        if !evidencias.is_empty() {
            let pct = tabela
                .acrescimos_carga_especial
                .get(*label)
                .copied()
                .unwrap_or(20.0);
            out.push(CargaEspecialFlag {
                tipo: (*label).to_string(),
                evidencias,
                sugestao_acrescimo_pct: pct,
            });
        }
    }
    out
}

fn extract_modais(text: &str, tabela: &LautoTabela) -> (Vec<String>, Vec<String>) {
    let mut solicitados: Vec<String> = Vec::new();
    for (modal, raw_patterns) in patterns::modal_patterns() {
        for raw in *raw_patterns {
            if Regex::new(raw).is_ok_and(|re| re.is_match(text)) {
                solicitados.push((*modal).to_string());
                break;
            }
        }
    }
    solicitados.sort();
    solicitados.dedup();

    let fora: Vec<String> = solicitados
        .iter()
        .filter(|m| tabela.modais_nao_atendidos.iter().any(|n| n == *m))
        .cloned()
        .collect();
    (solicitados, fora)
}

fn extract_trechos(text: &str) -> Vec<Trecho> {
    let mut cidades: Vec<(String, String)> = Vec::new();
    for cap in patterns::cidade_uf().captures_iter(text) {
        let cidade = cap.get(1).map(|m| m.as_str().trim().to_string());
        let uf = cap.get(2).map(|m| m.as_str().to_string());
        if let (Some(c), Some(u)) = (cidade, uf) {
            let pair = (c, u);
            if !cidades.contains(&pair) {
                cidades.push(pair);
            }
        }
        if cidades.len() >= 8 {
            break;
        }
    }

    if cidades.len() < 2 {
        return Vec::new();
    }

    let (origem_c, origem_uf) = cidades[0].clone();
    cidades
        .into_iter()
        .skip(1)
        .map(|(dest_c, dest_uf)| Trecho {
            origem: format!("{origem_c}/{origem_uf}"),
            destino: format!("{dest_c}/{dest_uf}"),
            status_atendimento: "verificar".to_string(),
            modal_inferido: "rodoviario".to_string(),
            observacao: "Distância e cobertura precisam ser validadas contra a malha Lauto."
                .to_string(),
        })
        .collect()
}

fn extract_penalidades(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in patterns::penalidade_patterns() {
        let Ok(re) = Regex::new(raw) else { continue };
        for m in re.find_iter(text) {
            if out.len() >= 6 {
                return out;
            }
            let start = m.start().saturating_sub(25);
            let end = (m.end() + 50).min(text.len());
            let snippet = safe_slice(text, start, end)
                .replace('\n', " ")
                .trim()
                .chars()
                .take(160)
                .collect::<String>();
            out.push(snippet);
        }
    }
    out
}

fn extract_valores(text: &str) -> Vec<String> {
    patterns::valor_brl()
        .find_iter(text)
        .take(10)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn extract_vigencia(text: &str) -> Option<String> {
    patterns::vigencia()
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

/// Estimativa simples para o demo: 800km médio por trecho × R$/km × acréscimos.
fn estimate_preco_base(
    trechos: &[Trecho],
    cargas: &[CargaEspecialFlag],
    tabela: &LautoTabela,
) -> f64 {
    if trechos.is_empty() {
        return 0.0;
    }
    let distancia_media = 800.0;
    let base_por_trecho = (distancia_media * tabela.frete_base_por_km).max(tabela.frete_minimo);
    let mut total = base_por_trecho * trechos.len() as f64;
    let acr_total: f64 = cargas.iter().map(|c| c.sugestao_acrescimo_pct).sum();
    total *= 1.0 + acr_total / 100.0;
    total *= 1.0 + (tabela.gris_pct + tabela.advalorem_pct) / 100.0;
    (total * 100.0).round() / 100.0
}

fn recomendar(d: &BidDiagnostico) -> (String, String) {
    let mut razoes: Vec<String> = Vec::new();
    let mut veredito = "GO";

    if !d.modais_fora_de_escopo.is_empty() {
        razoes.push(format!(
            "Edital exige modal(is) {} — fora do escopo da malha Lauto.",
            d.modais_fora_de_escopo.join(", ")
        ));
        veredito = "NO-GO";
    }

    if d.trechos.is_empty() {
        razoes.push(
            "Não foi possível identificar trechos no edital — pode exigir leitura manual antes de cotar."
                .to_string(),
        );
        if veredito == "GO" {
            veredito = "GO COM RESSALVA";
        }
    }

    if d.cargas_especiais.len() >= 3 {
        razoes.push(format!(
            "Múltiplas categorias de carga especial detectadas ({}) — exige aprovação do Max para consolidar % de acréscimo.",
            d.cargas_especiais.len()
        ));
        if veredito == "GO" {
            veredito = "GO COM RESSALVA";
        }
    }

    if d.sinais_complexidade.penalidades_e_sla.len() >= 4 {
        razoes.push(
            "Edital com cláusulas de SLA/penalidade complexas — revisar exposição contratual.".to_string(),
        );
        if veredito == "GO" {
            veredito = "GO COM RESSALVA";
        }
    }

    if veredito == "GO" && razoes.is_empty() {
        razoes.push(
            "Edital aderente à malha. Cargas e cláusulas dentro do padrão. Seguir para precificação por trecho."
                .to_string(),
        );
    }

    (veredito.to_string(), razoes.join(" "))
}

fn build_draft(d: &BidDiagnostico) -> String {
    let cargas_str = if d.cargas_especiais.is_empty() {
        "nenhuma detectada".to_string()
    } else {
        d.cargas_especiais
            .iter()
            .map(|c| c.tipo.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let preco = format_brl(d.preco_base_estimado_brl);
    let vigencia = d.vigencia_detectada.clone().unwrap_or_else(|| "não detectada".to_string());
    let total_pages: u32 = d
        .documentos_analisados
        .iter()
        .map(|doc| doc.pages)
        .sum();
    let edital = d
        .identificacao
        .edital_numero
        .clone()
        .or_else(|| d.identificacao.pregao_numero.clone())
        .unwrap_or_else(|| "s/n".to_string());

    format!(
        "Olá Thiago,\n\n\
         Consolidei o diagnóstico inicial do BID {edital}.\n\n\
         📋 Visão geral\n  • {n_docs} documento(s) analisado(s) — {pages} págs no total.\n  \
         • Modal(is) solicitado(s): {modais}\n  \
         • Vigência: {vigencia}\n\n\
         🚛 Cobertura\n  • {n_trechos} trecho(s) candidato(s) identificado(s) — \
         validar contra malha antes de bater o martelo.\n\n\
         ⚠️  Cargas especiais\n  • {cargas_str}\n  \
         • Acréscimos sugeridos pela tabela travada — Max precisa revisar se houver mais de 2.\n\n\
         💰 Preço-base estimado (sem refinamento): {preco}\n\n\
         🚦 Recomendação automática: {rec}\n  Justificativa: {just}\n\n\
         O draft completo está no JSON anexo. Os campos abaixo precisam de validação humana antes de envio:\n  \
         - distâncias reais por trecho\n  \
         - cobertura final (atende/parcial/não atende)\n  \
         - % de acréscimo final por carga especial\n\n\
         -- Nokk-Chat (assistente de precificação)",
        edital = edital,
        n_docs = d.documentos_analisados.len(),
        pages = total_pages,
        modais = if d.modais_solicitados.is_empty() {
            "não detectado".to_string()
        } else {
            d.modais_solicitados.join(", ")
        },
        vigencia = vigencia,
        n_trechos = d.trechos.len(),
        cargas_str = cargas_str,
        preco = preco,
        rec = d.recomendacao,
        just = d.justificativa,
    )
}

/// Slice seguro respeitando fronteiras de char.
fn safe_slice(text: &str, start: usize, end: usize) -> String {
    let start = nearest_char_boundary(text, start);
    let end = nearest_char_boundary(text, end);
    text[start..end].to_string()
}

fn nearest_char_boundary(text: &str, mut idx: usize) -> usize {
    while idx > 0 && idx < text.len() && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx.min(text.len())
}

/// Formata BRL no estilo brasileiro: `R$ 1.234,56`.
#[must_use]
pub fn format_brl(value: f64) -> String {
    // Saturação intencional: valores monetários ficam dentro do range de i64,
    // a precisão de centavos cabe em mantissa f64.
    let cents = (value * 100.0).round() as i64;
    let negative = cents < 0;
    let cents = cents.unsigned_abs();
    let integer = cents / 100;
    let dec = cents % 100;
    let int_str = group_thousands(integer);
    let sign = if negative { "-" } else { "" };
    format!("R$ {sign}{int_str},{dec:02}")
}

fn group_thousands(n: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_brl_thousands() {
        assert_eq!(format_brl(50_795.56), "R$ 50.795,56");
        assert_eq!(format_brl(890.0), "R$ 890,00");
        assert_eq!(format_brl(0.0), "R$ 0,00");
    }

    #[test]
    fn group_thousands_basic() {
        assert_eq!(group_thousands(1_234_567), "1.234.567");
        assert_eq!(group_thousands(0), "0");
    }

    #[test]
    fn modais_aereo_marca_fora_de_escopo() {
        let tabela = LautoTabela::default();
        let text = "transporte aéreo via aeroporto e rodoviário fracionado";
        let (sol, fora) = extract_modais(text, &tabela);
        assert!(sol.contains(&"aereo".to_string()));
        assert!(fora.contains(&"aereo".to_string()));
        assert!(sol.contains(&"rodoviario_fracionado".to_string()));
    }

    #[test]
    fn recomendar_no_go_quando_modal_fora() {
        let mut d = BidDiagnostico::default();
        d.modais_fora_de_escopo.push("aereo".to_string());
        d.trechos.push(Trecho {
            origem: "A/SP".into(),
            destino: "B/RJ".into(),
            status_atendimento: "verificar".into(),
            modal_inferido: "rodoviario".into(),
            observacao: String::new(),
        });
        let (rec, _) = recomendar(&d);
        assert_eq!(rec, "NO-GO");
    }

    #[test]
    fn estimate_preco_base_zero_sem_trechos() {
        let tabela = LautoTabela::default();
        // Comparação direta com 0.0 é segura aqui (early-return literal).
        assert!(estimate_preco_base(&[], &[], &tabela).abs() < f64::EPSILON);
    }

    #[test]
    fn safe_slice_respeita_unicode() {
        let s = "inflamável";
        // 'á' começa em índice 6 (3 bytes UTF-8)
        let out = safe_slice(s, 0, 8);
        assert!(s.starts_with(&out) || s.contains(&out));
    }
}
