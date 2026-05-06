# 🧮 CUSTO REAL DAS ANÁLISES — NOKK-CHAT
## Baseado em MiniMax M2.5 via OpenRouter · Maio 2026

**Modelo:** MiniMax M2.5 (OpenRouter)  
**Pricing oficial:**
- Input:  $0,15 / 1M tokens  → R$ 0,0000007350 / token
- Output: $1,15 / 1M tokens  → R$ 0,0000056465 / token
- Cache input: $0,075 / 1M tokens (50% desconto em prompts reutilizados)

**Taxa de câmbio:** USD/BRL = R$ 4,91 (06/mai/2026)  
**Contexto máximo:** 196.608 tokens | **Output máximo:** 131.072 tokens

> ⚠️ Todos os custos abaixo são em R$ (reais brasileiros)

---

## 1. GLOSSÁRIO DE UNIDADES

| Unidade | Definição |
|---------|-----------|
| **Interação** | Qualquer troca simples canal↔IA: triagem, status, cotação expressa, confirmação |
| **Análise** | Processamento profundo de documento ou contexto complexo com múltiplos passes LLM |
| **Token** | ~0,75 palavras em português (uma página A4 = ~600–800 tokens) |

---

## 2. CUSTO POR TIPO — CÁLCULO DETALHADO

### ⚡ INTERAÇÃO SIMPLES (canal qualquer — email, WhatsApp, portal)

Uma interação padrão envolve: parse da mensagem + consulta a base + resposta formatada.

```
Pipeline:  1 chamada LLM

Tokens de entrada:
  - Mensagem do cliente:      ~400 tokens
  - System prompt (cached):   ~800 tokens  → custo 50% = R$ 0,000294
  - Contexto/histórico:       ~300 tokens
  Total entrada (non-cached): ~700 tokens

Tokens de saída:
  - Resposta/triagem:         ~500 tokens

─────────────────────────────────────────────────────────
  Entrada (700 tok):   700 × R$0,0000007350 = R$ 0,000515
  Saída  (500 tok):    500 × R$0,0000056465 = R$ 0,002823
  Cache system prompt: 800 × R$0,0000003675 = R$ 0,000294
  ────────────────────────────────────────────────────────
  CUSTO LLM:                                = R$ 0,003632
  Infrastructure overhead (30%):            = R$ 0,001090
  ────────────────────────────────────────────────────────
  CUSTO CARREGADO POR INTERAÇÃO:            ≈ R$ 0,0047
```

| Métrica | Valor |
|---------|-------|
| Custo carregado | **R$ 0,0047** |
| Preço cobrado (padrão) | **R$ 0,76** |
| Preço cobrado (volume) | **R$ 0,46** |
| Margem bruta (padrão) | **99,4%** |
| Margem bruta (volume) | **99,0%** |

> ✅ Preços de interação estão corretos e saudáveis.

---

### 📧 COTAÇÃO PADRÃO (email/portal — sem documento anexo)

```
Pipeline:  1–2 chamadas LLM

Etapa 1 — Parse + cálculo:
  Entrada: 3.500 tokens (email + tabela de preços + rota)
  Saída:   1.200 tokens (cotação formatada)

Etapa 2 — Envio (optional formatting pass):
  Entrada: 2.000 tokens
  Saída:     800 tokens

─────────────────────────────────────────────────────────
  Entrada total (5.500 tok): × R$0,0000007350 = R$ 0,00404
  Saída total  (2.000 tok):  × R$0,0000056465 = R$ 0,01129
  ────────────────────────────────────────────────────────
  CUSTO LLM:                                 = R$ 0,01533
  Infrastructure overhead (30%):             = R$ 0,00460
  ────────────────────────────────────────────────────────
  CUSTO CARREGADO:                           ≈ R$ 0,020
```

---

### 📋 PROPOSTA COMERCIAL (email estruturado — sem PDF longo)

```
Pipeline:  2 chamadas LLM

Etapa 1 — Análise de aderência + contexto:
  Entrada: 6.000 tokens (email vendedor + base cobertura + contexto)
  Saída:   1.500 tokens (structured extraction)

Etapa 2 — Geração da proposta:
  Entrada: 5.500 tokens (extracted + templates + pricing)
  Saída:   2.500 tokens (proposta formatada)

─────────────────────────────────────────────────────────
  Entrada total (11.500 tok): × R$0,0000007350 = R$ 0,00845
  Saída total   ( 4.000 tok): × R$0,0000056465 = R$ 0,02259
  ────────────────────────────────────────────────────────
  CUSTO LLM:                                  = R$ 0,03104
  Infrastructure overhead (30%):              = R$ 0,00931
  ────────────────────────────────────────────────────────
  CUSTO CARREGADO:                            ≈ R$ 0,040
```

---

### 🔗 COTAÇÃO COM ANÁLISE DE SINERGIA (verifica mix com clientes existentes)

```
Pipeline:  3 chamadas LLM + consulta histórico

Etapa 1 — Parse da solicitação:
  Entrada:  3.000 tokens | Saída: 1.000 tokens

Etapa 2 — Busca e comparação de sinergia (histórico de cargas):
  Entrada: 10.000 tokens (request + clientes histórico + rotas)
  Saída:    2.000 tokens (match de sinergia)

Etapa 3 — Geração da cotação + recomendação:
  Entrada:  6.000 tokens | Saída: 2.500 tokens

─────────────────────────────────────────────────────────
  Entrada total (19.000 tok): × R$0,0000007350 = R$ 0,01397
  Saída total   ( 5.500 tok): × R$0,0000056465 = R$ 0,03106
  ────────────────────────────────────────────────────────
  CUSTO LLM:                                  = R$ 0,04503
  Infrastructure overhead (30%):              = R$ 0,01351
  ────────────────────────────────────────────────────────
  CUSTO CARREGADO:                            ≈ R$ 0,058
```

---

### 📄 ANÁLISE DE BID — Médio (documento 20–35 páginas)

```
Pipeline:  Jina extraction + 4 chamadas LLM

Jina AI (extração PDF/Word):
  Custo fixo por doc 20–35 págs:            = R$ 0,147
  ($0.030 × R$4,91)

Etapa 1 — Parsing estrutural (campos, volumes, specs):
  Entrada: 25.000 tok (conteúdo Jina + system prompt)
  Saída:    2.500 tok (JSON estruturado)

Etapa 2 — Diagnóstico de cobertura geográfica:
  Entrada:  8.500 tok (structured data + base de cobertura)
  Saída:    1.800 tok (mapa de cobertura + gaps)

Etapa 3 — Cálculo de preço + especificidades:
  Entrada: 10.000 tok (structured + tabela preços + regras)
  Saída:    2.500 tok (cenários fracionado/dedicado)

Etapa 4 — Síntese + relatório executivo:
  Entrada:  9.000 tok (all outputs + template relatório)
  Saída:    4.000 tok (relatório final completo)

─────────────────────────────────────────────────────────
  Entrada total (52.500 tok): × R$0,0000007350 = R$ 0,03859
  Saída total   (10.800 tok): × R$0,0000056465 = R$ 0,06098
  Jina extraction:                             = R$ 0,14700
  ────────────────────────────────────────────────────────
  CUSTO API DIRETO:                           = R$ 0,24657
  Infrastructure overhead (30%):              = R$ 0,07397
  ────────────────────────────────────────────────────────
  CUSTO CARREGADO — BID MÉDIO:               ≈ R$ 0,32
```

---

### 📄 ANÁLISE DE BID — Grande (documento 60–120 páginas)

```
Pipeline:  Jina extraction (2 passes) + 5 chamadas LLM

Jina AI (extração longa):
  Custo estimado:                            = R$ 0,245
  ($0.050 × R$4,91)

Etapas LLM (maior volume de tokens):
  Entrada total: ~105.000 tok
  Saída total:   ~15.000 tok

─────────────────────────────────────────────────────────
  Entrada (105.000 tok): × R$0,0000007350  = R$ 0,07718
  Saída   ( 15.000 tok): × R$0,0000056465  = R$ 0,08470
  Jina:                                     = R$ 0,24500
  ────────────────────────────────────────────────────────
  CUSTO API DIRETO:                         = R$ 0,40688
  Infrastructure overhead (30%):            = R$ 0,12206
  ────────────────────────────────────────────────────────
  CUSTO CARREGADO — BID GRANDE:            ≈ R$ 0,53
```

---

## 3. CONSOLIDADO DE CUSTOS

```
┌──────────────────────────────────┬────────────┬──────────────────┐
│ Tipo de Processamento            │ Custo API  │ Custo Carregado  │
│                                  │ (direto)   │ (infra +30%)     │
├──────────────────────────────────┼────────────┼──────────────────┤
│ Interação simples                │ R$ 0,0036  │ R$ 0,0047        │
│ Cotação Padrão                   │ R$ 0,0153  │ R$ 0,020         │
│ Proposta Comercial               │ R$ 0,0310  │ R$ 0,040         │
│ Cotação com Sinergia             │ R$ 0,0450  │ R$ 0,058         │
│ Análise de BID — médio (20–35p)  │ R$ 0,2466  │ R$ 0,32          │
│ Análise de BID — grande (60–120p)│ R$ 0,4069  │ R$ 0,53          │
└──────────────────────────────────┴────────────┴──────────────────┘
```

---

## 4. DEFINIÇÃO DE MARGEM E PREÇOS RECOMENDADOS

### Critérios de precificação:
- **Floor de margem bruta:** 95% (padrão SaaS plataforma B2B)
- **Referência de valor entregue:** % do custo de trabalho humano economizado
- **Fator competitivo:** Comparação com mercado logístico BR

### Tabela de precificação revisada:

```
┌──────────────────────────────┬──────────┬──────────┬────────────┬──────────┬─────────────────────────┐
│ Tipo                         │ Custo    │ Preço    │ Preço      │ Margem   │ Valor entregue          │
│                              │ Carregado│ Padrão   │ Volume     │ Bruta    │ ao cliente              │
├──────────────────────────────┼──────────┼──────────┼────────────┼──────────┼─────────────────────────┤
│ Interação simples            │ R$ 0,005 │ R$ 0,76  │ R$ 0,46   │ 99,3%    │ Fluidez operacional     │
│ Cotação Padrão               │ R$ 0,020 │ —        │ —          │ —        │ incluso no plano        │
│ Proposta Comercial           │ R$ 0,040 │ R$ 3,00  │ R$ 2,00   │ 98,7%    │ 30min economia (R$25)   │
│ Cotação com Sinergia         │ R$ 0,058 │ R$ 5,00  │ R$ 3,50   │ 98,8%    │ 1h economia (R$50)      │
│ Análise de BID — médio       │ R$ 0,32  │ R$ 12,00 │ R$ 8,00   │ 97,3%    │ 6h economia (R$300)     │
│ Análise de BID — grande      │ R$ 0,53  │ R$ 18,00 │ R$ 12,00  │ 97,1%    │ 1 dia+ economia (R$500+)│
└──────────────────────────────┴──────────┴──────────┴────────────┴──────────┴─────────────────────────┘
```

> **"Preço Volume"** se aplica a partir de pacotes mensais ou contratos anuais.

---

## 5. COMPARATIVO: PREÇO ANTERIOR vs REVISADO

```
                    ANTERIOR    REVISADO    DIFERENÇA   MARGEM BRUTA
────────────────────────────────────────────────────────────────────
BID análise          R$ 60,00   R$ 12,00    -80%         97,3%
Proposta Comercial   R$ 30,00   R$  3,00    -90%         98,7%
Cotação Sinergia     R$ 20,00   R$  5,00    -75%         98,8%
Interação            R$  0,76   R$  0,76    =            99,3%
────────────────────────────────────────────────────────────────────
```

> Os preços anteriores eram viáveis financeiramente (margem >99%), mas
> potencialmente bloqueavam a venda por pareceram caros sem justificativa.
> Os preços revisados mantêm margens excelentes (97%+) e são muito
> mais fáceis de justificar e fechar.

---

## 6. IMPACTO NA ASSINATURA — CASO LAUTOS

### ANTES (modelo antigo):
```
Email (100k):                     R$ 1.200,00
15 BIDs × R$ 60:                  R$   900,00
20 Propostas × R$ 30:             R$   600,00
30 Cotações Sinergia × R$ 20:     R$   600,00
────────────────────────────────────────────
TOTAL:                            R$ 3.300,00/mês
```

### DEPOIS (modelo revisado):
```
Email (100k):                     R$ 1.200,00
1.000 Interações:                 incluso
15 BIDs × R$ 12 (volume):         R$   180,00
20 Propostas × R$ 2 (volume):     R$    40,00
30 Cotações Sinergia × R$ 3,50:   R$   105,00
────────────────────────────────────────────
TOTAL:                            R$ 1.525,00/mês
```

### Novo ROI para Lautos:
```
Economia gerada:    R$ 9.000/mês
Investimento:       R$ 1.525/mês
────────────────────────────────
ROI:                490%
Lucro líquido:      R$ 7.475/mês
Payback:            5 dias
```

> De R$ 3.300 → R$ 1.525/mês com margem mantida acima de 97%.  
> O argumento de venda fica muito mais forte.

---

## 7. PLANOS REVISADOS DE ASSINATURA

### 📦 STARTER — R$ 1.200/mês
*(email + operações básicas, sem análises avançadas)*

| Componente | Incluso | Extra |
|------------|---------|-------|
| Emails recebidos | 100.000/mês | +R$10/10k |
| Interações | 500/mês | +R$0,76 |
| Análise BID | — | R$12/análise |
| Proposta Comercial | — | R$3,00/análise |
| Cotação com Sinergia | — | R$5,00/análise |

Perfil: operações pequenas, apenas automação de emails e cotações simples.

---

### 🚀 PROFESSIONAL — R$ 1.600/mês ⭐ RECOMENDADO LAUTOS

| Componente | Incluso | Extra |
|------------|---------|-------|
| Emails recebidos | 100.000/mês | +R$10/10k |
| Interações | 1.000/mês | +R$0,46 (volume) |
| Análise BID | 15/mês | +R$8,00/análise |
| Proposta Comercial | 20/mês | +R$2,00/análise |
| Cotação com Sinergia | 30/mês | +R$3,50/análise |
| Dashboard + Suporte | ✅ Incluso | — |
| Reunião mensal 1h | ✅ Incluso | — |
| Rollover de análises (30%) | ✅ Incluso | — |

**ROI para Lautos: 490% · Payback: 5 dias · Investimento: R$ 1.600/mês**

---

### 🏢 ENTERPRISE — R$ 3.200/mês

| Componente | Incluso | Extra |
|------------|---------|-------|
| Emails recebidos | 200.000/mês | +R$8/10k |
| Interações | 5.000/mês | +R$0,46 (volume) |
| Análise BID | 30/mês | +R$8,00/análise |
| Proposta Comercial | 50/mês | +R$2,00/análise |
| Cotação com Sinergia | 100/mês | +R$3,50/análise |
| WhatsApp Business | ✅ Incluso | — |
| Integração ERP/WMS | ✅ Incluso | — |
| SLA 99,5% + Manager dedicado | ✅ Incluso | — |

---

## 8. PITCH REVISADO (30 segundos)

> "Max, Thiago —
>
> R$ 1.600 por mês. Isso cobre 100.000 emails recebidos,
> 1.000 interações automáticas, 15 análises completas de BID,
> 20 propostas e 30 cotações com sinergia.
>
> Vocês economizam R$ 9.000/mês.
> ROI de 490%. Payback em 5 dias.
>
> Nenhum competitor faz análise de BID por R$ 12 por documento
> — porque a maioria nem faz. Risco zero. Piloto 30 dias."

---

## 9. NOTAS TÉCNICAS

### Por que os custos são tão baixos?
MiniMax M2.5 é um modelo de alto desempenho com precificação agressiva
(especialmente output a $1,15/M). Para contexto:
- GPT-4o: $2,50/$10,00 por M tokens (input/output) — 16x mais caro no output
- Claude Sonnet 4: $3/$15 por M tokens — 13x mais caro no output
- MiniMax M2.5: $0,15/$1,15 — base para este cálculo

### Modelo pode mudar?
Sim. Este cálculo é baseado em preços de maio/2026.
Recomenda-se revisar trimestralmente e construir uma camada de abstração
no backend (LLM router) para trocar de modelo conforme custos mudam.

### Overhead de infraestrutura (30% aplicado):
- Hosting Railway/VPS: ~R$200/mês (dividido por volume)
- Redis/Postgres: ~R$100/mês
- OpenRouter API key management: incluso no pricing
- Monitoramento + logs: ~R$50/mês
- Retries / erros médios: ~15% das chamadas

---

*Cálculos baseados em: MiniMax M2.5 via OpenRouter ($0,15/$1,15 por M tokens) · USD/BRL R$4,91 · 06/mai/2026*  
*Documento gerado pela Nexcode para revisão de modelo de negócio nokk-chat / LAUTOS*
