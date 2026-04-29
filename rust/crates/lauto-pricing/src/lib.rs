//! # `lauto-pricing`
//!
//! Biblioteca de apoio à proposta comercial Nokk × Lauto. Não chama LLM —
//! estima COGS e classifica editais via heurísticas determinísticas para
//! sustentar o pricing publicado em `docs/lauto-proposta-nokk.md`.
//!
//! Os binários `lauto-unit-econ` e `lauto-bid-demo` consomem este módulo.
//!
//! Premissas estão centralizadas em [`pricing::DEFAULT_PRICING`] e
//! [`PRICE_TABLE_BRL`] para que o setor comercial recalibre sem mexer em
//! lógica.

#![forbid(unsafe_code)]
// Casts numéricos f64 ↔ u64 ↔ usize são intencionais nesta crate: as
// quantidades que tratamos (tokens, valores em centavos) ficam muito abaixo
// dos limites de precisão do f64 (2⁵²) e dos limites de saturação dos
// tipos inteiros, e a aritmética é toda em USD/BRL com precisão monetária.
// Suprimir aqui evita poluir cada call-site com `#[allow]` e mantém o lint
// ativo nas demais crates do workspace.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
)]

pub mod pdf;
pub mod patterns;
pub mod pricing;
pub mod tier;
pub mod unit_econ;
pub mod bid;
