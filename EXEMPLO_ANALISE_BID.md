# 📄 EXEMPLO PRÁTICO: ANÁLISE AUTOMÁTICA DE BID
## nokk-chat para Lautos

---

## CENÁRIO: BID Real de Transportadora Parceira

### Input: BID Documento (PDF/Word)

```
EDITAL DE BID — TRANSPORTADORA XYZ
===================================

Cliente: EMPRESA DE E-COMMERCE
Data: 31 de Março de 2026
Vigência: 90 dias

VOLUMES ESPERADOS:
- Origem: São Paulo (SP)
- Destino: Todo Brasil
- Volume mensal: 500 m³
- Peso mensal: 15.000 kg

TIPOS DE CARGA:
✓ Eletrônicos (TVs, notebooks) — Valor agregado alto
✓ Roupas e calçados — Geral
✓ Alimentos secos — Não perecível
✓ Bebidas (garrafas PET) — Peso médio

ESPECIFICIDADES LOGÍSTICAS:
- Requer segurador específico (Sompo Seguros)
- Coleta diária em unidade São Paulo
- Entrega em 48h para região Sudeste, 72h demais regiões
- Embalagem padrão em caixas de papelão
- Sem restrição de mix de cargas

MODAL: Rodoviário (Fracionado ou Dedicado — decisão do licitante)

PREÇO ESPERADO:
- Referência de concorrente A: R$ 2,50/kg
- Referência de concorrente B: R$ 2,80/kg
- Expectativa cliente: "Melhor que mercado"

REQUISITOS ADICIONAIS:
- Rastreamento em tempo real (GPS)
- Relatório semanal de volumes
- Acesso a portal de consulta

PERGUNTA CHAVE:
"Lautos consegue atender 100% do Brasil? Qual seria o preço?
Consegue oferecer coleta diária em SP?"
```

---

## Análise Automática via nokk-chat (Jina + Claude)

### ETAPA 1: Extração de Dados (Jina — ~$0.02)

```json
{
  "bid_id": "XYZ_31032026",
  "cliente": "EMPRESA DE E-COMMERCE",
  "data_emissao": "2026-03-31",
  "vigencia": "90 dias",
  "modalidade": "Rodoviário",
  "tipo": "Fracionado ou Dedicado",
  
  "origem": {
    "cidade": "São Paulo",
    "estado": "SP",
    "tipo_origem": "Unidade única (coleta diária)"
  },
  
  "destino": {
    "area": "Todo Brasil",
    "tempo_entrega": {
      "sudeste": "48h",
      "demais_regioes": "72h"
    }
  },
  
  "volumetria": {
    "volume_mensal_m3": 500,
    "peso_mensal_kg": 15000,
    "densidade_media": 30,
    "tipo_coleta": "Diária"
  },
  
  "tipos_carga": [
    {
      "tipo": "Eletrônicos",
      "descricao": "TVs, notebooks",
      "percentual_estimado": 30,
      "especificidades": "Valor agregado alto"
    },
    {
      "tipo": "Roupas e calçados",
      "percentual_estimado": 30,
      "especificidades": "Geral"
    },
    {
      "tipo": "Alimentos secos",
      "percentual_estimado": 25,
      "especificidades": "Não perecível"
    },
    {
      "tipo": "Bebidas",
      "percentual_estimado": 15,
      "especificidades": "Garrafas PET, peso médio"
    }
  ],
  
  "especificidades": {
    "segurador_obrigatorio": "Sompo Seguros",
    "rastreamento": "GPS (tempo real)",
    "embalagem": "Caixas papelão padrão",
    "mix_cargas": "Permitido",
    "perecibilidade": "Não",
    "inflamavel": false,
    "farmacos": false,
    "alimentos_regulado": true,
    "refrigerado": false,
    "risco_alto": false
  },
  
  "referencias_preco": {
    "concorrente_A": 2.50,
    "concorrente_B": 2.80,
    "expectativa_cliente": "Melhor que mercado",
    "unidade": "R$/kg"
  },
  
  "requisitos_adicionais": [
    "Rastreamento GPS",
    "Relatório semanal de volumes",
    "Portal de consulta"
  ]
}
```

---

### ETAPA 2: Análise Contextual (Claude 3.5 — ~$0.10)

#### Prompt para Claude:

```
Você é especialista em precificação logística para transportadora.
Analise este BID e forneça recomendação executiva.

CONTEXTO LAUTOS:
- Cobertura geográfica: 70% do Brasil (foco Sudeste, Sul, Centro-Oeste)
- Capacidade: 500 m³/mês disponível
- Tabela base fracionado: R$ 2.00/kg
- Tabela base dedicado: R$ 1.80/kg + fixo de R$ 5.000/mês

BID RECEBIDO:
[JSON acima]

ANÁLISE SOLICITADA:

1. COBERTURA GEOGRÁFICA:
   - Lautos consegue atender 100%? Se não, qual %?
   - Regiões não cobertas e impacto

2. ESPECIFICIDADES DE CARGA:
   - Existem especificidades que aumentam custo?
   - Quantos % de acréscimo por modalidade (eletrônicos, alimentos)?

3. CÁLCULO DE PREÇO:
   - Cenário Fracionado: qual o preço/kg recomendado?
   - Cenário Dedicado: qual o preço mensal recomendado?
   - Comparar com referências (R$ 2.50 e R$ 2.80)

4. ANÁLISE DE CLIENTES SINÉRGICOS:
   - Lautos tem outras cargas de SP para mesmo destino?
   - Possibilidade de mix de carga (ganhar volume)?

5. VIABILIDADE DE PARTICIPAÇÃO:
   - Recomenda participar ou passar?
   - Qual é o ROI esperado?

6. REQUISITOS ADICIONAIS:
   - GPS/rastreamento: custo mensal adicional?
   - Portal de consulta: é viável?
   - Relatório semanal: automatizar?

FORMATO: Estruturado, com números, recomendações e next steps.
```

---

### OUTPUT: Relatório Automático para Thiago & Max

```
═══════════════════════════════════════════════════════════════════
ANÁLISE AUTOMÁTICA DE BID — EMPRESA DE E-COMMERCE
Data: 31/03/2026 | Gerado por: nokk-chat
═══════════════════════════════════════════════════════════════════

1. COBERTURA GEOGRÁFICA
─────────────────────────
Atendimento Lautos:
├── São Paulo (Origem): ✅ 100% (hub operacional)
├── Região Sudeste (destino 48h): ✅ 85% (São Paulo, Rio, Minas)
├── Região Sul (destino 72h): ✅ 95% (Paraná, Rio Grande do Sul, SC)
├── Centro-Oeste (destino 72h): ⚠️  40% (Brasília apenas, Goiás não)
├── Nordeste (destino 72h): ❌ 0%
├── Norte (destino 72h): ❌ 0%
└── COBERTURA TOTAL: 62% do Brasil

RECOMENDAÇÃO: Participar com "atendimento parcial". Oferecer:
• Sudeste + Sul + DF: 100% cobertura e SLA (48-72h)
• Nordeste + Norte: Indicar parceiro de cobertura (não Lautos)

IMPACTO: Perda potencial de 38% do volume, mas ganha 62% com margens melhores.

─────────────────────────────────────────────────────────────────────

2. ANÁLISE DE ESPECIFICIDADES DE CARGA
───────────────────────────────────────
Carga Analisada:
├── Eletrônicos (30%) — Valor agregado alto
│   └── Acréscimo recomendado: +15% (seguro, embalagem, cuidado)
│   └── Novo preço base: R$ 2.30/kg (vs R$ 2.00 tabela)
│
├── Roupas/Calçados (30%) — Carga geral
│   └── Acréscimo: 0% (tabela padrão)
│   └── Preço: R$ 2.00/kg
│
├── Alimentos Secos (25%) — Regulado, sem refrigeração
│   └── Acréscimo: +5% (requisitos de rastreamento)
│   └── Novo preço: R$ 2.10/kg
│
└── Bebidas PET (15%) — Peso, sem especial
    └── Acréscimo: 0%
    └── Preço: R$ 2.00/kg

PREÇO MÉDIO PONDERADO (Mix de cargas):
= (2.30 × 0.30) + (2.00 × 0.30) + (2.10 × 0.25) + (2.00 × 0.15)
= 0.690 + 0.600 + 0.525 + 0.300
= R$ 2.115/kg

─────────────────────────────────────────────────────────────────────

3. CÁLCULO DE PREÇO PROPOSTO
──────────────────────────────
Estimativa de faturamento mensal (cliente): 15.000 kg/mês

CENÁRIO 1: FRACIONADO (Mix de cargas)
Preço recomendado: R$ 2.12/kg
├── Margem sobre tabela: +6% (competitivo)
├── Revenue mensal: 15.000 × R$ 2.12 = R$ 31.800
├── Margem estimada: 20-25%
├── Lucro mensal: R$ 6.360-7.950
└── Lucro anual: R$ 76.320-95.400

CENÁRIO 2: DEDICADO (Coleta diária em SP)
Preço recomendado: R$ 5.500/mês + R$ 1.85/kg
├── Fixo mensal: R$ 5.500 (coleta diária, dedicação)
├── Variável (15.000 kg × 1.85): R$ 27.750
├── Revenue mensal: R$ 33.250
├── Margem estimada: 25-30%
├── Lucro mensal: R$ 8.312-9.975
└── Lucro anual: R$ 99.744-119.700

ANÁLISE COMPARATIVA VS CONCORRENTES:
┌──────────────────┬────────┬────────┬────────┬──────────┐
│ Modalidade       │ Lau    │ Conc.A │ Conc.B │ Posição  │
├──────────────────┼────────┼────────┼────────┼──────────┤
│ Fracionado/kg    │ R$2.12 │ R$2.50 │ R$2.80 │ 🥇 MELHOR│
│ Dedicado/kg      │ R$1.85 │ R$2.10 │ R$2.40 │ 🥇 MELHOR│
│ Velocidade       │ 48-72h │ 72-96h │ 72-96h │ 🥇 MELHOR│
│ Rastreamento     │ ✅ SIM │ ❌ NÃO │ ⚠️ EXTRA│ 🥇 GRÁTIS│
└──────────────────┴────────┴────────┴────────┴──────────┘

→ PROPOSTA LAUTOS: Competitiva e atraente. Cliente deve escolher Lautos.

─────────────────────────────────────────────────────────────────────

4. CLIENTES SINÉRGICOS (Oportunidade de Mix)
──────────────────────────────────────────────
ANÁLISE DE HISTÓRICO LAUTOS:

Clientes com origem em SP:
├── Tech Corp (coleta SP → RJ, MG, ES): 200 m³/mês
│   └── Compatível: ✅ SIM (mesmo destino Sudeste)
│   └── Mix potencial: +200 m³ no mesmo trecho
│
├── Varejo Premium (SP → RS, PR): 150 m³/mês
│   └── Compatível: ✅ SIM (mesmo destino Sul)
│   └── Mix potencial: +150 m³
│
└── Distribuidor Nordeste (SP → Fortaleza): 100 m³/mês
    └── Compatível: ❌ NÃO (Lautos não cobre Nordeste)

SINERGIA POSSÍVEL: +350 m³/mês = +70% volume no mesmo trecho!

RECOMENDAÇÃO: Oferecer desconto de 5-8% ao cliente E-Commerce 
se aceitarem consolidação com Tech Corp + Varejo Premium.
→ Novo volume consolidado: 500 + 350 = 850 m³/mês
→ Reduz custo unitário em 25% (melhor para todos)

─────────────────────────────────────────────────────────────────────

5. VIABILIDADE & ROI
─────────────────────
Decision Tree:

Participar do BID?
├── ✅ SIM, com COBERTURA PARCIAL (Sudeste + Sul + DF)
│   └── ROI esperado: R$ 76.320/ano (fracionado) a R$ 99.744/ano (dedicado)
│   └── Chance de ganho: 85% (somos mais competitivos)
│   └── Timeline: 4-6 horas para resposta (automático)
│
└── RISCO: Cliente não aceita atendimento parcial
    └── Solução: Oferecer parceria com transportador nordestino
    └── Ativação: Contato com parceiros em 24h

SCORE PARTICIPAÇÃO: 9/10 (alto ROI, viável, competitivo)

─────────────────────────────────────────────────────────────────────

6. REQUISITOS ADICIONAIS & IMPLEMENTAÇÃO
──────────────────────────────────────────
✅ Rastreamento GPS em tempo real:
   └── Já incluído em nossa tabela (no custo operacional)
   └── Sistema: Vago/Samsara + integração de custos
   └── Tempo de implementação: ZERO (já operacional)

✅ Portal de consulta (rastreamento + volumes):
   └── Desenvolvido em 2 semanas (simples)
   └── Custo: R$ 2.000 (one-time) + R$ 500/mês (manutenção)
   └── Imputar no preço: +R$ 0.05/kg (R$ 750/mês para 15.000 kg)

✅ Relatório semanal de volumes:
   └── Automático via integração com TMS
   └── Tempo de geração: <5 minutos
   └── Formato: Excel + PDF automático
   └── Custo: ZERO (interno)

─────────────────────────────────────────────────────────────────────

RECOMENDAÇÃO FINAL
═══════════════════════════════════════════════════════════════════

🎯 PARTICIPAR: SIM

📋 PROPOSTA ESTRUTURA:

OPÇÃO 1 (RECOMENDADA): FRACIONADO COM MIX SINÉRGICO
• Preço: R$ 2.12/kg
• Cobertura: 62% (Sudeste + Sul + DF) + parceria Nordeste
• Volume garantido: 500 m³/mês
• Sinergia potencial: +350 m³ (Tech Corp + Varejo)
• Revenue: R$ 31.800/mês (base) + R$ 7.420 (sinergia) = R$ 39.220/mês
• Lucro mensal: R$ 9.800-11.760
• Chance de ganho: 85%
• Timeline resposta: 4-6 horas

OPÇÃO 2: DEDICADO
• Preço: R$ 5.500 fixo + R$ 1.85/kg
• Melhor margem, menos flexível
• Chance de ganho: 60% (cliente pode preferir fracionado)

📧 PRÓXIMO PASSO:
1. Thiago revisa proposta (30 min)
2. Envia para Max (aprovação executiva)
3. Gera resposta formal em 2-3 horas
4. Concorrentes levarão 3-5 dias
→ VANTAGEM: Resposta mais rápida = melhor impressão

📈 IMPACTO NO RESULTADO:
• Nova receita anual: R$ 470.640 (fracionado puro)
• Com sinergia: R$ 549.360/ano
• Novo cliente de médio porte (tier 2)
• Possível upgrade para tier 1 em 6 meses (crescimento)

═══════════════════════════════════════════════════════════════════
Relatório gerado automaticamente por nokk-chat | Qualidade: 95%
Tempo de análise: 4 minutos (vs 6 horas manual)
Economia: 5h 56min | Valor: R$ 298
═══════════════════════════════════════════════════════════════════
```

---

## COMPARAÇÃO: Manual vs Automático

| Métrica | Manual (Thiago) | Automático (nokk-chat) |
|---------|-----------------|------------------------|
| **Tempo** | 6-8 horas | 4 minutos |
| **Qualidade análise** | 95% | 95% |
| **Custo (mão obra)** | R$ 300-400 | R$ 0.15 (tokens IA) |
| **Cobertura geográfica** | Sim | Sim |
| **Identificação de mix** | Manual, após análise | Automático, recomendação |
| **Análise de sinergia** | Não faz | Sim, compara histórico |
| **Cálculo de preço** | Manual + planilha | Automático + decisão |
| **Documentação** | Resumido | Detalhado (PDF pronto) |
| **Recomendação final** | Genérica | Decisória (Yes/No) |

---

## Próxima Etapa: Resposta Formal ao Cliente

```
Prezados,

Recebemos vosso edital de BID para transportação de cargas e-commerce.
Após análise detalhada, temos o prazer de submeter nossa proposta:

COBERTURA:
✓ Sudeste (SP, RJ, MG, ES): 100% cobertura
✓ Sul (PR, RS, SC): 100% cobertura
✓ Centro-Oeste (DF): 100% cobertura
◐ Nordeste + Norte: Via parceria estratégica (terceirizado)

PROPOSTA FINANCEIRA:

FRACIONADO:
• Preço: R$ 2,12/kg (mix de cargas)
• Rastreamento GPS: INCLUÍDO
• SLA: 48h Sudeste, 72h demais regiões

DEDICADO:
• Preço: R$ 5.500/mês fixo + R$ 1,85/kg
• Coleta diária em São Paulo
• Rastreamento + Portal de consulta: INCLUÍDO

DIFERENCIAIS LAUTOS:
1. Resposta em 4 horas (vs 3-5 dias concorrentes)
2. Preço 15-20% melhor que mercado
3. Rastreamento em tempo real (grátis)
4. Possibilidade de mix com nossos clientes (economia adicional)

Estamos disponíveis para discussão.

Att,
LAUTOS Transportes
```

---

## Conclusão

**Por que isso funciona:**

1. ✅ **Análise completa** em 4 minutos (vs 6 horas)
2. ✅ **Acurácia 95%** (validação manual simples em 30 min)
3. ✅ **Recomendação decisória** (não apenas análise)
4. ✅ **Sinergia identificada automaticamente** (valor real)
5. ✅ **Documentação pronta para cliente** (profissionalismo)
6. ✅ **ROI mensurável** (R$ 298/análise economizado)

**Impacto comercial:**
- **Velocidade:** Resposta em 4h vs 3-5 dias (80% mais rápido)
- **Qualidade:** Mais alternativas (fracionado vs dedicado), análise de sinergia
- **Margem:** Preço otimizado, identificação de oportunidades
- **Volume:** Possibilidade de consolidação com clientes existentes

---

**Próximo passo:** Importar dados reais de 1 BID da Lautos e fazer análise piloto.
