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

/// Derive the wider [`ProgramReflection`] webgl-wgpu now
/// exposes from `webgl-essl`'s reflection. Carries every
/// vertex attribute, every Block uniform with its std140 byte
/// offset, and every sampler. Also populates the narrow
/// `position_attribute` / `color_attribute` / `texcoord_attribute`
/// / `fragment_color_uniform` / `fragment_texture_uniform`
/// views that the pre-widening pipeline / draw path still
/// reads.
fn narrow_reflection(
    vertex: &EsslReflection,
    fragment: &EsslReflection,
    fragment_source: &str,
) -> Result<ProgramReflection, ShaderTranslationError> {
    let mut attributes = Vec::with_capacity(vertex.inputs.len());
    for input in &vertex.inputs {
        attributes.push(VertexAttributeReflection {
            name: input.name.clone(),
            location: input.location,
            kind: vertex_kind_from_essl(input.kind)
                .ok_or(ShaderTranslationError::UnsupportedCanonicalPair)?,
        });
    }
    let position_attribute = attributes
        .first()
        .cloned()
        .ok_or(ShaderTranslationError::UnsupportedCanonicalPair)?;
    let color_attribute = attributes
        .get(1)
        .filter(|a| a.kind == VertexAttributeKind::Float32x4)
        .cloned();
    let texcoord_attribute = attributes
        .get(1)
        .filter(|a| a.kind == VertexAttributeKind::Float32x2)
        .cloned();

    // Uniform Block draws from both stages. In production
    // shader pairs the same uniform is either (a) declared
    // only in the stage that reads it, or (b) declared
    // identically in both (matching source order so each
    // stage's emitted Block layout agrees). The dedup-by-name
    // here preserves both shapes; cross-stage layout drift
    // would only surface for shaders that declare overlapping
    // names with different kinds, which webgl-essl rejects
    // earlier as a link mismatch.
    let mut uniforms: Vec<UniformReflection> = Vec::new();
    let mut block_cursor: u32 = 0;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let push_uniform = |u: &webgl_essl::reflect::UniformBinding,
                            uniforms: &mut Vec<UniformReflection>,
                            block_cursor: &mut u32,
                            seen: &mut std::collections::HashSet<String>|
     -> Result<(), ShaderTranslationError> {
        if seen.contains(&u.name) {
            return Ok(());
        }
        seen.insert(u.name.clone());
        let kind = uniform_kind_from_essl(u.kind)
            .ok_or(ShaderTranslationError::UnsupportedCanonicalPair)?;
        let alignment = kind.block_alignment();
        if alignment > 0 {
            *block_cursor = align_up(*block_cursor, alignment);
        }
        let size = kind.block_size();
        let member_index = uniforms.len() as u32;
        uniforms.push(UniformReflection {
            name: u.name.clone(),
            binding: 0,
            kind,
            member_index,
            block_offset: *block_cursor,
            block_size: size,
        });
        *block_cursor += size;
        Ok(())
    };
    for u in &vertex.uniforms {
        push_uniform(u, &mut uniforms, &mut block_cursor, &mut seen)?;
    }
    for u in &fragment.uniforms {
        push_uniform(u, &mut uniforms, &mut block_cursor, &mut seen)?;
    }
    // The Block buffer itself must be sized to a 16-byte
    // multiple (wgpu requires UNIFORM buffers be ≥ 16 aligned).
    let uniform_block_size = if uniforms.is_empty() {
        0
    } else {
        align_up(block_cursor, 16)
    };

    // Samplers only come from the fragment stage today
    // (texture sampling is fragment-only in ESSL 1.00; ESSL
    // 3.00 vertex texture is feature-flagged out for now).
    let mut samplers = Vec::with_capacity(fragment.samplers.len());
    for s in &fragment.samplers {
        let kind = uniform_kind_from_essl(s.kind)
            .ok_or(ShaderTranslationError::UnsupportedCanonicalPair)?;
        samplers.push(SamplerReflection {
            name: s.name.clone(),
            image_binding: s.image_binding,
            sampler_binding: s.sampler_binding,
            kind,
        });
    }

    let fragment_color_uniform = uniforms
        .iter()
        .find(|u| u.kind == UniformKind::Float32x4)
        .cloned();
    let fragment_texture_uniform = samplers
        .iter()
        .find(|s| s.kind == UniformKind::Sampler2D)
        .map(|s| UniformReflection {
            name: s.name.clone(),
            binding: s.image_binding,
            kind: UniformKind::Sampler2D,
            member_index: 0,
            block_offset: 0,
            block_size: 0,
        });
    let fragment_float_precision = extract_fragment_float_precision(fragment_source)
        .unwrap_or(WebGlPrecision::Medium);
    Ok(ProgramReflection {
        attributes,
        uniforms,
        samplers,
        uniform_block_size,
        position_attribute,
        color_attribute,
        texcoord_attribute,
        fragment_color_uniform,
        fragment_texture_uniform,
        fragment_float_precision,
    })
}

fn align_up(offset: u32, alignment: u32) -> u32 {
    if alignment <= 1 {
        return offset;
    }
    (offset + alignment - 1) & !(alignment - 1)
}

fn vertex_kind_from_essl(kind: EsslTypeKind) -> Option<VertexAttributeKind> {
    match kind {
        EsslTypeKind::Float => Some(VertexAttributeKind::Float32),
        EsslTypeKind::Vec2 => Some(VertexAttributeKind::Float32x2),
        EsslTypeKind::Vec3 => Some(VertexAttributeKind::Float32x3),
        EsslTypeKind::Vec4 => Some(VertexAttributeKind::Float32x4),
        _ => None,
    }
}

fn uniform_kind_from_essl(kind: EsslTypeKind) -> Option<UniformKind> {
    match kind {
        EsslTypeKind::Float => Some(UniformKind::Float32),
        EsslTypeKind::Vec2 => Some(UniformKind::Float32x2),
        EsslTypeKind::Vec3 => Some(UniformKind::Float32x3),
        EsslTypeKind::Vec4 => Some(UniformKind::Float32x4),
        EsslTypeKind::Mat2 => Some(UniformKind::Matrix2),
        EsslTypeKind::Mat3 => Some(UniformKind::Matrix3),
        EsslTypeKind::Mat4 => Some(UniformKind::Matrix4),
        EsslTypeKind::Int => Some(UniformKind::Int),
        EsslTypeKind::Sampler2D => Some(UniformKind::Sampler2D),
        EsslTypeKind::SamplerCube => Some(UniformKind::SamplerCube),
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
