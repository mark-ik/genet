/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::fmt;

/// Canonical ESSL 1.00 vertex shader accepted by the W3 smoke.
pub const CANONICAL_TRIANGLE_VERTEX_SHADER: &str = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;

/// Canonical ESSL 1.00 fragment shader accepted by the W3 smoke.
pub const CANONICAL_TRIANGLE_FRAGMENT_SHADER: &str = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;

const CANONICAL_VERTEX_PREFIX: &str = "attribute vec2 ";
const CANONICAL_VERTEX_MIDDLE: &str = ";void main(){gl_Position=vec4(";
const CANONICAL_VERTEX_SUFFIX: &str = ",0.0,1.0);}";
const CANONICAL_VARYING_VERTEX_PREFIX: &str = "attribute vec2 ";
const CANONICAL_VARYING_VERTEX_COLOR_DECL: &str = ";attribute vec4 ";
const CANONICAL_VARYING_VERTEX_VARYING_DECL: &str = ";varying vec4 ";
const CANONICAL_VARYING_VERTEX_MAIN_PREFIX: &str = ";void main(){";
const CANONICAL_VARYING_VERTEX_ASSIGN_MIDDLE: &str = "=";
const CANONICAL_VARYING_VERTEX_POSITION_PREFIX: &str = ";gl_Position=vec4(";
const CANONICAL_VARYING_VERTEX_SUFFIX: &str = ",0.0,1.0);}";
const CANONICAL_FRAGMENT_COLOR_PREFIX: &str = "void main(){gl_FragColor=vec4(";
const CANONICAL_FRAGMENT_COLOR_SUFFIX: &str = ");}";
const CANONICAL_FRAGMENT_UNIFORM_PREFIX: &str = "uniform vec4 ";
const CANONICAL_FRAGMENT_UNIFORM_MIDDLE: &str = ";void main(){gl_FragColor=";
const CANONICAL_FRAGMENT_UNIFORM_SUFFIX: &str = ";}";
const CANONICAL_FRAGMENT_VARYING_PREFIX: &str = "varying vec4 ";
const CANONICAL_FRAGMENT_VARYING_MIDDLE: &str = ";void main(){gl_FragColor=";
const CANONICAL_FRAGMENT_VARYING_SUFFIX: &str = ";}";
const CANONICAL_TEXTURE_VERTEX_PREFIX: &str = "attribute vec2 ";
const CANONICAL_TEXTURE_VERTEX_UV_DECL: &str = ";attribute vec2 ";
const CANONICAL_TEXTURE_VERTEX_VARYING_DECL: &str = ";varying vec2 ";
const CANONICAL_TEXTURE_VERTEX_MAIN_PREFIX: &str = ";void main(){";
const CANONICAL_TEXTURE_VERTEX_ASSIGN_MIDDLE: &str = "=";
const CANONICAL_TEXTURE_VERTEX_POSITION_PREFIX: &str = ";gl_Position=vec4(";
const CANONICAL_TEXTURE_VERTEX_SUFFIX: &str = ",0.0,1.0);}";
const CANONICAL_FRAGMENT_TEXTURE_PREFIX: &str = "varying vec2 ";
const CANONICAL_FRAGMENT_TEXTURE_SAMPLER_DECL: &str = ";uniform sampler2D ";
const CANONICAL_FRAGMENT_TEXTURE_MAIN_PREFIX: &str = ";void main(){gl_FragColor=texture2D(";
const CANONICAL_FRAGMENT_TEXTURE_COORD_SEPARATOR: &str = ",";
const CANONICAL_FRAGMENT_TEXTURE_SUFFIX: &str = ");}";

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
    ThreadSpawn(String),
    ThreadJoin(String),
    NagaPanic(String),
    Parse(String),
    Validate(String),
    Emit(String),
}

impl fmt::Display for ShaderTranslationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCanonicalPair => formatter.write_str("unsupported ESSL shader pair"),
            Self::ThreadSpawn(message) => {
                write!(formatter, "failed to spawn naga thread: {message}")
            },
            Self::ThreadJoin(message) => {
                write!(formatter, "naga thread panicked at join: {message}")
            },
            Self::NagaPanic(message) => write!(formatter, "naga panicked: {message}"),
            Self::Parse(message) => write!(formatter, "GLSL->naga parse failed: {message}"),
            Self::Validate(message) => write!(formatter, "naga validation failed: {message}"),
            Self::Emit(message) => write!(formatter, "WGSL emit failed: {message}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum WebGlShaderStage {
    Vertex,
    Fragment,
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

    fn essl_token(self) -> &'static str {
        match self {
            Self::Low => "lowp",
            Self::Medium => "mediump",
            Self::High => "highp",
        }
    }
}

mod fragment;
mod lowering;
mod normalize;
mod vertex;

#[cfg(test)]
mod tests;

pub(crate) use lowering::{
    canonical_essl_cache_key, translate_canonical_essl_pair, validate_canonical_fragment_source,
    validate_canonical_vertex_source,
};
