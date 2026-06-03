/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cross-check receipts: [`reflect::reflect`]'s layout
//! decisions must match the `@location(N)` / `@binding(N)` /
//! `@group(N)` decorations the lowering's WGSL output carries.
//! A future refactor that drifts the two will fail this file.

use webgl_essl::ast::TypeKind;
use webgl_essl::reflect::{
    InputBinding, OutputBinding, ProgramReflection, SamplerBinding, UniformBinding, reflect,
};
use webgl_essl::validate::ShaderStage;
use webgl_essl::{compile, parse_source};

fn reflect_from(src: &str, stage: ShaderStage) -> ProgramReflection {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    reflect(&tu, stage)
}

// =====================================================================
// vertex: inputs / outputs / gl_Position
// =====================================================================

#[test]
fn reflect_vertex_canonical_triangle() {
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let r = reflect_from(src, ShaderStage::Vertex);
    assert_eq!(
        r.inputs,
        vec![InputBinding { name: "a_position".into(), kind: TypeKind::Vec2, location: 0 }]
    );
    assert!(r.outputs.is_empty());
    assert!(r.uniforms.is_empty());
    assert!(r.samplers.is_empty());
    // Cross-check with lowering: @location(0) appears for the
    // attribute, @builtin(position) for gl_Position.
    let wgsl = compile(src, ShaderStage::Vertex).expect("compile").wgsl;
    assert!(wgsl.contains("@location(0)"));
    assert!(wgsl.contains("@builtin(position)"));
}

#[test]
fn reflect_vertex_with_varying_passthrough() {
    let src = r#"
attribute vec2 a_position;
attribute vec3 a_color;
varying vec3 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let r = reflect_from(src, ShaderStage::Vertex);
    assert_eq!(r.inputs.len(), 2);
    assert_eq!(r.inputs[0].location, 0);
    assert_eq!(r.inputs[1].location, 1);
    assert_eq!(r.outputs.len(), 1);
    assert_eq!(r.outputs[0].name, "v_color");
    assert_eq!(r.outputs[0].location, 0);
    let wgsl = compile(src, ShaderStage::Vertex).expect("compile").wgsl;
    assert!(wgsl.contains("@location(0)"));
    assert!(wgsl.contains("@location(1)"));
}

#[test]
fn reflect_vertex_with_mat4_varying_column_splits_locations() {
    let src = r#"
attribute vec3 a_position;
varying mat4 v_xform;
uniform mat4 u_base;
void main() {
    v_xform = u_base;
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let r = reflect_from(src, ShaderStage::Vertex);
    assert_eq!(r.outputs.len(), 1);
    assert_eq!(r.outputs[0].kind, TypeKind::Mat4);
    assert_eq!(r.outputs[0].location, 0);
    // The next attribute (had there been one) would land at
    // @location(4) since mat4 consumes 4 slots — confirm via
    // the lowering by checking @location(3) is present.
    let wgsl = compile(src, ShaderStage::Vertex).expect("compile").wgsl;
    assert!(wgsl.contains("location(3)"), "wgsl: {wgsl}");
}

// =====================================================================
// fragment outputs: gl_FragColor vs user `out`
// =====================================================================

#[test]
fn reflect_essl100_fragment_implicit_gl_fragcolor() {
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.5);
}
"#;
    let r = reflect_from(src, ShaderStage::Fragment);
    assert_eq!(
        r.outputs,
        vec![OutputBinding {
            name: "gl_FragColor".into(),
            kind: TypeKind::Vec4,
            location: 0
        }]
    );
}

#[test]
fn reflect_essl300_fragment_user_outs_get_sequential_locations() {
    let src = r#"#version 300 es
precision mediump float;
out vec4 color0;
out vec4 color1;
void main() {
    color0 = vec4(1.0);
    color1 = vec4(0.0);
}
"#;
    let r = reflect_from(src, ShaderStage::Fragment);
    assert_eq!(r.outputs.len(), 2);
    assert_eq!(r.outputs[0].name, "color0");
    assert_eq!(r.outputs[0].location, 0);
    assert_eq!(r.outputs[1].name, "color1");
    assert_eq!(r.outputs[1].location, 1);
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("@location(0)"));
    assert!(wgsl.contains("@location(1)"));
}

// =====================================================================
// uniforms + samplers
// =====================================================================

#[test]
fn reflect_uniforms_then_sampler_match_lowering_bindings() {
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
uniform float u_amount;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv) * u_tint * u_amount;
}
"#;
    let r = reflect_from(src, ShaderStage::Fragment);
    assert_eq!(
        r.uniforms,
        vec![
            UniformBinding {
                name: "u_tint".into(),
                kind: TypeKind::Vec4,
                member_index: 0
            },
            UniformBinding {
                name: "u_amount".into(),
                kind: TypeKind::Float,
                member_index: 1
            },
        ]
    );
    assert_eq!(
        r.samplers,
        vec![SamplerBinding {
            name: "u_tex".into(),
            kind: TypeKind::Sampler2D,
            image_binding: 1,
            sampler_binding: 2,
            descriptor_set: 0,
        }]
    );
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    // Uniform block at @group(0) @binding(0); samplers start
    // at @binding(1).
    assert!(wgsl.contains("@binding(0)"));
    assert!(wgsl.contains("@binding(1)"));
    assert!(wgsl.contains("@binding(2)"));
}

#[test]
fn reflect_two_samplers_get_consecutive_pair_bindings() {
    let src = r#"
precision mediump float;
uniform sampler2D u_diffuse;
uniform sampler2D u_normal;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_diffuse, v_uv) + texture2D(u_normal, v_uv);
}
"#;
    let r = reflect_from(src, ShaderStage::Fragment);
    assert_eq!(r.samplers.len(), 2);
    assert_eq!(r.samplers[0].image_binding, 1);
    assert_eq!(r.samplers[0].sampler_binding, 2);
    assert_eq!(r.samplers[1].image_binding, 3);
    assert_eq!(r.samplers[1].sampler_binding, 4);
}

// =====================================================================
// ESSL 3.00 fragment + uniforms + sampler — production-shape pin
// =====================================================================

#[test]
fn reflect_essl300_fragment_full_shape() {
    let src = r#"#version 300 es
precision mediump float;
uniform vec4 u_tint;
uniform sampler2D u_tex;
in vec3 v_color;
in vec2 v_uv;
out vec4 out_color;
void main() {
    out_color = texture(u_tex, v_uv) * vec4(v_color, 1.0) * u_tint;
}
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let r = reflect(&tu, ShaderStage::Fragment);
    assert_eq!(r.inputs.len(), 2);
    assert_eq!(r.inputs[0].name, "v_color");
    assert_eq!(r.inputs[1].name, "v_uv");
    assert_eq!(r.outputs.len(), 1);
    assert_eq!(r.outputs[0].name, "out_color");
    assert_eq!(r.uniforms.len(), 1);
    assert_eq!(r.uniforms[0].name, "u_tint");
    assert_eq!(r.samplers.len(), 1);
    assert_eq!(r.samplers[0].name, "u_tex");
    assert_eq!(r.samplers[0].image_binding, 1);
}
