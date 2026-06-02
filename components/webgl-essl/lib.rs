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
pub mod lower;
pub mod parse;
pub mod span;
pub mod token;
pub mod validate;
pub mod visit;

pub use ast::TranslationUnit;
pub use error::{Error, ErrorKind};
pub use span::Span;

/// Lex + parse `source` into a [`TranslationUnit`]. Convenience over
/// calling [`lex::lex`] and [`parse::parse`] separately.
///
/// A leading `#version <N> <profile>` directive (the only
/// preprocessor construct ESSL exposes to authors at this layer) is
/// extracted before tokenization and attached to the resulting
/// `TranslationUnit` as the `version` field. The directive line is
/// blanked out in the lexer input so byte offsets and line numbers in
/// later diagnostics still match the original source.
pub fn parse_source(source: &str) -> Result<TranslationUnit, Error> {
    let (version, sans_version) = extract_version_directive(source);
    let tokens = lex::lex(&sans_version)?;
    let mut tu = parse::parse(tokens, sans_version.len())?;
    tu.version = version;
    Ok(tu)
}

/// Find a leading `#version <N> <profile>` line. Returns the numeric
/// version (e.g. `Some(300)` for `#version 300 es`) plus a copy of
/// the source with that line blanked to preserve byte offsets.
fn extract_version_directive(source: &str) -> (Option<u32>, String) {
    let mut version: Option<u32> = None;
    let mut consumed = false;
    let mut sans = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        if !consumed {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#version") {
                if let Some(after) = trimmed.strip_prefix("#version") {
                    if let Some(first_word) = after.split_whitespace().next() {
                        version = first_word.parse().ok();
                    }
                }
                consumed = true;
                // Blank the directive line; keep the trailing newline
                // (if any) so line numbers are preserved.
                let has_nl = line.ends_with('\n');
                let blank_len = line.len() - if has_nl { 1 } else { 0 };
                for _ in 0..blank_len {
                    sans.push(' ');
                }
                if has_nl {
                    sans.push('\n');
                }
                continue;
            }
            if !trimmed.is_empty() {
                consumed = true;
            }
        }
        sans.push_str(line);
    }
    (version, sans)
}
