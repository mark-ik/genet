/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! ESSL → WGSL via the standalone `webgl-essl` frontend.
//!
//! The narrow `Canonical{Vertex,Fragment}Info` parsers + naga
//! `glsl-in` pipeline that used to live here have been switched
//! out for `webgl-essl::compile`. The pair-interface check
//! (varying name + kind agreement) is re-implemented over
//! `webgl-essl::reflect` so a vertex-output / fragment-input
//! mismatch still surfaces as `UnsupportedCanonicalPair`. The
//! narrow [`ProgramReflection`] shape stays — its consumers
//! (`webgl/pipeline.rs`, `webgl/programs.rs`, `webgl/draw.rs`)
//! still want `position_attribute` / `color_attribute` /
//! `texcoord_attribute` etc. — but its binding numbers now
//! mirror what `webgl-essl` actually decorates the WGSL with
//! (uniform Block at `@binding(0)`; sampler image at
//! `@binding(1)`, sampler at `@binding(2)`).

use super::*;
use webgl_essl::ast::TypeKind as EsslTypeKind;
use webgl_essl::reflect::ProgramReflection as EsslReflection;
use webgl_essl::validate::ShaderStage as EsslStage;
use webgl_essl::{self, CompileError};

pub(crate) fn translate_canonical_essl_pair(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<TranslatedProgram, ShaderTranslationError> {
    let (vertex_wgsl, vertex_refl) = compile_with_reflection(vertex_source, EsslStage::Vertex)?;
    let (fragment_wgsl, fragment_refl) =
        compile_with_reflection(fragment_source, EsslStage::Fragment)?;
    check_pair_interface(&vertex_refl, &fragment_refl)?;
    let reflection =
        narrow_reflection(&vertex_refl, &fragment_refl, fragment_source)?;
    Ok(TranslatedProgram {
        vertex_wgsl,
        fragment_wgsl,
        reflection,
    })
}

pub(crate) fn validate_canonical_vertex_source(
    vertex_source: &str,
) -> Result<(), ShaderTranslationError> {
    compile_with_reflection(vertex_source, EsslStage::Vertex).map(|_| ())
}

pub(crate) fn validate_canonical_fragment_source(
    fragment_source: &str,
) -> Result<(), ShaderTranslationError> {
    compile_with_reflection(fragment_source, EsslStage::Fragment).map(|_| ())
}

pub(crate) fn canonical_essl_cache_key(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<ProgramCacheKey, ShaderTranslationError> {
    let (_, vertex_refl) = compile_with_reflection(vertex_source, EsslStage::Vertex)?;
    let (_, fragment_refl) = compile_with_reflection(fragment_source, EsslStage::Fragment)?;
    check_pair_interface(&vertex_refl, &fragment_refl)?;
    Ok(ProgramCacheKey {
        vertex: vertex_source.to_string(),
        fragment: fragment_source.to_string(),
    })
}

fn compile_with_reflection(
    source: &str,
    stage: EsslStage,
) -> Result<(String, EsslReflection), ShaderTranslationError> {
    let tu = webgl_essl::parse_source(source).map_err(|error| {
        ShaderTranslationError::Parse(format!("{error:?}"))
    })?;
    let check_result = webgl_essl::check::check(&tu);
    if !check_result.diagnostics.is_empty() {
        return Err(ShaderTranslationError::Validate(format!(
            "{} typecheck diagnostic(s)",
            check_result.diagnostics.len()
        )));
    }
    let validation = webgl_essl::validate::validate(&tu, source, stage);
    if validation.num_errors() > 0 {
        return Err(ShaderTranslationError::Validate(validation.info_log));
    }
    let wgsl = webgl_essl::lower::lower_to_wgsl(&tu, stage)
        .map_err(|error| ShaderTranslationError::Emit(format!("{error}")))?;
    let reflection = webgl_essl::reflect::reflect(&tu, stage);
    Ok((wgsl, reflection))
}

/// Verify that every varying the vertex shader writes has a
/// matching input declaration in the fragment shader (same name,
/// same kind). Returns `UnsupportedCanonicalPair` on the first
/// mismatch — production WebGL surfaces this as a link error.
fn check_pair_interface(
    vertex: &EsslReflection,
    fragment: &EsslReflection,
) -> Result<(), ShaderTranslationError> {
    for vertex_output in &vertex.outputs {
        let Some(fragment_input) = fragment
            .inputs
            .iter()
            .find(|input| input.name == vertex_output.name)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        if fragment_input.kind != vertex_output.kind {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
    }
    for fragment_input in &fragment.inputs {
        if !vertex
            .outputs
            .iter()
            .any(|output| output.name == fragment_input.name)
        {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
    }
    Ok(())
}

/// Derive the narrow `ProgramReflection` (position / color /
/// texcoord / single fragment-color uniform / single sampler)
/// the WebGL frontend wants from the wider `webgl-essl`
/// reflection. Binding numbers carry through unchanged — the
/// pipeline builder and bind-group factories use them directly.
fn narrow_reflection(
    vertex: &EsslReflection,
    fragment: &EsslReflection,
    fragment_source: &str,
) -> Result<ProgramReflection, ShaderTranslationError> {
    let position = vertex
        .inputs
        .first()
        .ok_or(ShaderTranslationError::UnsupportedCanonicalPair)?;
    let position_attribute = VertexAttributeReflection {
        name: position.name.clone(),
        location: position.location,
        kind: vertex_kind_from_essl(position.kind)
            .ok_or(ShaderTranslationError::UnsupportedCanonicalPair)?,
    };
    let mut color_attribute = None;
    let mut texcoord_attribute = None;
    if let Some(second) = vertex.inputs.get(1) {
        match second.kind {
            EsslTypeKind::Vec4 => {
                color_attribute = Some(VertexAttributeReflection {
                    name: second.name.clone(),
                    location: second.location,
                    kind: VertexAttributeKind::Float32x4,
                });
            },
            EsslTypeKind::Vec2 => {
                texcoord_attribute = Some(VertexAttributeReflection {
                    name: second.name.clone(),
                    location: second.location,
                    kind: VertexAttributeKind::Float32x2,
                });
            },
            _ => {
                return Err(ShaderTranslationError::UnsupportedCanonicalPair);
            },
        }
    }
    let fragment_color_uniform = fragment
        .uniforms
        .iter()
        .find(|u| u.kind == EsslTypeKind::Vec4)
        .map(|u| UniformReflection {
            name: u.name.clone(),
            // webgl-essl puts the uniform Block at @binding(0).
            binding: 0,
            kind: UniformKind::Float32x4,
        });
    let fragment_texture_uniform = fragment
        .samplers
        .iter()
        .find(|s| s.kind == EsslTypeKind::Sampler2D)
        .map(|s| UniformReflection {
            name: s.name.clone(),
            // webgl-essl emits the image at @binding(N), the
            // sampler at @binding(N+1). The narrow shape keeps
            // the image binding and pipeline.rs derives the
            // sampler one by `+1`.
            binding: s.image_binding,
            kind: UniformKind::Sampler2D,
        });
    let fragment_float_precision = extract_fragment_float_precision(fragment_source)
        .unwrap_or(WebGlPrecision::Medium);
    Ok(ProgramReflection {
        position_attribute,
        color_attribute,
        texcoord_attribute,
        fragment_color_uniform,
        fragment_texture_uniform,
        fragment_float_precision,
    })
}

fn vertex_kind_from_essl(kind: EsslTypeKind) -> Option<VertexAttributeKind> {
    match kind {
        EsslTypeKind::Vec2 => Some(VertexAttributeKind::Float32x2),
        EsslTypeKind::Vec4 => Some(VertexAttributeKind::Float32x4),
        _ => None,
    }
}

/// Scan the fragment source for a `precision <qualifier> float;`
/// declaration so the narrow reflection can carry it through.
/// `webgl-essl`'s validator handles the actual precision rules;
/// this is just a surface-level scrape for the public field.
fn extract_fragment_float_precision(source: &str) -> Option<WebGlPrecision> {
    let mut last = None;
    let mut rest = source;
    while let Some(start) = rest.find("precision") {
        let after = &rest[start + "precision".len()..];
        let Some(after) = after.strip_prefix(|c: char| c.is_whitespace()) else {
            rest = after;
            continue;
        };
        let mut parts = after.splitn(2, char::is_whitespace);
        let qualifier = parts.next().unwrap_or("");
        let tail = parts.next().unwrap_or("");
        let tail = tail.trim_start();
        if let Some(type_token) = tail.split(|c: char| c == ';' || c.is_whitespace()).next() {
            if type_token == "float" {
                if let Some(precision) = WebGlPrecision::parse(qualifier) {
                    last = Some(precision);
                }
            }
        }
        rest = tail;
    }
    last
}

impl From<CompileError> for ShaderTranslationError {
    fn from(error: CompileError) -> Self {
        match error {
            CompileError::Parse(error) => Self::Parse(format!("{error:?}")),
            CompileError::Check(diagnostics) => Self::Validate(format!(
                "{} typecheck diagnostic(s)",
                diagnostics.len()
            )),
            CompileError::Validate(result) => Self::Validate(result.info_log),
            CompileError::Lower(error) => Self::Emit(format!("{error}")),
        }
    }
}
