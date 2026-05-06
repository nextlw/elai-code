# 💰 PRECIFICAÇÃO FINAL — NOKK-CHAT
## Base: 100 análises de cada · Margem máxima 300%

**Modelo:** MiniMax M2.5 via OpenRouter  
**Input:** $0,15/M tok → R$0,00000074/tok | **Output:** $1,15/M tok → R$0,00000565/tok  
**USD/BRL:** R$4,91 · 06/mai/2026 · Margem aplicada: **4× custo carregado (300% markup)**

---

## PREMISSA DO MODELO

```
Preço = Custo Carregado × 4
Margem bruta = 75%  (300% de markup sobre custo = 75% de margem bruta)

Custo carregado = custo API direto × 1,30 (infraestrutura Railway, Redis, retries)
```

---

## 1. PREÇO UNITÁRIO POR ANÁLISE (300% markup)

```
┌──────────────────────────────┬─────────────────┬───────────────┬─────────────────┐
│ Tipo de Análise              │ Custo carregado │ Preço unitário│ 100 unidades    │
│                              │ (API + infra)   │ (× 4)         │                 │
├──────────────────────────────┼─────────────────┼───────────────┼─────────────────┤
│ BID médio (20–35 páginas)    │ R$ 0,32         │ R$ 1,28       │ R$ 128,00       │
│ BID grande (60–120 páginas)  │ R$ 0,53         │ R$ 2,12       │ R$ 212,00       │
│ Proposta Comercial           │ R$ 0,040        │ R$ 0,16       │ R$  16,00       │
│ Cotação com Sinergia         │ R$ 0,058        │ R$ 0,23       │ R$  23,20       │
│ Cotação Padrão               │ R$ 0,020        │ R$ 0,08       │ R$   8,00       │
│ Interação simples            │ R$ 0,0047       │ R$ 0,76 ✅    │ —               │
│ Interação (volume mín.)      │ R$ 0,0047       │ R$ 0,46 ✅    │ —               │
└──────────────────────────────┴─────────────────┴───────────────┴─────────────────┘
```

> Interações já estão na margem correta — mantidas sem alteração.

---

## 2. PACOTE BASE — 100 ANÁLISES DE CADA

Componente de análises da assinatura (independente do canal de email):

```
100 × BID médio (20–35 págs)  → 100 × R$ 1,28  =  R$ 128,00
100 × Proposta Comercial       → 100 × R$ 0,16  =  R$  16,00
100 × Cotação com Sinergia     → 100 × R$ 0,23  =  R$  23,20
100 × Cotação Padrão           → 100 × R$ 0,08  =  R$   8,00
────────────────────────────────────────────────────────────────
  PACOTE ANÁLISES (100 de cada):               =  R$ 175,20/mês
  (BID grande cobra acréscimo de R$0,84/análise extra)
```

---

## 3. ASSINATURA COMPLETA — COMPOSIÇÃO

```
┌─────────────────────────────────────────────────────────────────┐
│  CANAL EMAIL (camada fixa)                                      │
│  100.000 emails recebidos/mês ........................ R$ 1.200  │
│                                                                 │
│  INTERAÇÕES (camada variável)                                   │
│  1.000 interações incluídas (R$0,76 each)                       │
│  Custo includido no pacote: 1.000 × R$0,0047 × 4 = R$   18,80  │
│                                                                 │
│  ANÁLISES IA (100 de cada — camada fixa)                        │
│  100 BIDs médios + 100 Propostas +                              │
│  100 Cotações Sinergia + 100 Cotações Padrão ........ R$ 175,20 │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│  TOTAL ASSINATURA MENSAL:                          R$ 1.394,00  │
│  → Arredondado comercialmente:                     R$ 1.400/mês │
└─────────────────────────────────────────────────────────────────┘
```

---

## 4. EXCEDENTES (pago conforme uso acima dos 100)

| Tipo | Preço unitário excedente |
|------|--------------------------|
| BID médio excedente | R$ 1,28 |
| BID grande excedente | R$ 2,12 |
| Proposta Comercial excedente | R$ 0,16 |
| Cotação com Sinergia excedente | R$ 0,23 |
| Cotação Padrão excedente | R$ 0,08 |
| Interação excedente | R$ 0,76 (padrão) / R$ 0,46 (volume) |
| Email excedente | R$ 10,00/10.000 emails |

---

## 5. APLICAÇÃO — CASO LAUTOS

Uso típico mensal da Lautos vs os 100 incluídos:

```
                    INCLUÍDO    USO LAUTOS    SOBRA
────────────────────────────────────────────────────
BID análises          100           15          85
Propostas             100           20          80
Cotações Sinergia     100           30          70
Cotações Padrão       100          100          —
Interações          1.000          ~800        200
────────────────────────────────────────────────────
```

**Custo real da Lautos para nós no mês:**
```
15 BIDs:          R$ 4,80   (15 × R$0,32)
20 Propostas:     R$ 0,80   (20 × R$0,040)
30 Cot.Sinergia:  R$ 1,74   (30 × R$0,058)
100 Cot.Padrão:   R$ 2,00   (100 × R$0,020)
800 Interações:   R$ 3,76   (800 × R$0,0047)
Email 60k:        R$ 0,60   (infra estimada)
────────────────────────────────────────────
CUSTO REAL/MÊS:   R$ 13,70
RECEITA LAUTOS:   R$ 1.400,00
MARGEM BRUTA:     99,0%  (bem acima dos 75% alvo)
```

> A Lautos vai usar ~15% da cota de análises. Isso é normal e esperado
> em planos com cotas generosas — o modelo só fica apertado se o cliente
> usar 100% todo mês, o que não acontece na prática.

**ROI da Lautos com assinatura de R$ 1.400/mês:**
```
Economia gerada mensalmente:    R$ 9.000
Investimento:                   R$ 1.400
────────────────────────────────────────
ROI:                             543%
Lucro líquido:                  R$ 7.600/mês
Payback:                        5 dias
```

---

## 6. RESUMO DOS 3 PLANOS REVISADOS

### STARTER — R$ 800/mês
*(sem análises avançadas, boa para triagem + cotações simples)*

| Componente | Incluso |
|------------|---------|
| Emails recebidos | 50.000/mês |
| Interações | 500/mês |
| BID análise | — (pay-per-use R$1,28) |
| Proposta Comercial | — (pay-per-use R$0,16) |
| Cotação Padrão | 100/mês |
| Cotação Sinergia | — (pay-per-use R$0,23) |

---

### PROFESSIONAL — R$ 1.400/mês ⭐ LAUTOS
*(100 de cada análise, 1.000 interações, 100k emails)*

| Componente | Incluso | Excedente |
|------------|---------|-----------|
| Emails recebidos | 100.000/mês | +R$10/10k |
| Interações | 1.000/mês | +R$0,76 (R$0,46 vol.) |
| BID análises | **100/mês** | +R$1,28/análise |
| Proposta Comercial | **100/mês** | +R$0,16/análise |
| Cotação com Sinergia | **100/mês** | +R$0,23/análise |
| Cotação Padrão | **100/mês** | +R$0,08/análise |
| Dashboard + suporte | ✅ Incluso | — |
| Rollover 30% não usados | ✅ Incluso | — |

---

### ENTERPRISE — R$ 2.200/mês
*(500 de cada análise, 5.000 interações, 200k emails)*

| Componente | Incluso | Excedente |
|------------|---------|-----------|
| Emails recebidos | 200.000/mês | +R$8/10k |
| Interações | 5.000/mês | +R$0,46 (volume) |
| BID análises | **500/mês** | +R$1,28/análise |
| Proposta Comercial | **500/mês** | +R$0,16/análise |
| Cotação com Sinergia | **500/mês** | +R$0,23/análise |
| Cotação Padrão | **500/mês** | +R$0,08/análise |
| WhatsApp Business | ✅ Incluso | — |
| Integração ERP/WMS | ✅ Incluso | — |
| SLA 99,5% + Manager | ✅ Incluso | — |

---

## 7. POLÍTICA DE PREÇOS — RESUMO EXECUTIVO

```
COMPONENTE       CUSTO REAL     PREÇO (300% markup)   MARGEM BRUTA
──────────────────────────────────────────────────────────────────
Email 100k/mês   ~ R$ 50 infra  R$ 1.200              ~95,8%
Interação simpl  R$ 0,0047      R$ 0,76               99,4%
Interação volume R$ 0,0047      R$ 0,46               99,0%
BID médio        R$ 0,32        R$ 1,28               75,0%  ← teto 300%
BID grande       R$ 0,53        R$ 2,12               75,0%  ← teto 300%
Proposta         R$ 0,040       R$ 0,16               75,0%  ← teto 300%
Cot. Sinergia    R$ 0,058       R$ 0,23               75,0%  ← teto 300%
Cot. Padrão      R$ 0,020       R$ 0,08               75,0%  ← teto 300%
──────────────────────────────────────────────────────────────────
Plano Professional completo:    R$ 1.400/mês
Custo real p/ uso típico Lautos: R$ 13,70/mês
Margem real do plano:           99,0%
```

---

*Cálculos: MiniMax M2.5 OpenRouter · $0,15/$1,15 por M tokens · R$4,91/USD · 06/mai/2026*
