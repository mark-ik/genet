/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::fmt;

/// Canonical ESSL 1.00 vertex shader used by the W3 smoke. The
/// switch-over to `webgl-essl` lifted the per-character canonical
/// restriction the old narrow parser enforced, but the smoke
/// surface keeps these two shaders as the documented starting
/// point.
pub const CANONICAL_TRIANGLE_VERTEX_SHADER: &str = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;

/// Canonical ESSL 1.00 fragment shader used by the W3 smoke.
pub const CANONICAL_TRIANGLE_FRAGMENT_SHADER: &str = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;

#[derive(Clone)]
pub(crate) struct TranslatedProgram {
    pub(crate) vertex_wgsl: String,
    pub(crate) fragment_wgsl: String,
    pub(crate) reflection: ProgramReflection,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct ProgramCacheKey {
    vertex: String,
    fragment: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ProgramReflection {
    pub(crate) position_attribute: VertexAttributeReflection,
    pub(crate) color_attribute: Option<VertexAttributeReflection>,
    pub(crate) texcoord_attribute: Option<VertexAttributeReflection>,
    pub(crate) fragment_color_uniform: Option<UniformReflection>,
    pub(crate) fragment_texture_uniform: Option<UniformReflection>,
    pub(crate) fragment_float_precision: WebGlPrecision,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct VertexAttributeReflection {
    pub(crate) name: String,
    pub(crate) location: u32,
    pub(crate) kind: VertexAttributeKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum VertexAttributeKind {
    Float32x2,
    Float32x4,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct UniformReflection {
    pub(crate) name: String,
    pub(crate) binding: u32,
    pub(crate) kind: UniformKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum UniformKind {
    Float32x4,
    Sampler2D,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum ShaderTranslationError {
    UnsupportedCanonicalPair,
    Parse(String),
    Validate(String),
    Emit(String),
}

impl fmt::Display for ShaderTranslationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCanonicalPair => formatter.write_str("unsupported ESSL shader pair"),
            Self::Parse(message) => write!(formatter, "parse error: {message}"),
            Self::Validate(message) => write!(formatter, "validation failed: {message}"),
            Self::Emit(message) => write!(formatter, "WGSL emit failed: {message}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum WebGlPrecision {
    Low,
    Medium,
    High,
}

impl WebGlPrecision {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "lowp" => Some(Self::Low),
            "mediump" => Some(Self::Medium),
            "highp" => Some(Self::High),
            _ => None,
        }
    }
}

mod lowering;

#[cfg(test)]
mod tests;

pub(crate) use lowering::{
    canonical_essl_cache_key, translate_canonical_essl_pair, validate_canonical_fragment_source,
    validate_canonical_vertex_source,
};
