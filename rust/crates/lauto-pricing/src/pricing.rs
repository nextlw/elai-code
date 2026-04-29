//! Pricing público (USD por 1M tokens) e tabela de preços lista (BRL).
//!
//! Os números aqui correspondem 1:1 ao protótipo Python e às tabelas
//! publicadas em `docs/pricing/unit-economics.md`. Recalibre **aqui**
//! quando o setor comercial mudar a régua.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Pricing por modelo, em USD por 1M tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
}

/// Câmbio padrão BRL/USD usado nos relatórios.
pub const DEFAULT_FX: f64 = 5.20;

/// Pricing default por modelo (recalibrar via `--config <pricing.json>`).
#[must_use]
pub fn default_pricing() -> HashMap<String, ModelPrice> {
    [
        ("haiku_3_5", ModelPrice { input: 0.80, output: 4.00 }),
        ("sonnet_3_7", ModelPrice { input: 3.00, output: 15.00 }),
        ("gpt_4o_mini", ModelPrice { input: 0.15, output: 0.60 }),
        ("gpt_4_1", ModelPrice { input: 2.50, output: 10.00 }),
        ("jina_reader", ModelPrice { input: 0.02, output: 0.00 }),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Preço de tabela ao cliente, em BRL.
#[derive(Debug, Clone, Copy)]
pub struct PriceTable {
    pub cotacao: f64,
    pub proposta: f64,
    pub bid_light: f64,
    pub bid_standard: f64,
    pub bid_heavy: f64,
}

impl Default for PriceTable {
    fn default() -> Self {
        Self {
            cotacao: 2.80,
            proposta: 18.00,
            bid_light: 149.00,
            bid_standard: 390.00,
            bid_heavy: 890.00,
        }
    }
}

/// Tabela padrão da Lauto (mock) usada pelo demo de BID.
///
/// Em produção os valores vêm de banco de dados gerenciado pelo cliente.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LautoTabela {
    pub frete_base_por_km: f64,
    pub frete_minimo: f64,
    pub gris_pct: f64,
    pub advalorem_pct: f64,
    pub icms_pct: f64,
    pub acrescimos_carga_especial: HashMap<String, f64>,
    pub modais_atendidos: Vec<String>,
    pub modais_nao_atendidos: Vec<String>,
}

impl Default for LautoTabela {
    fn default() -> Self {
        let acrescimos = [
            ("inflamavel", 35.0),
            ("farmaceutico", 25.0),
            ("perigosa", 40.0),
            ("alimentos", 15.0),
            ("refrigerada", 30.0),
            ("valor_agregado", 20.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        Self {
            frete_base_por_km: 4.20,
            frete_minimo: 280.00,
            gris_pct: 0.30,
            advalorem_pct: 0.15,
            icms_pct: 12.00,
            acrescimos_carga_especial: acrescimos,
            modais_atendidos: vec![
                "rodoviario_fracionado".into(),
                "rodoviario_dedicado".into(),
            ],
            modais_nao_atendidos: vec!["aereo".into(), "maritimo".into(), "ferroviario".into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pricing_has_expected_models() {
        let p = default_pricing();
        assert!(p.contains_key("jina_reader"));
        assert!(p.contains_key("sonnet_3_7"));
        assert!((p["jina_reader"].input - 0.02).abs() < 1e-9);
    }

    #[test]
    fn price_table_default_bid_heavy() {
        assert!((PriceTable::default().bid_heavy - 890.00).abs() < 1e-9);
    }

    #[test]
    fn lauto_tabela_modais() {
        let t = LautoTabela::default();
        assert!(t.modais_nao_atendidos.contains(&"aereo".into()));
        assert!(t.modais_atendidos.contains(&"rodoviario_fracionado".into()));
    }
}
