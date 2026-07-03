/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 fifth chunk, R12: ESSL 1.00 Appendix A constrains array,
//! vector, and matrix indices to constant-index-expressions. The
//! first-pass acceptance set is literal integer constants and
//! currently-active loop induction variables. ESSL 3.00 relaxes the
//! rule, so each test pins the version it targets.
//!
//! Test shape note: the parser does not yet accept global array
//! declarations like `uniform vec4 u[4];`. Matrix column indexing
//! (`mat4 m; m[i]`) exercises the same `Expr::Index` path R12
//! validates and stays inside the parser's supported surface.

use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

fn r12_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::IndirectArrayIndex))
        .count()
}

// ---------- accept set: literal int, loop var, constant expr ----------

#[test]
fn literal_int_index_passes_r12() {
    let src = r#"
precision mediump float;
uniform mat4 m;
void main() {
    gl_FragColor = m[0];
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r12_count(&r), 0, "literal index should pass R12");
}

#[test]
fn loop_var_index_passes_r12() {
    let src = r#"
precision mediump float;
uniform mat4 m;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 4; ++i) {
        acc += m[i];
    }
    gl_FragColor = acc;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r12_count(&r),
        0,
        "loop induction var should pass R12: {:#?}",
        r.errors
    );
}

#[test]
fn constant_arithmetic_index_passes_r12() {
    let src = r#"
precision mediump float;
uniform mat4 m;
void main() {
    gl_FragColor = m[2 + 1];
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r12_count(&r), 0, "constant arithmetic should pass R12");
}

#[test]
fn loop_var_plus_constant_passes_r12() {
    let src = r#"
precision mediump float;
uniform mat4 m;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 3; ++i) {
        acc += m[i + 1];
    }
    gl_FragColor = acc;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r12_count(&r), 0, "loop_var + const should pass R12");
}

// ---------- reject set: dynamic ident, runtime expr ------------------

#[test]
fn dynamic_uniform_index_emits_r12() {
    // `idx` is a uniform; not a constant, not a loop var. WebGL 1
    // rejects.
    let src = r#"
precision mediump float;
uniform int idx;
uniform mat4 m;
void main() {
    gl_FragColor = m[idx];
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r12_count(&r), 1, "dynamic uniform index should fail R12");
}

#[test]
fn non_loop_var_index_emits_r12() {
    // `j` is a local int but not a loop induction variable.
    let src = r#"
precision mediump float;
uniform mat4 m;
void main() {
    int j = 1;
    gl_FragColor = m[j];
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r12_count(&r), 1, "non-loop-var index should fail R12");
}

// ---------- loop-var scope rule --------------------------------------

#[test]
fn loop_var_used_outside_loop_emits_r12() {
    // Once the loop ends, the induction var is no longer "active"
    // in the visitor's stack; using its name as an index outside
    // the loop must fail R12.
    let src = r#"
precision mediump float;
uniform mat4 m;
void main() {
    for (int k = 0; k < 4; ++k) {}
    int k = 1;
    gl_FragColor = m[k];
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r12_count(&r), 1, "loop var outside loop should fail R12");
}

#[test]
fn nested_loops_both_vars_accepted_as_indices() {
    let src = r#"
precision mediump float;
uniform mat4 m;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 2; ++i) {
        for (int j = 0; j < 2; ++j) {
            acc += m[i] + m[j];
        }
    }
    gl_FragColor = acc;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r12_count(&r),
        0,
        "both loop vars should pass: {:#?}",
        r.errors
    );
}

// ---------- version gate ----------------------------------------------

#[test]
fn essl_300_relaxes_r12() {
    // ESSL 3.00 permits indirect indexing on non-sampler arrays /
    // matrices; R12 must not fire under `#version 300 es`.
    let src = r#"#version 300 es
precision mediump float;
uniform int idx;
uniform mat4 m;
out vec4 out_color;
void main() {
    out_color = m[idx];
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r12_count(&r),
        0,
        "ESSL 3.00 should not fire R12: {:#?}",
        r.errors
    );
}
