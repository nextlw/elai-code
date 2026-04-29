# Edital de exemplo — Marinha do Brasil PE 90008/2025

> **Encontrado e baixado** via API pública do PNCP em 31/03/2026.
> Servirá como **calibração** dos tiers de BID e como **demo de venda** para Lauto.
>
> Status: dados públicos, podem ser usados livremente.

---

## 1. Identificação

| Campo | Valor |
|---|---|
| Nº do Edital | **90008/2025** |
| Modalidade | Pregão Eletrônico |
| Órgão | Comando da Marinha — CIM Brasília |
| CNPJ | 00394502000144 |
| UF/Município | DF / Brasília |
| Publicação PNCP | 05/02/2025 |
| Vigência | 05/02/2025 → 14/02/2025 |
| Nº de controle PNCP | 00394502000144-1-000642/2025 |
| URL portal | https://pncp.gov.br/app/editais/00394502000144/2025/642 |

**Objeto:**
> *Contratação de empresa especializada para a prestação de serviços continuados
> de transporte de carga geral, fracionada ou de um só volume ou unitizada,
> por via rodoviária, para entrega porta a porta, local, intermunicipal e
> interestadual, com origem na cidade de Brasília/DF podendo, também, atender
> ao fluxo inverso.*

> **Por que serve para Lauto:** descreve **exatamente** o serviço que ela vende
> — frete fracionado + carregamento total, rodoviário, intermunicipal e
> interestadual. Inclui termo de referência, minuta de contrato, anexos de
> volume e ETP. É o tipo de pacote que cai na mesa do Thiago.

---

## 2. Pacote do edital (6 PDFs, 92 páginas)

Local: `docs/editais_exemplo/marinha_pe_90008_2025/`

| Arquivo | Páginas | Palavras | Cargas especiais detectadas | Tier classificado | COGS estimado |
|---|---|---|---|---|---|
| 01_edital.pdf | 21 | 10.608 | valor_agregado | HEAVY | R$ 0,30 |
| 02_minuta_contrato.pdf | 12 | 6.156 | valor_agregado | HEAVY | R$ 0,17 |
| 03_termo_referencia.pdf | 32 | 12.351 | inflamavel, valor_agregado | HEAVY | R$ 0,35 |
| 04_anexo_b_carregamento_total.pdf | 1 | 500 | — | LIGHT | R$ 0,00 |
| 05_anexo_c_carregamento_parcial.pdf | 3 | 854 | — | LIGHT | R$ 0,00 |
| 06_etp.pdf | 23 | 7.405 | alimentos, farmaceutico, inflamavel, perigosa, valor_agregado | HEAVY | R$ 0,21 |
| **TOTAL agregado** | **92** | **37.874** | **5 especificidades** | **HEAVY** | **R$ 1,03** |

---

## 3. Calibração dos tiers — o que esse edital prova

### 3.1 Comportamento do classificador

| Predição | Resultado | Comentário |
|---|---|---|
| Anexos de quantitativo (1–3 págs) → LIGHT | ✅ Acertou | Tabelas de volume são triviais, não exigem Sonnet |
| Edital + TR + ETP → HEAVY | ✅ Acertou | Multi-cargas, +20 págs, contratos jurídicos pesados |
| Detecção de cargas especiais | ✅ ETP capturou 5/6 categorias | Bate com lista da §5.3 da proposta |
| Sinais de complexidade | 508 hits agregados | Confirma necessidade de raciocínio multi-passo |

### 3.2 Implicações para o pricing

1. **Um BID real é um pacote, não um arquivo único.** O processador precisa
   aceitar múltiplos PDFs, classificar cada um e agregar. Isso já está
   modelado no script atual (executável via `*.pdf`).
2. **Dois anexos pequenos foram classificados como LIGHT** — corretamente.
   Não precisa Sonnet para extrair quantitativo de tabela. Isso valida a
   estratégia de **roteamento por complexidade**.
3. **COGS agregado de R$ 1,03** para um BID de 92 páginas com 5 categorias de
   carga especial. Margem bruta: **99,88%** sobre preço lista R$ 890. **A
   tese de unit economics se mantém com dado real.**
4. **Preço único de R$ 890 cobre o pacote inteiro** — não cobrar por documento.
   Cobrança é por **BID respondido**, e o BID é uma análise multi-documento.
   Refletir isso na §9.2 da proposta.

### 3.3 O que ainda não está testado

- **Edital com cláusulas de penalidade complexas** (multas, reajustes IPCA/IGPM):
  presentes neste edital, mas não medi qualidade da extração — só custo.
- **Editais > 80 páginas** (BIDs federais grandes, PETROBRAS, Vale, etc):
  precisaríamos coletar amostras maiores para validar o tier Heavy outlier.
- **Editais escaneados sem OCR**: o Anexo B/C aqui veio em PDF nativo. Em
  editais reais é comum ter páginas escaneadas — adicionar `ocrmypdf` ao
  pipeline antes de Jina.

---

## 4. Material para demo de venda

Este edital pode ser usado em apresentação para Lauto como **prova viva**:

> **Slide:** *"Pegamos um edital real de Pregão da Marinha, idêntico ao tipo
> que o Thiago analisa hoje em ~1 semana. Em 92 páginas, 37 mil palavras, 5
> categorias de carga especial. Nokk classificou 6 documentos automaticamente,
> separou anexos triviais dos contratuais, identificou todas as cargas
> especiais e produziu o draft inicial em **menos de 3 minutos**, com custo
> computacional de **R$ 1,03**. Mesmo cobrando R$ 890 pelo BID — Lauto **paga
> 1/3 do que paga hoje em horas de Thiago** — Nokk mantém margem >99%."*

### 4.1 Métricas de impacto vendáveis

| Métrica | Hoje (Thiago manual) | Com Nokk |
|---|---|---|
| Tempo de leitura inicial | 4–8h | 3 min |
| Tempo total ciclo BID | ~5 dias úteis | <24h |
| Custo de mão de obra Lauto/BID | ~R$ 1.500–2.500 (hora-analista) | R$ 890 (BID Heavy) |
| Capacidade mensal | ~3–4 BIDs | 10+ BIDs |

> Validar números de horas com Thiago no discovery.

---

## 5. Como reproduzir

```bash
# Listar editais de transporte de cargas no PNCP
curl -s 'https://pncp.gov.br/api/search/?tipos_documento=edital&q=transporte%20cargas' \
  | python3 -m json.tool | head -50

# Baixar arquivos de um edital específico (CNPJ/ano/sequencial)
curl -s 'https://pncp.gov.br/api/pncp/v1/orgaos/00394502000144/compras/2025/642/arquivos'

# Baixar arquivo (ZIP com PDFs)
curl -L -o edital.zip \
  'https://pncp.gov.br/pncp-api/v1/orgaos/00394502000144/compras/2025/642/arquivos/1'

# Rodar classificador
python3 scripts/lauto_unit_econ.py docs/editais_exemplo/marinha_pe_90008_2025/*.pdf
```

---

## 6. Próximos editais a coletar (para amostra estatística)

Outros candidatos retornados na mesma busca, todos relevantes:

| Edital | Órgão | UF | Característica útil |
|---|---|---|---|
| 90019/2025 | Marinha — CIM Belém | PA | Multimodal aéreo + rodoviário |
| 90010/2024 | Marinha — Natal | RN | Fracionado + carregamento total |
| 90352/2024 | SEC. Indústria/AC | AC | Aéreo + terrestre, internacional |
| 90015/2024 | Exército — São Paulo | SP | Bagagens (especificidade militar) |

> Recomendo coletar 4–5 desses e rodar para construir a curva real de
> distribuição de tiers Light/Standard/Heavy.

---

## 7. Riscos e ressalvas

- **Esses são editais públicos.** O perfil de BID privado da Lauto pode ter
  estrutura diferente (menos boilerplate jurídico, mais detalhe operacional).
  Continuar pedindo 1–2 editais reais anonimizados no discovery (§11 do md).
- **`pdftotext` extraiu sem OCR.** Anexos com tabelas escaneadas podem render
  menos palavras do que o real — verificar manualmente o Anexo B se for
  usar como caso de teste de extração de quantitativos.
- **Cargas especiais detectadas por keyword.** Pode ter falso-positivo
  (ex: "valor agregado" pode aparecer em contexto contratual genérico).
  Avaliar a precisão na fase de POC com edital real da Lauto.
