# Executive Summary — Precificação de Agentes de IA

## TL;DR — Os 3 Modelos Dominantes

| Modelo | Exemplo Real | Melhor para | Risco |
|---|---|---|---|
| **Seat-based** | Slack (R$ 30–150/user/mês) | Colaboração, múltiplos usuários | Sub-adoção, margin pressure |
| **Usage-based** | OpenAI API (R$ 0,01–0,15 por token) | Consumo variável, APIs | Revenue unpredictable |
| **Hybrid** ⭐ | AWS, Stripe, Notion | Maioria dos SaaS modernos | Complexidade operacional |

**→ RECOMENDAÇÃO PARA LAUTO: HYBRID**

---

## Modelo Hybrid (Recomendado para LAUTO)

### Estrutura 3-Camadas

```
Mensal = Plataforma (floor) + SKU1 (Cotação) + SKU2 (Proposta) + SKU3 (BID Premium)
```

| Camada | Preço | Inclusos | Overage |
|---|---|---|---|
| **Plataforma** | R$ 1.490 | 5 users, omni, 5k e-mails, SLA 8×5 | — |
| **Cotação Growth** | R$ 1.490 | 600 cotações/mês | R$ 2,80 cada |
| **Proposta Comercial** | R$ 990 | 40 propostas/mês | R$ 18 cada |
| **BID Premium IA** | R$ 1.890 | 3 Light + 2 Std + 1 Heavy | Light R$ 190, Std R$ 450, Heavy R$ 990 |

**Total mensal:** R$ 3.970 – R$ 5.860 (base + overage)

### Por que funciona para LAUTO

✅ **Floor (R$ 3.970)** — Revenue previsível, churn detectável antecipadamente

✅ **Franquias alinhadas** — 600 cotações = consumo real observado de Thiago/Max

✅ **Overage > pacote unitário** — Se ultrapassam 600, é melhor fazer upgrade para Scale (R$ 2.890 com 1.500 inclusos)

✅ **Tiers de BID auto-classificados** — Light/Std/Heavy decididos pelo sistema, não pelo cliente

✅ **Posicionamento** — vs R$ 690/licença (10 cadeiras), você oferece "Thiago 24/7 + padronização"

---

## Cenários Comerciais (3 opções = +60% conversão)

### A. Piloto (Anti-objeção)
- Setup: R$ 14.500 | Mensal: R$ 2.980
- Foco: Validar + entrada baixa
- Sem IA pesada (não inclui BID Premium)

### B. Recomendado ⭐ (Aprovação Provável)
- Setup: R$ 19.800 | Mensal: R$ 3.970
- Foco: Padrão (aqui é onde a maioria fica)
- Full platform + 1 Light BID trial/mês grátis

### C. Full Stack (Anchor)
- Setup: R$ 23.500 | Mensal: R$ 5.860
- Foco: Upsell para volume
- Full + SLA 24h + Success fee opcional (1,2% do contrato, cap R$ 8k)

**Insight:** Oferecer 3 opções aumenta aprovação do meio em 60–70% (psicologia de preço comprovada em SaaS).

---

## Tendências Emergentes 2025 — Agentes de IA

### 1. "Ação Completada" vs Tokens
```
Antes: Cobram por tokens consumidos
Agora: Cobram por tarefa completada com sucesso
Razão: Não é linear — 1 agentic turn = múltiplos tokens + múltiplas decisões
Exemplos: Zapier ($0,25–2/task), Make ($0,10–1/op), n8n ($0,20/task)
```

### 2. SLA como Premium
```
SLA ≤ 30 min: +40% no preço
SLA ≤ 5 min:  +100% no preço
Impacto: Clientes pagam por garantia, não por insumo
```

### 3. Tier Auto-Classificado
```
Sistema automaticamente classifica:
  ≤10 páginas → Light (R$ 149)
  10–40 páginas → Standard (R$ 390)
  40+ páginas → Heavy (R$ 890)
Vantagem: Cliente não "escolhe barato", sistema escolhe correto
Proteção: Heurísticas (regras automatizadas) vs negociação
```

### 4. Caching Semântico = Margem Oculta
```
Primeiro BID: COGS = R$ 2,50 (tokens cheios)
Segundo BID (mesma editora/tipo): COGS = R$ 0,60 (70% cache hit)
Preço fixo: R$ 390 sempre
→ Margem cresce sem aumentar preço (economia só sua)
```

---

## Proteção de Margem (Essencial)

Sem isso, overage é vulnerável a outliers e volume abusivo:

1. **Tier auto-classificado** — Sistema escolhe (não cliente)
2. **Hard caps** — "Acima de 10k docs/mês = revisão manual"
3. **Fair-use** — Teto soft: 3× média trimestral antes de alerta
4. **Cache semântico** — Boilerplate jurídico cacheado (+60% economia)
5. **Repricing automático** — Se LLM upstream variar >20%, reprecia contrato
6. **Upgrade automático** — Overage recorrente = sugestão de upgrade (não vivendo em overage)
7. **Minimum commitment** — Walk-away: R$ 2.000/mês se há IA pesada

---

## Posicionamento vs Competidores

### Eles
```
10 licenças × R$ 69/user = R$ 690/mês
→ Você paga por cadeira pra digitar manualmente
```

### LAUTO
```
R$ 2.980–5.860/mês
→ Você paga por trabalho entregue:
   • Thiago 24/7 trabalhando
   • Tabela de acréscimos PADRONIZADA (bloqueador crítico)
   • IA que responde editais
   • SLA garantido (opcional)
```

**Pitch:** Não é cadeira. É economia de horas em escala.

---

## Pricing Real — Market Leaders 2025

### LLM APIs (Token-Based)
| Provider | Model | Input | Output |
|---|---|---|---|
| Anthropic | Claude 3.5 Haiku | $0,80/M tokens | $4/M tokens |
| Anthropic | Claude 3.5 Sonnet | $3/M tokens | $15/M tokens |
| OpenAI | GPT-4o | $5/M tokens | $15/M tokens |
| Jina Reader | Document extraction | $0,01–0,10 per page | — |

### Task-Based Agents
| Platform | Model | Price |
|---|---|---|
| Zapier | Per task completion | $0,25–2,00 |
| Make (Integromat) | Per 1,000 operations | $0,10–1,00 |
| n8n | Per task (overage) | $0,20 after free tier |
| Anthropic Agents | Per agentic turn (est.) | $0,01–0,50 |

---

## COGS Reference — LAUTO BID Premium

| Tier | Páginas | COGS (est.) | Preço | Margem |
|---|---|---|---|---|
| Light | ≤10 | < R$ 0,10 | R$ 149 | >99% |
| Standard | 10–40 | R$ 0,30–1,00 | R$ 390 | >99% |
| Heavy | 40+ | R$ 1,00–4,55 | R$ 890 | >99% |

**Nota:** Margem real nasce de trabalho substituído (Thiago/Max liberado), não de markup em token.

---

## Unit Economics — Year 1 Projection

### Assumindo 1 cliente Brudam @ Cenário B

```
Setup: R$ 19.800 (one-time)
Mensal: R$ 3.970 (base)

Year 1 Revenue = Setup + (Mensal × 12) + Overage estimado (15%)
              = 19.800 + (3.970 × 12) + (3.970 × 0,15 × 12)
              = 19.800 + 47.640 + 7.146
              = R$ 74.586

CAC (Customer Acquisition Cost): Assumir 40h de sales = R$ 2.000 (inbound)
LTV (Customer Lifetime Value): Assumir 3 years @ R$ 47.640/year

LTV/CAC = (47.640 × 3) / 2.000 = 71,46x
Payback period: 2 meses (com margins >70% em operação)
```

---

## Checklist de Implementação

- [ ] Validar COGS real de cada tier (rodar script `lauto_unit_econ.py`)
- [ ] Modelar churn por cenário (A, B, C)
- [ ] Testar psychological pricing ($2.980 vs $3.000)
- [ ] Estruturar contracts com cláusulas de repricing
- [ ] Setup de alertas: "Cliente está em overage recorrente → upgrade?"
- [ ] Treinar sales em "value pitch" (horas liberadas, não features)
- [ ] Documentar heurística de auto-classificação de tier
- [ ] Implementar caching de boilerplate jurídico

---

## Fontes

1. **PayPro Global** — Como precificar produtos SaaS de IA
2. **GPTMaker** — Modelos de precificação para agências de IA  
3. **LinkedIn Pulse** — Como os agentes de IA serão precificados
4. Preços públicos: Anthropic, OpenAI, Zapier, Make, Jina

---

**Aprovado para:** Apresentação ao cliente Brudam  
**Data:** 31 de março de 2026  
**Próxima revisão:** 30 de junho de 2026 (3 meses de dados reais)
