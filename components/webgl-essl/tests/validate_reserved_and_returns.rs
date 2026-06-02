/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 sixth chunk, R7 extension + R16:
//! - R7 (extended): ESSL §3.6 future-reserved keyword names are
//!   rejected as identifiers in addition to the prefixes the
//!   original R7 already covered (`gl_` / `webgl_` / `_webgl_` /
//!   names containing `__`).
//! - R16: a non-void user function whose body does not return on
//!   every path emits `MissingReturnInNonVoidFunction`. First-pass
//!   is structural — the last top-level body stmt must be Return,
//!   or recursively the last stmt of a Block, or both branches of
//!   the trailing If with else.

use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

fn reserved_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::ReservedIdentifier { .. }))
        .count()
}

fn missing_return_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::MissingReturnInNonVoidFunction { .. }))
        .count()
}

// ---------- R7 extension: future-reserved keyword names --------------

#[test]
fn local_named_goto_is_reserved() {
    let src = r#"
precision mediump float;
void main() {
    float goto = 1.0;
    gl_FragColor = vec4(goto);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(reserved_count(&r) >= 1, "`goto` should be reserved: {:#?}", r.errors);
}

#[test]
fn local_named_template_is_reserved() {
    let src = r#"
precision mediump float;
void main() {
    float template = 1.0;
    gl_FragColor = vec4(template);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(reserved_count(&r) >= 1, "`template` should be reserved");
}

#[test]
fn local_named_unsigned_is_reserved() {
    let src = r#"
precision mediump float;
void main() {
    float unsigned = 1.0;
    gl_FragColor = vec4(unsigned);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(reserved_count(&r) >= 1, "`unsigned` should be reserved");
}

#[test]
fn local_named_sampler1d_is_reserved() {
    let src = r#"
precision mediump float;
void main() {
    float sampler1D = 1.0;
    gl_FragColor = vec4(sampler1D);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(reserved_count(&r) >= 1, "`sampler1D` (no WebGL counterpart) should be reserved");
}

#[test]
fn ordinary_identifier_is_not_reserved() {
    let src = r#"
precision mediump float;
void main() {
    float t = 1.0;
    gl_FragColor = vec4(t);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(reserved_count(&r), 0, "`t` should not be reserved: {:#?}", r.errors);
}

// ---------- R16: missing return in non-void function -----------------

#[test]
fn non_void_function_without_return_emits_r16() {
    let src = r#"
precision mediump float;
float helper(float x) {
    float t = x * 2.0;
}
void main() {
    gl_FragColor = vec4(helper(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(missing_return_count(&r) >= 1, "missing return should fire R16: {:#?}", r.errors);
}

#[test]
fn non_void_function_with_return_passes_r16() {
    let src = r#"
precision mediump float;
float helper(float x) {
    return x * 2.0;
}
void main() {
    gl_FragColor = vec4(helper(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(missing_return_count(&r), 0, "return should pass R16: {:#?}", r.errors);
}

#[test]
fn void_function_without_explicit_return_passes_r16() {
    let src = r#"
precision mediump float;
void noop() {}
void main() {
    noop();
    gl_FragColor = vec4(0.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(missing_return_count(&r), 0, "void noop should pass R16: {:#?}", r.errors);
}

#[test]
fn non_void_with_if_else_both_returning_passes_r16() {
    let src = r#"
precision mediump float;
float pick(float a) {
    if (a > 0.0) {
        return 1.0;
    } else {
        return 0.0;
    }
}
void main() {
    gl_FragColor = vec4(pick(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(missing_return_count(&r), 0, "if/else both returning should pass: {:#?}", r.errors);
}

#[test]
fn non_void_with_if_only_returning_emits_r16() {
    // Only the then-branch returns — the else-implied path falls
    // through. First-pass structural check rejects.
    let src = r#"
precision mediump float;
float pick(float a) {
    if (a > 0.0) {
        return 1.0;
    }
}
void main() {
    gl_FragColor = vec4(pick(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(missing_return_count(&r) >= 1, "incomplete path should fire R16");
}
