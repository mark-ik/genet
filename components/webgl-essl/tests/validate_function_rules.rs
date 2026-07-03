/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 sixth chunk, R17 / R18 / R19:
//! - R17: `return <expr>;` whose expression type doesn't match the
//!   enclosing function's declared return type.
//! - R18: two user functions with the same name and the same
//!   parameter types.
//! - R19: `attribute T x;` declared in a fragment shader.

use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

fn r17_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::ReturnTypeMismatch { .. }))
        .count()
}

fn r18_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::FunctionRedefinition { .. }))
        .count()
}

fn r19_count(r: &webgl_essl::validate::ValidationResult) -> usize {
    r.errors
        .iter()
        .filter(|d| {
            matches!(
                d.kind,
                WebGlDiagnosticKind::AttributeInFragmentShader { .. }
            )
        })
        .count()
}

// ---------- R17: return type mismatch -------------------------------

#[test]
fn return_float_from_int_function_emits_r17() {
    let src = r#"
precision mediump float;
int pick() {
    return 1.5;
}
void main() {
    int i = pick();
    gl_FragColor = vec4(float(i));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(
        r17_count(&r) >= 1,
        "float -> int return should fire R17: {:#?}",
        r.errors
    );
}

#[test]
fn return_matching_type_passes_r17() {
    let src = r#"
precision mediump float;
float pick(float x) {
    return x * 2.0;
}
void main() {
    gl_FragColor = vec4(pick(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r17_count(&r),
        0,
        "matching types should pass R17: {:#?}",
        r.errors
    );
}

#[test]
fn return_vec3_from_vec4_function_emits_r17() {
    let src = r#"
precision mediump float;
vec4 promote(vec3 v) {
    return v;
}
void main() {
    gl_FragColor = promote(vec3(1.0));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(r17_count(&r) >= 1, "vec3 -> vec4 return should fire R17");
}

// ---------- R18: function redefinition ------------------------------

#[test]
fn two_functions_same_name_same_params_emits_r18() {
    let src = r#"
precision mediump float;
float helper(float x) { return x + 1.0; }
float helper(float y) { return y - 1.0; }
void main() {
    gl_FragColor = vec4(helper(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(
        r18_count(&r) >= 1,
        "redefined helper should fire R18: {:#?}",
        r.errors
    );
}

#[test]
fn two_functions_same_name_different_params_passes_r18() {
    // Overloading by parameter type is permitted in ESSL.
    let src = r#"
precision mediump float;
float helper(float x) { return x + 1.0; }
float helper(vec2 v) { return v.x + v.y; }
void main() {
    gl_FragColor = vec4(helper(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r18_count(&r),
        0,
        "different param types should pass R18: {:#?}",
        r.errors
    );
}

#[test]
fn distinct_function_names_pass_r18() {
    let src = r#"
precision mediump float;
float helper(float x) { return x + 1.0; }
float other(float x) { return x - 1.0; }
void main() {
    gl_FragColor = vec4(helper(0.5) + other(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r18_count(&r), 0, "distinct names should pass R18");
}

// ---------- R19: attribute in fragment shader -----------------------

#[test]
fn attribute_in_fragment_shader_emits_r19() {
    let src = r#"
precision mediump float;
attribute vec3 a_pos;
void main() {
    gl_FragColor = vec4(a_pos, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(
        r19_count(&r) >= 1,
        "attribute in fragment should fire R19: {:#?}",
        r.errors
    );
}

#[test]
fn attribute_in_vertex_shader_passes_r19() {
    let src = r#"
attribute vec3 a_pos;
void main() {
    gl_Position = vec4(a_pos, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert_eq!(
        r19_count(&r),
        0,
        "attribute in vertex should pass R19: {:#?}",
        r.errors
    );
}

#[test]
fn varying_in_fragment_does_not_emit_r19() {
    let src = r#"
precision mediump float;
varying vec3 v_color;
void main() {
    gl_FragColor = vec4(v_color, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r19_count(&r), 0, "varying in fragment should pass R19");
}
