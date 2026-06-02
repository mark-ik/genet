/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 6 path-A receipt: ESSL → SPIR-V (rspirv) → naga IR
//! (`spv-in`) → WGSL (`wgsl-out`). The lowering today only handles
//! the constant-color shape; this test file exists to prove the seam
//! end-to-end (naga validation included) and to anchor the WGSL
//! output text for visual inspection.

use webgl_essl::lower::lower_to_wgsl;
use webgl_essl::parse_source;
use webgl_essl::validate::ShaderStage;

#[test]
fn const_color_fragment_lowers_through_spirv_naga_to_wgsl() {
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (fragment) ---\n{wgsl}");
    assert!(wgsl.contains("vec4"), "WGSL should mention vec4 somewhere");
    assert!(wgsl.contains("main"), "WGSL should have a main entry point");
}

#[test]
fn const_color_vertex_lowers_through_spirv_naga_to_wgsl() {
    let src = r#"
void main() {
    gl_Position = vec4(0.0, 0.0, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (vertex) ---\n{wgsl}");
    assert!(wgsl.contains("vec4"));
    assert!(wgsl.contains("main"));
}

#[test]
fn lowering_with_no_main_returns_no_main_error() {
    let src = "void helper() {}";
    let tu = parse_source(src).expect("parse");
    let err = lower_to_wgsl(&tu, ShaderStage::Fragment).unwrap_err();
    assert!(
        matches!(err, webgl_essl::lower::LoweringError::NoMain),
        "got: {err:?}"
    );
}

#[test]
fn lowering_an_unsupported_shape_returns_unsupported() {
    // Two statements in main is outside today's accepted shape.
    let src = r#"
precision mediump float;
void main() {
    float t = 0.5;
    gl_FragColor = vec4(t, t, t, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let err = lower_to_wgsl(&tu, ShaderStage::Fragment).unwrap_err();
    assert!(
        matches!(err, webgl_essl::lower::LoweringError::UnsupportedShape { .. }),
        "got: {err:?}"
    );
}

#[test]
fn fragment_target_with_vertex_lhs_is_rejected() {
    // Lowering catches stage mismatch (the validator would too at a
    // higher level, but the lowering's own narrow shape check guards it).
    let src = "void main() { gl_Position = vec4(0.0, 0.0, 0.0, 1.0); }";
    let tu = parse_source(src).expect("parse");
    let err = lower_to_wgsl(&tu, ShaderStage::Fragment).unwrap_err();
    assert!(
        matches!(err, webgl_essl::lower::LoweringError::UnsupportedShape { .. }),
        "got: {err:?}"
    );
}

// ---------- widening: attribute → vec4 constructor --------------------

#[test]
fn canonical_vertex_with_vec2_attribute_lowers_to_wgsl() {
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (canonical vertex) ---\n{wgsl}");
    // naga renders the input variable somewhere; the location-0
    // decoration must come through.
    assert!(wgsl.contains("location(0)"), "WGSL should expose @location(0) for the attribute: {wgsl}");
    assert!(wgsl.contains("@vertex"));
}

#[test]
fn vertex_with_vec3_attribute_lowers_to_wgsl() {
    let src = r#"
attribute vec3 a_position;
void main() {
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (vec3 attribute) ---\n{wgsl}");
    assert!(wgsl.contains("vec3<f32>"));
}

#[test]
fn vertex_with_two_attributes_assigns_distinct_locations() {
    // The shader uses only a_position; a_other is declared but not
    // referenced in main. Both should be registered with their own
    // @location decorations in the WGSL output.
    let src = r#"
attribute vec2 a_position;
attribute vec3 a_other;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (two attributes) ---\n{wgsl}");
    assert!(wgsl.contains("location(0)"));
    assert!(wgsl.contains("location(1)"));
}

#[test]
fn nested_vec3_inside_vec4_constructor_lowers() {
    let src = r#"
void main() {
    gl_Position = vec4(vec3(0.0), 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (nested vec3) ---\n{wgsl}");
    assert!(wgsl.contains("vec4<f32>"));
}

#[test]
fn referencing_unknown_ident_in_vec4_returns_unsupported() {
    // a_position is not declared, so the lowering's input lookup
    // misses and reports UnsupportedShape.
    let src = r#"
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let err = lower_to_wgsl(&tu, ShaderStage::Vertex).unwrap_err();
    assert!(
        matches!(err, webgl_essl::lower::LoweringError::UnsupportedShape { .. }),
        "got: {err:?}"
    );
}

#[test]
fn vec2_plus_scalar_add_still_unsupported() {
    // Component-wise addition between a vec and a scalar requires the
    // OpFAdd dispatch to handle the broadcast case; the Mul / Div
    // path has the broadcast (via OpVectorTimesScalar), Add / Sub
    // still want both sides to be same-shape.
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position + 1.0, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let err = lower_to_wgsl(&tu, ShaderStage::Vertex).unwrap_err();
    assert!(
        matches!(err, webgl_essl::lower::LoweringError::UnsupportedShape { .. }),
        "got: {err:?}"
    );
}

// ---------- widening: binary ops on float / vec_n ---------------------

#[test]
fn vec2_plus_vec2_componentwise_add_lowers() {
    let src = r#"
attribute vec2 a;
attribute vec2 b;
void main() {
    gl_Position = vec4(a + b, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (vec2 add) ---\n{wgsl}");
    assert!(wgsl.contains("vec2<f32>"));
}

#[test]
fn vec2_times_scalar_lowers_via_vector_times_scalar() {
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position * 2.0, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (vec2 * scalar) ---\n{wgsl}");
    assert!(wgsl.contains("vec2<f32>"));
}

#[test]
fn scalar_times_vec3_swaps_to_vector_times_scalar() {
    // The lowering canonicalizes `scalar * vec` into the SPIR-V
    // `OpVectorTimesScalar(vec, scalar)` opcode shape.
    let src = r#"
attribute vec3 a_color;
void main() {
    gl_Position = vec4(0.5 * a_color, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (scalar * vec3) ---\n{wgsl}");
    assert!(wgsl.contains("vec3<f32>"));
}

#[test]
fn float_division_lowers() {
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position * (1.0 / 2.0), 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (float div) ---\n{wgsl}");
    assert!(wgsl.contains("vec4<f32>"));
}

// ---------- widening: uniforms ---------------------------------------

#[test]
fn uniform_vec4_fragment_passthrough_lowers() {
    let src = r#"
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (uniform vec4 passthrough) ---\n{wgsl}");
    // naga should emit a uniform group binding at descriptor set 0,
    // binding 0.
    assert!(wgsl.contains("@group(0)") && wgsl.contains("@binding(0)"));
}

#[test]
fn uniform_float_scaled_attribute_vertex_lowers() {
    let src = r#"
uniform float u_scale;
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position * u_scale, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (uniform float + vec2 attribute) ---\n{wgsl}");
    assert!(wgsl.contains("vec2<f32>"));
}

#[test]
fn multiple_uniforms_in_one_block() {
    let src = r#"
uniform vec4 u_color;
uniform vec4 u_tint;
void main() {
    gl_FragColor = u_color;
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (two uniforms in block) ---\n{wgsl}");
    // u_tint is declared but unused; both should appear as members of
    // the same uniform block.
    assert!(wgsl.contains("@group(0)") && wgsl.contains("@binding(0)"));
}
