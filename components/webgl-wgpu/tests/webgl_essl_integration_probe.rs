/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Integration probe: route the canonical shaders that
//! `webgl-wgpu`'s narrow parsers accept through the full
//! `webgl-essl` pipeline and confirm a clean WGSL round-trip.
//!
//! `webgl-wgpu` today validates and translates ESSL via a set
//! of `Canonical{Vertex,Fragment}Info` parsers that accept a
//! tightly-scoped subset of ESSL and re-emit it as
//! `#version 450` GLSL for naga's `glsl-in`. Replacing that
//! path with `webgl-essl` would broaden the accepted surface
//! to most of ESSL 1.00 / 3.00. This probe stays read-only:
//! it doesn't change `webgl-wgpu`'s production path, just
//! verifies that:
//!   1. `webgl-essl` accepts every canonical shape that
//!      `webgl-wgpu` accepts today, and
//!   2. its WGSL output exposes the same interface
//!      decorations production code relies on (`@location`,
//!      `@builtin(position)`, etc.).
//!
//! Receipts that fail here indicate work needed before a
//! production switch-over.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;
use webgl_wgpu::{CANONICAL_TRIANGLE_FRAGMENT_SHADER, CANONICAL_TRIANGLE_VERTEX_SHADER};

// =====================================================================
// the two canonical shaders webgl-wgpu's narrow parsers accept
// =====================================================================

#[test]
fn canonical_triangle_vertex_round_trips_through_webgl_essl() {
    let r = compile(CANONICAL_TRIANGLE_VERTEX_SHADER, ShaderStage::Vertex).expect("vertex compile");
    // The attribute lands at @location(0); the primary output
    // carries the BuiltIn::Position decoration.
    assert!(
        r.wgsl.contains("@location(0)"),
        "missing attribute location: {}",
        r.wgsl
    );
    assert!(
        r.wgsl.contains("@builtin(position)"),
        "missing BuiltIn::Position: {}",
        r.wgsl
    );
    assert!(
        r.info_log.is_empty(),
        "vertex info_log nonempty: {}",
        r.info_log
    );
}

#[test]
fn canonical_triangle_fragment_round_trips_through_webgl_essl() {
    let r = compile(CANONICAL_TRIANGLE_FRAGMENT_SHADER, ShaderStage::Fragment)
        .expect("fragment compile");
    // Naga emits the fragment output at @location(0).
    assert!(
        r.wgsl.contains("@location(0)"),
        "missing output location: {}",
        r.wgsl
    );
    assert!(
        r.info_log.is_empty(),
        "fragment info_log nonempty: {}",
        r.info_log
    );
}

// =====================================================================
// shapes the narrow parsers reject but production WebGL handles —
// these prove the integration would broaden the accepted surface
// =====================================================================

#[test]
fn webgl_essl_accepts_varying_vec3_color_passthrough_vertex_shader() {
    let src = r#"
attribute vec2 a_position;
attribute vec3 a_color;
varying vec3 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("@location(0)"));
    assert!(r.wgsl.contains("@location(1)"));
}

#[test]
fn webgl_essl_accepts_uniform_tinted_fragment_shader() {
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
varying vec3 v_color;
void main() {
    gl_FragColor = vec4(v_color, 1.0) * u_tint;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    // The uniform block lives at @group(0) @binding(0).
    assert!(
        r.wgsl.contains("@group(0)"),
        "expected group decoration: {}",
        r.wgsl
    );
    assert!(
        r.wgsl.contains("@binding(0)"),
        "expected binding decoration: {}",
        r.wgsl
    );
}

#[test]
fn webgl_essl_accepts_sampler2d_textured_fragment_shader() {
    let src = r#"
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    // Sampler descriptor goes to @binding(1) (the uniform Block
    // takes @binding(0) when present, kept at 0 even when
    // absent — see register_samplers).
    assert!(
        r.wgsl.contains("@binding(1)"),
        "expected sampler binding: {}",
        r.wgsl
    );
}

// =====================================================================
// info_log routing — production code calls getShaderInfoLog after a
// failed compile; verify webgl-essl produces the right shape
// =====================================================================

#[test]
fn webgl_essl_info_log_carries_line_number_for_failed_compile() {
    // `discard` in a vertex shader fails the validator at R2
    // — exactly the kind of error WebGL impls surface through
    // getShaderInfoLog.
    let src = "void main() {\n    discard;\n    gl_Position = vec4(0.0);\n}\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    use webgl_essl::CompileError;
    match err {
        CompileError::Validate(r) => {
            // The info_log line shape: `ERROR: 0:<line>: <message>`
            // — matches what ANGLE / Chrome emit for getShaderInfoLog.
            assert!(
                r.info_log.contains("ERROR: 0:2:"),
                "expected line-2 info_log, got: {}",
                r.info_log
            );
        },
        other => panic!("expected CompileError::Validate, got {other:?}"),
    }
}

// =====================================================================
// what the integration would look like — sketches the function shape
// =====================================================================

/// Sketch of the integrated path: a single call replaces
/// `validate_canonical_vertex_source` + `lower_canonical_pair_to_naga_glsl`'s
/// per-shader work.
///
/// Production would route through this (with an additional
/// `ProgramReflection` derivation from the WGSL or directly
/// from the SPIR-V module). For this probe it stays as a
/// reference implementation in the test crate.
fn integrated_compile(source: &str, stage: ShaderStage) -> Result<String, String> {
    compile(source, stage)
        .map(|r| r.wgsl)
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn integrated_compile_returns_wgsl_for_canonical_pair() {
    let v =
        integrated_compile(CANONICAL_TRIANGLE_VERTEX_SHADER, ShaderStage::Vertex).expect("vertex");
    let f = integrated_compile(CANONICAL_TRIANGLE_FRAGMENT_SHADER, ShaderStage::Fragment)
        .expect("fragment");
    assert!(v.contains("@vertex"));
    assert!(f.contains("@fragment"));
}

// =====================================================================
// pipeline-layout probe — `reflect()` carries enough info for a
// consumer to build wgpu BindGroupLayoutEntry rows directly. This
// exercises the alignment between webgl-essl's `@group(0)
// @binding(N)` emission and the wgpu layout types webgl-wgpu's
// pipeline builder already speaks.
// =====================================================================

use webgl_essl::ast::TypeKind;
use webgl_essl::parse_source;
use webgl_essl::reflect::reflect;

/// Translate webgl-essl's [`reflect::ProgramReflection`] into
/// the `wgpu::BindGroupLayoutEntry` rows a pipeline layout
/// builder would feed into `wgpu::Device::create_bind_group_layout`.
///
/// The mapping is deliberately small:
/// - Each non-sampler uniform contributes to the single
///   uniform `Block` struct at `@binding(0)` — represented as
///   a single `Buffer { Uniform }` entry (since wgpu doesn't
///   expose per-member binding info; the offset / size are
///   the consumer's concern).
/// - Each sampler contributes two entries: a `Texture` at the
///   image binding, and a `Sampler` at the next binding.
fn bind_group_layout_from_reflection(
    r: &webgl_essl::reflect::ProgramReflection,
    visibility: wgpu::ShaderStages,
) -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = Vec::new();
    if !r.uniforms.is_empty() {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
    }
    for s in &r.samplers {
        let view_dim = match s.kind {
            TypeKind::Sampler2D => wgpu::TextureViewDimension::D2,
            TypeKind::SamplerCube => wgpu::TextureViewDimension::Cube,
            other => panic!("unexpected sampler kind {other:?}"),
        };
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: s.image_binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: view_dim,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: s.sampler_binding,
            visibility,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    entries
}

#[test]
fn reflection_translates_to_uniform_block_bind_group_layout_entry() {
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
uniform float u_amount;
varying vec3 v_color;
void main() {
    gl_FragColor = vec4(v_color, 1.0) * u_tint * u_amount;
}
"#;
    let tu = parse_source(src).unwrap();
    let r = reflect(&tu, ShaderStage::Fragment);
    let entries = bind_group_layout_from_reflection(&r, wgpu::ShaderStages::FRAGMENT);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].binding, 0);
    assert!(matches!(
        entries[0].ty,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            ..
        }
    ));
}

#[test]
fn reflection_translates_to_texture_and_sampler_bind_group_layout_entries() {
    let src = r#"
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
"#;
    let tu = parse_source(src).unwrap();
    let r = reflect(&tu, ShaderStage::Fragment);
    let entries = bind_group_layout_from_reflection(&r, wgpu::ShaderStages::FRAGMENT);
    // No regular uniforms → no Block entry. One sampler → two
    // entries: Texture(1) + Sampler(2).
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].binding, 1);
    assert!(matches!(
        entries[0].ty,
        wgpu::BindingType::Texture {
            view_dimension: wgpu::TextureViewDimension::D2,
            ..
        }
    ));
    assert_eq!(entries[1].binding, 2);
    assert!(matches!(entries[1].ty, wgpu::BindingType::Sampler(_)));
}

#[test]
fn reflection_full_uniform_plus_two_samplers_pipeline_layout() {
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
uniform sampler2D u_diffuse;
uniform sampler2D u_normal;
varying vec2 v_uv;
void main() {
    gl_FragColor = (texture2D(u_diffuse, v_uv) + texture2D(u_normal, v_uv)) * u_tint;
}
"#;
    let tu = parse_source(src).unwrap();
    let r = reflect(&tu, ShaderStage::Fragment);
    let entries = bind_group_layout_from_reflection(&r, wgpu::ShaderStages::FRAGMENT);
    // 1 uniform block + 2 samplers × 2 entries each = 5
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0].binding, 0); // Block
    assert_eq!(entries[1].binding, 1); // diffuse texture
    assert_eq!(entries[2].binding, 2); // diffuse sampler
    assert_eq!(entries[3].binding, 3); // normal texture
    assert_eq!(entries[4].binding, 4); // normal sampler
}

#[test]
fn reflection_samplerCube_maps_to_cube_texture_view_dimension() {
    let src = r#"
precision mediump float;
uniform samplerCube u_env;
varying vec3 v_dir;
void main() {
    gl_FragColor = textureCube(u_env, v_dir);
}
"#;
    let tu = parse_source(src).unwrap();
    let r = reflect(&tu, ShaderStage::Fragment);
    let entries = bind_group_layout_from_reflection(&r, wgpu::ShaderStages::FRAGMENT);
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        entries[0].ty,
        wgpu::BindingType::Texture {
            view_dimension: wgpu::TextureViewDimension::Cube,
            ..
        }
    ));
}

#[test]
fn reflection_and_wgsl_agree_on_binding_numbers() {
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv) * u_tint;
}
"#;
    let tu = parse_source(src).unwrap();
    let r = reflect(&tu, ShaderStage::Fragment);
    let wgsl = webgl_essl::compile(src, ShaderStage::Fragment)
        .expect("compile")
        .wgsl;
    // Reflection says sampler image at binding(1).
    assert_eq!(r.samplers[0].image_binding, 1);
    // WGSL must agree.
    assert!(
        wgsl.contains("@binding(1)"),
        "reflection / wgsl disagree on sampler image binding: {wgsl}"
    );
}
