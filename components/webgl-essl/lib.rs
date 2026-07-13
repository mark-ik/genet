/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pure-Rust ESSL frontend.
//!
//! This crate ships the full ESSL → WGSL pipeline as separate modules
//! plus a production-shaped [`compile`] entry point that runs the
//! pipeline in order and stops at the first failure:
//!
//! 1. [`lex`] / [`parse`] — byte-level lexer and a recursive-descent +
//!    Pratt parser. ESSL 1.00 covered; the ESSL 3.00 delta is partial
//!    (shift / bitwise / `~`, `#version` directive, `in` / `out` /
//!    `centroid` / `flat` / `smooth`, `switch` / `case` / `default`).
//! 2. [`check`] — symbol resolution, literal types, identifier types,
//!    binary / unary / ternary / member / call / index types, ESSL
//!    constructor rules, swizzles, and the §8 built-in function
//!    registry.
//! 3. [`validate`] — ANGLE-inspired WebGL restrictions (R1 recursion,
//!    R2 stage-gated `discard`, R3 `main` signature, R4 Appendix A
//!    `for` loops, R5 reserved identifiers, R6 expression complexity,
//!    R7 call-stack depth, R8 fragment float-family precision)
//!    rendered as `getShaderInfoLog`-shaped lines.
//! 4. [`lower`] — ESSL → SPIR-V (rspirv) → naga `spv-in` → naga
//!    validation → WGSL (`wgsl-out`). Today's accepted shape covers
//!    vertex / fragment shaders with attribute / uniform globals,
//!    vec_n constructors, and Float/Vec_n binary arithmetic.
//!
//! Architecture note:
//! `genet/docs/2026-05-28_webgl_essl_rust_frontend_spike.md`. Borrow
//! doctrine: read mature peers (chumsky / mozangle / naga / rspirv)
//! for technique, take what fits, attribute when copying expression.

#![deny(unsafe_code)]

pub mod ast;
pub mod check;
pub mod error;
pub mod lex;
pub mod lower;
pub mod parse;
pub mod reflect;
pub mod span;
pub mod token;
pub mod validate;
pub mod visit;

pub use ast::TranslationUnit;
pub use error::{Error, ErrorKind};
pub use span::Span;

/// Production-shaped entry: run the full ESSL → WGSL pipeline on
/// `source`. Stops at the first failing stage. Source text is
/// threaded through to the validator so info-log lines render with
/// real line numbers.
pub fn compile(source: &str, stage: validate::ShaderStage) -> Result<CompileResult, CompileError> {
    let tu = parse_source(source).map_err(CompileError::Parse)?;
    let check_result = check::check(&tu);
    if !check_result.diagnostics.is_empty() {
        return Err(CompileError::Check(check_result.diagnostics));
    }
    let validation = validate::validate(&tu, source, stage);
    if validation.num_errors() > 0 {
        return Err(CompileError::Validate(validation));
    }
    let wgsl = lower::lower_to_wgsl(&tu, stage).map_err(CompileError::Lower)?;
    Ok(CompileResult {
        wgsl,
        info_log: validation.info_log,
    })
}

/// Success result of [`compile`]. `info_log` carries any non-error
/// warnings; an empty string means a fully clean compile.
#[derive(Debug, Clone)]
pub struct CompileResult {
    pub wgsl: String,
    pub info_log: String,
}

/// Failure result of [`compile`]. Each variant identifies which
/// pipeline stage produced the failure; the inner payload is the
/// stage's native diagnostic shape so a caller that wants to render
/// `getShaderInfoLog` text has everything it needs.
#[derive(Debug)]
pub enum CompileError {
    Parse(Error),
    Check(Vec<check::TypeDiagnostic>),
    Validate(validate::ValidationResult),
    Lower(lower::LoweringError),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Parse(e) => write!(f, "parse error: {:?}", e),
            CompileError::Check(diags) => {
                write!(f, "typecheck failed with {} diagnostic(s)", diags.len())
            },
            CompileError::Validate(r) => {
                write!(f, "validation failed with {} error(s)", r.num_errors())
            },
            CompileError::Lower(e) => write!(f, "lowering failed: {e}"),
        }
    }
}

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
