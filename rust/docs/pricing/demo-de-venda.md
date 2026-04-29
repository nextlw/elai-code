# Demo de venda — Pipeline Lauto BID rodando em editais reais

> Script: `scripts/lauto_bid_demo.py`
> Editais: `docs/editais_exemplo/*/pdfs/`
> Saídas JSON: `docs/pricing/demo_*.json`
>
> Este documento é o **material de apresentação** para Lauto. Mostra
> exatamente o que o pipeline Nokk produziria a partir de editais reais —
> rodado em <5 segundos por BID, custo de tokens <R$ 2.

---

## 1. Resumo executivo (slide 1)

> *"Pegamos 3 BIDs reais publicados no Pregão Eletrônico — todos com objeto
> idêntico ao que Lauto cota. Nokk leu **329 páginas distribuídas em 13
> documentos**, classificou cada arquivo por complexidade, identificou
> cargas especiais com evidência textual, detectou cláusulas de penalidade
> e SLA, calculou preço-base e produziu **drafts prontos para Thiago
> revisar**. Custo computacional total: **R$ 3,82**. Tempo: ~12 segundos."*

---

## 2. Os 3 BIDs analisados

### BID 1 — Pregão Eletrônico 90008/2025 (Comando da Marinha / Brasília-DF)

| Campo | Valor |
|---|---|
| Páginas | 92 (em 6 docs) |
| Modais | rodoviário fracionado + dedicado |
| Cargas especiais detectadas | inflamável, farmacêutico, perigosa, alimentos |
| Valor do contrato (extraído) | **R$ 3.168.452,94** |
| **Recomendação Nokk** | **GO COM RESSALVA** |
| Justificativa | Múltiplas cargas especiais → exige aprovação Max; 8 cláusulas SLA/penalidade |
| COGS pipeline | R$ 1,03 |

**Por que vale ouro:** Nokk identificou que **modal está dentro do escopo**
(rodoviário) e flaggou os 4 tipos de carga especial, citando o trecho original
("etanol, gás natural", "ANVISA", "carga perigosa", "alimentos"). Este BID
**vale a pena cotar**.

---

### BID 2 — Pregão Eletrônico 90019/2025 (Comando da Marinha / Belém-PA)

| Campo | Valor |
|---|---|
| Páginas | 153 (em 5 docs) |
| Modais | **rodoviário fracionado + AÉREO** |
| Cargas especiais | perigosa (álcool, aerossol), alimentos |
| Valor extraído (1º) | R$ 166.778,61 |
| **Recomendação Nokk** | **NO-GO** ⛔ |
| Justificativa | "Edital exige modal aéreo — fora do escopo da malha Lauto" |
| COGS pipeline | R$ 1,64 |

**Por que vale ouro:** **Nokk economiza 6 horas do Thiago** — esse BID seria
descartado depois de leitura inicial. Sistema descartou em 4 segundos com
justificativa textual. Isso é diretamente aderente ao princípio do §6 do md
("Aéreo (ex.: Azul Cargo) requer cotação externa — fora do escopo de
automação").

---

### BID 3 — Pregão Eletrônico 90015/2024 (Exército / São Paulo-SP)

| Campo | Valor |
|---|---|
| Páginas | 84 (em 2 docs) |
| Modais | **rodoviário + AÉREO + MARÍTIMO** |
| Cargas especiais | farmacêutico (medicamentos), alimentos |
| Valor extraído (3º) | R$ 2.438.100,00 |
| **Recomendação Nokk** | **NO-GO** ⛔ |
| Justificativa | "Edital exige modais aéreo, marítimo — fora do escopo da malha Lauto" |
| COGS pipeline | R$ 1,15 |

**Por que vale ouro:** Mesmo o BID tendo R$ 2,4 milhões de orçamento, Nokk
identificou que dois modais críticos não são atendidos. **Evita a Lauto
cotar e perder.**

---

## 3. Estatísticas agregadas (calibração de pricing)

| Métrica | Valor | Implicação |
|---|---|---|
| Total de documentos analisados | 13 | Pacotes reais são multi-doc |
| Páginas totais | 329 | Confirma tier Heavy típico |
| Classificação por documento | 15% Light / 15% Std / 69% Heavy | **Edital público é heavy-skew** |
| COGS total | R$ 3,82 | <0,4% de qualquer ticket dos BIDs |
| COGS médio por BID | R$ 1,27 | Margem ~99% sobre R$ 890 (tier Heavy) |
| COGS por página | R$ 0,0116 | Linear, previsível |
| Decisões automáticas | 1 GO-RESS / 2 NO-GO | **2 BIDs evitados sem custo de Thiago** |

> **Insight crítico para a proposta:** o pricing de tier Heavy a R$ 890
> sustenta confortavelmente o pior caso (R$ 1,64 de COGS). A franquia
> **"3 Light + 2 Std + 1 Heavy"** do BID Premium IA precisa ser **revisada**
> — em editais públicos, 70% caem em Heavy. Sugiro ajustar para
> **"2 Light + 2 Std + 2 Heavy"** se o perfil de Lauto for similar
> (validar no discovery).

---

## 4. O que Nokk extrai por BID (campos do JSON)

```json
{
  "identificacao": {
    "edital_numero": "26/2025",
    "pregao_numero": "90019/2025"
  },
  "documentos_analisados": [
    {"arquivo": "01_edital.pdf", "pages": 20, "words": 10833},
    ...
  ],
  "modais_solicitados": ["aereo", "rodoviario_fracionado"],
  "modais_fora_de_escopo": ["aereo"],
  "cargas_especiais": [
    {
      "tipo": "perigosa",
      "evidencias": ["…ser demandado o transporte de carga perigosa, tais como: álcool, aerossol,…"],
      "sugestao_acrescimo_pct": 40.0
    }
  ],
  "trechos": [...],
  "sinais_complexidade": {
    "penalidades_e_sla": ["umulativamente ou não, à penalidade de multa…", ...]
  },
  "valores_detectados_brl": ["R$ 166.778,61", "R$ 35,89", ...],
  "vigencia_detectada": "12 meses",
  "preco_base_estimado_brl": 36620.05,
  "recomendacao": "NO-GO",
  "justificativa": "Edital exige modal(is) aereo — fora do escopo da malha Lauto.",
  "draft_resposta_para_thiago": "Olá Thiago, ..."
}
```

---

## 5. Onde a heurística pega ruído (e como resolver em produção)

A versão demo usa **regex puro** para extrair trechos cidade/UF. Isso pega
falsos-positivos em texto burocrático brasileiro:

| Falso-positivo no demo | Real | Ruído por |
|---|---|---|
| "IN SEGES/ME → ..." | "Instrução Normativa (referência regulatória)" | Sigla casa com `cidade/UF` |
| "Brasília/DF → CNPJ/MF" | "CNPJ do Ministério da Fazenda" | Sigla casa com `cidade/UF` |
| "→ DD/MM" | placeholder de data | Padrão genérico demais |

Em **produção**, isso é resolvido com:
- LLM dedicado para extração estruturada (`extract_trechos_via_llm`) com
  prompt focado em "rotas de transporte solicitadas no edital".
- Validação contra base de cidades brasileiras (IBGE).
- Filtro por contexto: "trecho" só se mencionado em seção de logística/operação.

**Custo adicional na produção:** ~R$ 0,30 por BID a mais (chamada Sonnet
focada em ~20% do texto). Mantém margem >98%.

---

## 6. Roteiro de demo (para apresentação ao vivo)

1. **Abrir terminal** ao lado do PDF do edital (slide com PDF + terminal).
2. Rodar:
   ```bash
   python3 scripts/lauto_bid_demo.py docs/editais_exemplo/marinha_pe_90008_2025/
   ```
3. **Cronometrar** — fica em ~5 segundos.
4. Mostrar:
   - "Olha aqui, ele identificou 4 categorias de carga especial. Cada uma com
     o trecho do edital onde aparece — não chuta."
   - "Aqui ele já detectou que tem 8 cláusulas de penalidade. O Max ia ler
     isso 3 vezes pra ter certeza. Aqui já está flaggado."
   - "Recomendação? **GO COM RESSALVA**. Já diz pro Thiago e Max que precisam
     conversar antes."
5. Rodar o BID 2 (Belém):
   - "Esse aqui pediu modal aéreo. **NO-GO**. Lauto não cota — economizou as
     6 horas do Thiago."
6. Fechar com a frase-bala da §10 do md.

---

## 7. Limitações honestas (declarar ao cliente)

- **Demo usa regex, não LLM.** Em produção a extração é mais precisa.
- **Cálculo de preço-base é stub** (assume 800 km médio). Em produção,
  busca distância via API (Google Maps / Open Source Routing Machine /
  malha própria) e aplica tabela travada com acréscimos.
- **Trechos detectados precisam validação humana** — o demo já indica isso
  no draft ("validar contra malha antes de bater o martelo").
- **OCR não está ativado.** Se Lauto recebe BID escaneado, adicionar
  `ocrmypdf` antes do pipeline.
- **Edital privado da Lauto** pode ter padrão de citação diferente (menos
  jurídico, mais operacional) — calibrar com 1–2 amostras reais.

---

## 8. Métricas para a apresentação

| Métrica | Hoje (Thiago manual) | Com Nokk |
|---|---|---|
| Tempo até GO/NO-GO | 4–8h por edital | <10 segundos |
| BIDs descartados antes de cotar | Após leitura completa | **No primeiro segundo** |
| Custo computacional | — | R$ 1–2 por BID |
| Preço Nokk | — | R$ 890 (BID Heavy) |
| Custo de mão de obra Lauto evitado | R$ 1.500–2.500 (hora-analista) | — |
| Capacidade mensal | 3–4 BIDs | 10+ BIDs |

> Validar números em itálico com Thiago no discovery antes de levar à apresentação.

---

## 9. Como reproduzir tudo

```bash
# Listar BIDs de transporte no PNCP
curl -s 'https://pncp.gov.br/api/search/?tipos_documento=edital&q=transporte%20cargas' | head

# Já temos 3 baixados em docs/editais_exemplo/

# Rodar demo num BID:
python3 scripts/lauto_bid_demo.py docs/editais_exemplo/marinha_pe_90008_2025/ \
  --out docs/pricing/demo_marinha_pe90008.json

# Rodar análise de unit economics:
python3 scripts/lauto_unit_econ.py docs/editais_exemplo/marinha_pe_90008_2025/*.pdf

# Saídas:
ls docs/pricing/
#   demo_marinha_pe90008.json   ← JSON estruturado para Thiago
#   demo_marinha_belem.json
#   demo_exercito_sp.json
#   marinha_pe90008_individual.json   ← unit economics doc-a-doc
#   belem_individual.json
#   exercito_individual.json
```
