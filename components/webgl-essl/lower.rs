/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 6 spike: lower a typechecked ESSL translation unit to WGSL via
//! path A (ESSL → SPIR-V (rspirv) → naga IR (`spv-in`) → WGSL
//! (`wgsl-out`)).
//!
//! Widened from the first proof to handle:
//!
//! * `void main() { gl_FragColor = <expr>; }` (fragment)
//! * `void main() { gl_Position  = <expr>; }` (vertex)
//!
//! where `<expr>` is built from:
//!
//! * Float / int literals (int promoted to float at the SPIR-V seam).
//! * `attribute vec_n a_name;` references in vertex shaders, loaded
//!   via OpLoad from sequentially-Location-decorated Input variables.
//! * `vec2(...)` / `vec3(...)` / `vec4(...)` constructors over any
//!   mix of scalars and vectors whose total component count matches.
//!
//! Unsupported shapes still return `LoweringError::UnsupportedShape`
//! with a descriptive message. Each follow-up extension (uniforms,
//! varyings, binary ops, function calls, swizzles, texture samples)
//! drops into the same expression-driven shape.

use std::collections::HashMap;

use rspirv::binary::Assemble;
use rspirv::dr::{Builder, Module, Operand};
use rspirv::spirv::{
    AddressingModel, BuiltIn, Capability, Decoration, ExecutionMode, ExecutionModel,
    FunctionControl, MemoryModel, StorageClass, Word,
};

use crate::ast::{
    BinOp, Expr, ExternalDecl, FunctionDef, StorageQualifier, Stmt, TranslationUnit, TypeKind,
};
use crate::span::Span;
use crate::validate::ShaderStage;

#[derive(Debug)]
pub enum LoweringError {
    NoMain,
    UnsupportedShape { what: String },
    SpirvBuild(String),
    NagaParse(String),
    NagaValidate(String),
    WgslEmit(String),
}

impl std::fmt::Display for LoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoweringError::NoMain => write!(f, "no `main` function defined"),
            LoweringError::UnsupportedShape { what } => write!(f, "unsupported shape: {what}"),
            LoweringError::SpirvBuild(m) => write!(f, "SPIR-V build error: {m}"),
            LoweringError::NagaParse(m) => write!(f, "naga spv-in parse error: {m}"),
            LoweringError::NagaValidate(m) => write!(f, "naga validation error: {m}"),
            LoweringError::WgslEmit(m) => write!(f, "WGSL emit error: {m}"),
        }
    }
}

/// Public entry: lower `tu` to WGSL.
pub fn lower_to_wgsl(tu: &TranslationUnit, stage: ShaderStage) -> Result<String, LoweringError> {
    // Run the typecheck pass so the lowering can consult the per-span
    // type annotations when emitting SPIR-V opcodes that depend on
    // operand types (e.g. OpFMul vs OpVectorTimesScalar). Typecheck
    // diagnostics are not surfaced here; the caller is expected to
    // have run [`crate::check::check`] separately if they care.
    let types = crate::check::check(tu).types;
    let spirv = build_spirv(tu, stage, &types)?;
    let words = spirv.assemble();
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();

    let module = naga::front::spv::parse_u8_slice(&bytes, &naga::front::spv::Options::default())
        .map_err(|e| LoweringError::NagaParse(format!("{e:?}")))?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| LoweringError::NagaValidate(format!("{e:?}")))?;

    naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| LoweringError::WgslEmit(format!("{e:?}")))
}

// ---------- AST navigation --------------------------------------------

fn find_main(tu: &TranslationUnit) -> Result<&FunctionDef, LoweringError> {
    tu.decls
        .iter()
        .find_map(|d| match d {
            ExternalDecl::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .ok_or(LoweringError::NoMain)
}

fn find_output_assign<'a>(
    main: &'a FunctionDef,
    stage: ShaderStage,
) -> Result<&'a Expr, LoweringError> {
    let expected = match stage {
        ShaderStage::Fragment => "gl_FragColor",
        ShaderStage::Vertex => "gl_Position",
    };
    let body_stmt = main.body.stmts.first().ok_or_else(|| LoweringError::UnsupportedShape {
        what: "main has empty body".into(),
    })?;
    let (lhs_name, rhs): (&str, &Expr) = match body_stmt {
        Stmt::Expr(Expr::Assign { lhs, rhs, .. }) => match lhs.as_ref() {
            Expr::Ident { name, .. } => (name.as_str(), rhs.as_ref()),
            _ => {
                return Err(LoweringError::UnsupportedShape {
                    what: "main body lhs is not an identifier".into(),
                });
            },
        },
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "main body is not a single assignment".into(),
            });
        },
    };
    if lhs_name != expected {
        return Err(LoweringError::UnsupportedShape {
            what: format!("main body assigns to `{lhs_name}`, expected `{expected}` for {stage:?}"),
        });
    }
    Ok(rhs)
}

// ---------- SPIR-V emission -------------------------------------------

struct Ctx<'a> {
    b: Builder,
    type_float: Word,
    type_int: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
    /// ESSL identifier name -> Input variable binding. Today this
    /// holds vertex attributes; fragment varyings would be added the
    /// same way under the Fragment stage.
    inputs: HashMap<String, InputBinding>,
    /// ESSL uniform name -> the member index of that uniform inside
    /// the per-shader uniform block. Empty when the shader declares
    /// no uniforms.
    uniforms: HashMap<String, UniformBinding>,
    /// SPIR-V Word for the uniform-block OpVariable (the single struct
    /// holding all uniforms). `None` when the shader has no uniforms.
    uniform_block_var: Option<Word>,
    /// Cached `OpConstant int <i>` words for OpAccessChain indices.
    int_constants: HashMap<i32, Word>,
    /// Per-span type annotations from the typecheck pass; used to
    /// dispatch on operand types in binary-op lowering.
    types: &'a HashMap<Span, TypeKind>,
}

struct InputBinding {
    /// SPIR-V Word for the OpVariable itself.
    var: Word,
    /// SPIR-V Word for the pointee type (the variable's value type).
    pointee_type: Word,
    /// ESSL value type of this binding. Tracked so the emitter knows
    /// the loaded type without re-querying SPIR-V.
    kind: TypeKind,
}

struct UniformBinding {
    /// Zero-based index of this uniform inside the block struct.
    member_index: u32,
    /// SPIR-V Word for the value type of this member (used in
    /// OpAccessChain return type construction).
    pointee_type: Word,
    /// ESSL value type. Mirror of InputBinding::kind.
    kind: TypeKind,
}

fn build_spirv(
    tu: &TranslationUnit,
    stage: ShaderStage,
    types: &HashMap<Span, TypeKind>,
) -> Result<Module, LoweringError> {
    let main = find_main(tu)?;
    let output_expr = find_output_assign(main, stage)?;

    let mut b = Builder::new();
    b.set_version(1, 0);
    b.capability(Capability::Shader);
    b.memory_model(AddressingModel::Logical, MemoryModel::GLSL450);

    let type_void = b.type_void();
    let type_float = b.type_float(32, None);
    let type_int = b.type_int(32, 1);
    let type_vec2 = b.type_vector(type_float, 2);
    let type_vec3 = b.type_vector(type_float, 3);
    let type_vec4 = b.type_vector(type_float, 4);

    // Register input variables (attributes in vertex stage).
    let inputs = register_inputs(&mut b, tu, stage, type_float, type_vec2, type_vec3, type_vec4);

    // Register uniforms wrapped in a single Block-decorated struct
    // (the WebGL / Vulkan convention naga's spv-in understands).
    let (uniforms, uniform_block_var) =
        register_uniforms(&mut b, tu, type_float, type_vec2, type_vec3, type_vec4);

    // Register the output variable (always vec4 in the cases this
    // module handles).
    let ptr_output = b.type_pointer(None, StorageClass::Output, type_vec4);
    let output_var = b.variable(ptr_output, None, StorageClass::Output, None);
    match stage {
        ShaderStage::Vertex => {
            b.decorate(output_var, Decoration::BuiltIn, [Operand::BuiltIn(BuiltIn::Position)]);
        },
        ShaderStage::Fragment => {
            b.decorate(output_var, Decoration::Location, [Operand::LiteralBit32(0)]);
        },
    }

    // void main() { ... }
    let fn_type = b.type_function(type_void, []);
    let main_fn = b
        .begin_function(type_void, None, FunctionControl::NONE, fn_type)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    b.begin_block(None).map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    let mut ctx = Ctx {
        b,
        type_float,
        type_int,
        type_vec2,
        type_vec3,
        type_vec4,
        inputs,
        uniforms,
        uniform_block_var,
        int_constants: HashMap::new(),
        types,
    };
    let value_id = lower_expr(&mut ctx, output_expr)?;
    ctx.b
        .store(output_var, value_id, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b.ret().map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b.end_function().map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // Entry point: interface includes the output plus every input the
    // shader actually exposes. Uniforms are not part of the SPIR-V
    // entry-point interface (they are bound through DescriptorSet).
    let execution_model = match stage {
        ShaderStage::Vertex => ExecutionModel::Vertex,
        ShaderStage::Fragment => ExecutionModel::Fragment,
    };
    let mut interface: Vec<Word> = ctx.inputs.values().map(|b| b.var).collect();
    interface.push(output_var);
    ctx.b.entry_point(execution_model, main_fn, "main", interface);
    if stage == ShaderStage::Fragment {
        ctx.b.execution_mode(main_fn, ExecutionMode::OriginUpperLeft, []);
    }

    Ok(ctx.b.module())
}

fn register_uniforms(
    b: &mut Builder,
    tu: &TranslationUnit,
    type_float: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
) -> (HashMap<String, UniformBinding>, Option<Word>) {
    let mut uniforms: Vec<(String, TypeKind, Word, u32)> = Vec::new();
    let mut offset: u32 = 0;
    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        if g.storage != StorageQualifier::Uniform {
            continue;
        }
        let (pointee_type, kind, size) = match g.ty.kind {
            TypeKind::Float => (type_float, TypeKind::Float, 4u32),
            TypeKind::Vec2 => (type_vec2, TypeKind::Vec2, 8u32),
            TypeKind::Vec3 => (type_vec3, TypeKind::Vec3, 16u32),
            TypeKind::Vec4 => (type_vec4, TypeKind::Vec4, 16u32),
            // mat / sampler uniforms are queued.
            _ => continue,
        };
        // std140-ish offset alignment: vec3 / vec4 align to 16 bytes,
        // vec2 to 8, scalars to 4. Simplistic but enough for this
        // chunk's corpus.
        let align = match kind {
            TypeKind::Vec3 | TypeKind::Vec4 => 16,
            TypeKind::Vec2 => 8,
            _ => 4,
        };
        offset = (offset + align - 1) / align * align;
        uniforms.push((g.name.clone(), kind, pointee_type, offset));
        offset += size;
    }
    if uniforms.is_empty() {
        return (HashMap::new(), None);
    }
    let member_types: Vec<Word> = uniforms.iter().map(|(_, _, ty, _)| *ty).collect();
    let struct_ty = b.type_struct(member_types);
    b.decorate(struct_ty, Decoration::Block, []);
    for (i, (_, _, _, off)) in uniforms.iter().enumerate() {
        b.member_decorate(
            struct_ty,
            i as u32,
            Decoration::Offset,
            [Operand::LiteralBit32(*off)],
        );
    }
    let ptr_uniform = b.type_pointer(None, StorageClass::Uniform, struct_ty);
    let var = b.variable(ptr_uniform, None, StorageClass::Uniform, None);
    b.decorate(var, Decoration::DescriptorSet, [Operand::LiteralBit32(0)]);
    b.decorate(var, Decoration::Binding, [Operand::LiteralBit32(0)]);
    let mut map = HashMap::new();
    for (i, (name, kind, pointee, _)) in uniforms.into_iter().enumerate() {
        map.insert(name, UniformBinding { member_index: i as u32, pointee_type: pointee, kind });
    }
    (map, Some(var))
}

fn register_inputs(
    b: &mut Builder,
    tu: &TranslationUnit,
    stage: ShaderStage,
    type_float: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
) -> HashMap<String, InputBinding> {
    let mut inputs = HashMap::new();
    if stage != ShaderStage::Vertex {
        // Fragment varyings / uniforms are queued for a follow-up.
        return inputs;
    }
    let mut location: u32 = 0;
    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        if g.storage != StorageQualifier::Attribute {
            continue;
        }
        let (pointee_type, kind) = match g.ty.kind {
            TypeKind::Float => (type_float, TypeKind::Float),
            TypeKind::Vec2 => (type_vec2, TypeKind::Vec2),
            TypeKind::Vec3 => (type_vec3, TypeKind::Vec3),
            TypeKind::Vec4 => (type_vec4, TypeKind::Vec4),
            // Other attribute types (int / ivec / mat) are not exercised
            // by today's spike corpus; emit nothing so the expression
            // emitter will error if they are referenced.
            _ => continue,
        };
        let ptr_ty = b.type_pointer(None, StorageClass::Input, pointee_type);
        let var = b.variable(ptr_ty, None, StorageClass::Input, None);
        b.decorate(var, Decoration::Location, [Operand::LiteralBit32(location)]);
        location += 1;
        inputs.insert(g.name.clone(), InputBinding { var, pointee_type, kind });
    }
    inputs
}

/// Best-effort AST-side classification of an expression's lowered
/// type, using the typecheck pass's span->type map when present and
/// falling back to AST inspection.
fn classify_arg_kind(ctx: &Ctx, e: &Expr) -> Option<TypeKind> {
    if let Some(ty) = ctx.types.get(&e.span()).copied() {
        return Some(ty);
    }
    match e {
        Expr::FloatLit { .. } | Expr::IntLit { .. } => Some(TypeKind::Float),
        Expr::Ident { name, .. } => ctx
            .inputs
            .get(name)
            .map(|b| b.kind)
            .or_else(|| ctx.uniforms.get(name).map(|u| u.kind)),
        Expr::Call { callee, .. } => match callee.as_str() {
            "vec2" => Some(TypeKind::Vec2),
            "vec3" => Some(TypeKind::Vec3),
            "vec4" => Some(TypeKind::Vec4),
            _ => None,
        },
        _ => None,
    }
}

fn spv_type_for_kind(ctx: &Ctx, kind: TypeKind) -> Option<Word> {
    Some(match kind {
        TypeKind::Float => ctx.type_float,
        TypeKind::Int => ctx.type_int,
        TypeKind::Vec2 => ctx.type_vec2,
        TypeKind::Vec3 => ctx.type_vec3,
        TypeKind::Vec4 => ctx.type_vec4,
        _ => return None,
    })
}

fn int_constant(ctx: &mut Ctx, value: i32) -> Word {
    if let Some(&w) = ctx.int_constants.get(&value) {
        return w;
    }
    let w = ctx.b.constant_bit32(ctx.type_int, value as u32);
    ctx.int_constants.insert(value, w);
    w
}

fn lower_expr(ctx: &mut Ctx, expr: &Expr) -> Result<Word, LoweringError> {
    match expr {
        Expr::FloatLit { value, .. } => {
            Ok(ctx.b.constant_bit32(ctx.type_float, (*value as f32).to_bits()))
        },
        Expr::IntLit { value, .. } => {
            // Promote to float at the SPIR-V seam. ESSL's vec_n
            // constructors accept int args and coerce; the lowering
            // bakes the coercion in at the boundary.
            Ok(ctx.b.constant_bit32(ctx.type_float, (*value as f32).to_bits()))
        },
        Expr::Ident { name, .. } => {
            if let Some(binding) = ctx.inputs.get(name) {
                let pointee = binding.pointee_type;
                let var = binding.var;
                return ctx
                    .b
                    .load(pointee, None, var, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if let Some(binding) = ctx.uniforms.get(name) {
                let pointee = binding.pointee_type;
                let member_idx = binding.member_index as i32;
                let block_var = ctx.uniform_block_var.ok_or_else(|| {
                    LoweringError::SpirvBuild(
                        "uniform binding present without block variable".into(),
                    )
                })?;
                let idx_const = int_constant(ctx, member_idx);
                let ptr_ty = ctx.b.type_pointer(None, StorageClass::Uniform, pointee);
                let access = ctx
                    .b
                    .access_chain(ptr_ty, None, block_var, [idx_const])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                return ctx
                    .b
                    .load(pointee, None, access, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            Err(LoweringError::UnsupportedShape {
                what: format!("identifier `{name}` is not a registered input or uniform"),
            })
        },
        Expr::Binary { op, lhs, rhs, span } => lower_binary(ctx, *op, lhs, rhs, *span),
        Expr::Call { callee, args, .. } => {
            let (result_ty, component_count) = match callee.as_str() {
                "vec2" => (ctx.type_vec2, 2usize),
                "vec3" => (ctx.type_vec3, 3usize),
                "vec4" => (ctx.type_vec4, 4usize),
                other => {
                    return Err(LoweringError::UnsupportedShape {
                        what: format!("call `{other}` is not lowered yet"),
                    });
                },
            };
            // ESSL `vec_n(s)` with a single scalar broadcasts to all
            // components. SPIR-V's OpCompositeConstruct requires exactly
            // n constituents, so we lower once and replicate.
            if args.len() == 1 {
                let single_kind = classify_arg_kind(ctx, &args[0]);
                if single_kind == Some(TypeKind::Float)
                    || matches!(&args[0], Expr::IntLit { .. })
                {
                    let v = lower_expr(ctx, &args[0])?;
                    let constituents: Vec<Word> = std::iter::repeat(v).take(component_count).collect();
                    return ctx
                        .b
                        .composite_construct(result_ty, None, constituents)
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
                }
                // Vector copy / truncation paths (vec3(vec4)) defer.
                return Err(LoweringError::UnsupportedShape {
                    what: format!("single-arg `{callee}(...)` with non-scalar arg is not lowered"),
                });
            }
            let mut constituents = Vec::with_capacity(args.len());
            for a in args {
                constituents.push(lower_expr(ctx, a)?);
            }
            ctx.b
                .composite_construct(result_ty, None, constituents)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
        },
        other => Err(LoweringError::UnsupportedShape {
            what: format!("expression shape not lowered: {other:?}"),
        }),
    }
}

fn lower_binary(
    ctx: &mut Ctx,
    op: BinOp,
    lhs: &Expr,
    rhs: &Expr,
    span: Span,
) -> Result<Word, LoweringError> {
    let lhs_kind =
        classify_arg_kind(ctx, lhs).ok_or_else(|| LoweringError::UnsupportedShape {
            what: format!("could not classify lhs of binary `{op:?}`"),
        })?;
    let rhs_kind =
        classify_arg_kind(ctx, rhs).ok_or_else(|| LoweringError::UnsupportedShape {
            what: format!("could not classify rhs of binary `{op:?}`"),
        })?;
    let result_kind = ctx.types.get(&span).copied().or_else(|| {
        // Fall back to a structural rule if typecheck did not annotate
        // (e.g. when no diagnostics were emitted but the span did not
        // make it into the types map). Conservative.
        match (op, lhs_kind, rhs_kind) {
            (BinOp::Mul | BinOp::Div, TypeKind::Float, k) | (BinOp::Mul | BinOp::Div, k, TypeKind::Float)
                if matches!(k, TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4) =>
            {
                Some(k)
            },
            _ if lhs_kind == rhs_kind => Some(lhs_kind),
            _ => None,
        }
    });
    let result_kind = result_kind.ok_or_else(|| LoweringError::UnsupportedShape {
        what: format!("could not infer result type for `{lhs_kind:?} {op:?} {rhs_kind:?}`"),
    })?;
    let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("result type {result_kind:?} not representable in SPIR-V emitter"),
        }
    })?;

    let lhs_id = lower_expr(ctx, lhs)?;
    let rhs_id = lower_expr(ctx, rhs)?;

    // Dispatch on the operand type pair. ESSL 3.00 integer ops are
    // queued; today's matrix is float-family only.
    let scalar_lhs = matches!(lhs_kind, TypeKind::Float);
    let scalar_rhs = matches!(rhs_kind, TypeKind::Float);
    let vec_lhs = matches!(lhs_kind, TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4);
    let vec_rhs = matches!(rhs_kind, TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4);

    match op {
        BinOp::Add | BinOp::Sub => {
            if (scalar_lhs && scalar_rhs) || (vec_lhs && vec_rhs && lhs_kind == rhs_kind) {
                let r = if op == BinOp::Add {
                    ctx.b.f_add(result_ty, None, lhs_id, rhs_id)
                } else {
                    ctx.b.f_sub(result_ty, None, lhs_id, rhs_id)
                };
                return r.map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
        },
        BinOp::Mul => {
            if scalar_lhs && scalar_rhs {
                return ctx
                    .b
                    .f_mul(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if vec_lhs && vec_rhs && lhs_kind == rhs_kind {
                return ctx
                    .b
                    .f_mul(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if vec_lhs && scalar_rhs {
                return ctx
                    .b
                    .vector_times_scalar(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if scalar_lhs && vec_rhs {
                // OpVectorTimesScalar wants (vec, scalar) order; swap.
                return ctx
                    .b
                    .vector_times_scalar(result_ty, None, rhs_id, lhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
        },
        BinOp::Div => {
            if scalar_lhs && scalar_rhs {
                return ctx
                    .b
                    .f_div(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if vec_lhs && vec_rhs && lhs_kind == rhs_kind {
                return ctx
                    .b
                    .f_div(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
        },
        _ => {},
    }
    Err(LoweringError::UnsupportedShape {
        what: format!("binary `{op:?}` on {lhs_kind:?} and {rhs_kind:?} not lowered"),
    })
}
