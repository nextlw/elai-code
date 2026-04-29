#!/usr/bin/env python3
"""Unit economics simulator — Nokk × Lauto.

Lê um (ou vários) PDF de edital/proposta e estima:
  - páginas, palavras, tokens (heurística PT-BR ~1.35 tok/palavra)
  - classificação de tier (Light / Standard / Heavy)
  - flags de cargas especiais detectadas
  - COGS por estágio (Jina Reader, modelo de extração, modelo de raciocínio)
  - margem bruta vs preço de tabela
  - recomendação de roteamento de modelo

Não chama LLM. Estima custo a partir de pricing público configurável.

Uso:
    python3 scripts/lauto_unit_econ.py docs/edital_x.pdf docs/edital_y.pdf
    python3 scripts/lauto_unit_econ.py --json out.json docs/*.pdf
    python3 scripts/lauto_unit_econ.py --fx 5.10 docs/edital_x.pdf

Saída humana por padrão; --json grava também relatório estruturado.

Premissas (todas configuráveis via --config pricing.json):
  - 1 página ≈ 380 palavras de texto útil em edital BR.
  - 1 palavra PT-BR ≈ 1.35 tokens (Anthropic / OpenAI tokenizer médio).
  - Reasoning output ≈ 35% do input (sumarização + JSON estruturado).
  - Cache hit em boilerplate ≈ 50% para Standard, 65% para Heavy.
  - Jina Reader: $0.02 / 1M tokens equivalentes.
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


# --- Pricing default (USD por 1M tokens). Ajustar via --config. -----------------

DEFAULT_PRICING: dict[str, dict[str, float]] = {
    "haiku_3_5":     {"input": 0.80,  "output": 4.00},
    "sonnet_3_7":    {"input": 3.00,  "output": 15.00},
    "gpt_4o_mini":   {"input": 0.15,  "output": 0.60},
    "gpt_4_1":       {"input": 2.50,  "output": 10.00},
    "jina_reader":   {"input": 0.02,  "output": 0.00},
}

DEFAULT_FX = 5.20  # BRL por USD — ajustar com cenário real

# Pricing de tabela proposto (BRL) por unidade de cobrança ao cliente
PRICE_TABLE_BRL: dict[str, float] = {
    "cotacao":           2.80,
    "proposta":         18.00,
    "bid_light":       149.00,
    "bid_standard":    390.00,
    "bid_heavy":       890.00,
}

# Heurística de classificação
TIER_RULES = {
    "light":    {"max_pages": 10,  "max_specials": 0},
    "standard": {"max_pages": 40,  "max_specials": 1},
    "heavy":    {"max_pages": 9_999, "max_specials": 99},
}

# Palavras-chave de cargas especiais e cláusulas de complexidade (PT-BR).
SPECIAL_LOAD_PATTERNS: dict[str, list[str]] = {
    "inflamavel":   [r"inflam[aá]vel", r"combust[ií]vel", r"\bONU\b", r"classe\s*3"],
    "farmaceutico": [r"farm[aá]c", r"medicament", r"ANVISA", r"RDC\s*\d+"],
    "perigosa":     [r"carga perigosa", r"produtos perigosos", r"\bMOPP\b", r"DGR"],
    "alimentos":    [r"alimento", r"perec[ií]vel", r"\bSIF\b", r"\bMAPA\b"],
    "refrigerada":  [r"refrigerad", r"climatizad", r"cadeia fr[ií]a", r"reefer"],
    "valor_agregado": [r"valor agregado", r"alto valor", r"bens de capital", r"eletr[oô]nico"],
}

COMPLEXITY_PATTERNS: list[str] = [
    r"BID\b", r"edital", r"concorr[eê]ncia",
    r"trecho", r"rota", r"malha",
    r"frota dedicada", r"dedicado",
    r"SLA", r"penalidade", r"multa",
    r"cl[aá]usula", r"contrato",
]


@dataclass
class StageCost:
    name: str
    model: str
    input_tokens: int
    output_tokens: int
    cost_usd: float


@dataclass
class DocReport:
    path: str
    pages: int
    words: int
    estimated_input_tokens: int
    specials_detected: list[str]
    complexity_hits: int
    tier: str
    recommended_model: str
    stages: list[StageCost] = field(default_factory=list)
    total_cogs_usd: float = 0.0
    total_cogs_brl: float = 0.0
    list_price_brl: float = 0.0
    gross_margin_pct: float = 0.0


# --- Extração ------------------------------------------------------------------

def pdf_to_text(pdf: Path) -> tuple[str, int]:
    """Retorna (texto, páginas). Usa pdftotext em modo layout."""
    if not pdf.exists():
        raise FileNotFoundError(pdf)

    # Página: usa pdfinfo se disponível; senão estima por contagem de form-feeds.
    pages = _pdf_pages(pdf)

    proc = subprocess.run(
        ["pdftotext", "-layout", str(pdf), "-"],
        capture_output=True, check=False, text=True,
    )
    if proc.returncode != 0 or not proc.stdout.strip():
        # Tentativa OCR-less alternativa: sem -layout.
        proc = subprocess.run(
            ["pdftotext", str(pdf), "-"],
            capture_output=True, check=False, text=True,
        )
    text = proc.stdout or ""
    return text, pages


def _pdf_pages(pdf: Path) -> int:
    proc = subprocess.run(
        ["pdfinfo", str(pdf)], capture_output=True, check=False, text=True,
    )
    for line in proc.stdout.splitlines():
        if line.lower().startswith("pages:"):
            try:
                return int(line.split(":", 1)[1].strip())
            except ValueError:
                pass
    # fallback: form-feed count
    try:
        return max(1, pdf.read_bytes().count(b"\f"))
    except OSError:
        return 1


# --- Análise -------------------------------------------------------------------

def count_words(text: str) -> int:
    return len(re.findall(r"\w+", text, flags=re.UNICODE))


def detect_specials(text: str) -> list[str]:
    found: list[str] = []
    lowered = text.lower()
    for label, patterns in SPECIAL_LOAD_PATTERNS.items():
        for p in patterns:
            if re.search(p, lowered, flags=re.IGNORECASE):
                found.append(label)
                break
    return sorted(set(found))


def count_complexity_hits(text: str) -> int:
    hits = 0
    for p in COMPLEXITY_PATTERNS:
        hits += len(re.findall(p, text, flags=re.IGNORECASE))
    return hits


def classify_tier(pages: int, specials: int, complexity_hits: int) -> str:
    if pages <= TIER_RULES["light"]["max_pages"] and specials == 0 and complexity_hits < 8:
        return "light"
    if pages <= TIER_RULES["standard"]["max_pages"] and specials <= 1 and complexity_hits < 25:
        return "standard"
    return "heavy"


def estimate_input_tokens(words: int, factor: float = 1.35) -> int:
    return int(round(words * factor))


# --- Custo ---------------------------------------------------------------------

def cost_for(stage_name: str, model: str, in_toks: int, out_toks: int,
             pricing: dict[str, dict[str, float]]) -> StageCost:
    if model not in pricing:
        raise KeyError(f"modelo {model!r} ausente no pricing")
    p = pricing[model]
    cost = (in_toks / 1_000_000.0) * p["input"] + (out_toks / 1_000_000.0) * p["output"]
    return StageCost(stage_name, model, in_toks, out_toks, round(cost, 6))


def simulate_pipeline(report: DocReport, pricing: dict[str, dict[str, float]]) -> None:
    in_toks = report.estimated_input_tokens
    tier = report.tier

    # Estágio 1: Jina Reader sempre. Limpa/normaliza PDF→markdown.
    stages: list[StageCost] = [
        cost_for("jina_reader", "jina_reader", in_toks, 0, pricing),
    ]

    if tier == "light":
        # Modelo único barato. Output ~25% do input.
        stages.append(cost_for("extract+answer", "gpt_4o_mini",
                               in_toks, int(in_toks * 0.25), pricing))
        report.recommended_model = "gpt_4o_mini"

    elif tier == "standard":
        # Cache hit ~50% no input pesado; reasoning em Sonnet só pelo restante.
        cached_ratio = 0.50
        live_input = int(in_toks * (1 - cached_ratio))
        stages.append(cost_for("extract", "haiku_3_5",
                               in_toks, int(in_toks * 0.10), pricing))
        stages.append(cost_for("reason", "sonnet_3_7",
                               live_input, int(live_input * 0.30), pricing))
        report.recommended_model = "haiku_3_5+sonnet_3_7"

    else:  # heavy
        # Map-reduce. Cache hit ~65%. Sonnet só na redução final.
        cached_ratio = 0.65
        map_input = in_toks
        live_input = int(in_toks * (1 - cached_ratio))
        stages.append(cost_for("map_extract", "haiku_3_5",
                               map_input, int(map_input * 0.08), pricing))
        stages.append(cost_for("reduce_reason", "sonnet_3_7",
                               live_input, int(live_input * 0.35), pricing))
        report.recommended_model = "haiku_map+sonnet_reduce"

    report.stages = stages
    total_usd = sum(s.cost_usd for s in stages)
    report.total_cogs_usd = round(total_usd, 4)


# --- Pipeline público ---------------------------------------------------------

def analyze_document(pdf: Path, pricing: dict[str, dict[str, float]],
                     fx: float) -> DocReport:
    text, pages = pdf_to_text(pdf)
    words = count_words(text)
    in_toks = estimate_input_tokens(words)
    specials = detect_specials(text)
    complexity_hits = count_complexity_hits(text)
    tier = classify_tier(pages, len(specials), complexity_hits)

    report = DocReport(
        path=str(pdf),
        pages=pages,
        words=words,
        estimated_input_tokens=in_toks,
        specials_detected=specials,
        complexity_hits=complexity_hits,
        tier=tier,
        recommended_model="",
    )

    simulate_pipeline(report, pricing)

    report.total_cogs_brl = round(report.total_cogs_usd * fx, 2)
    list_price = PRICE_TABLE_BRL[f"bid_{tier}"]
    report.list_price_brl = list_price
    if list_price > 0:
        margin = (list_price - report.total_cogs_brl) / list_price * 100
        report.gross_margin_pct = round(margin, 2)

    return report


# --- CLI ----------------------------------------------------------------------

def render_human(report: DocReport) -> str:
    bar = "─" * 64
    lines = [
        bar,
        f"📄 {report.path}",
        bar,
        f"  Páginas estimadas:        {report.pages}",
        f"  Palavras:                 {report.words:,}".replace(",", "."),
        f"  Tokens input estimados:   {report.estimated_input_tokens:,}".replace(",", "."),
        f"  Cargas especiais:         {', '.join(report.specials_detected) or '—'}",
        f"  Sinais de complexidade:   {report.complexity_hits} ocorrências",
        f"  ▶ Tier classificado:      {report.tier.upper()}",
        f"  ▶ Modelo recomendado:     {report.recommended_model}",
        "",
        "  Estágios (USD):",
    ]
    for s in report.stages:
        lines.append(
            f"    • {s.name:<14} [{s.model:<22}] in={s.input_tokens:>7,} "
            f"out={s.output_tokens:>6,}  ${s.cost_usd:.4f}"
            .replace(",", ".")
        )
    lines += [
        "",
        f"  COGS total:               US$ {report.total_cogs_usd:.4f}  "
        f"(R$ {report.total_cogs_brl:.2f})",
        f"  Preço lista (tier):       R$ {report.list_price_brl:.2f}",
        f"  ▶ Margem bruta:           {report.gross_margin_pct:.1f}%",
        bar,
    ]
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("pdfs", nargs="+", type=Path)
    parser.add_argument("--fx", type=float, default=DEFAULT_FX,
                        help=f"USD→BRL (default {DEFAULT_FX})")
    parser.add_argument("--config", type=Path,
                        help="JSON com override de pricing por modelo")
    parser.add_argument("--json", type=Path,
                        help="grava relatório estruturado neste arquivo")
    args = parser.parse_args(argv)

    pricing = dict(DEFAULT_PRICING)
    if args.config:
        try:
            override = json.loads(args.config.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            print(f"falha ao ler config: {exc}", file=sys.stderr)
            return 2
        for k, v in override.items():
            pricing[k] = v

    reports: list[dict[str, Any]] = []
    for pdf in args.pdfs:
        try:
            r = analyze_document(pdf, pricing, args.fx)
        except FileNotFoundError:
            print(f"⚠️  arquivo não encontrado: {pdf}", file=sys.stderr)
            continue
        except subprocess.CalledProcessError as exc:
            print(f"⚠️  pdftotext falhou em {pdf}: {exc}", file=sys.stderr)
            continue
        print(render_human(r))
        reports.append(asdict(r))

    if args.json and reports:
        args.json.write_text(json.dumps(reports, ensure_ascii=False, indent=2),
                             encoding="utf-8")
        print(f"\n📝 relatório JSON: {args.json}")

    if not reports:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
