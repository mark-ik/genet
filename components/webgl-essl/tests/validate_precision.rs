/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 fourth chunk: R8 missing-precision check on float-family
//! declarations in fragment shaders (ESSL 1.00 §4.5.3).
//!
//! Vertex shaders default float precision to highp, so this check is
//! gated on `ShaderStage::Fragment`.

use webgl_essl::ast::TypeKind;
use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, stage)
}

fn precision_missing(r: &webgl_essl::validate::ValidationResult) -> Vec<(String, TypeKind)> {
    r.errors
        .iter()
        .filter_map(|d| match &d.kind {
            WebGlDiagnosticKind::PrecisionMissingForFloat { name, ty } => {
                Some((name.clone(), *ty))
            },
            _ => None,
        })
        .collect()
}

// ---------- accepted shapes -------------------------------------------

#[test]
fn fragment_with_default_precision_for_float_is_clean() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(precision_missing(&r).is_empty(), "got {:?}", precision_missing(&r));
}

#[test]
fn fragment_with_inline_precision_qualifier_is_clean() {
    let src = r#"
uniform mediump vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(precision_missing(&r).is_empty(), "got {:?}", precision_missing(&r));
}

#[test]
fn vertex_does_not_require_precision_for_float_family() {
    // Vertex shaders default float to highp; the check does not fire.
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert!(precision_missing(&r).is_empty(), "got {:?}", precision_missing(&r));
}

#[test]
fn fragment_with_non_float_types_is_clean() {
    // Integer / bool / sampler types have implementation-defined
    // defaults; R8 only fires for float-family types.
    let src = r#"
uniform int u_count;
uniform bool u_flag;
void main() {
    gl_FragColor = vec4(0.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    // gl_FragColor itself does not declare a precision, but it is a
    // builtin not a user decl. Same for the const-vec4 expression.
    assert!(precision_missing(&r).is_empty(), "got {:?}", precision_missing(&r));
}

// ---------- rejected shapes -------------------------------------------

#[test]
fn fragment_uniform_vec4_without_precision_is_rejected() {
    let src = r#"
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let missing = precision_missing(&r);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].0, "u_color");
    assert_eq!(missing[0].1, TypeKind::Vec4);
}

#[test]
fn fragment_varying_float_without_precision_is_rejected() {
    let src = r#"
varying float v_intensity;
void main() {
    gl_FragColor = vec4(v_intensity);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let missing = precision_missing(&r);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].0, "v_intensity");
    assert_eq!(missing[0].1, TypeKind::Float);
}

#[test]
fn fragment_local_decl_float_without_precision_is_rejected() {
    let src = r#"
void main() {
    float t = 0.5;
    gl_FragColor = vec4(t);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let missing = precision_missing(&r);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].0, "t");
}

#[test]
fn fragment_decl_before_default_precision_still_flagged() {
    // Spec: default precision only applies to declarations made AFTER
    // the directive. A `uniform vec4 u_color;` declared before
    // `precision mediump float;` is still missing precision.
    let src = r#"
uniform vec4 u_color;
precision mediump float;
void main() {
    gl_FragColor = u_color;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let missing = precision_missing(&r);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].0, "u_color");
}

#[test]
fn fragment_multiple_missing_emit_per_decl_diagnostic() {
    let src = r#"
uniform vec4 u_color;
uniform vec3 u_normal;
varying vec2 v_uv;
void main() {
    gl_FragColor = vec4(0.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let missing = precision_missing(&r);
    assert_eq!(missing.len(), 3, "got {missing:?}");
}

#[test]
fn fragment_matrix_uniform_also_requires_precision() {
    let src = r#"
uniform mat4 u_mvp;
void main() {
    gl_FragColor = vec4(0.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let missing = precision_missing(&r);
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].1, TypeKind::Mat4);
}

// ---------- info_log rendering ---------------------------------------

#[test]
fn precision_missing_info_log_line_mentions_name_and_type() {
    let src = r#"
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let lines: Vec<&str> = r.info_log.lines().filter(|l| !l.is_empty()).collect();
    assert!(!lines.is_empty());
    let precision_line = lines
        .iter()
        .find(|l| l.contains("u_color"))
        .expect("a line should mention u_color");
    assert!(precision_line.contains("precision"), "got: {precision_line}");
}
