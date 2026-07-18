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
    /// Every vertex attribute the shader declared, in source
    /// order. `attributes[0]` is the position attribute under
    /// the WebGL convention; the narrow `position_attribute`
    /// view below points to it.
    pub(crate) attributes: Vec<VertexAttributeReflection>,
    /// Every uniform that lives inside the per-shader `Block`
    /// struct (vec_n / float / int / mat_n), in source order.
    /// Each entry carries the byte offset and size inside the
    /// Block, so a setter can mutate one member without
    /// disturbing the others.
    pub(crate) uniforms: Vec<UniformReflection>,
    /// Every sampler uniform (sampler2D / samplerCube), in
    /// source order. webgl-essl puts the image at
    /// `@binding(N)` and the sampler at `@binding(N+1)`.
    pub(crate) samplers: Vec<SamplerReflection>,
    /// Total byte size of the uniform `Block` buffer. `0` when
    /// the program has no non-sampler uniforms.
    pub(crate) uniform_block_size: u32,
    /// Narrow-shape convenience: the first vertex attribute.
    /// Always populated.
    pub(crate) position_attribute: VertexAttributeReflection,
    /// Narrow-shape convenience: `attributes[1]` if it's vec4.
    pub(crate) color_attribute: Option<VertexAttributeReflection>,
    /// Narrow-shape convenience: `attributes[1]` if it's vec2.
    pub(crate) texcoord_attribute: Option<VertexAttributeReflection>,
    /// Narrow-shape convenience: the first vec4 uniform.
    pub(crate) fragment_color_uniform: Option<UniformReflection>,
    /// Narrow-shape convenience: the first sampler2D.
    pub(crate) fragment_texture_uniform: Option<UniformReflection>,
    pub(crate) fragment_float_precision: WebGlPrecision,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct VertexAttributeReflection {
    pub(crate) name: String,
    pub(crate) location: u32,
    pub(crate) kind: VertexAttributeKind,
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) enum VertexAttributeKind {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct UniformReflection {
    pub(crate) name: String,
    /// `@binding(N)` on the descriptor: `0` for the uniform
    /// Block (every non-sampler uniform shares this binding),
    /// `image_binding` of the corresponding sampler otherwise.
    pub(crate) binding: u32,
    pub(crate) kind: UniformKind,
    /// Member index inside the Block (0 for the first non-
    /// sampler uniform, 1 for the second, ...). Samplers live
    /// outside the Block and carry index `0`.
    pub(crate) member_index: u32,
    /// Byte offset inside the Block buffer where this member
    /// starts (std140-style). Samplers carry `0`.
    pub(crate) block_offset: u32,
    /// Byte size of this member's contribution inside the
    /// Block (includes std140 padding for mat3 columns). `0`
    /// for samplers.
    pub(crate) block_size: u32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct SamplerReflection {
    pub(crate) name: String,
    pub(crate) image_binding: u32,
    pub(crate) sampler_binding: u32,
    pub(crate) kind: UniformKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum UniformKind {
    Float32,
    Float32x2,
    Float32x3,
    Float32x4,
    Matrix2,
    Matrix3,
    Matrix4,
    Int,
    Sampler2D,
    SamplerCube,
}

impl UniformKind {
    /// std140-style alignment (matches WGSL's uniform-storage
    /// rules for the kinds webgl-essl admits). Scalars: 4,
    /// vec2: 8, vec3 / vec4 / matrices: 16.
    pub(crate) fn block_alignment(self) -> u32 {
        match self {
            Self::Float32 | Self::Int => 4,
            Self::Float32x2 => 8,
            Self::Float32x3 | Self::Float32x4 | Self::Matrix2 | Self::Matrix3 | Self::Matrix4 => 16,
            Self::Sampler2D | Self::SamplerCube => 0,
        }
    }

    /// std140-style size: vec3 is 12 (the next member aligns
    /// up if its alignment exceeds 12). Matrices are stored as
    /// column-major arrays of vec4-padded columns.
    pub(crate) fn block_size(self) -> u32 {
        match self {
            Self::Float32 | Self::Int => 4,
            Self::Float32x2 => 8,
            Self::Float32x3 => 12,
            Self::Float32x4 => 16,
            Self::Matrix2 => 32,
            Self::Matrix3 => 48,
            Self::Matrix4 => 64,
            Self::Sampler2D | Self::SamplerCube => 0,
        }
    }
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
