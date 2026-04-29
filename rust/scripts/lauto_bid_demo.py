#!/usr/bin/env python3
"""Lauto BID demo — produz o JSON estruturado que Thiago/Max receberiam.

Pega um pacote de PDFs (um BID multi-documento) e gera, por heurística
determinística (sem chamar LLM), um *draft* do diagnóstico que o pipeline
Nokk produziria. Serve como **demo de venda**: roda em ~3 segundos, gera
saída estruturada, mostra ao cliente o que ele veria na inbox do Thiago.

NÃO é o sistema de produção. É um proxy que demonstra o **formato** e a
**capacidade de extração** com regex + classificação. Em produção, cada
campo abaixo é preenchido por LLM com prompt dedicado.

Uso:
    python3 scripts/lauto_bid_demo.py PASTA_OU_PDFS [...] \\
        --tabela docs/pricing/tabela_lauto.json \\
        --out docs/pricing/demo_marinha_pe90008.json

Saída:
    JSON com:
      - identificacao (órgão, número, vigência se detectável)
      - cobertura: lista de trechos com origem/destino/status_atendimento
      - cargas_especiais: flags + sugestão de % acréscimo
      - preco_base: cálculo aplicando tabela padrão
      - sinais_de_complexidade: penalidade, multa, SLA
      - recomendacao: GO / NO-GO / GO COM RESSALVA
      - draft_resposta: texto inicial para Thiago revisar
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any


# ---------- Tabela padrão (mock — em produção vem de banco do cliente) -------

DEFAULT_TABELA: dict[str, Any] = {
    "frete_base_por_km": 4.20,
    "frete_minimo": 280.00,
    "gris_pct": 0.30,
    "advalorem_pct": 0.15,
    "icms_pct": 12.00,
    "acrescimos_carga_especial": {
        "inflamavel":     35.0,
        "farmaceutico":   25.0,
        "perigosa":       40.0,
        "alimentos":      15.0,
        "refrigerada":    30.0,
        "valor_agregado": 20.0,
    },
    "modais_atendidos": ["rodoviario_fracionado", "rodoviario_dedicado"],
    "modais_nao_atendidos": ["aereo", "maritimo", "ferroviario"],
}


# ---------- Padrões de extração (PT-BR) --------------------------------------

CARGAS_ESPECIAIS_PATTERNS: dict[str, list[str]] = {
    "inflamavel":     [r"inflam[aá]vel", r"combust[ií]vel", r"\bONU\b"],
    "farmaceutico":   [r"farm[aá]c", r"medicament", r"ANVISA", r"RDC\s*\d+"],
    "perigosa":       [r"carga perigosa", r"produtos perigosos", r"\bMOPP\b", r"\bDGR\b"],
    "alimentos":      [r"alimento", r"perec[ií]vel", r"\bSIF\b", r"\bMAPA\b"],
    "refrigerada":    [r"refrigerad", r"climatizad", r"cadeia fr[ií]a", r"reefer"],
    "valor_agregado": [r"valor agregado", r"alto valor", r"bens de capital"],
}

ORIGEM_DESTINO_PATTERNS: list[tuple[str, str]] = [
    (r"origem\s*:?\s*([A-ZÀ-Ý][\wÀ-ÿ\s\-]{2,40}?)\s*[/\-,–]?\s*([A-Z]{2})\b",
     "origem"),
    (r"destino\s*:?\s*([A-ZÀ-Ý][\wÀ-ÿ\s\-]{2,40}?)\s*[/\-,–]?\s*([A-Z]{2})\b",
     "destino"),
    (r"de\s+([A-ZÀ-Ý][\wÀ-ÿ\s\-]{2,30}?)/([A-Z]{2})\s+(?:para|a|até)\s+([A-ZÀ-Ý][\wÀ-ÿ\s\-]{2,30}?)/([A-Z]{2})",
     "rota"),
]

CIDADE_UF_PATTERN = re.compile(
    r"\b([A-ZÀ-Ý][\wÀ-ÿ]+(?:\s+(?:de|da|do|dos|das)?\s*[A-ZÀ-Ý][\wÀ-ÿ]+){0,3})\s*[/\-,]\s*([A-Z]{2})\b"
)

MODAL_PATTERNS: dict[str, list[str]] = {
    "rodoviario_fracionado": [r"fracionad", r"transporte rodoviário"],
    "rodoviario_dedicado":   [r"dedicad", r"carregamento total", r"unitizada"],
    "aereo":                 [r"a[eé]rea?\b", r"aeroporto"],
    "maritimo":              [r"mar[ií]tim", r"cabotagem", r"porto de"],
    "ferroviario":           [r"ferrovi[aá]ri", r"trem de carga"],
}

PENALIDADE_PATTERNS = [
    r"multa de\s*\d", r"penalidade", r"resc[íi]s[ãa]o", r"juros de\s*\d",
    r"sla\s*[:\-]\s*\d", r"prazo\s+de\s+\d+\s+dias",
]

VALOR_PATTERNS = [
    r"R\$\s*[\d\.]+,\d{2}",
]

VIGENCIA_PATTERNS = [
    r"vig[eê]ncia\s*(?:de\s*)?(\d{1,2}\s*\(?[\w]*\)?\s*(?:meses|anos))",
    r"prazo\s+contratual\s*(?:de\s*)?(\d{1,2}\s*(?:meses|anos))",
]


# ---------- Modelo de saída ---------------------------------------------------

@dataclass
class Trecho:
    origem: str
    destino: str
    distancia_estimada_km: float | None = None
    status_atendimento: str = "verificar"   # atende / parcial / nao_atende / verificar
    modal_inferido: str = "rodoviario"
    preco_base_brl: float = 0.0
    observacao: str = ""


@dataclass
class CargaEspecialFlag:
    tipo: str
    evidencias: list[str]
    sugestao_acrescimo_pct: float


@dataclass
class BidDiagnostico:
    identificacao: dict[str, Any] = field(default_factory=dict)
    documentos_analisados: list[dict[str, Any]] = field(default_factory=list)
    modais_solicitados: list[str] = field(default_factory=list)
    modais_fora_de_escopo: list[str] = field(default_factory=list)
    cargas_especiais: list[CargaEspecialFlag] = field(default_factory=list)
    trechos: list[Trecho] = field(default_factory=list)
    sinais_complexidade: dict[str, list[str]] = field(default_factory=dict)
    valores_detectados_brl: list[str] = field(default_factory=list)
    vigencia_detectada: str | None = None
    preco_base_estimado_brl: float = 0.0
    recomendacao: str = ""
    justificativa: str = ""
    draft_resposta_para_thiago: str = ""


# ---------- Extração ---------------------------------------------------------

def pdf_text(pdf: Path) -> tuple[str, int]:
    pages_proc = subprocess.run(
        ["pdfinfo", str(pdf)], capture_output=True, check=False, text=True,
    )
    pages = 1
    for line in pages_proc.stdout.splitlines():
        if line.lower().startswith("pages:"):
            try:
                pages = int(line.split(":", 1)[1].strip())
            except ValueError:
                pass
    text = subprocess.run(
        ["pdftotext", "-layout", str(pdf), "-"],
        capture_output=True, check=False, text=True,
    ).stdout or ""
    return text, pages


def extract_cargas_especiais(text: str, tabela: dict[str, Any]) -> list[CargaEspecialFlag]:
    found: list[CargaEspecialFlag] = []
    acrescimos = tabela["acrescimos_carga_especial"]
    for tipo, patterns in CARGAS_ESPECIAIS_PATTERNS.items():
        evidencias: list[str] = []
        for p in patterns:
            for m in re.finditer(p, text, flags=re.IGNORECASE):
                # captura janela de contexto de 60 chars
                start = max(0, m.start() - 30)
                end = min(len(text), m.end() + 30)
                snippet = text[start:end].replace("\n", " ").strip()
                evidencias.append(snippet[:120])
                if len(evidencias) >= 3:
                    break
            if len(evidencias) >= 3:
                break
        if evidencias:
            found.append(CargaEspecialFlag(
                tipo=tipo,
                evidencias=evidencias,
                sugestao_acrescimo_pct=float(acrescimos.get(tipo, 20.0)),
            ))
    return found


def extract_modais(text: str, tabela: dict[str, Any]) -> tuple[list[str], list[str]]:
    solicitados: list[str] = []
    for modal, patterns in MODAL_PATTERNS.items():
        for p in patterns:
            if re.search(p, text, flags=re.IGNORECASE):
                solicitados.append(modal)
                break
    fora = [m for m in solicitados if m in tabela["modais_nao_atendidos"]]
    return sorted(set(solicitados)), sorted(set(fora))


def extract_trechos(text: str) -> list[Trecho]:
    """Heurística simples: pega pares cidade/UF e gera trechos prováveis.
    Em produção isso é tarefa de LLM com extração estruturada."""
    matches = CIDADE_UF_PATTERN.findall(text)
    # Deduplica preservando ordem
    seen: set[tuple[str, str]] = set()
    cidades: list[tuple[str, str]] = []
    for cidade, uf in matches:
        key = (cidade.strip(), uf)
        if key not in seen:
            seen.add(key)
            cidades.append(key)

    # Limita a 8 cidades únicas para o demo
    cidades = cidades[:8]

    if len(cidades) < 2:
        return []

    # Para o demo, assume primeira cidade como origem (hub) e demais como destinos
    origem = cidades[0]
    trechos: list[Trecho] = []
    for destino in cidades[1:]:
        trechos.append(Trecho(
            origem=f"{origem[0]}/{origem[1]}",
            destino=f"{destino[0]}/{destino[1]}",
            status_atendimento="verificar",
            modal_inferido="rodoviario",
            observacao="Distância e cobertura precisam ser validadas contra a malha Lauto.",
        ))
    return trechos


def extract_complexidade(text: str) -> dict[str, list[str]]:
    out: dict[str, list[str]] = {"penalidades_e_sla": [], "valores_mencionados": [],
                                   "vigencia": []}
    for p in PENALIDADE_PATTERNS:
        for m in re.finditer(p, text, flags=re.IGNORECASE):
            start, end = max(0, m.start() - 25), min(len(text), m.end() + 50)
            snippet = text[start:end].replace("\n", " ").strip()
            out["penalidades_e_sla"].append(snippet[:160])
            if len(out["penalidades_e_sla"]) >= 6:
                break
    for p in VALOR_PATTERNS:
        for m in re.finditer(p, text):
            out["valores_mencionados"].append(m.group(0))
            if len(out["valores_mencionados"]) >= 10:
                break
    for p in VIGENCIA_PATTERNS:
        m = re.search(p, text, flags=re.IGNORECASE)
        if m:
            out["vigencia"].append(m.group(1))
            break
    return out


def extract_identificacao(text: str) -> dict[str, Any]:
    ident: dict[str, Any] = {}
    # Edital nº ...
    m = re.search(r"edital\s+(?:n[º°.]?\s*)?([\d\./-]+)", text, flags=re.IGNORECASE)
    if m:
        ident["edital_numero"] = m.group(1).strip()
    m = re.search(r"preg[aã]o\s+(?:eletr[oô]nico\s+)?(?:n[º°.]?\s*)?([\d\./-]+)",
                  text, flags=re.IGNORECASE)
    if m:
        ident["pregao_numero"] = m.group(1).strip()
    m = re.search(r"CNPJ[:\s]+([\d\.\-/]+)", text)
    if m:
        ident["cnpj_orgao"] = m.group(1).strip()
    return ident


# ---------- Cálculo de preço-base --------------------------------------------

def estimate_preco_base(trechos: list[Trecho], cargas: list[CargaEspecialFlag],
                        tabela: dict[str, Any]) -> float:
    """Stub determinístico: em produção, calcula por trecho com distância real."""
    if not trechos:
        return 0.0
    # Para demo: assume 800km médio por trecho
    distancia_media = 800
    base_por_trecho = max(
        tabela["frete_minimo"],
        distancia_media * tabela["frete_base_por_km"],
    )
    total = base_por_trecho * len(trechos)
    # Aplica acréscimos
    acrescimo_total = sum(c.sugestao_acrescimo_pct for c in cargas)
    total = total * (1 + acrescimo_total / 100.0)
    # Aplica GRIS + AdValorem (sobre total como estimativa simplificada)
    total = total * (1 + (tabela["gris_pct"] + tabela["advalorem_pct"]) / 100.0)
    return round(total, 2)


# ---------- Recomendação ------------------------------------------------------

def recomendar(d: BidDiagnostico, tabela: dict[str, Any]) -> tuple[str, str]:
    """Lógica de gate: GO / NO-GO / GO COM RESSALVA."""
    razoes: list[str] = []
    veredito = "GO"

    if d.modais_fora_de_escopo:
        razoes.append(
            f"Edital exige modal(is) {', '.join(d.modais_fora_de_escopo)} — "
            "fora do escopo da malha Lauto."
        )
        veredito = "NO-GO"

    if not d.trechos:
        razoes.append("Não foi possível identificar trechos no edital — "
                      "pode exigir leitura manual antes de cotar.")
        if veredito == "GO":
            veredito = "GO COM RESSALVA"

    if len(d.cargas_especiais) >= 3:
        razoes.append(
            f"Múltiplas categorias de carga especial detectadas "
            f"({len(d.cargas_especiais)}) — exige aprovação do Max para "
            "consolidar % de acréscimo."
        )
        if veredito == "GO":
            veredito = "GO COM RESSALVA"

    if len(d.sinais_complexidade.get("penalidades_e_sla", [])) >= 4:
        razoes.append("Edital com cláusulas de SLA/penalidade complexas — "
                      "revisar exposição contratual.")
        if veredito == "GO":
            veredito = "GO COM RESSALVA"

    if veredito == "GO" and not razoes:
        razoes.append("Edital aderente à malha. Cargas e cláusulas dentro do "
                      "padrão. Seguir para precificação por trecho.")

    return veredito, " ".join(razoes)


# ---------- Draft de resposta ------------------------------------------------

def draft_resposta(d: BidDiagnostico) -> str:
    cargas_str = ", ".join(c.tipo for c in d.cargas_especiais) or "nenhuma detectada"
    n_trechos = len(d.trechos)
    preco = f"R$ {d.preco_base_estimado_brl:,.2f}".replace(",", "X").replace(".", ",").replace("X", ".")
    vigencia = d.vigencia_detectada or "não detectada"

    return (
        f"Olá Thiago,\n\n"
        f"Consolidei o diagnóstico inicial do BID {d.identificacao.get('edital_numero', 's/n')}.\n\n"
        f"📋 Visão geral\n"
        f"  • {len(d.documentos_analisados)} documento(s) analisado(s) — "
        f"{sum(doc['pages'] for doc in d.documentos_analisados)} págs no total.\n"
        f"  • Modal(is) solicitado(s): {', '.join(d.modais_solicitados) or 'não detectado'}\n"
        f"  • Vigência: {vigencia}\n\n"
        f"🚛 Cobertura\n"
        f"  • {n_trechos} trecho(s) candidato(s) identificado(s) — "
        f"validar contra malha antes de bater o martelo.\n\n"
        f"⚠️  Cargas especiais\n"
        f"  • {cargas_str}\n"
        f"  • Acréscimos sugeridos pela tabela travada — Max precisa revisar se houver mais de 2.\n\n"
        f"💰 Preço-base estimado (sem refinamento): {preco}\n\n"
        f"🚦 Recomendação automática: {d.recomendacao}\n"
        f"  Justificativa: {d.justificativa}\n\n"
        f"O draft completo está no JSON anexo. Os campos abaixo precisam de "
        f"validação humana antes de envio:\n"
        f"  - distâncias reais por trecho\n"
        f"  - cobertura final (atende/parcial/não atende)\n"
        f"  - % de acréscimo final por carga especial\n\n"
        f"-- Nokk-Chat (assistente de precificação)"
    )


# ---------- Pipeline público --------------------------------------------------

def analyze_bid(pdfs: list[Path], tabela: dict[str, Any]) -> BidDiagnostico:
    full_text_parts: list[str] = []
    docs_meta: list[dict[str, Any]] = []
    for pdf in pdfs:
        text, pages = pdf_text(pdf)
        full_text_parts.append(text)
        words = len(re.findall(r"\w+", text))
        docs_meta.append({
            "arquivo": pdf.name,
            "pages": pages,
            "words": words,
        })
    full_text = "\n\n".join(full_text_parts)

    d = BidDiagnostico()
    d.documentos_analisados = docs_meta
    d.identificacao = extract_identificacao(full_text)
    d.cargas_especiais = extract_cargas_especiais(full_text, tabela)
    d.modais_solicitados, d.modais_fora_de_escopo = extract_modais(full_text, tabela)
    d.trechos = extract_trechos(full_text)
    d.sinais_complexidade = extract_complexidade(full_text)
    d.valores_detectados_brl = d.sinais_complexidade.pop("valores_mencionados", [])
    vigs = d.sinais_complexidade.pop("vigencia", [])
    d.vigencia_detectada = vigs[0] if vigs else None
    d.preco_base_estimado_brl = estimate_preco_base(
        d.trechos, d.cargas_especiais, tabela,
    )
    d.recomendacao, d.justificativa = recomendar(d, tabela)
    d.draft_resposta_para_thiago = draft_resposta(d)
    return d


# ---------- CLI --------------------------------------------------------------

def collect_pdfs(paths: list[Path]) -> list[Path]:
    pdfs: list[Path] = []
    for p in paths:
        if p.is_dir():
            pdfs.extend(sorted(p.glob("*.pdf")))
        elif p.suffix.lower() == ".pdf" and p.exists():
            pdfs.append(p)
        else:
            print(f"⚠️  ignorando {p} (não é PDF nem diretório)", file=sys.stderr)
    return pdfs


def render_human(d: BidDiagnostico) -> str:
    bar = "═" * 70
    soft = "─" * 70
    out: list[str] = [bar, "  📑 LAUTO BID — DIAGNÓSTICO AUTOMÁTICO (DEMO)", bar, ""]

    out.append("🔖 Identificação")
    if d.identificacao:
        for k, v in d.identificacao.items():
            out.append(f"   • {k}: {v}")
    else:
        out.append("   • não foi possível extrair identificação")
    out.append("")

    out.append(f"📂 Documentos analisados: {len(d.documentos_analisados)}")
    for doc in d.documentos_analisados:
        out.append(f"   • {doc['arquivo']} — {doc['pages']} págs / "
                   f"{doc['words']:,} palavras".replace(",", "."))
    out.append("")

    out.append("🚚 Modais")
    out.append(f"   • Solicitados: {', '.join(d.modais_solicitados) or '—'}")
    out.append(f"   • Fora de escopo Lauto: "
               f"{', '.join(d.modais_fora_de_escopo) or 'nenhum'}")
    out.append("")

    out.append(f"⚠️  Cargas especiais detectadas: {len(d.cargas_especiais)}")
    for c in d.cargas_especiais:
        out.append(f"   • {c.tipo} → +{c.sugestao_acrescimo_pct}% (sugestão tabela)")
        for ev in c.evidencias[:2]:
            out.append(f"       evidência: \"…{ev}…\"")
    out.append("")

    out.append(f"🛣  Trechos candidatos: {len(d.trechos)}")
    for t in d.trechos[:6]:
        out.append(f"   • {t.origem}  →  {t.destino}   [{t.status_atendimento}]")
    if len(d.trechos) > 6:
        out.append(f"   • ... +{len(d.trechos)-6} trechos")
    out.append("")

    out.append("📋 Sinais de complexidade")
    pen = d.sinais_complexidade.get("penalidades_e_sla", [])
    out.append(f"   • Cláusulas de penalidade/SLA: {len(pen)} ocorrências")
    for line in pen[:3]:
        out.append(f"       \"{line}\"")
    if d.vigencia_detectada:
        out.append(f"   • Vigência detectada: {d.vigencia_detectada}")
    out.append(f"   • Valores monetários no edital: "
               f"{len(d.valores_detectados_brl)} (ex: "
               f"{', '.join(d.valores_detectados_brl[:3]) or '—'})")
    out.append("")

    out.append(f"💰 Preço-base estimado: R$ {d.preco_base_estimado_brl:,.2f}"
               .replace(",", "X").replace(".", ",").replace("X", "."))
    out.append("")

    out.append(soft)
    out.append(f"🚦 RECOMENDAÇÃO: {d.recomendacao}")
    out.append(f"   {d.justificativa}")
    out.append(soft)
    out.append("")
    out.append("✉️  DRAFT PARA THIAGO")
    out.append(soft)
    out.append(d.draft_resposta_para_thiago)
    out.append(bar)
    return "\n".join(out)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("paths", nargs="+", type=Path,
                        help="PDFs ou diretório com PDFs do BID")
    parser.add_argument("--tabela", type=Path,
                        help="JSON com tabela padrão da Lauto (override)")
    parser.add_argument("--out", type=Path,
                        help="grava JSON estruturado (saída-modelo)")
    parser.add_argument("--quiet", action="store_true",
                        help="só grava JSON, não imprime humano")
    args = parser.parse_args(argv)

    tabela = dict(DEFAULT_TABELA)
    if args.tabela:
        try:
            override = json.loads(args.tabela.read_text(encoding="utf-8"))
            tabela.update(override)
        except (OSError, json.JSONDecodeError) as exc:
            print(f"falha ao ler tabela: {exc}", file=sys.stderr)
            return 2

    pdfs = collect_pdfs(args.paths)
    if not pdfs:
        print("nenhum PDF encontrado", file=sys.stderr)
        return 1

    diag = analyze_bid(pdfs, tabela)

    if not args.quiet:
        print(render_human(diag))

    if args.out:
        # Converter dataclasses aninhadas
        payload = asdict(diag)
        args.out.write_text(json.dumps(payload, ensure_ascii=False, indent=2),
                            encoding="utf-8")
        if not args.quiet:
            print(f"\n📝 JSON estruturado: {args.out}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
