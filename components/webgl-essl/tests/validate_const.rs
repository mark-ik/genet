/* This Source Code Work is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 sixth chunk, R14 + R15: `const`-qualified locals must be
//! initialized with a constant expression and must not be assigned
//! to. The first-pass constant-expression acceptance set is
//! literal IntLit / FloatLit / BoolLit and recursive unary / binary
//! on constants (no constant folding for identifiers yet).

use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

fn r14_without_init_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::ConstWithoutInit { .. }))
        .count()
}

fn r14_not_constant_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::ConstInitNotConstant { .. }))
        .count()
}

fn r15_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::ConstAssignment { .. }))
        .count()
}

// ---------- R14: const initializer required + constant ----------------

#[test]
fn const_with_float_literal_init_passes_r14() {
    let src = r#"
precision mediump float;
void main() {
    const float pi = 3.14;
    gl_FragColor = vec4(pi);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r14_not_constant_count(&r),
        0,
        "literal init should pass: {:#?}",
        r.errors
    );
    assert_eq!(r14_without_init_count(&r), 0);
}

#[test]
fn const_with_arithmetic_constant_init_passes_r14() {
    let src = r#"
precision mediump float;
void main() {
    const float two_pi = 3.14 * 2.0;
    gl_FragColor = vec4(two_pi);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r14_not_constant_count(&r),
        0,
        "constant arithmetic should pass: {:#?}",
        r.errors
    );
}

#[test]
fn const_with_unary_constant_init_passes_r14() {
    let src = r#"
precision mediump float;
void main() {
    const float neg = -1.5;
    gl_FragColor = vec4(neg);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r14_not_constant_count(&r),
        0,
        "-literal should pass: {:#?}",
        r.errors
    );
}

#[test]
fn const_with_identifier_init_emits_r14() {
    // First-pass: identifier (even bound to another const) is not
    // folded. Future widening: track const-ident bindings.
    let src = r#"
precision mediump float;
uniform float u;
void main() {
    const float k = u;
    gl_FragColor = vec4(k);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r14_not_constant_count(&r),
        1,
        "uniform init should fail: {:#?}",
        r.errors
    );
}

#[test]
fn const_without_init_emits_r14() {
    // Parser may or may not accept `const T x;` without an init.
    // If it does parse, validator must flag.
    let src = r#"
precision mediump float;
void main() {
    const float k;
    gl_FragColor = vec4(0.0);
}
"#;
    let r = parse_source(src);
    match r {
        Err(_) => {
            // Parser rejects; nothing for the validator to check.
            // Receipt still pins the constraint: a `const` without
            // init must not reach lowering.
        },
        Ok(tu) => {
            let v = validate(&tu, src, ShaderStage::Fragment);
            assert_eq!(
                r14_without_init_count(&v),
                1,
                "const without init should fail: {:#?}",
                v.errors
            );
        },
    }
}

// ---------- R15: cannot assign to const ------------------------------

#[test]
fn assignment_to_const_local_emits_r15() {
    let src = r#"
precision mediump float;
void main() {
    const float k = 1.0;
    k = 2.0;
    gl_FragColor = vec4(k);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r15_count(&r),
        1,
        "assignment to const should fail R15: {:#?}",
        r.errors
    );
}

#[test]
fn assignment_to_non_const_local_passes_r15() {
    let src = r#"
precision mediump float;
void main() {
    float k = 1.0;
    k = 2.0;
    gl_FragColor = vec4(k);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r15_count(&r),
        0,
        "assignment to non-const should pass: {:#?}",
        r.errors
    );
}

#[test]
fn read_of_const_local_passes_r15() {
    // Reading a const is fine; only writing fires R15.
    let src = r#"
precision mediump float;
void main() {
    const float k = 0.5;
    float x = k + 0.5;
    gl_FragColor = vec4(x);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r15_count(&r), 0, "const read should pass: {:#?}", r.errors);
}
