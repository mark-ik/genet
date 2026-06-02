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

use crate::ast::{Expr, ExternalDecl, FunctionDef, StorageQualifier, Stmt, TranslationUnit, TypeKind};
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
    let spirv = build_spirv(tu, stage)?;
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

struct Ctx {
    b: Builder,
    type_float: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
    /// ESSL identifier name -> Input variable binding. Today this
    /// holds vertex attributes; fragment varyings would be added the
    /// same way under the Fragment stage.
    inputs: HashMap<String, InputBinding>,
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

fn build_spirv(tu: &TranslationUnit, stage: ShaderStage) -> Result<Module, LoweringError> {
    let main = find_main(tu)?;
    let output_expr = find_output_assign(main, stage)?;

    let mut b = Builder::new();
    b.set_version(1, 0);
    b.capability(Capability::Shader);
    b.memory_model(AddressingModel::Logical, MemoryModel::GLSL450);

    let type_void = b.type_void();
    let type_float = b.type_float(32, None);
    let type_vec2 = b.type_vector(type_float, 2);
    let type_vec3 = b.type_vector(type_float, 3);
    let type_vec4 = b.type_vector(type_float, 4);

    // Register input variables (attributes in vertex stage).
    let inputs = register_inputs(&mut b, tu, stage, type_float, type_vec2, type_vec3, type_vec4);

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

    let mut ctx = Ctx { b, type_float, type_vec2, type_vec3, type_vec4, inputs };
    let value_id = lower_expr(&mut ctx, output_expr)?;
    ctx.b
        .store(output_var, value_id, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b.ret().map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b.end_function().map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // Entry point: interface includes the output plus every input the
    // shader actually exposes.
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
/// type. Used by the single-arg vector-constructor broadcast path to
/// distinguish `vec3(0.0)` (broadcast scalar) from `vec3(vec4(...))`
/// (truncation, not yet lowered).
fn classify_arg_kind(ctx: &Ctx, e: &Expr) -> Option<TypeKind> {
    match e {
        Expr::FloatLit { .. } | Expr::IntLit { .. } => Some(TypeKind::Float),
        Expr::Ident { name, .. } => ctx.inputs.get(name).map(|b| b.kind),
        Expr::Call { callee, .. } => match callee.as_str() {
            "vec2" => Some(TypeKind::Vec2),
            "vec3" => Some(TypeKind::Vec3),
            "vec4" => Some(TypeKind::Vec4),
            _ => None,
        },
        _ => None,
    }
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
            let binding = ctx.inputs.get(name).ok_or_else(|| LoweringError::UnsupportedShape {
                what: format!("identifier `{name}` is not a registered input"),
            })?;
            let pointee = binding.pointee_type;
            let var = binding.var;
            ctx.b
                .load(pointee, None, var, None, [])
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
        },
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
