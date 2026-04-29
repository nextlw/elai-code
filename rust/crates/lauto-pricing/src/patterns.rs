//! Padrões regex de extração (PT-BR). Compilados uma vez via `OnceLock`.
//!
//! Equivalentes diretos aos padrões usados no protótipo Python — qualquer
//! ajuste aqui muda **as duas ferramentas** simultaneamente. Mantenha a
//! lista sincronizada com `docs/lauto-proposta-nokk.md` §5.3.

use std::sync::OnceLock;

use regex::Regex;

/// Categorias de carga especial e seus padrões de detecção.
#[must_use]
pub fn cargas_especiais() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        (
            "inflamavel",
            &[r"(?i)inflam[aá]vel", r"(?i)combust[ií]vel", r"\bONU\b", r"(?i)classe\s*3"],
        ),
        (
            "farmaceutico",
            &[r"(?i)farm[aá]c", r"(?i)medicament", r"ANVISA", r"(?i)RDC\s*\d+"],
        ),
        (
            "perigosa",
            &[
                r"(?i)carga perigosa",
                r"(?i)produtos perigosos",
                r"\bMOPP\b",
                r"\bDGR\b",
            ],
        ),
        (
            "alimentos",
            &[r"(?i)alimento", r"(?i)perec[ií]vel", r"\bSIF\b", r"\bMAPA\b"],
        ),
        (
            "refrigerada",
            &[r"(?i)refrigerad", r"(?i)climatizad", r"(?i)cadeia fr[ií]a", r"(?i)reefer"],
        ),
        (
            "valor_agregado",
            &[r"(?i)valor agregado", r"(?i)alto valor", r"(?i)bens de capital", r"(?i)eletr[oô]nico"],
        ),
    ]
}

/// Sinais de complexidade contratual usados pelo classificador de tier.
#[must_use]
pub fn complexity_patterns() -> &'static [&'static str] {
    &[
        r"\bBID\b",
        r"(?i)edital",
        r"(?i)concorr[eê]ncia",
        r"(?i)trecho",
        r"(?i)\brota\b",
        r"(?i)\bmalha\b",
        r"(?i)frota dedicada",
        r"(?i)\bdedicado\b",
        r"\bSLA\b",
        r"(?i)penalidade",
        r"(?i)\bmulta\b",
        r"(?i)cl[aá]usula",
        r"(?i)\bcontrato\b",
    ]
}

/// Padrões de modal solicitado em editais.
#[must_use]
pub fn modal_patterns() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        (
            "rodoviario_fracionado",
            &[r"(?i)fracionad", r"(?i)transporte rodoviário"],
        ),
        (
            "rodoviario_dedicado",
            &[r"(?i)dedicad", r"(?i)carregamento total", r"(?i)unitizada"],
        ),
        ("aereo", &[r"(?i)a[eé]rea?\b", r"(?i)aeroporto"]),
        ("maritimo", &[r"(?i)mar[ií]tim", r"(?i)cabotagem", r"(?i)porto de"]),
        ("ferroviario", &[r"(?i)ferrovi[aá]ri", r"(?i)trem de carga"]),
    ]
}

/// Padrões de cláusula de penalidade / SLA.
#[must_use]
pub fn penalidade_patterns() -> &'static [&'static str] {
    &[
        r"(?i)multa de\s*\d",
        r"(?i)penalidade",
        r"(?i)resc[íi]s[ãa]o",
        r"(?i)juros de\s*\d",
        r"(?i)sla\s*[:\-]\s*\d",
        r"(?i)prazo\s+de\s+\d+\s+dias",
    ]
}

/// Regex para valores monetários em formato BRL.
pub fn valor_brl() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"R\$\s*[\d\.]+,\d{2}").expect("regex literal válida"))
}

/// Regex para "Cidade/UF" — heurística para extrair trechos.
pub fn cidade_uf() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\b([A-ZÀ-Ý][\wÀ-ÿ]+(?:\s+(?:de|da|do|dos|das)?\s*[A-ZÀ-Ý][\wÀ-ÿ]+){0,3})\s*[/\-,]\s*([A-Z]{2})\b",
        )
        .expect("regex de cidade/UF válida")
    })
}

/// Regex para extrair número do edital.
pub fn edital_numero() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)edital\s+(?:n[º°.]?\s*)?([\d\./-]+)").expect("regex edital válida")
    })
}

/// Regex para extrair número do pregão.
pub fn pregao_numero() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)preg[aã]o\s+(?:eletr[oô]nico\s+)?(?:n[º°.]?\s*)?([\d\./-]+)")
            .expect("regex pregão válida")
    })
}

/// Regex para extrair vigência contratual.
pub fn vigencia() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)vig[eê]ncia\s*(?:de\s*)?(\d{1,2}\s*\(?[\w]*\)?\s*(?:meses|anos))")
            .expect("regex vigência válida")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_inflamavel() {
        let pats = cargas_especiais()
            .iter()
            .find(|(name, _)| *name == "inflamavel")
            .map(|(_, p)| *p)
            .expect("categoria inflamavel definida");
        let text = "transporte de combustível e produto inflamável";
        let any_match = pats
            .iter()
            .any(|p| Regex::new(p).expect("regex válida").is_match(text));
        assert!(any_match);
    }

    #[test]
    fn cidade_uf_basic() {
        let m = cidade_uf().captures("Brasília/DF").expect("match");
        assert_eq!(&m[1], "Brasília");
        assert_eq!(&m[2], "DF");
    }

    #[test]
    fn pregao_numero_basic() {
        let m = pregao_numero().captures("Pregão Eletrônico nº 90008/2025").expect("match");
        assert_eq!(&m[1], "90008/2025");
    }
}
