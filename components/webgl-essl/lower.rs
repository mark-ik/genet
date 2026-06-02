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
    /// Failed to spawn the 8 MB-stack worker thread for naga work.
    /// Effectively only possible under OS resource exhaustion.
    ThreadSpawn(String),
    /// The worker thread panicked before signaling completion. The
    /// payload, if it was a string, is captured.
    ThreadJoin(String),
    /// naga's SPIR-V frontend or validator panicked. Caught via
    /// `std::panic::catch_unwind`; mirrors ANGLE / mozangle's
    /// `catch_unwind` posture on the GLSL→SPIR-V path. naga's
    /// recursive validator and a few WGSL-emit paths can throw on
    /// malformed intermediate IR; for adversarial input this boundary
    /// is load-bearing rather than just defensive.
    NagaPanic(String),
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
            LoweringError::ThreadSpawn(m) => write!(f, "failed to spawn naga worker thread: {m}"),
            LoweringError::ThreadJoin(m) => write!(f, "naga worker thread panicked at join: {m}"),
            LoweringError::NagaPanic(m) => write!(f, "naga panicked: {m}"),
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

    naga_pipeline(bytes)
}

/// Run naga's `spv-in` parser, validator, and WGSL emitter inside an
/// 8 MB-stack worker thread, capturing any panic via
/// `std::panic::catch_unwind`. Mirrors ANGLE's hardening posture on
/// the GLSL → SPIR-V path: naga's recursive validator can overflow
/// Windows' default 1 MB stack on deeply nested IR, and a few WGSL
/// emit paths can panic on malformed intermediate IR. For adversarial
/// shader input the boundary is load-bearing, not just defensive.
fn naga_pipeline(bytes: Vec<u8>) -> Result<String, LoweringError> {
    let join_result = std::thread::Builder::new()
        .name("webgl-essl-naga".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_naga(&bytes))))
        .map_err(|e| LoweringError::ThreadSpawn(format!("{e}")))?
        .join()
        .map_err(|e| LoweringError::ThreadJoin(format!("{e:?}")))?;

    match join_result {
        Ok(r) => r,
        Err(payload) => Err(LoweringError::NagaPanic(panic_payload_msg(payload))),
    }
}

fn run_naga(bytes: &[u8]) -> Result<String, LoweringError> {
    let module = naga::front::spv::parse_u8_slice(bytes, &naga::front::spv::Options::default())
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

fn panic_payload_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "naga panicked with non-string payload".to_string()
    }
}

#[cfg(test)]
mod safety_boundary_tests {
    use super::*;

    #[test]
    fn panic_payload_msg_extracts_str_slice() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("oops");
        assert_eq!(panic_payload_msg(payload), "oops");
    }

    #[test]
    fn panic_payload_msg_extracts_string() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(String::from("longer message"));
        assert_eq!(panic_payload_msg(payload), "longer message");
    }

    #[test]
    fn panic_payload_msg_falls_back_on_non_string_payload() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(42i64);
        let msg = panic_payload_msg(payload);
        assert!(msg.contains("non-string"), "got: {msg}");
    }

    #[test]
    fn worker_thread_returns_inner_ok_unchanged() {
        // Verify the thread + catch_unwind wrapper is transparent for
        // happy-path bytes. Build a tiny SPIR-V module by hand (the
        // canonical const-color fragment skeleton) and feed it
        // through `naga_pipeline`.
        let mut b = Builder::new();
        b.set_version(1, 0);
        b.capability(Capability::Shader);
        b.memory_model(AddressingModel::Logical, MemoryModel::GLSL450);
        let type_void = b.type_void();
        let type_float = b.type_float(32, None);
        let type_vec4 = b.type_vector(type_float, 4);
        let ptr_output = b.type_pointer(None, StorageClass::Output, type_vec4);
        let output_var = b.variable(ptr_output, None, StorageClass::Output, None);
        b.decorate(output_var, Decoration::Location, [Operand::LiteralBit32(0)]);
        let c1 = b.constant_bit32(type_float, 1.0f32.to_bits());
        let color = b.constant_composite(type_vec4, [c1, c1, c1, c1]);
        let fn_type = b.type_function(type_void, []);
        let main_fn = b
            .begin_function(type_void, None, FunctionControl::NONE, fn_type)
            .unwrap();
        b.begin_block(None).unwrap();
        b.store(output_var, color, None, []).unwrap();
        b.ret().unwrap();
        b.end_function().unwrap();
        b.entry_point(ExecutionModel::Fragment, main_fn, "main", [output_var]);
        b.execution_mode(main_fn, ExecutionMode::OriginUpperLeft, []);
        let words = b.module().assemble();
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let wgsl = naga_pipeline(bytes).expect("naga_pipeline round-trip");
        assert!(wgsl.contains("@fragment"));
    }

    #[test]
    fn worker_thread_reports_naga_parse_error_for_garbage_bytes() {
        // Bytes that are not valid SPIR-V should fail in `run_naga`
        // at the parse step — not panic. The boundary still
        // propagates the typed error rather than swallowing it.
        let bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03];
        let result = naga_pipeline(bytes);
        match result {
            Err(LoweringError::NagaParse(_)) => {},
            other => panic!("expected NagaParse, got {other:?}"),
        }
    }
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


// ---------- SPIR-V emission -------------------------------------------

struct Ctx<'a> {
    b: Builder,
    type_float: Word,
    type_int: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
    type_mat2: Word,
    type_mat3: Word,
    type_mat4: Word,
    /// ESSL identifier name -> Input variable binding. Vertex
    /// attributes always live here; fragment varyings join when
    /// stage == Fragment.
    inputs: HashMap<String, InputBinding>,
    /// ESSL identifier name -> Output variable binding. Vertex
    /// varyings live here when stage == Vertex; fragment shaders use
    /// the single primary output (gl_FragColor) directly.
    outputs: HashMap<String, OutputBinding>,
    /// ESSL uniform name -> the member index of that uniform inside
    /// the per-shader uniform block. Empty when the shader declares
    /// no uniforms.
    uniforms: HashMap<String, UniformBinding>,
    /// SPIR-V Word for the uniform-block OpVariable (the single struct
    /// holding all uniforms). `None` when the shader has no uniforms.
    uniform_block_var: Option<Word>,
    /// Cached `OpConstant int <i>` words for OpAccessChain indices.
    int_constants: HashMap<i32, Word>,
    /// User-defined function name -> SPIR-V function id + signature.
    /// Populated by the pre-pass before main is lowered.
    user_fns: HashMap<String, UserFnBinding>,
    /// While lowering a user function's body, parameter names map to
    /// their OpFunctionParameter Words. Cleared on function exit.
    fn_params: HashMap<String, FnParamBinding>,
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

struct OutputBinding {
    var: Word,
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

struct UserFnBinding {
    /// SPIR-V Word for the OpFunction (the callable id).
    func_id: Word,
    /// Parameter types in source order.
    param_types: Vec<TypeKind>,
    /// Return type.
    result: TypeKind,
}

struct FnParamBinding {
    /// SPIR-V Word for the OpFunctionParameter (the SSA value, not a
    /// pointer; function-parameter storage is Function class).
    value_id: Word,
    /// ESSL value type.
    kind: TypeKind,
}

fn build_spirv(
    tu: &TranslationUnit,
    stage: ShaderStage,
    types: &HashMap<Span, TypeKind>,
) -> Result<Module, LoweringError> {
    let main = find_main(tu)?;

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
    let type_mat2 = b.type_matrix(type_vec2, 2);
    let type_mat3 = b.type_matrix(type_vec3, 3);
    let type_mat4 = b.type_matrix(type_vec4, 4);

    // Inputs: vertex attributes always; fragment varyings under
    // ShaderStage::Fragment.
    let inputs = register_inputs(&mut b, tu, stage, type_float, type_vec2, type_vec3, type_vec4);

    // Outputs: under ShaderStage::Vertex, varyings become Output
    // variables decorated with sequential Locations. Fragment shaders
    // use the single gl_FragColor output and don't register varyings
    // here.
    let outputs = register_varying_outputs(
        &mut b,
        tu,
        stage,
        type_float,
        type_vec2,
        type_vec3,
        type_vec4,
    );

    // Uniforms wrapped in a single Block-decorated struct.
    let (uniforms, uniform_block_var) = register_uniforms(
        &mut b,
        tu,
        type_float,
        type_vec2,
        type_vec3,
        type_vec4,
        type_mat2,
        type_mat3,
        type_mat4,
    );

    // Primary output variable (gl_FragColor / gl_Position; always
    // vec4 in the cases this module handles).
    let ptr_output = b.type_pointer(None, StorageClass::Output, type_vec4);
    let primary_output = b.variable(ptr_output, None, StorageClass::Output, None);
    match stage {
        ShaderStage::Vertex => {
            b.decorate(primary_output, Decoration::BuiltIn, [Operand::BuiltIn(BuiltIn::Position)]);
        },
        ShaderStage::Fragment => {
            b.decorate(primary_output, Decoration::Location, [Operand::LiteralBit32(0)]);
        },
    }

    let mut ctx = Ctx {
        b,
        type_float,
        type_int,
        type_vec2,
        type_vec3,
        type_vec4,
        type_mat2,
        type_mat3,
        type_mat4,
        inputs,
        outputs,
        uniforms,
        uniform_block_var,
        int_constants: HashMap::new(),
        user_fns: HashMap::new(),
        fn_params: HashMap::new(),
        types,
    };

    // Emit user function definitions before main, so OpFunctionCall
    // in main can reference them by id. ESSL allows forward
    // references; this matches that ordering.
    emit_user_functions(&mut ctx, tu)?;

    // void main() { ... }
    let fn_type = ctx.b.type_function(type_void, []);
    let main_fn = ctx
        .b
        .begin_function(type_void, None, FunctionControl::NONE, fn_type)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b.begin_block(None).map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    lower_main_body(&mut ctx, main, stage, primary_output)?;
    ctx.b.ret().map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b.end_function().map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // Entry-point interface: every input and every output variable
    // the shader exposes. Uniforms are bound via DescriptorSet and
    // are not in the interface list.
    let execution_model = match stage {
        ShaderStage::Vertex => ExecutionModel::Vertex,
        ShaderStage::Fragment => ExecutionModel::Fragment,
    };
    let mut interface: Vec<Word> = ctx.inputs.values().map(|b| b.var).collect();
    interface.extend(ctx.outputs.values().map(|b| b.var));
    interface.push(primary_output);
    ctx.b.entry_point(execution_model, main_fn, "main", interface);
    if stage == ShaderStage::Fragment {
        ctx.b.execution_mode(main_fn, ExecutionMode::OriginUpperLeft, []);
    }

    Ok(ctx.b.module())
}

fn register_varying_outputs(
    b: &mut Builder,
    tu: &TranslationUnit,
    stage: ShaderStage,
    type_float: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
) -> HashMap<String, OutputBinding> {
    let mut outputs = HashMap::new();
    if stage != ShaderStage::Vertex {
        // Fragment varyings register as inputs; user-defined
        // fragment outputs are queued (ESSL 3.00).
        return outputs;
    }
    // Start Location at 1 — Location 0 is reserved for the primary
    // gl_Position output decoration on the BuiltIn slot, but
    // SPIR-V Output Locations are an independent set from
    // Input Locations, so we count from 0 for the varying outputs.
    let mut location: u32 = 0;
    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        if g.storage != StorageQualifier::Varying {
            continue;
        }
        let (pointee_type, kind) = match g.ty.kind {
            TypeKind::Float => (type_float, TypeKind::Float),
            TypeKind::Vec2 => (type_vec2, TypeKind::Vec2),
            TypeKind::Vec3 => (type_vec3, TypeKind::Vec3),
            TypeKind::Vec4 => (type_vec4, TypeKind::Vec4),
            _ => continue,
        };
        let ptr_ty = b.type_pointer(None, StorageClass::Output, pointee_type);
        let var = b.variable(ptr_ty, None, StorageClass::Output, None);
        b.decorate(var, Decoration::Location, [Operand::LiteralBit32(location)]);
        location += 1;
        outputs.insert(g.name.clone(), OutputBinding { var, kind });
    }
    outputs
}

fn lower_main_body(
    ctx: &mut Ctx,
    main: &FunctionDef,
    stage: ShaderStage,
    primary_output: Word,
) -> Result<(), LoweringError> {
    let primary_name = match stage {
        ShaderStage::Vertex => "gl_Position",
        ShaderStage::Fragment => "gl_FragColor",
    };
    for stmt in &main.body.stmts {
        match stmt {
            Stmt::Expr(Expr::Assign { lhs, rhs, .. }) => {
                let target_name = match lhs.as_ref() {
                    Expr::Ident { name, .. } => name.as_str(),
                    _ => {
                        return Err(LoweringError::UnsupportedShape {
                            what: "main body lhs is not an identifier".into(),
                        });
                    },
                };
                let value = lower_expr(ctx, rhs)?;
                if target_name == primary_name {
                    ctx.b
                        .store(primary_output, value, None, [])
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                } else if let Some(out) = ctx.outputs.get(target_name) {
                    let var = out.var;
                    ctx.b
                        .store(var, value, None, [])
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                } else {
                    return Err(LoweringError::UnsupportedShape {
                        what: format!(
                            "main body assigns to `{target_name}`, expected `{primary_name}` for {stage:?}"
                        ),
                    });
                }
            },
            Stmt::Expr(Expr::Call { .. }) => {
                // Discarded call result (`helper();`). Lower the
                // call for its side effects.
                if let Stmt::Expr(call_expr) = stmt {
                    let _ = lower_expr(ctx, call_expr)?;
                }
            },
            _ => {
                return Err(LoweringError::UnsupportedShape {
                    what: "main body statement is not yet lowered".into(),
                });
            },
        }
    }
    Ok(())
}

fn emit_user_functions(ctx: &mut Ctx, tu: &TranslationUnit) -> Result<(), LoweringError> {
    for d in &tu.decls {
        let ExternalDecl::Function(f) = d else { continue };
        if f.name == "main" {
            continue;
        }
        emit_user_function(ctx, f)?;
    }
    Ok(())
}

fn emit_user_function(ctx: &mut Ctx, f: &FunctionDef) -> Result<(), LoweringError> {
    let return_kind = f.return_ty.kind;
    let return_ty = match spv_type_for_kind(ctx, return_kind) {
        Some(t) => t,
        None => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("function `{}` return type {return_kind:?} is not lowered", f.name),
            });
        },
    };
    // Parameter types must be representable in SPIR-V (no samplers /
    // structs / matrices yet — float, vec_n only for first widening).
    let mut param_types_spv: Vec<Word> = Vec::new();
    let mut param_kinds: Vec<TypeKind> = Vec::new();
    for p in &f.params {
        let pt = match spv_type_for_kind(ctx, p.ty.kind) {
            Some(t) => t,
            None => {
                return Err(LoweringError::UnsupportedShape {
                    what: format!(
                        "function `{}` parameter `{}` type {:?} is not lowered",
                        f.name, p.name, p.ty.kind,
                    ),
                });
            },
        };
        param_types_spv.push(pt);
        param_kinds.push(p.ty.kind);
    }

    let fn_type = ctx.b.type_function(return_ty, param_types_spv.clone());
    let func_id = ctx
        .b
        .begin_function(return_ty, None, FunctionControl::NONE, fn_type)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // OpFunctionParameter for each param, recording the SSA value id
    // by name so the body can reference them.
    let mut fn_params: HashMap<String, FnParamBinding> = HashMap::new();
    for (p, pt) in f.params.iter().zip(param_types_spv.iter()) {
        let pid = ctx
            .b
            .function_parameter(*pt)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        fn_params.insert(p.name.clone(), FnParamBinding { value_id: pid, kind: p.ty.kind });
    }

    ctx.b
        .begin_block(None)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // Restrict to function bodies of the shape `return <expr>;` so
    // the lowering stays simple. Multi-statement function bodies
    // (locals, control flow) are queued.
    let return_value = match f.body.stmts.as_slice() {
        [Stmt::Return { value: Some(e), .. }] => e,
        [] if return_kind == TypeKind::Void => {
            // void f() {} — no return needed.
            ctx.b.ret().map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            ctx.b
                .end_function()
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            ctx.user_fns.insert(
                f.name.clone(),
                UserFnBinding { func_id, param_types: param_kinds, result: return_kind },
            );
            return Ok(());
        },
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: format!(
                    "function `{}` body shape not lowered (only `return <expr>;` is supported)",
                    f.name
                ),
            });
        },
    };
    // Push the function parameters into the Ctx scope, lower the
    // return expression, emit OpReturnValue, then pop.
    let saved_params = std::mem::take(&mut ctx.fn_params);
    ctx.fn_params = fn_params;
    let value = lower_expr(ctx, return_value)?;
    ctx.fn_params = saved_params;
    ctx.b
        .ret_value(value)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b
        .end_function()
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    ctx.user_fns.insert(
        f.name.clone(),
        UserFnBinding { func_id, param_types: param_kinds, result: return_kind },
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn register_uniforms(
    b: &mut Builder,
    tu: &TranslationUnit,
    type_float: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
    type_mat2: Word,
    type_mat3: Word,
    type_mat4: Word,
) -> (HashMap<String, UniformBinding>, Option<Word>) {
    let mut uniforms: Vec<(String, TypeKind, Word, u32, Option<u32>)> = Vec::new();
    let mut offset: u32 = 0;
    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        if g.storage != StorageQualifier::Uniform {
            continue;
        }
        // (pointee_type, kind, size, matrix_stride). matrix_stride
        // is Some only for matrix types and matches the natural
        // column-vector size (mat2 = 8, mat3 = 16, mat4 = 16). naga's
        // spv-in validates the stride matches the column dimension;
        // padding wider than that fails as UnsupportedMatrixStride.
        let (pointee_type, kind, size, matrix_stride) = match g.ty.kind {
            TypeKind::Float => (type_float, TypeKind::Float, 4u32, None),
            TypeKind::Vec2 => (type_vec2, TypeKind::Vec2, 8u32, None),
            TypeKind::Vec3 => (type_vec3, TypeKind::Vec3, 16u32, None),
            TypeKind::Vec4 => (type_vec4, TypeKind::Vec4, 16u32, None),
            TypeKind::Mat2 => (type_mat2, TypeKind::Mat2, 16u32, Some(8u32)),
            TypeKind::Mat3 => (type_mat3, TypeKind::Mat3, 48u32, Some(16u32)),
            TypeKind::Mat4 => (type_mat4, TypeKind::Mat4, 64u32, Some(16u32)),
            // Sampler uniforms are queued (different storage class).
            _ => continue,
        };
        let align = match (kind, matrix_stride) {
            (_, Some(s)) => s,
            (TypeKind::Vec3 | TypeKind::Vec4, _) => 16,
            (TypeKind::Vec2, _) => 8,
            _ => 4,
        };
        offset = (offset + align - 1) / align * align;
        uniforms.push((g.name.clone(), kind, pointee_type, offset, matrix_stride));
        offset += size;
    }
    if uniforms.is_empty() {
        return (HashMap::new(), None);
    }
    let member_types: Vec<Word> = uniforms.iter().map(|(_, _, ty, _, _)| *ty).collect();
    let struct_ty = b.type_struct(member_types);
    b.decorate(struct_ty, Decoration::Block, []);
    for (i, (_, _, _, off, matrix_stride)) in uniforms.iter().enumerate() {
        b.member_decorate(
            struct_ty,
            i as u32,
            Decoration::Offset,
            [Operand::LiteralBit32(*off)],
        );
        if let Some(stride) = *matrix_stride {
            // Column-major storage with the natural column-vector
            // stride per matrix size (8 for mat2, 16 for mat3 / mat4).
            // naga's spv-in rejects strides that do not match the
            // column dimension.
            b.member_decorate(struct_ty, i as u32, Decoration::ColMajor, []);
            b.member_decorate(
                struct_ty,
                i as u32,
                Decoration::MatrixStride,
                [Operand::LiteralBit32(stride)],
            );
        }
    }
    let ptr_uniform = b.type_pointer(None, StorageClass::Uniform, struct_ty);
    let var = b.variable(ptr_uniform, None, StorageClass::Uniform, None);
    b.decorate(var, Decoration::DescriptorSet, [Operand::LiteralBit32(0)]);
    b.decorate(var, Decoration::Binding, [Operand::LiteralBit32(0)]);
    let mut map = HashMap::new();
    for (i, (name, kind, pointee, _, _)) in uniforms.into_iter().enumerate() {
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
    let mut location: u32 = 0;
    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        // Per-stage input filter:
        //   Vertex stage: `attribute` (ESSL 1.00) + `in` (ESSL 3.00)
        //   Fragment stage: `varying` (ESSL 1.00) + `in` (ESSL 3.00)
        let is_input = match stage {
            ShaderStage::Vertex => matches!(
                g.storage,
                StorageQualifier::Attribute | StorageQualifier::In
            ),
            ShaderStage::Fragment => matches!(
                g.storage,
                StorageQualifier::Varying | StorageQualifier::In
            ),
        };
        if !is_input {
            continue;
        }
        let (pointee_type, kind) = match g.ty.kind {
            TypeKind::Float => (type_float, TypeKind::Float),
            TypeKind::Vec2 => (type_vec2, TypeKind::Vec2),
            TypeKind::Vec3 => (type_vec3, TypeKind::Vec3),
            TypeKind::Vec4 => (type_vec4, TypeKind::Vec4),
            // Other input types (int / ivec / mat) are not exercised
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
        TypeKind::Mat2 => ctx.type_mat2,
        TypeKind::Mat3 => ctx.type_mat3,
        TypeKind::Mat4 => ctx.type_mat4,
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
            // Function-parameter SSA values shadow everything else
            // while lowering a user function body.
            if let Some(p) = ctx.fn_params.get(name) {
                return Ok(p.value_id);
            }
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
                what: format!("identifier `{name}` is not a registered input, uniform, or function parameter"),
            })
        },
        Expr::Binary { op, lhs, rhs, span } => lower_binary(ctx, *op, lhs, rhs, *span),
        Expr::Call { callee, args, .. } => {
            // User-defined function calls dispatch to OpFunctionCall.
            // Constructor calls (vec2 / vec3 / vec4) flow through the
            // composite-construct path below.
            if let Some(user_fn) = ctx.user_fns.get(callee) {
                let func_id = user_fn.func_id;
                let result_kind = user_fn.result;
                let expected_arity = user_fn.param_types.len();
                if args.len() != expected_arity {
                    return Err(LoweringError::UnsupportedShape {
                        what: format!(
                            "call `{callee}` has {} arg(s) but takes {expected_arity}",
                            args.len()
                        ),
                    });
                }
                let mut arg_ids = Vec::with_capacity(args.len());
                for a in args {
                    arg_ids.push(lower_expr(ctx, a)?);
                }
                let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: format!("call `{callee}` returns {result_kind:?} which is not lowered"),
                    }
                })?;
                return ctx
                    .b
                    .function_call(result_ty, None, func_id, arg_ids)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            let (result_ty, component_count) = match callee.as_str() {
                "vec2" => (ctx.type_vec2, 2usize),
                "vec3" => (ctx.type_vec3, 3usize),
                "vec4" => (ctx.type_vec4, 4usize),
                other => {
                    return Err(LoweringError::UnsupportedShape {
                        what: format!("call `{other}` is not a constructor or registered user function"),
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
        Expr::Member { base, field, span, .. } => lower_swizzle(ctx, base, field, *span),
        other => Err(LoweringError::UnsupportedShape {
            what: format!("expression shape not lowered: {other:?}"),
        }),
    }
}

fn lower_swizzle(
    ctx: &mut Ctx,
    base: &Expr,
    field: &str,
    span: Span,
) -> Result<Word, LoweringError> {
    let base_id = lower_expr(ctx, base)?;
    let base_kind = classify_arg_kind(ctx, base).ok_or_else(|| {
        LoweringError::UnsupportedShape { what: "swizzle base type unknown".into() }
    })?;
    let base_size = match base_kind {
        TypeKind::Vec2 => 2u32,
        TypeKind::Vec3 => 3,
        TypeKind::Vec4 => 4,
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("swizzle on non-vector base type {base_kind:?}"),
            });
        },
    };
    let indices = parse_swizzle_indices(field, base_size).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("invalid swizzle `.{field}` on {base_kind:?}"),
        }
    })?;
    let result_kind = ctx.types.get(&span).copied().unwrap_or_else(|| {
        match indices.len() {
            1 => TypeKind::Float,
            2 => TypeKind::Vec2,
            3 => TypeKind::Vec3,
            4 => TypeKind::Vec4,
            _ => TypeKind::Float, // unreachable from parse_swizzle_indices
        }
    });
    let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("swizzle result type {result_kind:?} not lowered"),
        }
    })?;
    if indices.len() == 1 {
        // Single-component access: OpCompositeExtract result_ty base [idx].
        ctx.b
            .composite_extract(result_ty, None, base_id, [indices[0]])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
    } else {
        // Multi-component shuffle: OpVectorShuffle result_ty base
        // base [indices...]. We pass the same base as both operands
        // since we are picking from one vector.
        ctx.b
            .vector_shuffle(result_ty, None, base_id, base_id, indices)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
    }
}

fn parse_swizzle_indices(field: &str, base_size: u32) -> Option<Vec<u32>> {
    if field.is_empty() || field.len() > 4 {
        return None;
    }
    const SETS: [&[char]; 3] = [
        &['x', 'y', 'z', 'w'],
        &['r', 'g', 'b', 'a'],
        &['s', 't', 'p', 'q'],
    ];
    let chars: Vec<char> = field.chars().collect();
    let set = SETS.iter().find(|s| chars.iter().all(|c| s.contains(c)))?;
    let mut out = Vec::with_capacity(chars.len());
    for c in &chars {
        let idx = set.iter().position(|sc| sc == c)? as u32;
        if idx >= base_size {
            return None;
        }
        out.push(idx);
    }
    Some(out)
}

fn matches_mat_vec(lhs: TypeKind, rhs: TypeKind) -> bool {
    matches!(
        (lhs, rhs),
        (TypeKind::Mat4, TypeKind::Vec4)
            | (TypeKind::Mat3, TypeKind::Vec3)
            | (TypeKind::Mat2, TypeKind::Vec2)
    )
}

fn matches_vec_mat(lhs: TypeKind, rhs: TypeKind) -> bool {
    matches!(
        (lhs, rhs),
        (TypeKind::Vec4, TypeKind::Mat4)
            | (TypeKind::Vec3, TypeKind::Mat3)
            | (TypeKind::Vec2, TypeKind::Mat2)
    )
}

fn matches_mat_mat(lhs: TypeKind, rhs: TypeKind) -> bool {
    matches!(
        (lhs, rhs),
        (TypeKind::Mat2, TypeKind::Mat2)
            | (TypeKind::Mat3, TypeKind::Mat3)
            | (TypeKind::Mat4, TypeKind::Mat4)
    )
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
            (BinOp::Mul | BinOp::Div, TypeKind::Float, k)
            | (BinOp::Mul | BinOp::Div, k, TypeKind::Float)
                if matches!(k, TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4) =>
            {
                Some(k)
            },
            (BinOp::Mul, _, _) if matches_mat_vec(lhs_kind, rhs_kind) => Some(rhs_kind),
            (BinOp::Mul, _, _) if matches_vec_mat(lhs_kind, rhs_kind) => Some(lhs_kind),
            (BinOp::Mul, _, _) if matches_mat_mat(lhs_kind, rhs_kind) => Some(lhs_kind),
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
            // mat_n * vec_n -> OpMatrixTimesVector when dimensions
            // match (mat4 * vec4, mat3 * vec3, mat2 * vec2).
            if matches_mat_vec(lhs_kind, rhs_kind) {
                return ctx
                    .b
                    .matrix_times_vector(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // vec_n * mat_n -> OpVectorTimesMatrix (row-vector mul).
            if matches_vec_mat(lhs_kind, rhs_kind) {
                return ctx
                    .b
                    .vector_times_matrix(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // mat_n * mat_n (same n) -> OpMatrixTimesMatrix.
            if matches_mat_mat(lhs_kind, rhs_kind) {
                return ctx
                    .b
                    .matrix_times_matrix(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // mat_n * float -> OpMatrixTimesScalar.
            if matches!(lhs_kind, TypeKind::Mat2 | TypeKind::Mat3 | TypeKind::Mat4)
                && scalar_rhs
            {
                return ctx
                    .b
                    .matrix_times_scalar(result_ty, None, lhs_id, rhs_id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if scalar_lhs
                && matches!(rhs_kind, TypeKind::Mat2 | TypeKind::Mat3 | TypeKind::Mat4)
            {
                return ctx
                    .b
                    .matrix_times_scalar(result_ty, None, rhs_id, lhs_id)
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
