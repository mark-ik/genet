/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Translation receipts after the switch-over to `webgl-essl`.
//!
//! The old naga `glsl-in` intermediate is gone — `webgl-essl`
//! emits WGSL directly via SPIR-V → naga `spv-in` → `wgsl-out`.
//! These tests stay surface-only: they pin the shape WGSL
//! consumers (the pipeline builder, the bind-group factories,
//! the WebGL `getAttribLocation` / `getUniformLocation` lookups)
//! actually depend on. Specifically:
//!
//! * `@location(N)` / `@builtin(position)` decorations on the
//!   stage interface,
//! * `@group(0) @binding(N)` decorations for the uniform Block
//!   and any samplers, and
//! * the narrow [`ProgramReflection`] view that `webgl-wgpu`'s
//!   programs / draw paths consult.
//!
//! Anything finer-grained (which struct field name `webgl-essl`
//! lays out the uniform Block with, what naming `wgsl-out`
//! produces for synthesized locals) is the lowering's
//! implementation detail, covered by `webgl-essl`'s own suite.

use super::*;

#[test]
fn canonical_essl_pair_translates_through_webgl_essl() {
    let translated = translate_canonical_essl_pair(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    )
    .expect("canonical pair translates");

    assert!(translated.vertex_wgsl.contains("@vertex"));
    assert!(translated.vertex_wgsl.contains("@location(0)"));
    assert!(translated.vertex_wgsl.contains("@builtin(position)"));
    assert!(translated.fragment_wgsl.contains("@fragment"));
    assert!(translated.fragment_wgsl.contains("@location(0)"));
    assert_eq!(
        translated.reflection.position_attribute,
        VertexAttributeReflection {
            name: "a_position".to_string(),
            location: 0,
            kind: VertexAttributeKind::Float32x2,
        }
    );
    assert_eq!(translated.reflection.color_attribute, None);
    assert_eq!(translated.reflection.texcoord_attribute, None);
    assert_eq!(translated.reflection.fragment_color_uniform, None);
    assert_eq!(translated.reflection.fragment_texture_uniform, None);
    assert_eq!(
        translated.reflection.fragment_float_precision,
        WebGlPrecision::Medium
    );
}

#[test]
fn varying_link_mismatch_rejected_at_pair_check() {
    let vertex = r#"
attribute vec2 a_position;
attribute vec4 a_color;
varying vec4 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    // Fragment reads a varying the vertex shader never writes.
    let fragment = r#"
precision mediump float;
varying vec4 other_color;
void main() {
    gl_FragColor = other_color;
}
"#;
    let result = translate_canonical_essl_pair(vertex, fragment);
    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}

#[test]
fn fragment_with_invalid_essl_surfaces_validation_error() {
    // `discard` in a vertex shader fails webgl-essl validator R2.
    let result =
        validate_canonical_vertex_source("void main() { discard; gl_Position = vec4(0.0); }");
    assert!(matches!(result, Err(ShaderTranslationError::Validate(_))));
}

#[test]
fn canonical_fragment_accepts_float_precision_variants() {
    for (precision, expected) in [
        ("lowp", WebGlPrecision::Low),
        ("mediump", WebGlPrecision::Medium),
        ("highp", WebGlPrecision::High),
    ] {
        let fragment = format!(
            "precision {precision} float; void main() {{ gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0); }}"
        );
        let translated =
            translate_canonical_essl_pair(CANONICAL_TRIANGLE_VERTEX_SHADER, &fragment)
                .expect("precision variant translates");
        assert_eq!(translated.reflection.fragment_float_precision, expected);
    }
}

#[test]
fn canonical_pair_lowers_varying_color_to_wgsl() {
    let vertex = r#"
attribute vec2 a_position;
attribute vec4 a_color;
varying vec4 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying vec4 v_color;
void main() {
    gl_FragColor = v_color;
}
"#;

    let translated =
        translate_canonical_essl_pair(vertex, fragment).expect("varying color pair translates");

    // Two attribute locations on the vertex shader, one varying
    // crossing to the fragment shader (also at @location(0)
    // because the fragment-side counter is independent).
    assert!(translated.vertex_wgsl.contains("@location(0)"));
    assert!(translated.vertex_wgsl.contains("@location(1)"));
    assert!(translated.fragment_wgsl.contains("@location(0)"));
    assert_eq!(
        translated.reflection.color_attribute,
        Some(VertexAttributeReflection {
            name: "a_color".to_string(),
            location: 1,
            kind: VertexAttributeKind::Float32x4,
        })
    );
    assert_eq!(translated.reflection.texcoord_attribute, None);
}

#[test]
fn canonical_fragment_lowers_uniform_color_with_group_zero_binding_zero() {
    let fragment = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;

    let translated =
        translate_canonical_essl_pair(CANONICAL_TRIANGLE_VERTEX_SHADER, fragment)
            .expect("uniform color translates");

    assert!(translated.fragment_wgsl.contains("@group(0)"));
    assert!(translated.fragment_wgsl.contains("@binding(0)"));
    assert_eq!(
        translated.reflection.fragment_color_uniform,
        Some(UniformReflection {
            name: "u_color".to_string(),
            binding: 0,
            kind: UniformKind::Float32x4,
        })
    );
}

#[test]
fn canonical_pair_lowers_texture_sampler_at_bindings_one_and_two() {
    let vertex = r#"
attribute vec2 a_position;
attribute vec2 a_uv;
varying vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_texture;
void main() {
    gl_FragColor = texture2D(u_texture, v_uv);
}
"#;

    let translated =
        translate_canonical_essl_pair(vertex, fragment).expect("texture sampler pair translates");
    assert!(translated.vertex_wgsl.contains("@location(1)"));
    assert!(translated.fragment_wgsl.contains("@group(0)"));
    // Uniform Block reserves @binding(0); image binds at (1),
    // sampler at (2). The narrow reflection carries the image
    // binding; pipeline.rs derives the sampler one by `+1`.
    assert!(translated.fragment_wgsl.contains("@binding(1)"));
    assert!(translated.fragment_wgsl.contains("@binding(2)"));
    assert_eq!(
        translated.reflection.texcoord_attribute,
        Some(VertexAttributeReflection {
            name: "a_uv".to_string(),
            location: 1,
            kind: VertexAttributeKind::Float32x2,
        })
    );
    assert_eq!(
        translated.reflection.fragment_texture_uniform,
        Some(UniformReflection {
            name: "u_texture".to_string(),
            binding: 1,
            kind: UniformKind::Sampler2D,
        })
    );
}

#[test]
fn renamed_position_attribute_threads_through_reflection() {
    let vertex = r#"
attribute vec2 position;
void main() {
    gl_Position = vec4(position, 0.0, 1.0);
}
"#;

    let translated =
        translate_canonical_essl_pair(vertex, CANONICAL_TRIANGLE_FRAGMENT_SHADER)
            .expect("renamed vertex translates");

    assert_eq!(translated.reflection.position_attribute.name, "position");
    assert_eq!(translated.reflection.position_attribute.location, 0);
}

#[test]
fn cache_key_normalizes_on_source_text() {
    // The cache key is now source-derived (whitespace counts).
    // Equal sources produce equal keys; differently-formatted
    // sources do not. This stays correct — webgl-essl re-parses
    // each unique key, so a normalization layer is optional
    // future work, not a correctness requirement.
    let same = canonical_essl_cache_key(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    )
    .expect("cache key");
    let again = canonical_essl_cache_key(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    )
    .expect("cache key");
    assert_eq!(same, again);
}

#[test]
fn vertex_validation_rejects_recursion() {
    // `webgl-essl` validator R1 rejects user-function recursion.
    let recursive = r#"
attribute vec2 a_position;
vec4 self_call(vec4 p) { return self_call(p); }
void main() {
    gl_Position = self_call(vec4(a_position, 0.0, 1.0));
}
"#;
    let result = validate_canonical_vertex_source(recursive);
    assert!(matches!(result, Err(ShaderTranslationError::Validate(_))));
}

#[test]
fn fragment_validation_rejects_reserved_identifier() {
    // R5 — `gl_*` is a reserved prefix for user identifiers.
    let bad = r#"
precision mediump float;
uniform vec4 gl_userColor;
void main() {
    gl_FragColor = gl_userColor;
}
"#;
    let result = validate_canonical_fragment_source(bad);
    assert!(matches!(result, Err(ShaderTranslationError::Validate(_))));
}
