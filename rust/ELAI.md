# ELAI.md

This file provides guidance to Elai Code (elai.dev) when working with code in this repository.

## Detected stack
- Languages: Rust.
- Frameworks: none detected from the supported starter markers.

## Verification
- Run Rust verification from the repo root: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`

## Working agreement
- Prefer small, reviewable changes and keep generated bootstrap files aligned with actual repo workflows.
- Keep shared defaults in `.elai.json`; reserve `.elai/settings.local.json` for machine-local overrides.
- Do not overwrite existing `ELAI.md` content automatically; update it intentionally when repo workflows change.

## Budget Save — 1777240125
- Reason: Cost: $1.2743/$ 1.0000 (100%)
- Tokens: 1235742/0
- Turns: 24
- Cost: $1.2743
- Model: claude-haiku-4-5-20251001
