# Unit Economics — Nokk × Lauto

> Apêndice da proposta `lauto-proposta-nokk.md` §9.
> Calibrado via `scripts/lauto_unit_econ.py` (Haiku 3.5 + Sonnet 3.7 + Jina, FX 5,20).
> **Status:** estimativa. Recalibrar com 1–2 editais reais da Lauto antes de fechar.

---

## 1. Premissas

| Premissa | Valor | Justificativa |
|---|---|---|
| FX BRL/USD | 5,20 | Cenário base; cláusula de revisão se variar > 20% |
| Tokens por palavra PT-BR | 1,35 | Tokenizer médio Anthropic/OpenAI |
| Output / Input ratio | 25–35% | JSON estruturado + sumário executivo |
| Cache hit boilerplate | Std 50% / Heavy 65% | Editais BR repetem cláusulas jurídicas |
| Modelo Cotação | GPT-4o-mini | $0,15/$0,60 por M tokens |
| Modelo BID Std/Heavy map | Haiku 3.5 | $0,80/$4,00 por M tokens |
| Modelo BID reduce | Sonnet 3.7 | $3,00/$15,00 por M tokens |
| Pré-processamento | Jina Reader | $0,02 / M tokens equivalentes |

---

## 2. Custo por unidade de trabalho

### 2.1 Cotação Padrão (alto volume, baixa complexidade)

| Componente | Tokens IN | Tokens OUT | Custo USD | Custo BRL |
|---|---|---|---|---|
| Jina Reader | 1.500 | — | $0,00003 | R$ 0,0002 |
| GPT-4o-mini extract+answer | 1.500 | 375 | $0,00045 | R$ 0,002 |
| **TOTAL** | — | — | **$0,0005** | **R$ 0,003** |

**Preço lista (overage):** R$ 2,80 → **margem 99,9%**.

### 2.2 Proposta Comercial (média)

| Componente | Tokens IN | Tokens OUT | Custo USD | Custo BRL |
|---|---|---|---|---|
| Jina Reader | 6.000 | — | $0,00012 | R$ 0,0006 |
| Haiku extract | 6.000 | 600 | $0,0072 | R$ 0,037 |
| Sonnet reason (50% cache) | 3.000 | 900 | $0,022 | R$ 0,11 |
| **TOTAL** | — | — | **$0,029** | **R$ 0,15** |

**Preço lista (overage):** R$ 18,00 → **margem 99,2%**.

### 2.3 BID — 3 tiers

#### Tier Light (≤10 págs, sem cargas especiais)

| Componente | Tokens IN | Tokens OUT | Custo USD | Custo BRL |
|---|---|---|---|---|
| Jina Reader | 4.000 | — | $0,00008 | R$ 0,0004 |
| GPT-4o-mini extract+answer | 4.000 | 1.000 | $0,0012 | R$ 0,006 |
| **TOTAL** | — | — | **$0,0013** | **R$ 0,007** |

**Preço lista:** R$ 149 → **margem 99,99%**.

#### Tier Standard (10–40 págs, 1 especificidade)

| Componente | Tokens IN | Tokens OUT | Custo USD | Custo BRL |
|---|---|---|---|---|
| Jina Reader | 14.000 | — | $0,00028 | R$ 0,001 |
| Haiku map | 14.000 | 1.400 | $0,017 | R$ 0,087 |
| Sonnet reduce (50% cache) | 7.000 | 2.100 | $0,053 | R$ 0,27 |
| **TOTAL** | — | — | **$0,070** | **R$ 0,36** |

**Preço lista:** R$ 390 → **margem 99,9%**.

#### Tier Heavy (40+ págs, multi-trecho/multi-cargas)

| Componente | Tokens IN | Tokens OUT | Custo USD | Custo BRL |
|---|---|---|---|---|
| Jina Reader | 40.000 | — | $0,0008 | R$ 0,004 |
| Haiku map | 40.000 | 3.200 | $0,045 | R$ 0,23 |
| Sonnet reduce (65% cache) | 14.000 | 4.900 | $0,116 | R$ 0,60 |
| **TOTAL** | — | — | **$0,162** | **R$ 0,84** |

**Preço lista:** R$ 890 → **margem 99,9%**.

#### Worst case (Heavy outlier 60k palavras, 0% cache, FX 5,50)

| Componente | Tokens IN | Tokens OUT | Custo USD |
|---|---|---|---|
| Jina + Haiku map + Sonnet reduce (sem cache) | 81.000 | ~40.000 | $0,83 |

**COGS BRL worst case:** R$ 4,55. Mesmo a R$ 890 de preço → **margem 99,5%**.

---

## 3. Cenários mensais — receita vs COGS

### 3.1 Cenário A (Piloto)

| Item | Valor |
|---|---|
| Plataforma (5k e-mails) | R$ 1.490 |
| Cotação Growth (600 inclusas) | R$ 1.490 |
| **Receita mensal** | **R$ 2.980** |
| COGS plataforma (e-mail SES + infra) | ~R$ 65 |
| COGS Cotação (600 × R$ 0,003) | ~R$ 2 |
| **COGS total** | **~R$ 67** |
| **Margem bruta** | **97,8%** |

### 3.2 Cenário B (Recomendado ★)

| Item | Valor |
|---|---|
| Plataforma + Cotação Growth + Proposta + BID Light trial | R$ 3.970 |
| **Receita mensal** | **R$ 3.970** |
| COGS plataforma | ~R$ 65 |
| COGS Cotação (600 × R$ 0,003) | ~R$ 2 |
| COGS Proposta (40 × R$ 0,15) | ~R$ 6 |
| COGS BID trial (1 Light) | ~R$ 0,01 |
| **COGS total** | **~R$ 73** |
| **Margem bruta** | **98,2%** |

### 3.3 Cenário C (Full Stack)

| Item | Valor |
|---|---|
| Plataforma + Cotação Growth + Proposta + BID Premium | R$ 5.860 |
| **Receita mensal** | **R$ 5.860** |
| COGS plataforma | ~R$ 65 |
| COGS Cotação (600) | ~R$ 2 |
| COGS Proposta (40) | ~R$ 6 |
| COGS BID Premium (3 Light + 2 Std + 1 Heavy) | ~R$ 1,57 |
| **COGS total** | **~R$ 75** |
| **Margem bruta** | **98,7%** |

---

## 4. Estresses

| Cenário de estresse | Impacto | Margem após estresse |
|---|---|---|
| Volume 3× do plano (overage massivo) | +Receita, +COGS proporcional | mantém >97% |
| Custo LLM dobra (GPT-5/Claude 5) | COGS por unidade dobra | mantém >97% |
| Cliente joga 5 BIDs Heavy outliers no mês | +R$ 23 de COGS | mantém >99% |
| Cliente exige flat all-you-can-eat | sem cap → risco real | **bloquear no contrato** |
| FX 6,50 por desvalorização | +25% no COGS de tokens | mantém >97% |

**Insight estrutural:** o COGS de tokens é tão pequeno que **nem dobra de preço de upstream + FX ruim quebra margem**. O risco real é **operacional** (suporte, edge cases, falha de classificador), não computacional.

---

## 5. Onde o custo real mora (não é token)

A precificação é defendida por **custo de servir**, não custo de inferência:

| Item de custo real | Estimativa mensal | % da receita Cenário B |
|---|---|---|
| Suporte L1/L2 (parte de 1 FTE) | R$ 1.200 | 30% |
| Engenharia de manutenção (parte de 1 FTE) | R$ 800 | 20% |
| Infra Nokk (compute/storage/observabilidade) | R$ 250 | 6% |
| Tokens + Jina | R$ 75 | 2% |
| **Total custo de servir** | **R$ 2.325** | **58%** |
| **Margem operacional efetiva (B)** | **R$ 1.645** | **42%** |

**Conclusão:** margem bruta nominal ≈ 98%, mas margem **operacional realista** após overhead de suporte e manutenção é 35–45%. Esse é o número que vale para tomada de decisão de pricing — e ainda assim é saudável.

---

## 6. CSV equivalente (para colar em Sheets)

```csv
unidade,cogs_brl_min,cogs_brl_max,preco_lista_brl,margem_bruta_pct
cotacao_padrao,0.003,0.40,2.80,99
proposta_comercial,0.15,2.50,18.00,99
bid_light,0.007,0.10,149.00,99
bid_standard,0.36,1.00,390.00,99
bid_heavy,0.84,4.55,890.00,99
email_outbound_1k,2.50,4.00,25.00,85
```

---

## 7. Recalibração obrigatória pós-fechamento

Em até 60 dias após go-live, rodar `scripts/lauto_unit_econ.py` contra **30 dias reais de tráfego** e:

1. Conferir distribuição de tier de BID (esperado: ~60% Std / 30% Light / 10% Heavy).
2. Conferir cache hit real vs estimado.
3. Conferir tokens médios por cotação (esperado < 2k IN).
4. Ajustar franquias do Cenário B no aniversário 90 dias.
5. Documentar desvios e revisar cláusula 9.4.5 se LLM upstream variou.
