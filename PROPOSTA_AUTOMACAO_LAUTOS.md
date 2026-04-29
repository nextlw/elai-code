# 📊 PROPOSTA DE AUTOMAÇÃO VIA NOKK-CHAT
## Cliente: LAUTOS
**Data:** 31 de Março de 2026  
**Preparado por:** Nexcode / TLW  
**Status:** Proposta Comercial

---

## 1. VISÃO GERAL DA SOLUÇÃO

A **nokk-chat** pode automatizar os 3 fluxos de precificação logística da Lautos:
- ✅ **Cotação Padrão** (Baixa complexidade, alto volume)
- ✅ **Proposta Comercial** (Média complexidade, médio volume)
- ✅ **BID** (Alta complexidade, baixo volume — maior ROI de automação)

Além disso, pode ser integrada em múltiplos canais:
- **Portal BRUDAM** (integração API)
- **WhatsApp** (recebimento de demandas)
- **Email** (parsing automático de solicitações e BIDs)
- **Dashboard interno** (consulta de propostas)

---

## 2. OPORTUNIDADES DE AUTOMAÇÃO

### 2.1 Cotação Padrão
| Atividade | Automação | Economia |
|-----------|-----------|----------|
| Receber demanda via email/portal | 80% | Parsing automático de emails e formas |
| Analisar trecho e cobertura | 95% | Base de dados + validação geográfica |
| Calcular distâncias | 100% | API Google Maps / OpenRouteService |
| Consultar tabela padrão | 100% | Integração com planilha/BD |
| Preencher planilha de custos | 100% | Cálculo automático |
| Enviar cotação | 100% | Email automático |
| **Tempo atual:** ~30-45 min | **Novo tempo:** ~2-3 min | **Economia:** 90% |

**Custo de tokens:** Baixo (parsing simples, sem análise de docs)

---

### 2.2 Proposta Comercial
| Atividade | Automação | Economia |
|-----------|-----------|----------|
| Receber dados do vendedor | 70% | Parsing de email + WhatsApp |
| Analisar aderência à malha | 90% | Base de dados + validação |
| Aplicar tabela padrão | 100% | Cálculo automático |
| Preencher planilha de custos | 100% | Automático |
| Enviar proposta | 100% | Email automático |
| **Tempo atual:** ~2-4 horas | **Novo tempo:** ~10-15 min | **Economia:** 85% |

**Custo de tokens:** Baixo-Médio (parsing de contexto, sem análise profunda)

---

### 2.3 BID (Maior potencial de ROI)
| Atividade | Automação | Economia |
|-----------|-----------|----------|
| Receber edital (PDF/Word) | 100% | Upload automático no portal |
| **Analisar informações do BID** ⭐ | **70%** | **Análise de documento (Jina/Claude Vision)** |
| Identificar especificidades | **80%** | IA identifica: inflamável, fármacos, perigo, alimentos, refrigerada |
| Definir áreas atendidas | **90%** | Comparação automática com mapa de cobertura |
| Calcular preço base | **95%** | Tabela + acréscimos automáticos |
| Avaliar aderência ao mercado | **60%** | Comparação com histórico + clientes sinérgicos |
| Negociar redução (parcial) | **50%** | Sugestões de redução com margem simulada |
| **Tempo atual:** ~4-7 dias | **Novo tempo:** ~4-6 horas | **Economia:** 90% |

**Custo de tokens:** **ALTO** (análise de documentos PDF/Word — necessário Jina ou Claude Vision)

---

## 3. DESAFIOS TÉCNICOS & FINANCEIROS

### 3.1 Análise de Documentos (BID)
**Problema:** Editar e analisar PDFs/Word é caro em tokens  
**Soluções:**

| Solução | Custo/doc | Velocidade | Qualidade |
|---------|-----------|-----------|-----------|
| Claude 3.5 Vision (nativo) | $0,10-0,30 | Rápido | Excelente |
| Jina AI (document extraction) | $0,02-0,05 | Muito rápido | Bom |
| Azure Document Intelligence | $0,50-1,00 | Moderado | Excelente |
| **Recomendado:** Jina + Claude | $0,07-0,15 | Rápido | Excelente |

**Recomendação:** Usar **Jina** para extração inicial (barato), depois Claude para análise contextual (caro, mas necessário para decisões).

### 3.2 Impacto Financeiro por Modalidade
```
COTAÇÃO PADRÃO:
  - Volume: 100 solicitações/mês
  - Custo IA/mês: ~$5-10 (parsing simples)
  - Tempo economizado: ~60 horas/mês
  - Valor da automação: R$ 3.000-5.000/mês (a R$ 50/hora)

PROPOSTA COMERCIAL:
  - Volume: 20 solicitações/mês
  - Custo IA/mês: ~$10-20 (parsing + análise leve)
  - Tempo economizado: ~30 horas/mês
  - Valor da automação: R$ 1.500-2.500/mês

BID:
  - Volume: 5-10 propostas/mês
  - Custo IA/mês: ~$3-7 por BID (análise de doc)
  - Tempo economizado: ~30-40 horas/mês
  - Valor da automação: R$ 1.500-2.000/mês (mas impacto alto em ganho/perda de contrato!)
```

---

## 4. MODELO DE NEGÓCIO PROPOSTO

### ❌ NÃO VIÁVEL: Licença Fixa
- Competitors cobram $69/licença com mínimo 10 unidades = $690/mês
- Lautos teria ROI em 2-3 meses, depois quereria desconto
- Sem análise de documentos, você compete com chatbots baratos
- Incompatível com custos reais de IA para BID

### ✅ VIÁVEL: Modelo Híbrido (Recomendado)

#### **Opção 1: Por Interação + Análise de Documento**
```
BASE MENSAL: R$ 1.500-2.000/mês
  - Plataforma omni-channel (chat, email, portal, WhatsApp)
  - 500 interações simples/mês (Cotações, Propostas)
  - Suporte técnico
  - Dashboard de métricas

EXTRAS POR USO:
  - Análise de BID (documento): R$ 50-80 por análise
    * Inclui: extração, diagnóstico de cobertura, comparação sinergias
    * Economiza 16-24 horas de análise manual
  
  - Integração extra com sistema legado: R$ 500-1.000 (setup único)
  - Customização de prompt por modalidade: R$ 300-500

ESTIMATIVA PARA LAUTOS:
  - Base: R$ 1.500/mês
  - ~8 BIDs/mês × R$ 65 = R$ 520/mês
  - **Total: ~R$ 2.020/mês**
  - **Economia gerada: R$ 6.000-8.000/mês**
  - **ROI: 300-400% em 2-3 meses**
```

**Vantagem:** Você cobre custos reais de IA, cliente vê ROI claro, pode escalar com volume.

---

#### **Opção 2: Baseado em Volume + Margens Compartilhadas**
```
BASE MENSAL: R$ 1.000/mês (plataforma)

PAGAMENTO POR COTAÇÃO GERADA:
  - Cotação Padrão: R$ 5 por cotação
    * Economia: ~30 min de trabalho
    * Volume esperado: 100/mês → R$ 500
  
  - Proposta Comercial: R$ 15 por proposta
    * Economia: ~2 horas de trabalho
    * Volume esperado: 20/mês → R$ 300
  
  - BID: R$ 60-80 por análise
    * Economia: ~16 horas de trabalho
    * Volume esperado: 8/mês → R$ 560

ESTIMATIVA PARA LAUTOS:
  - Base: R$ 1.000
  - Cotações: R$ 500
  - Propostas: R$ 300
  - BIDs: R$ 560
  - **Total: ~R$ 2.360/mês**
```

**Vantagem:** Modelo verdadeiramente baseado em uso, sem surpresas de custo.

---

#### **Opção 3: Freemium + Premium (Maior Escalabilidade)**
```
PLANO GRATUITO:
  - Cotação Padrão automática (sem limite)
  - Até 50 interações/mês
  - Sem análise de BID
  - → Objetivo: Product-led growth, reduzir fricção

PLANO PROFISSIONAL: R$ 1.500/mês
  - Análise de BID automática (até 10/mês)
  - Propostas comerciais ilimitadas
  - Integração com portal BRUDAM + WhatsApp + Email
  - Dashboard customizável
  - Suporte prioritário

PLANO ENTERPRISE: A negociar
  - SLA customizado
  - Análise de BID ilimitada
  - Integração com ERP/WMS interno
  - Dedicated account manager

PARA LAUTOS:
  - Começar com Profissional (R$ 1.500)
  - Upgrade para Enterprise após 6 meses (se volume justificar)
```

**Vantagem:** Menor fricção de venda, pode começar grátis (Cotações), depois upsell para BID.

---

## 5. RECOMENDAÇÃO FINAL

### 🎯 **Combinar Opções 2 + 3:**

```
OFERTA PARA LAUTOS:

▪️ MÊS 1-2: Piloto Freemium
   - Cotações Padrão automáticas (gratuito)
   - 1-2 BIDs analisados (gratuito/com desconto)
   - Objetivo: Validar qualidade, ganhar confiança

▪️ MÊS 3+: Contrato Profissional
   - Base: R$ 1.200/mês (plataforma omni-channel)
   - Por Análise de BID: R$ 60/análise
   - Estimativa mensal: R$ 1.680/mês
   - Economia gerada: R$ 6.000-8.000/mês → ROI 375%

▪️ Escalabilidade:
   - Se volume de BID crescer → pode negociar pacote anual com desconto
   - Se quiser integração com ERP → add-on Enterprise (R$ 500-1.000)
   - Redução automática se volume cair (sem compromisso de mínimo)
```

---

## 6. PROPOSTA COMERCIAL ESTRUTURADA

### 📋 Escopo Incluído

#### **Fase 1: Setup & Integração (2-3 semanas)**
- [ ] Integração com portal BRUDAM (API)
- [ ] Integração com email (IMAP + parsing automático)
- [ ] Integração com WhatsApp (opcional, +R$ 300)
- [ ] Base de dados geográfica (cobertura Lautos)
- [ ] Upload de tabela padrão de preços
- [ ] Testes de cotações padrão & propostas
- [ ] 1 BID de teste (análise automática)
- **Investimento:** R$ 2.500-3.500 (onetime)

#### **Fase 2: Operação Contínua (Mensal)**
- **Base (Plataforma):** R$ 1.200/mês
  - Omni-channel (Email + Portal + WhatsApp)
  - Dashboard de métricas em tempo real
  - Relatórios de automação/economia
  - Suporte 9-18h segunda-sexta
  - Atualização de base de cobertura geográfica

- **Por Análise de BID:** R$ 60/análise
  - Extração automática de informações
  - Diagnóstico de cobertura (completo/parcial/não)
  - Identificação de especificidades (inflamável, fármacos, etc.)
  - Comparação com clientes sinérgicos
  - Estimativa de competitividade de preço
  - Recomendação de participação ou não

#### **Fase 3: Otimizações Futuras (A cobrar separadamente)**
- Integração com ERP/WMS interno
- Análise de margem por cliente/rota
- Previsão de volume sazonal
- Otimização automática de tabelas de preço

---

### 💰 Resumo Financeiro

| Componente | Valor | Descrição |
|-----------|-------|-----------|
| **Setup Inicial** | R$ 3.000 | Integração, setup, testes |
| **Base Mensal** | R$ 1.200 | Plataforma omni-channel |
| **BID (até 15/mês)** | R$ 900 | 15 análises × R$ 60 |
| **Whatsapp (opcional)** | R$ 300 | Integração com WhatsApp |
| **TOTAL MÊS 1** | **R$ 5.400** | (Setup + Base + 15 BIDs + WhatsApp) |
| **TOTAL MÊS 2+** | **R$ 2.400/mês** | (Base + 15 BIDs médios) |

**Economia Gerada (mensal):**
- Cotações Padrão: 100 × 30 min = 50 horas = R$ 2.500
- Propostas: 20 × 2h = 40 horas = R$ 2.000
- BIDs: 15 × 6h = 90 horas = R$ 4.500
- **TOTAL: ~R$ 9.000/mês**
- **ROI: 375% (payback em 3,6 semanas)**

---

## 7. DIFERENCIAIS vs. CONCORRÊNCIA

| Aspecto | Concorrentes ($69 × 10) | nokk-chat |
|---------|-------------------------|-----------|
| **Preço mínimo** | $690/mês | R$ 1.200/mês (sem mínimos) |
| **Análise de Documentos** | ❌ Não | ✅ Sim (Jina + Claude) |
| **Omni-channel** | ❌ Email apenas | ✅ Email + Portal + WhatsApp |
| **ROI em BID** | ❌ Não ajuda | ✅ 300-400% em 2-3 meses |
| **Escalabilidade** | ❌ Pago mínimo mesmo sem uso | ✅ Paga apenas por BIDs analisados |
| **Integração com sistemas** | ❌ Manual | ✅ API + automação |
| **Margem compartilhada** | ❌ Não | ✅ Possível em contratos long-term |

---

## 8. PRÓXIMOS PASSOS

1. **Reunião com stakeholders Lautos:**
   - Thiago (Analista de Precificação)
   - Max (Diretor Comercial)
   - TI (para validar integrações)

2. **Demonstração de Prototipo:**
   - Analisar 1 BID real com nokk-chat
   - Mostrar dashboard de propostas
   - Simular economia de tempo/custo

3. **Contrato Piloto (1 mês):**
   - Cotações Padrão automáticas (grátis)
   - 2-3 BIDs analisados (teste)
   - Feedback antes de assinar contrato anual

4. **Implementação Full (Semana 1-3):**
   - Setup de integrações
   - Treinamento de time
   - Go-live com cotações automáticas

5. **Otimização (Mês 1-3):**
   - Ajuste de prompts e tabelas
   - Análise de qualidade de propostas
   - Redefinição de KPIs

---

## 9. RISCOS & MITIGAÇÕES

| Risco | Impacto | Mitigação |
|-------|---------|-----------|
| Qualidade baixa de análise de BID | Alto | Validação manual nas primeiras 10 análises + feedback loop |
| Custo de tokens acima do previsto | Médio | Limitar para Jina (mais barato), usar Claude apenas para análise final |
| Integração com BRUDAM complexa | Médio | Usar middleware com conversão de dados, testar antes de produção |
| Resistência do time (Thiago) | Baixo | Envolver Thiago no piloto, mostrar redução de trabalho repetitivo |
| Concorrente oferece licença mais barata | Médio | Enfatizar análise de documentos (diferencial real) + ROI comprovado |

---

## 10. CONTRATO SUGERIDO

```markdown
CONTRATO-PILOTO (30 dias)
=======================

1. ESCOPO
   - Setup de integrações (email + portal BRUDAM)
   - Cotações Padrão automáticas (sem limite)
   - Análise de até 5 BIDs com nokk-chat
   - Dashboard de visualização
   - 1 reunião de feedback no final

2. INVESTIMENTO
   - Setup: R$ 2.500
   - BIDs (5): R$ 300
   - TOTAL: R$ 2.800

3. RESULTADO ESPERADO
   - Validação de qualidade
   - Identificação de gaps
   - Confirmação de economia (esperada: 60-80 horas em 1 mês)

4. PRÓXIMOS PASSOS (pós-piloto)
   - Contrato anual com base de R$ 1.200/mês
   - Análise de BID: R$ 60/análise
   - SLA: 99,5% de disponibilidade
   - Suporte: 9-18h segunda-sexta

5. CANCELAMENTO
   - Piloto: sem penalidade após 30 dias
   - Contrato anual: sem penalidade após 6 meses, depois R$ 500 taxa de cancelamento
```

---

## 11. MATERIAL DE APRESENTAÇÃO

### Slide 1: Problema
- Thiago gasta **4-7 dias** por BID (leitura + análise + cálculo)
- **100+ cotações/mês** = trabalho repetitivo
- Falta de sistematização em carga especial = preços inconsistentes
- Vendedor aguarda **2-3 dias** por resposta de cotação

### Slide 2: Solução (nokk-chat)
- Análise automática de BID em **4-6 horas** (vs 4-7 dias)
- Cotações geradas em **2-3 minutos** (vs 30-45 min)
- Consistência de preço (tabela automática)
- Recomendações em tempo real

### Slide 3: ROI Comprovado
```
ECONOMIA MENSAL:
- 100 Cotações × 30 min = 50h → R$ 2.500
- 20 Propostas × 2h = 40h → R$ 2.000
- 15 BIDs × 6h = 90h → R$ 4.500
TOTAL: R$ 9.000/mês

CUSTO AUTOMAÇÃO: R$ 2.400/mês
ROI: 375% | PAYBACK: 3,6 semanas
```

### Slide 4: Plano de Implementação
- Semana 1: Setup + testes
- Semana 2-3: Piloto com cotações
- Semana 4: Go-live BID

---

## Conclusão

A nokk-chat **não é apenas um chatbot de suporte** — é uma **plataforma de automação de processos logísticos** que resolve o problema real da Lautos: **análise de documentos complexos com IA de verdade**.

Modelo de negócio viável = **Base + Por Uso**, não licença fixa.  
Diferencial real = **Análise de BID** (caro em tokens, mas ainda mais caro fazer manualmente).  
ROI claro = **375% em 2 meses**.

---

**Próximo passo:** Agendar demo com Thiago + Max para validar análise automática de 1 BID real.
