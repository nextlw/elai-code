# 💳 MODELO DE ASSINATURA — NOKK-CHAT
## Estrutura de Precificação por Componente + Planos Mensais

**Preparado por:** Nexcode / TLW  
**Data:** 06 de Maio de 2026  
**Referência:** Proposta LAUTOS + Estrutura de Custos Real

---

## 1. LÓGICA DE PRECIFICAÇÃO POR COMPONENTE

O modelo de assinatura é construído em **três camadas independentes**, cada uma com precificação própria:

```
┌─────────────────────────────────────────────────────────────────┐
│  CAMADA 1 — CANAL DE EMAIL (valor fixo por faixa de volume)     │
│  CAMADA 2 — INTERAÇÕES (valor variável por unidade processada)  │
│  CAMADA 3 — ANÁLISES IA (valor por documento/complexidade)      │
└─────────────────────────────────────────────────────────────────┘
```

---

### 📧 CAMADA 1 — Canal de Email (Recebimento)

Modelo de **preço fixo por faixa**, independente do número de análises ou interações.

| Faixa de Emails Recebidos/mês | Valor Mensal |
|-------------------------------|--------------|
| Até 10.000 emails             | R$ 300       |
| Até 30.000 emails             | R$ 600       |
| Até 60.000 emails             | R$ 900       |
| **Até 100.000 emails**        | **R$ 1.200** |
| Até 200.000 emails            | R$ 1.800     |
| Acima de 200.000 emails       | A negociar   |

> ⚠️ Este valor cobre apenas o **recebimento, parsing e roteamento automático** dos emails.  
> Respostas automáticas e triagem são contabilizadas como **interações (Camada 2)**.  
> Análises de documentos (BIDs, propostas) são contabilizadas na **Camada 3**.

---

### ⚡ CAMADA 2 — Interações

Custo unitário base: **R$ 0,0076 por interação** (0,76 centavos)

Uma **interação** é qualquer processamento simples da plataforma:
- Resposta automática a email de cotação
- Triagem e classificação de mensagem recebida
- Consulta a base de dados geográfica/preços
- Envio de proposta padrão
- Mensagem WhatsApp processada
- Confirmação de recebimento / status
- Notificação gerada automaticamente

> **Nota:** Uma cotação padrão completa consome em média **8–12 interações**  
> (recebimento → parsing → cálculo → consulta → formatação → envio → confirmação)

#### Custo real por fluxo (referência interna):

| Fluxo                          | Interações estimadas | Custo IA (0,0076×) |
|-------------------------------|----------------------|--------------------|
| Cotação Padrão completa        | ~10                  | R$ 0,076           |
| Proposta Comercial simples     | ~15                  | R$ 0,114           |
| Resposta FAQ / status          | ~3                   | R$ 0,023           |
| Triagem + encaminhamento email | ~5                   | R$ 0,038           |

---

### 🧠 CAMADA 3 — Análises IA (por documento)

Análises são processamentos **profundos de documentos** que envolvem extração via Jina + raciocínio via Claude. Precificadas individualmente por tipo e complexidade.

| Tipo de Análise               | Tempo economizado | Valor por análise |
|-------------------------------|-------------------|-------------------|
| **Análise de BID**            | 4–7 dias → 4–6h   | **R$ 60,00**      |
| **Proposta Comercial**        | 2–4h → 15min      | **R$ 30,00**      |
| **Cotação com Sinergia**      | 1–2h → 10min      | **R$ 20,00**      |
| **Análise de Cobertura**      | 30min → 2min      | **R$ 10,00**      |
| Relatório de Concorrência     | 3–5h → 20min      | **R$ 40,00**      |

> Cada análise de BID inclui: extração automática, diagnóstico de cobertura,  
> identificação de especificidades, análise de sinergia e recomendação decisória.

---

## 2. PLANOS DE ASSINATURA

### Composição de um plano:

```
PLANO = (Email fixo) + (Pacote de interações incluso) + (Cota de análises)
```

Excedentes são cobrados conforme tabela acima.

---

### 📦 PLANO STARTER — R$ 1.500/mês

Ideal para: Operações pequenas, início de automação (piloto pós-validação)

| Componente                         | Limite incluído | Valor extra |
|------------------------------------|-----------------|-------------|
| Emails recebidos                   | até 100.000/mês | +R$10/10k   |
| Interações                         | até 1.000/mês   | +R$0,05/int |
| Análises de BID                    | até 5/mês       | +R$60/BID   |
| Análises de Proposta Comercial     | até 5/mês       | +R$30/prop  |
| Análises de Cotação com Sinergia   | até 10/mês      | +R$20/cot   |
| Dashboard de métricas              | ✅ Incluso       | —           |
| Suporte técnico (9–18h seg-sex)    | ✅ Incluso       | —           |

**Estimativa para perfil básico:** R$ 1.500/mês  
**Economia gerada (estimativa):** R$ 3.500–4.500/mês  
**ROI:** ~233%

---

### 🚀 PLANO PROFESSIONAL — R$ 2.500/mês ⭐ RECOMENDADO LAUTOS

Ideal para: Operações com volume médio-alto de BIDs e cotações

| Componente                         | Limite incluído | Valor extra |
|------------------------------------|-----------------|-------------|
| Emails recebidos                   | até 100.000/mês | +R$10/10k   |
| Interações                         | até 1.000/mês   | +R$0,05/int |
| Análises de BID                    | até 15/mês      | +R$60/BID   |
| Análises de Proposta Comercial     | até 20/mês      | +R$30/prop  |
| Análises de Cotação com Sinergia   | até 30/mês      | +R$20/cot   |
| Relatórios de Concorrência         | até 5/mês       | +R$40/rel   |
| Dashboard de métricas              | ✅ Incluso       | —           |
| Suporte técnico (9–18h seg-sex)    | ✅ Incluso       | —           |
| Reunião mensal de otimização       | ✅ Incluso (1h)  | —           |
| Atualizações de cobertura geográf. | ✅ Incluso       | —           |

**Estimativa para LAUTOS (15 BIDs + 20 prop + 30 cot):** R$ 2.500/mês  
**Economia gerada (estimativa):** R$ 9.000/mês  
**ROI: 360% | Payback: 8 dias**

---

### 🏢 PLANO ENTERPRISE — R$ 4.500/mês

Ideal para: Grandes transportadoras, múltiplas filiais, volumes altos

| Componente                         | Limite incluído | Valor extra |
|------------------------------------|-----------------|-------------|
| Emails recebidos                   | até 200.000/mês | +R$8/10k    |
| Interações                         | até 5.000/mês   | +R$0,04/int |
| Análises de BID                    | até 30/mês      | +R$55/BID   |
| Análises de Proposta Comercial     | até 50/mês      | +R$25/prop  |
| Análises de Cotação com Sinergia   | até 100/mês     | +R$15/cot   |
| Relatórios de Concorrência         | até 10/mês      | +R$35/rel   |
| Integração ERP/WMS (1 sistema)     | ✅ Incluso       | —           |
| Integração WhatsApp Business       | ✅ Incluso       | —           |
| Dashboard customizável             | ✅ Incluso       | —           |
| SLA 99,5% + suporte prioritário    | ✅ Incluso       | —           |
| Reuniões quinzenais                | ✅ Incluso (2/mês)| —          |
| Account Manager dedicado           | ✅ Incluso       | —           |

**Estimativa:** R$ 4.500/mês  
**Economia gerada (estimativa):** R$ 20.000–30.000/mês  
**ROI: 444–667%**

---

## 3. RESUMO COMPARATIVO DOS PLANOS

```
                          STARTER       PROFESSIONAL    ENTERPRISE
                         ─────────────────────────────────────────
Mensalidade               R$ 1.500      R$ 2.500        R$ 4.500
Emails incluídos          100k          100k            200k
Interações incluídas      1.000         1.000           5.000
BIDs incluídos/mês        5             15              30
Propostas incluídas/mês   5             20              50
Cotações sinérgicas/mês   10            30              100
Integração ERP/WMS        ❌ Add-on     ❌ Add-on        ✅ Incluso
WhatsApp Business         ❌ Add-on     ❌ Add-on        ✅ Incluso
Account Manager           ❌            ❌               ✅
SLA garantido             —             99,5%           99,5%
Reuniões de otimização    —             1/mês           2/mês
─────────────────────────────────────────────────────────────────
ROI estimado              ~230%         ~360%           ~450%+
Perfil ideal              Piloto/PME    Lautos atual    Grandes op.
```

---

## 4. COMPOSIÇÃO DETALHADA — PLANO PROFESSIONAL (LAUTOS)

Decomposição do valor R$ 2.500/mês:

```
CAMADA 1 — EMAIL
  100.000 emails recebidos/mês:            R$ 1.200,00
  ─────────────────────────────────────────────────────
  Subtotal Email:                          R$ 1.200,00

CAMADA 2 — INTERAÇÕES (1.000 incluso)
  1.000 interações × R$ 0,0076:           R$     7,60 (custo)
  Margem operacional + overhead:           R$    92,40
  ─────────────────────────────────────────────────────
  Subtotal Interações:                     R$   100,00

CAMADA 3 — ANÁLISES IA
  15 análises de BID × R$ 60:             R$   900,00
  20 análises de Proposta × R$ 30:        R$   600,00
  30 análises de Cotação Siner. × R$ 20: (não cobrado no plano)
  ─────────────────────────────────────────────────────
  Subtotal Análises:                       R$ 1.500,00

OVERHEAD / PLATAFORMA / MARGEM
  Dashboard, relatórios, suporte:         (absorvido)
  ─────────────────────────────────────────────────────
  DESCONTO BUNDLE (pacote fechado):       -R$   300,00

══════════════════════════════════════════════════════
  TOTAL MENSAL:                           R$ 2.500,00
══════════════════════════════════════════════════════
```

---

## 5. EXCEDENTES (cobrados a partir do mês seguinte)

Para uso além dos limites incluídos:

| Tipo de excedente               | Preço unitário    |
|---------------------------------|-------------------|
| Email extra (por 10.000 emails) | R$ 10,00          |
| Interação extra                 | R$ 0,05           |
| BID extra (acima do plano)      | R$ 60,00          |
| Proposta extra                  | R$ 30,00          |
| Cotação sinérgica extra         | R$ 20,00          |
| Relatório de concorrência extra | R$ 40,00          |

> Excedentes são cobrados **em D+30**, com relatório detalhado de consumo.

---

## 6. ADD-ONS DISPONÍVEIS (todos os planos)

| Add-on                                 | Valor       | Tipo        |
|----------------------------------------|-------------|-------------|
| Integração WhatsApp Business           | R$ 300/mês  | Recorrente  |
| Integração com Portal BRUDAM           | R$ 500      | Setup único |
| Integração com ERP/WMS interno         | R$ 500–1.000| Setup único |
| Análise de margem por cliente/rota     | R$ 200/mês  | Recorrente  |
| Customização de prompts por modalidade | R$ 300–500  | Setup único |
| Treinamento de time (até 5 pessoas)    | R$ 800      | One-time    |
| Relatório executivo mensal PDF         | R$ 150/mês  | Recorrente  |

---

## 7. APLICAÇÃO PRÁTICA — CASO LAUTOS

### Cenário realista mês a mês:

```
MÊS 1 (Setup + Piloto):
  Setup inicial (integração email + BRUDAM):  R$ 2.500 (one-time)
  Plano Professional:                         R$ 2.500
  Add-on WhatsApp:                            R$ 300
  ──────────────────────────────────────────────────────
  TOTAL MÊS 1:                                R$ 5.300

MÊS 2+ (Operação normal):
  Plano Professional:                         R$ 2.500
  Add-on WhatsApp:                            R$ 300
  ──────────────────────────────────────────────────────
  TOTAL MENSAL RECORRENTE:                    R$ 2.800

ECONOMIA GERADA LAUTOS (mensal):
  Cotações Padrão (100 × 30min):              R$ 2.500
  Propostas Comerciais (20 × 2h):             R$ 2.000
  BIDs (15 × 6h):                             R$ 4.500
  ──────────────────────────────────────────────────────
  TOTAL ECONOMIA:                             R$ 9.000/mês

ROI MENSAL:
  Economia:     R$ 9.000
  Investimento: R$ 2.800
  ROI:          321% | Lucro líquido: R$ 6.200/mês
  Payback:      10 dias
```

---

## 8. POLÍTICA COMERCIAL

### Descontos disponíveis:

| Condição                                    | Desconto  |
|---------------------------------------------|-----------|
| Contrato anual (pagamento mensal)           | -10%      |
| Contrato anual (pagamento semestral)        | -15%      |
| Contrato anual (pagamento anual antecipado) | -20%      |
| Indicação de novo cliente                   | 1 mês grátis |
| Upgrade de plano (Starter → Professional)  | Sem multa |

### Cancelamento:

- **Primeiros 3 meses:** sem penalidade (aviso de 30 dias)  
- **Após 3 meses:** sem penalidade, aviso de 30 dias  
- **Contrato anual antecipado:** reembolso proporcional ao período restante

### SLA garantido (Professional + Enterprise):

- Disponibilidade: **99,5%/mês**  
- Tempo de resposta suporte: **<4h** (horário comercial)  
- Análise de BID processada em: **<6h** após upload  
- Rollover de análises não utilizadas: **até 30% do mês seguinte**

---

## 9. RESUMO EXECUTIVO — PITCH FINANCEIRO

> "Nossa assinatura Professional cobre **tudo que a Lautos precisa**:  
> 100.000 emails recebidos, 1.000 interações automáticas,  
> **15 análises de BID, 20 propostas, 30 cotações com sinergia**.  
>
> Por **R$ 2.500/mês**, vocês economizam **R$ 9.000/mês**.  
> ROI de 360%. Payback em 10 dias.  
>
> Excedentes são cobrados só se usar mais — sem surpresas."

---

*Documento criado em 06/05/2026 — Nexcode / nokk-chat*  
*Versão: 1.0 | Para uso interno e negociação com LAUTOS*
