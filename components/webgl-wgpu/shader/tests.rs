/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::lowering::lower_canonical_pair_to_naga_glsl;
use super::*;

#[test]
fn canonical_essl_pair_translates_through_naga() {
    let translated = translate_canonical_essl_pair(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    )
    .expect("canonical pair translates");

    assert!(translated.vertex_wgsl.contains("fn main"));
    assert!(translated.vertex_wgsl.contains("@location(0)"));
    assert!(translated.fragment_wgsl.contains("fn main"));
    assert!(translated.fragment_wgsl.contains("webgl_FragColor"));
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
fn noncanonical_essl_pair_is_not_translated() {
    let result = translate_canonical_essl_pair(
        "attribute vec2 a_position; void main() { gl_Position = vec4(0.0); }",
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    );

    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}

#[test]
fn canonical_lowering_targets_naga_glsl_boundaries() {
    let lowered = lower_canonical_pair_to_naga_glsl(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    )
    .expect("canonical pair lowers");

    assert_eq!(lowered.vertex.stage, WebGlShaderStage::Vertex);
    assert!(lowered.vertex.source.contains("#version 450"));
    assert!(
        lowered
            .vertex
            .source
            .contains("layout(location = 0) in vec2 a_position")
    );
    assert_eq!(lowered.fragment.stage, WebGlShaderStage::Fragment);
    assert_eq!(
        lowered.fragment.float_precision,
        Some(WebGlPrecision::Medium)
    );
    assert_eq!(
        lowered.reflection.position_attribute.kind,
        VertexAttributeKind::Float32x2
    );
    assert_eq!(lowered.reflection.position_attribute.location, 0);
    assert_eq!(lowered.reflection.position_attribute.name, "a_position");
    assert_eq!(lowered.reflection.color_attribute, None);
    assert_eq!(lowered.reflection.texcoord_attribute, None);
    assert_eq!(lowered.reflection.fragment_color_uniform, None);
    assert_eq!(lowered.reflection.fragment_texture_uniform, None);
    assert_eq!(
        lowered.reflection.fragment_float_precision,
        WebGlPrecision::Medium
    );
    assert!(
        lowered
            .fragment
            .source
            .contains("layout(location = 0) out vec4 webgl_FragColor")
    );
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
        let lowered =
            lower_canonical_pair_to_naga_glsl(CANONICAL_TRIANGLE_VERTEX_SHADER, &fragment)
                .expect("precision variant lowers");

        assert_eq!(lowered.fragment.float_precision, Some(expected));
    }
}

#[test]
fn canonical_fragment_accepts_additional_int_precision_declaration() {
    let fragment = r#"
            precision mediump float;
            precision highp int;
            void main() {
                gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
            }
        "#;

    let lowered = lower_canonical_pair_to_naga_glsl(CANONICAL_TRIANGLE_VERTEX_SHADER, fragment)
        .expect("precision stack lowers");
    assert_eq!(
        lowered.fragment.float_precision,
        Some(WebGlPrecision::Medium)
    );
}

#[test]
fn canonical_fragment_rejects_duplicate_float_precision_declarations() {
    let result = lower_canonical_pair_to_naga_glsl(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        "precision mediump float; precision highp float; void main() { gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0); }",
    );
    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}

#[test]
fn canonical_fragment_lowers_literal_color() {
    let fragment = r#"
            precision mediump float;
            void main() {
                gl_FragColor = vec4(1.0, 0.0, 0.5, 1.0);
            }
        "#;

    let lowered = lower_canonical_pair_to_naga_glsl(CANONICAL_TRIANGLE_VERTEX_SHADER, fragment)
        .expect("literal color lowers");

    assert!(
        lowered
            .fragment
            .source
            .contains("webgl_FragColor = vec4(1.0, 0.0, 0.5, 1.0)")
    );
}

#[test]
fn canonical_fragment_lowers_uniform_color() {
    let fragment = r#"
            precision mediump float;
            uniform vec4 u_color;
            void main() {
                gl_FragColor = u_color;
            }
        "#;

    let lowered = lower_canonical_pair_to_naga_glsl(CANONICAL_TRIANGLE_VERTEX_SHADER, fragment)
        .expect("uniform color lowers");

    assert!(
        lowered
            .fragment
            .source
            .contains("layout(set = 0, binding = 0) uniform WebGlUniforms")
    );
    assert!(lowered.fragment.source.contains("vec4 u_color"));
    assert_eq!(
        lowered.reflection.fragment_color_uniform,
        Some(UniformReflection {
            name: "u_color".to_string(),
            binding: 0,
            kind: UniformKind::Float32x4,
        })
    );
}

#[test]
fn canonical_fragment_rejects_uniform_name_mismatch() {
    let result = lower_canonical_pair_to_naga_glsl(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        "precision mediump float; uniform vec4 u_color; void main() { gl_FragColor = color; }",
    );

    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}

#[test]
fn canonical_pair_lowers_varying_color() {
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

    let lowered =
        lower_canonical_pair_to_naga_glsl(vertex, fragment).expect("varying color pair lowers");

    assert!(
        lowered
            .vertex
            .source
            .contains("layout(location = 1) in vec4 a_color")
    );
    assert!(
        lowered
            .vertex
            .source
            .contains("layout(location = 0) out vec4 v_color")
    );
    assert!(
        lowered
            .fragment
            .source
            .contains("layout(location = 0) in vec4 v_color")
    );
    assert_eq!(
        lowered.reflection.color_attribute,
        Some(VertexAttributeReflection {
            name: "a_color".to_string(),
            location: 1,
            kind: VertexAttributeKind::Float32x4,
        })
    );
}

#[test]
fn canonical_pair_rejects_varying_link_mismatch() {
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
            varying vec4 other_color;
            void main() {
                gl_FragColor = other_color;
            }
        "#;

    let result = lower_canonical_pair_to_naga_glsl(vertex, fragment);

    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}

#[test]
fn canonical_pair_lowers_texture_sampler_path() {
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
            binding: 0,
            kind: UniformKind::Sampler2D,
        })
    );
}

#[test]
fn canonical_vertex_reflects_attribute_name() {
    let vertex = r#"
            attribute vec2 position;
            void main() {
                gl_Position = vec4(position, 0.0, 1.0);
            }
        "#;

    let lowered = lower_canonical_pair_to_naga_glsl(vertex, CANONICAL_TRIANGLE_FRAGMENT_SHADER)
        .expect("renamed canonical vertex lowers");

    assert_eq!(lowered.reflection.position_attribute.name, "position");
    assert!(
        lowered
            .vertex
            .source
            .contains("layout(location = 0) in vec2 position")
    );
    assert!(
        lowered
            .vertex
            .source
            .contains("gl_Position = vec4(position, 0.0, 1.0)")
    );
}

#[test]
fn canonical_vertex_rejects_attribute_name_mismatch() {
    let result = lower_canonical_pair_to_naga_glsl(
        "attribute vec2 position; void main() { gl_Position = vec4(a_position, 0.0, 1.0); }",
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    );

    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}

#[test]
fn canonical_fragment_rejects_nonliteral_color() {
    let result = lower_canonical_pair_to_naga_glsl(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        "precision mediump float; void main() { gl_FragColor = vec4(1.0, 0.0, 2.0, 1.0); }",
    );

    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}

#[test]
fn canonical_pair_accepts_comments_and_whitespace() {
    let vertex = r#"
            // WebGL-facing ESSL stays the input contract.
            attribute   vec2   a_position;
            void main() {
                gl_Position = vec4(
                    a_position, /* z */ 0.0,
                    1.0
                );
            }
        "#;
    let fragment = r#"
            precision highp float;
            void main() {
                // canonical smoke color
                gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
            }
        "#;

    let lowered = lower_canonical_pair_to_naga_glsl(vertex, fragment)
        .expect("commented canonical shaders lower");

    assert_eq!(lowered.fragment.float_precision, Some(WebGlPrecision::High));
}

#[test]
fn canonical_cache_key_uses_validated_shape() {
    let formatted = r#"
            precision mediump float;
            void main() {
                gl_FragColor = vec4(
                    0.0, 1.0,
                    0.0, 1.0
                );
            }
        "#;

    let canonical = canonical_essl_cache_key(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    )
    .expect("canonical cache key");
    let reformatted = canonical_essl_cache_key(CANONICAL_TRIANGLE_VERTEX_SHADER, formatted)
        .expect("formatted cache key");

    assert_eq!(canonical, reformatted);
}

#[test]
fn canonical_fragment_requires_float_precision() {
    let result = lower_canonical_pair_to_naga_glsl(
        CANONICAL_TRIANGLE_VERTEX_SHADER,
        "void main() { gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0); }",
    );

    assert!(matches!(
        result,
        Err(ShaderTranslationError::UnsupportedCanonicalPair)
    ));
}
