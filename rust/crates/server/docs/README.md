# elai-server — API Docs

Documentação interativa (OpenAPI 3.1 + Redoc + Swagger UI) para a API HTTP do `elai-server`.

## Estrutura

```
docs/
├── README.md          # este arquivo
├── serve.sh           # serve o site local (python3 ou npx http-server)
└── site/              # site estático pronto para hospedagem
    ├── index.html     # landing + abas (Redoc / Swagger / YAML)
    ├── redoc.html     # Redoc
    ├── swagger.html   # Swagger UI (com Try it out)
    └── openapi.yaml   # especificação OpenAPI 3.1 (fonte da verdade)
```

## Rodar localmente

```bash
# A partir da raiz do repo:
./rust/crates/server/docs/serve.sh          # http://127.0.0.1:8080
./rust/crates/server/docs/serve.sh 9090     # porta customizada

# Ou manualmente:
cd rust/crates/server/docs/site
python3 -m http.server 8080
```

Abra http://127.0.0.1:8080/ no navegador.

## Hospedar (GitHub Pages)

Como o site é 100 % estático, basta apontar o GitHub Pages para o diretório `rust/crates/server/docs/site/`:

1. Em `Settings → Pages`, escolha branch `main` (ou `gh-pages`) e a pasta `rust/crates/server/docs/site`.
2. O site fica disponível em `https://<owner>.github.io/<repo>/`.

Alternativas: Cloudflare Pages, Netlify, Vercel, S3 + CloudFront — todos consomem o diretório `site/` direto.

## Atualizar a especificação

A fonte da verdade é `site/openapi.yaml`. Para regenerar clientes/SDKs:

```bash
# TypeScript
npx openapi-typescript site/openapi.yaml -o ./elai-server.d.ts

# Cliente Rust (reqwest)
openapi-generator-cli generate -i site/openapi.yaml -g rust -o ./elai-client-rs
```

## Autenticação

Quase todos os endpoints exigem `Authorization: Bearer <token>`. O token é gerado/persistido pelo binário `elai-server` em `~/.elai/server-token`. Você pode usá-lo no Swagger UI clicando em **Authorize** no canto superior direito.
