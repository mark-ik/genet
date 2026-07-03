/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Program reflection layer over a parsed [`TranslationUnit`].
//!
//! `webgl-essl`'s lowering and a consumer like `webgl-wgpu`'s
//! pipeline builder both walk the same global decls in source
//! order to assign `@location(N)` / descriptor bindings. This
//! module exposes those decisions as a stable, query-friendly
//! [`ProgramReflection`] without re-running the full lowering.
//! Consumers use it to derive vertex-buffer layouts, bind-group
//! layouts, and uniform offsets from the same ESSL source the
//! lowering will compile.
//!
//! The layout rules here MUST match what `lower.rs` emits;
//! receipts in `tests/reflect_alignment.rs` cross-check the
//! @location(N) decorations the lowering produces against
//! [`reflect`]'s assignments.

use crate::ast::{ExternalDecl, StorageQualifier, TranslationUnit, TypeKind};
use crate::validate::ShaderStage;

/// Everything a consumer needs to wire up vertex buffers,
/// bind groups, and pipeline layouts without re-parsing the
/// ESSL source itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProgramReflection {
    /// Stage inputs in source order. For Vertex, these are
    /// the `attribute` / `in` decls; for Fragment, the
    /// `varying` / `in` decls. Each one's `location` matches
    /// the SPIR-V `@location(N)` the lowering emits.
    pub inputs: Vec<InputBinding>,
    /// Stage outputs in source order. For Vertex, these are
    /// the `varying` / `out` decls; for Fragment under ESSL
    /// 3.00, the user-declared `out` decls. ESSL 1.00
    /// fragments use the implicit `gl_FragColor` output —
    /// rendered as a one-entry list with name `"gl_FragColor"`
    /// at `location: 0`.
    pub outputs: Vec<OutputBinding>,
    /// Non-sampler uniforms in source order. Each carries the
    /// member index it occupies inside the per-shader uniform
    /// `Block` struct (`@binding(0)` of `@group(0)`).
    pub uniforms: Vec<UniformBinding>,
    /// Sampler uniforms in source order. The image and
    /// sampler variables take consecutive bindings starting at
    /// `1` (the uniform Block reserves `0` when present —
    /// otherwise still skipped for shape consistency).
    pub samplers: Vec<SamplerBinding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputBinding {
    pub name: String,
    pub kind: TypeKind,
    /// First Location consumed. Matrix kinds consume
    /// `column_count(kind)` consecutive Locations.
    pub location: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputBinding {
    pub name: String,
    pub kind: TypeKind,
    pub location: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UniformBinding {
    pub name: String,
    pub kind: TypeKind,
    /// Zero-based index of this uniform inside the per-shader
    /// `Block`-decorated struct.
    pub member_index: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SamplerBinding {
    pub name: String,
    /// `Sampler2D` or `SamplerCube`.
    pub kind: TypeKind,
    /// `@binding(N)` of the image variable.
    pub image_binding: u32,
    /// `@binding(N+1)` of the sampler variable.
    pub sampler_binding: u32,
    /// Always `0` today; webgl-essl emits a single
    /// `@group(0)`.
    pub descriptor_set: u32,
}

/// Build a [`ProgramReflection`] for `tu` at `stage`. Always
/// succeeds — this is a pure layout walk that consults only
/// declared storage qualifiers and types, not main-body
/// semantics. Run validation / typecheck separately if you
/// need ESSL diagnostics.
pub fn reflect(tu: &TranslationUnit, stage: ShaderStage) -> ProgramReflection {
    let mut r = ProgramReflection::default();
    let mut input_loc: u32 = 0;
    let mut output_loc: u32 = 0;
    let mut sampler_binding: u32 = 1;
    let mut uniform_member: u32 = 0;

    let has_user_fragment_outs = stage == ShaderStage::Fragment
        && tu.decls.iter().any(|d| {
            matches!(
                d,
                ExternalDecl::Global(g) if g.storage == StorageQualifier::Out
            )
        });

    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        let is_input = match stage {
            ShaderStage::Vertex => matches!(
                g.storage,
                StorageQualifier::Attribute | StorageQualifier::In
            ),
            ShaderStage::Fragment => {
                matches!(g.storage, StorageQualifier::Varying | StorageQualifier::In)
            },
        };
        let is_output = match stage {
            ShaderStage::Vertex => {
                matches!(g.storage, StorageQualifier::Varying | StorageQualifier::Out)
            },
            ShaderStage::Fragment => g.storage == StorageQualifier::Out,
        };
        if is_input {
            let span = location_span_for(g.ty.kind);
            if span > 0 {
                r.inputs.push(InputBinding {
                    name: g.name.clone(),
                    kind: g.ty.kind,
                    location: input_loc,
                });
                input_loc += span;
            }
            continue;
        }
        if is_output {
            let span = location_span_for(g.ty.kind);
            if span > 0 {
                r.outputs.push(OutputBinding {
                    name: g.name.clone(),
                    kind: g.ty.kind,
                    location: output_loc,
                });
                output_loc += span;
            }
            continue;
        }
        if g.storage == StorageQualifier::Uniform {
            match g.ty.kind {
                TypeKind::Sampler2D | TypeKind::SamplerCube => {
                    r.samplers.push(SamplerBinding {
                        name: g.name.clone(),
                        kind: g.ty.kind,
                        image_binding: sampler_binding,
                        sampler_binding: sampler_binding + 1,
                        descriptor_set: 0,
                    });
                    sampler_binding += 2;
                },
                kind if uniform_block_slot_for(kind) > 0 => {
                    r.uniforms.push(UniformBinding {
                        name: g.name.clone(),
                        kind,
                        member_index: uniform_member,
                    });
                    uniform_member += 1;
                },
                _ => {
                    // Skip types the uniform block can't hold
                    // (Bool, Void, Struct — first cut).
                },
            }
        }
    }

    // ESSL 1.00 fragments use the implicit `gl_FragColor`
    // output at `@location(0)`. Surface it so consumers see a
    // consistent output list regardless of ESSL version.
    if stage == ShaderStage::Fragment && !has_user_fragment_outs {
        r.outputs.push(OutputBinding {
            name: "gl_FragColor".into(),
            kind: TypeKind::Vec4,
            location: 0,
        });
    }

    r
}

/// Number of `@location` slots a kind consumes when used as a
/// stage input or output. Matrix kinds are column-split so
/// they consume one slot per column. Sampler / void / struct
/// kinds aren't location-shaped and return 0.
fn location_span_for(kind: TypeKind) -> u32 {
    match kind {
        TypeKind::Float
        | TypeKind::Int
        | TypeKind::Bool
        | TypeKind::Vec2
        | TypeKind::Vec3
        | TypeKind::Vec4
        | TypeKind::Ivec2
        | TypeKind::Ivec3
        | TypeKind::Ivec4
        | TypeKind::Bvec2
        | TypeKind::Bvec3
        | TypeKind::Bvec4 => 1,
        TypeKind::Mat2 => 2,
        TypeKind::Mat3 => 3,
        TypeKind::Mat4 => 4,
        TypeKind::Void | TypeKind::Sampler2D | TypeKind::SamplerCube | TypeKind::Struct(_) => 0,
    }
}

/// Whether a kind has a slot inside the uniform `Block` struct
/// in `lower.rs`. Returns 1 for the kinds the lowering admits
/// today, 0 otherwise.
fn uniform_block_slot_for(kind: TypeKind) -> u32 {
    match kind {
        TypeKind::Float
        | TypeKind::Int
        | TypeKind::Vec2
        | TypeKind::Vec3
        | TypeKind::Vec4
        | TypeKind::Ivec2
        | TypeKind::Ivec3
        | TypeKind::Ivec4
        | TypeKind::Mat2
        | TypeKind::Mat3
        | TypeKind::Mat4 => 1,
        _ => 0,
    }
}
