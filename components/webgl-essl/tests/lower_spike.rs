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
    // Write-side (LHS) swizzle is outside today's accepted
    // shape — `lower_stmt`'s assignment branch only accepts
    // `Expr::Ident` as the LHS.
    let src = r#"
precision mediump float;
varying vec4 v_color;
void main() {
    v_color.x = 1.0;
    gl_FragColor = v_color;
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

// ---------- widening: matrix uniforms + mat * vec --------------------

#[test]
fn canonical_mvp_transform_vertex_lowers() {
    let src = r#"
uniform mat4 u_mvp;
attribute vec3 a_position;
void main() {
    gl_Position = u_mvp * vec4(a_position, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (MVP transform) ---\n{wgsl}");
    assert!(wgsl.contains("mat4x4<f32>"));
    assert!(wgsl.contains("@group(0)") && wgsl.contains("@binding(0)"));
    assert!(wgsl.contains("@vertex"));
}

#[test]
fn mat3_times_vec3_uniform_lowers() {
    let src = r#"
uniform mat3 u_normal_mat;
attribute vec3 a_normal;
void main() {
    gl_Position = vec4(u_normal_mat * a_normal, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (mat3 * vec3) ---\n{wgsl}");
    assert!(wgsl.contains("mat3x3<f32>"));
}

#[test]
fn mat2_times_vec2_uniform_lowers() {
    let src = r#"
uniform mat2 u_rot;
attribute vec2 a_position;
void main() {
    gl_Position = vec4(u_rot * a_position, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (mat2 * vec2) ---\n{wgsl}");
    assert!(wgsl.contains("mat2x2<f32>"));
}

#[test]
fn mat4_times_scalar_lowers() {
    let src = r#"
uniform mat4 u_mvp;
attribute vec3 a_position;
void main() {
    gl_Position = (u_mvp * 0.5) * vec4(a_position, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (mat4 * scalar) ---\n{wgsl}");
    assert!(wgsl.contains("mat4x4<f32>"));
}

// ---------- widening: varyings ---------------------------------------

#[test]
fn vertex_writes_varying_then_gl_position() {
    let src = r#"
attribute vec3 a_position;
attribute vec3 a_color;
varying vec3 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (vertex varying out) ---\n{wgsl}");
    // The varying output decoration shows up as @location(0) on a
    // separate non-builtin output.
    assert!(wgsl.contains("@vertex"));
    assert!(wgsl.contains("vec3<f32>"));
}

#[test]
fn fragment_reads_varying_input() {
    let src = r#"
precision mediump float;
varying vec3 v_color;
void main() {
    gl_FragColor = vec4(v_color, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (fragment varying in) ---\n{wgsl}");
    // The varying input should show up as @location(0).
    assert!(wgsl.contains("location(0)"));
    assert!(wgsl.contains("@fragment"));
}

// ---------- widening: function calls ---------------------------------

#[test]
fn user_function_with_single_float_param_lowers() {
    let src = r#"
precision mediump float;
float double_it(float x) { return x * 2.0; }
void main() {
    gl_FragColor = vec4(double_it(0.5));
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (user fn double_it) ---\n{wgsl}");
    assert!(wgsl.contains("double_it") || wgsl.contains("fn function"));
}

#[test]
fn user_function_taking_vec_arg_lowers() {
    let src = r#"
precision mediump float;
vec3 brighten(vec3 c) { return c * 1.5; }
varying vec3 v_color;
void main() {
    gl_FragColor = vec4(brighten(v_color), 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (vec arg brighten) ---\n{wgsl}");
    assert!(wgsl.contains("vec3<f32>"));
}

#[test]
fn user_function_with_two_params_lowers() {
    let src = r#"
precision mediump float;
float lerp_scalar(float a, float b) { return a + b; }
void main() {
    gl_FragColor = vec4(lerp_scalar(0.25, 0.5));
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (two params) ---\n{wgsl}");
    assert!(wgsl.contains("@fragment"));
}

#[test]
fn user_function_chain_calling_another_user_fn_lowers() {
    let src = r#"
precision mediump float;
float square(float x) { return x * x; }
float quad(float x) { return square(x) * square(x); }
void main() {
    gl_FragColor = vec4(quad(0.5));
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (user fn chain) ---\n{wgsl}");
    assert!(wgsl.contains("@fragment"));
}

// ---------- widening: swizzles ----------------------------------------

#[test]
fn swizzle_single_component_x_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = vec4(u_color.x);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (.x swizzle) ---\n{wgsl}");
    assert!(wgsl.contains("@fragment"));
}

#[test]
fn swizzle_rgb_of_vec4_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
void main() {
    gl_FragColor = vec4(u_tint.rgb, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (.rgb swizzle) ---\n{wgsl}");
    // naga's wgsl-out may render the swizzle as `.xyz` field access
    // inline rather than as an explicit `vec3<f32>` type construction.
    // The compile itself succeeding through naga validation is the
    // primary receipt.
    assert!(wgsl.contains("@fragment"));
    assert!(wgsl.contains(".xyz") || wgsl.contains("vec3<f32>"));
}

#[test]
fn swizzle_bgra_reorder_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color.bgra;
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (.bgra reorder) ---\n{wgsl}");
    assert!(wgsl.contains("vec4<f32>"));
}

#[test]
fn swizzle_xy_of_vec3_lowers() {
    let src = r#"
precision mediump float;
uniform vec3 u_pos;
void main() {
    gl_FragColor = vec4(u_pos.xy, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (.xy from vec3) ---\n{wgsl}");
    assert!(wgsl.contains("@fragment"));
    assert!(wgsl.contains(".xy") || wgsl.contains("vec2<f32>"));
}

// ---------- integration: realistic vertex pipeline -------------------

#[test]
fn realistic_vertex_pipeline_lowers() {
    // Combines attributes + uniforms + matrix * vec + varying out +
    // gl_Position. The canonical "real" vertex shader shape.
    let src = r#"
uniform mat4 u_mvp;
attribute vec3 a_position;
attribute vec3 a_color;
varying vec3 v_color;
void main() {
    v_color = a_color;
    gl_Position = u_mvp * vec4(a_position, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse");
    let wgsl = lower_to_wgsl(&tu, ShaderStage::Vertex)
        .unwrap_or_else(|e| panic!("lowering failed: {e}"));
    eprintln!("--- WGSL (realistic vertex pipeline) ---\n{wgsl}");
    assert!(wgsl.contains("mat4x4<f32>"));
    assert!(wgsl.contains("@vertex"));
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
