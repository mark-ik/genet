/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pure-Rust ESSL frontend, spike-grade.
//!
//! This crate exists to test the "Rust replacement for ANGLE's shader
//! translator" hypothesis. Today it ships:
//!
//! - A byte-level lexer for ESSL 1.00 ([`lex`]).
//! - A faithful parse tree for the canonical-triangle shader corpus plus
//!   uniform / varying / binary `*` ([`ast`]).
//! - A recursive-descent + Pratt parser that builds the tree ([`parse`]).
//!
//! It does not ship a type checker, validator, or lowering. Those layers
//! sit above this crate; the parse tree is the contract they consume.
//!
//! See `serval/docs/2026-05-28_webgl_essl_rust_frontend_spike.md` for
//! the architectural framing and the ANGLE-as-oracle differential plan.

#![deny(unsafe_code)]

pub mod ast;
pub mod check;
pub mod error;
pub mod lex;
pub mod parse;
pub mod span;
pub mod token;
pub mod visit;

pub use ast::TranslationUnit;
pub use error::{Error, ErrorKind};
pub use span::Span;

/// Lex + parse `source` into a [`TranslationUnit`]. Convenience over
/// calling [`lex::lex`] and [`parse::parse`] separately.
pub fn parse_source(source: &str) -> Result<TranslationUnit, Error> {
    let tokens = lex::lex(source)?;
    parse::parse(tokens, source.len())
}
