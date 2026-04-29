# Modelos de Precificação para Agentes de IA — Pesquisa de Mercado 2025

**Data:** 31 de março de 2026  
**Fontes:** PayPro Global, GPTMaker, LinkedIn Pulse, análise de market leaders

---

## 1. Panorama Geral

A precificação de agentes de IA em SaaS consolidou **três modelos principais**:

| Modelo | Aplicabilidade | Vantagem | Risco |
|---|---|---|---|
| **Seat-based (por usuário)** | Plataformas com múltiplos usuários | Previsibilidade, CAC amortizado | Incentiva sub-adoção, frena expansão |
| **Usage-based (por ação/token)** | Processamento, chamadas API, documentos | Alinhado com custo, fair-play | Volatilidade, billing complexo |
| **Value-based (por resultado/outcome)** | Automação, workflows, ROI direto | Maximiza margem, CLV alto | Difícil mensurar, negocia muito |
| **Hybrid (combinado)** | Maioria dos players modernos | Flexibilidade, múltiplas UVP | Complexidade operacional |

---

## 2. Modelos Detalhados

### 2.1 Seat-Based (Por Usuário/Licença)

**Estrutura:**
```
Preço mensal = N usuários × Preço por usuário
Exemplo: 5 usuários × R$ 300 = R$ 1.500/mês
```

**Quem usa:**
- Slack, Notion, Figma, Microsoft Teams (modelos híbridos)
- Plataformas de automação com interface visual
- Ferramentas de colaboração

**Vantagens:**
- ✅ Fácil de explicar ao cliente
- ✅ Revenue previsível (churn visível antecipadamente)
- ✅ Suporte escalável por número de usuários
- ✅ Lock-in natural (adicionar usuário é expansão frictionless)

**Desvantagens:**
- ❌ Não captura valor real gerado
- ❌ Clientes tentam compartilhar contas
- ❌ Inibe adoção em grandes organizações ("pague per head")
- ❌ Commoditizável (pressão para baixar preço)

**Fórmulas de Variação:**
```
1. Tiers por funcionalidade:
   Starter: R$ 99 (3 users, features básicas)
   Pro: R$ 299 (10 users, automação)
   Enterprise: Custom (unlimited)

2. Tiers por volume de ações:
   Starter: R$ 199 (100 tarefas/mês por usuário)
   Pro: R$ 499 (1.000 tarefas/mês por usuário)
   Enterprise: Custom
```

---

### 2.2 Usage-Based (Por Consumo)

**Estrutura:**
```
Preço mensal = Base (floor) + (Volume × Preço unitário)
Exemplo: R$ 500 base + (1.000 documentos × R$ 2,50) = R$ 3.000/mês
```

**Unidades de Consumo Típicas:**
1. **Por token (LLM puro)** → Anthropic Claude, OpenAI GPT
2. **Por requisição/chamada API**
3. **Por documento processado**
4. **Por tarefa executada**
5. **Por minuto de processamento**
6. **Por página analisada**

**Pricing Real (Market Leaders):**

| Fornecedor | Modelo | Preço | Observação |
|---|---|---|---|
| **Anthropic Claude API** | Por token | $3/MTok (input), $15/MTok (output) | Plus/Pro: usage flat-rate |
| **OpenAI GPT-4** | Por token | $0.03/K input, $0.06/K output | Modelos menores mais baratos |
| **Jina Reader** | Por documento | ~$0,01–0,10 por página | Integrado em apps de pesquisa |
| **Zapier** | Por tarefa ("task") | $0,25–2,00 por execução | Overage acima de plano |
| **Make (Integromat)** | Por operação | $0,10–1,00 por operação | Baseado em histórico de consumo |

**Quem usa:**
- APIs de LLM puro (Anthropic, OpenAI, Mistral)
- Plataformas de automação (Zapier, Make, n8n)
- Serviços de processamento de documentos
- Ferramentas de análise em batch

**Vantagens:**
- ✅ Alinhado com custo real (COGS transparente)
- ✅ Zero friction para experimentação (cliente paga só o que usa)
- ✅ Escala naturalmente com sucesso do cliente
- ✅ Reduz risco de implementação

**Desvantagens:**
- ❌ Revenue imprevisível (churn é lento, ramp imprevisível)
- ❌ Billing complexo e auditável (cliente discute cada uso)
- ❌ Margem atacada por volume (quanto maior, menor %)
- ❌ Competição por "preço por token" ativa
- ❌ Spike de consumo não planejado = surpresa na fatura

**Estratégias de Proteção de Margem:**
```
1. Floor (piso mínimo):
   "Pague no mínimo R$ 500/mês, mesmo que use menos"

2. Caps/Tetos:
   "Acima de 10.000 documents/mês, aciona revisão manual"

3. Tiers escalonados:
   - 0–500: R$ 5,00 / doc
   - 501–2.000: R$ 3,50 / doc
   - 2.001+: R$ 2,00 / doc

4. Franquias (allowances):
   Inclusos 500 documents/mês.
   Acima: R$ 3,00 / doc adicional.
```

---

### 2.3 Value-Based (Por Resultado/Impacto)

**Estrutura:**
```
Preço = Função(métrica de resultado, SLA, complexidade)
Exemplos:
  - R$ X por lead gerado
  - R$ Y por contrato fechado
  - R$ Z por economia de tempo (horas liberadas)
  - Success fee: N% do valor total do resultado
```

**Quem usa:**
- Soluções de lead generation & scoring
- Plataformas de compliance/legal tech
- Ferramentas de RPA (automação de processos)
- Consultoria SaaS

**Vantagens:**
- ✅ Maximiza margem (sem teto de consumo)
- ✅ Alinhamento 100% com cliente (ganha se cliente ganha)
- ✅ Reduz objeção de preço ("pague só se der resultado")
- ✅ CLV muito mais alto

**Desvantagens:**
- ❌ Difícil mensurar ("quanto vale um lead?")
- ❌ Auditoria complexa (cliente questiona métrica)
- ❌ Requer contrato customizado (não escalável)
- ❌ Tempo de sales + legal muito maior
- ❌ Conflito de interesses (seu resultado != resultado do cliente)

**Exemplo Real — Proposta + BID (LAUTO):**
```
SKU: Análise de Edital com Resposta (BID Premium IA)
Preço: R$ 890 por BID respondido
Value prop: "Você vende R$ 50-200k em concreto por BID.
            R$ 890 é 0,45–1,8% do valor fechado."

Success fee alternativo:
  Preço: R$ 350 por BID + 0,8% do contrato fechado atribuído ao Nokk
  (cap R$ 8k/mês para proteção de margem)
```

---

### 2.4 Hybrid (Combinado) — MODELO RECOMENDADO

**Estrutura:**
```
Preço mensal = Floor (plataforma) + Franquia (inclusos) + Overage (acima)

Exemplo (3 camadas):
  1. Plataforma: R$ 1.490/mês
     (workspace, users, dashboards, SLA 8×5)
  
  2. Automação padrão (Cotação): R$ 1.490/mês
     (600 cotações inclusos, R$ 2,80 cada adicional)
  
  3. IA pesada (BID Premium): R$ 1.890/mês
     (3 Light + 2 Standard + 1 Heavy inclusos)
     Overage: Light R$ 190, Std R$ 450, Heavy R$ 990

Total mensal: R$ 4.870 → R$ 5.370 (dependendo de overage)
```

**Por que hybrid é dominante:**

1. **Plataforma floor** → Revenue previsível, churn visível
2. **Franquia inclusa** → Alinha incerteza inicial com consumo real
3. **Overage com overage > pacote unitário** → Incentiva upgrade (não viver no overage)
4. **Tiers por complexidade** → Value-based natural (Heavy custa mais porque vale mais)

**Quem usa:**
- Slack (seats + usage)
- AWS (floor + per-service usage)
- Stripe (percentage + minimum)
- Notion (seats + API usage)
- Salesforce (seats + number of records)
- **LAUTO (proposto)** — Plataforma + Cotações + BID com tiers

---

## 3. Comparação de Modelos Para LAUTO Especificamente

### 3.1 Cenário Alternativo 1: Pure Usage-Based

```
Setup: R$ 9.800 one-time
Mensal: R$ 0,98 por cotação + R$ 14 por proposta + R$ 290 por BID

Consumo esperado (Thiago + Max):
  - 800 cotações/mês × R$ 0,98 = R$ 784
  - 80 propostas/mês × R$ 14 = R$ 1.120
  - 4 BIDs/mês × R$ 290 = R$ 1.160
  Total: R$ 3.064/mês

Problema:
  ❌ Revenue impredictível (se vendem mais, pagam mais — desincentiva)
  ❌ Clientequer "flat rate" para orçamento previsível
  ❌ Sem floor = margem baixa se consumo ↓
```

### 3.2 Cenário Alternativo 2: Pure Seat-Based

```
Mensal: R$ 1.990 × 5 usuários (Thiago, Max, 3 vendedores) = R$ 9.950

Problema:
  ❌ Não captura diferença de valor (cotação ≠ BID)
  ❌ Adicionar vendedor = +R$ 1.990 (pode ser proibitivo)
  ❌ Não alinha com payback (plataforma × consumo real)
  ❌ Competitors cobram "R$ 690 por 10 licenças" = commoditizado
```

### 3.3 Modelo Hybrid Proposto (RECOMENDADO)

```
Setup: R$ 19.800 (3× sem juros)
Mensal base: R$ 3.970 (Plataforma + Cotação Growth + Proposta)

Detalhes:
  - Plataforma: R$ 1.490 (5 users, omni, 5k e-mails, SLA 8×5)
  - Cotação Growth: R$ 1.490 (600 inclusos, R$ 2,80 overage)
  - Proposta Comercial: R$ 990 (40 inclusos, R$ 18 overage)
  - BID (trial): Grátis 1 Light/mês

Vantagem:
  ✅ Floor (R$ 3.970) = revenue previsível
  ✅ Franquia = alinha com padrão de uso (não "grátis", não "cara")
  ✅ Overage > pacote = incentiva upgrade (Growth → Scale)
  ✅ Tiers de BID (Light, Std, Heavy) = value-based natural
  ✅ Comparável a "R$ 690 × 5 = R$ 3.450" mas com IA + consultoria
  ✅ ARR Year 1: R$ 47.640 + R$ 19.800 setup = R$ 67.440
```

---

## 4. Precificação de Agentes de IA — Tendências 2025

### 4.1 O Modelo Emergente: "Ação Completada"

Ao invés de cobrar por token ou por usuário, cobram **por tarefa executada com sucesso**:

| Plataforma | Modelo | Preço |
|---|---|---|
| **Zapier** | Per task completion | $0,25–2,00 |
| **n8n** | Per task | Base included, overage $0,20 |
| **Make** | Per 1k operations | $0,10–1,00 |
| **Anthropic Agents** | Per agentic turn | $0,01–0,50 (estimado) |

**Por que funciona para agentes:**
```
Tarefa = sequência de steps (pesquisa + análise + decisão + ação)
Não é linear em tokens, mas em "ciclos de raciocínio"
Cliente paga por "trabalho entregue", não por insumo.
```

### 4.2 SLA como Diferenciador de Preço

Agentes com **garantia de entrega** cobram premium:

```
Padrão: R$ 2,50 / tarefa
SLA ≤ 30 min: R$ 3,50 / tarefa (+40%)
SLA ≤ 5 min: R$ 5,00 / tarefa (+100%)
Manual review included: +R$ 0,50/tarefa
```

### 4.3 Tier Auto-Classificado (Sem Choice do Cliente)

Modelo emergente em **IA pesada**:

```
Sistema classifica tarefa por complexidade (automático):
  Light (≤10 págs, padrão) → R$ 149
  Standard (10–40 págs, 1–2 especificidades) → R$ 390
  Heavy (40+ págs, multi-variáveis) → R$ 890

Vantagem:
  ✓ Cliente não quer "escolher tier" (complexo demais)
  ✓ Sistema sabe melhor que cliente qual será o esforço
  ✓ Reduz "tier creep" (cliente upgrada por insegurança)
  ✓ Margem protegida por heurística, não por comunicação
```

### 4.4 Caching Semântico como Lever de Margem

Com cache automático em boilerplate jurídico:

```
Primeiro BID (sem cache): COGS = R$ 2,50 (tokens cheios)
Segundo BID (70% cache hit): COGS = R$ 0,60 (cache é 10% do custo)

Preço = R$ 390 (fixo)
Margem cresce com volume do cliente sem aumentar preço.
```

---

## 5. Recomendações Finais para LAUTO

### 5.1 Estratégia Comercial Recomendada

**Usar modelo HYBRID com 3 cenários** (decoy pricing):

| Cenário | Setup | Mensal | Diferenciador | Foco |
|---|---|---|---|---|
| **A — Piloto** | R$ 14.500 | R$ 2.980 | Sem IA pesada | Validar + Entrada baixa |
| **B — Recomendado** ⭐ | R$ 19.800 | R$ 3.970 | Com BID trial | Padrão adotado |
| **C — Full** | R$ 23.500 | R$ 5.860 | Full stack + SLA | Anchor alto / Upsell |

Pesquisa em B2B SaaS mostra: **Oferecer 3 opções aumenta conversão do meio em 60–70%.**

### 5.2 Proteção de Margem (Must-Have)

Sem isso, overage é vulnerável a outliers:

1. **Tier auto-classificado** — Cliente não escolhe (sistema escolhe)
2. **Hard caps** — "Acima de 10k documents/mês = revisão manual"
3. **Fair-use** — Teto soft de 3× média trimestral antes de alerta
4. **Cache semântico** — Boilerplate jurídico cacheado (60%+ economia)
5. **Cláusula de revisão automática** — Se LLM upstream variar >20%, reprecia
6. **Sugestão de upgrade** — Se overage recorrente, migra pacote automatically

### 5.3 Posicionamento Contra Competidores

**Eles cobram:** R$ 69/licença × 10 cadeiras = R$ 690  
**Nokk cobra:** R$ 2.980–5.860

**Conversa:**
```
"R$ 690 te dão 10 cadeiras pra digitar manualmente.
R$ 2.980 te dão um Thiago a mais trabalhando 24/7,
com a tabela de acréscimos finalmente PADRONIZADA.

Você não paga por cadeira.
Você paga por trabalho liberado."
```

---

## 6. Fontes de Referência

**Estudos citados:**
- [PayPro Global] Como precificar produtos SaaS de IA
- [GPTMaker] Modelos de precificação para agências de IA
- [LinkedIn Pulse] Como os agentes de IA serão precificados

**Pricing públicos analisados:**
- Anthropic Claude API
- OpenAI GPT models
- Jina Reader (document processing)
- Zapier (task-based)
- Make (operations-based)
- Salesforce Agentforce (emerging)

---

## 7. Próximos Passos

- [ ] Validar COGS real de cada tier de BID
- [ ] Modelar churn por cenário (A, B, C)
- [ ] Testar psychological pricing (R$ 2.980 vs R$ 3.000)
- [ ] Estruturar contracts com cláusulas de revisão
- [ ] Treinar time de sales em "value pitch" vs "feature pitch"

---

**Document gerado:** 31/03/2026  
**Próxima revisão:** Quando pricing for ativo em produção + 3 meses de dados reais
