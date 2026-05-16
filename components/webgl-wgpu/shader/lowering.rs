/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::fragment::{CanonicalFragmentInfo, parse_canonical_fragment};
use super::vertex::CanonicalVertexInfo;
use super::*;

struct CanonicalProgramInfo {
    pub(super) vertex: CanonicalVertexInfo,
    pub(super) fragment: CanonicalFragmentInfo,
}

pub(super) struct NagaGlslShader {
    pub(super) stage: WebGlShaderStage,
    pub(super) name: &'static str,
    pub(super) source: String,
    pub(super) float_precision: Option<WebGlPrecision>,
}

pub(super) struct NagaGlslProgram {
    pub(super) vertex: NagaGlslShader,
    pub(super) fragment: NagaGlslShader,
    pub(super) reflection: ProgramReflection,
}

pub(crate) fn translate_canonical_essl_pair(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<TranslatedProgram, ShaderTranslationError> {
    let lowered = lower_canonical_pair_to_naga_glsl(vertex_source, fragment_source)?;

    Ok(TranslatedProgram {
        vertex_wgsl: translate_to_wgsl(lowered.vertex)?,
        fragment_wgsl: translate_to_wgsl(lowered.fragment)?,
        reflection: lowered.reflection,
    })
}

pub(crate) fn validate_canonical_vertex_source(
    vertex_source: &str,
) -> Result<(), ShaderTranslationError> {
    CanonicalVertexInfo::parse(vertex_source).map(|_| ())
}

pub(crate) fn validate_canonical_fragment_source(
    fragment_source: &str,
) -> Result<(), ShaderTranslationError> {
    parse_canonical_fragment(fragment_source).map(|_| ())
}

pub(crate) fn canonical_essl_cache_key(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<ProgramCacheKey, ShaderTranslationError> {
    let info = validate_canonical_essl_pair(vertex_source, fragment_source)?;
    Ok(ProgramCacheKey {
        vertex: info.vertex.normalized_source(),
        fragment: format!(
            "precision {} float;{}",
            info.fragment.float_precision.essl_token(),
            info.fragment.color.normalized_body()
        ),
    })
}

pub(super) fn lower_canonical_pair_to_naga_glsl(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<NagaGlslProgram, ShaderTranslationError> {
    let info = validate_canonical_essl_pair(vertex_source, fragment_source)?;

    Ok(NagaGlslProgram {
        vertex: NagaGlslShader {
            stage: WebGlShaderStage::Vertex,
            name: "canonical_triangle_vertex",
            source: info.vertex.naga_glsl(),
            float_precision: None,
        },
        fragment: NagaGlslShader {
            stage: WebGlShaderStage::Fragment,
            name: "canonical_triangle_fragment",
            source: info.fragment.color.naga_glsl(),
            float_precision: Some(info.fragment.float_precision),
        },
        reflection: ProgramReflection {
            position_attribute: VertexAttributeReflection {
                name: info.vertex.position_attribute_name,
                location: 0,
                kind: VertexAttributeKind::Float32x2,
            },
            color_attribute: info.vertex.color_attribute_name.map(|name| {
                VertexAttributeReflection {
                    name,
                    location: 1,
                    kind: VertexAttributeKind::Float32x4,
                }
            }),
            texcoord_attribute: info.vertex.texcoord_attribute_name.map(|name| {
                VertexAttributeReflection {
                    name,
                    location: 1,
                    kind: VertexAttributeKind::Float32x2,
                }
            }),
            fragment_color_uniform: info.fragment.color.uniform_reflection(),
            fragment_texture_uniform: info.fragment.color.texture_uniform_reflection(),
            fragment_float_precision: info.fragment.float_precision,
        },
    })
}

fn validate_canonical_essl_pair(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<CanonicalProgramInfo, ShaderTranslationError> {
    let vertex = CanonicalVertexInfo::parse(vertex_source)?;
    let fragment = parse_canonical_fragment(fragment_source)?;
    match (
        vertex.varying_color_name.as_deref(),
        fragment.color.varying_name(),
    ) {
        (Some(vertex_varying), Some(fragment_varying)) if vertex_varying == fragment_varying => {},
        (None, None) => {},
        _ => return Err(ShaderTranslationError::UnsupportedCanonicalPair),
    }
    match (
        vertex.varying_texcoord_name.as_deref(),
        fragment.color.texture_varying_name(),
    ) {
        (Some(vertex_varying), Some(fragment_varying)) if vertex_varying == fragment_varying => {},
        (None, None) => {},
        _ => return Err(ShaderTranslationError::UnsupportedCanonicalPair),
    }
    Ok(CanonicalProgramInfo { vertex, fragment })
}

fn translate_to_wgsl(shader: NagaGlslShader) -> Result<String, ShaderTranslationError> {
    use naga::{
        back::wgsl,
        front::glsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    let glsl_owned = shader.source;
    let name = shader.name;
    let _float_precision = shader.float_precision;
    let stage: naga::ShaderStage = shader.stage.into();
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            std::panic::catch_unwind(move || {
                let mut frontend = glsl::Frontend::default();
                let module = frontend
                    .parse(&glsl::Options::from(stage), &glsl_owned)
                    .map_err(|error| {
                        ShaderTranslationError::Parse(format!("[{name}]: {error:?}"))
                    })?;
                let info = Validator::new(ValidationFlags::all(), Capabilities::all())
                    .validate(&module)
                    .map_err(|error| {
                        ShaderTranslationError::Validate(format!("[{name}]: {error:?}"))
                    })?;
                wgsl::write_string(&module, &info, wgsl::WriterFlags::empty())
                    .map_err(|error| ShaderTranslationError::Emit(format!("[{name}]: {error:?}")))
            })
        })
        .map_err(|error| ShaderTranslationError::ThreadSpawn(format!("[{name}]: {error}")))?;

    match handle.join().map_err(|panic| {
        ShaderTranslationError::ThreadJoin(format!("[{name}]: {}", panic_message(&*panic)))
    })? {
        Ok(result) => result,
        Err(panic) => Err(ShaderTranslationError::NagaPanic(format!(
            "[{name}]: {}",
            panic_message(&*panic)
        ))),
    }
}

impl From<WebGlShaderStage> for naga::ShaderStage {
    fn from(stage: WebGlShaderStage) -> Self {
        match stage {
            WebGlShaderStage::Vertex => Self::Vertex,
            WebGlShaderStage::Fragment => Self::Fragment,
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "unknown panic".to_string()
    }
}
