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
fn binary_op_inside_vec4_constructor_still_unsupported() {
    // Binary ops are queued for a follow-up; today the lowering only
    // handles ident loads and nested constructors.
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
