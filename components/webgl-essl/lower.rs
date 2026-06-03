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
    AddressingModel, BuiltIn, Capability, Decoration, Dim, ExecutionMode, ExecutionModel,
    FunctionControl, ImageFormat, ImageOperands, LoopControl, MemoryModel, SelectionControl,
    StorageClass, Word,
};

use crate::ast::{
    AssignOp, BinOp, Expr, ExternalDecl, ForInit, FunctionDef, StorageQualifier, Stmt,
    TranslationUnit, TypeKind, UnaryOp,
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
    if std::env::var("WEBGL_ESSL_DUMP_SPIRV").is_ok() {
        use rspirv::binary::Disassemble;
        eprintln!("--- SPIR-V ---\n{}\n", spirv.disassemble());
    }
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
    type_void: Word,
    type_float: Word,
    type_int: Word,
    type_bool: Word,
    type_bvec2: Word,
    type_bvec3: Word,
    type_bvec4: Word,
    type_ivec2: Word,
    type_ivec3: Word,
    type_ivec4: Word,
    type_image_2d: Word,
    type_image_cube: Word,
    type_sampler: Word,
    type_sampled_image_2d: Word,
    type_sampled_image_cube: Word,
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
    /// ESSL sampler name -> its OpVariable in UniformConstant
    /// storage and the matching SPIR-V SampledImage type id.
    samplers: HashMap<String, SamplerBinding>,
    /// SPIR-V Word for the uniform-block OpVariable (the single struct
    /// holding all uniforms). `None` when the shader has no uniforms.
    uniform_block_var: Option<Word>,
    /// Cached `OpConstant int <i>` words for OpAccessChain indices.
    int_constants: HashMap<i32, Word>,
    /// User-defined function name -> list of overload bindings.
    /// Populated by the pre-pass before main is lowered. Each
    /// `Vec` entry is a distinct (name, param-kinds) signature so
    /// ESSL §6.1.1 overloads dispatch to the matching SPIR-V
    /// function id at the call site.
    user_fns: HashMap<String, Vec<UserFnBinding>>,
    /// While lowering a user function's body, parameter names map to
    /// their OpFunctionParameter Words. Cleared on function exit.
    fn_params: HashMap<String, FnParamBinding>,
    /// Function-scope local variables in the body currently being
    /// lowered. Keyed by the declaration's `Span` so two distinct
    /// `int i` declarations in nested loops each get their own
    /// `OpVariable` (and the runtime semantics stay correct under
    /// shadowing). The pre-pass populates this map; the main pass
    /// reads it via `lookup_local`. Cleared on function exit.
    locals: HashMap<Span, LocalBinding>,
    /// Stack of name resolution scopes. Each scope maps an
    /// identifier name to the `Span` of the declaration it resolves
    /// to in the current lexical context. Pushed at every Block /
    /// for-init / function entry; popped on exit. `lookup_local`
    /// walks inner-out so an inner shadow correctly hides an outer
    /// binding without aliasing its `OpVariable`.
    scope_stack: Vec<HashMap<String, Span>>,
    /// Cached `OpTypePointer Function T` ids, one per value type.
    /// Avoids emitting duplicate pointer type decls each time a
    /// local variable is allocated.
    function_ptr_types: HashMap<TypeKind, Word>,
    /// Stack of `break;` target labels. Each `emit_loop_cfg` pushes
    /// the loop's merge; each `Stmt::Switch` pushes its merge.
    /// `Stmt::Break` branches to the innermost entry.
    break_targets: Vec<Word>,
    /// Stack of `continue;` target labels. Pushed by `emit_loop_cfg`
    /// only — a switch does not push a continue target, so
    /// `continue;` inside a switch falls through to the enclosing
    /// loop's continue block (or errors if no loop encloses).
    continue_targets: Vec<Word>,
    /// Per-span type annotations from the typecheck pass; used to
    /// dispatch on operand types in binary-op lowering.
    types: &'a HashMap<Span, TypeKind>,
    /// SPIR-V Word for the `GLSL.std.450` extended instruction set
    /// import. Lazily allocated on the first built-in call that
    /// uses it, so trivial shaders that never call sin/cos/etc.
    /// don't carry the import.
    glsl_std_450: Option<Word>,
    /// User-defined struct types. Index matches
    /// [`TypeKind::Struct`] (and the parser's struct-index
    /// assignment). Each entry caches the `OpTypeStruct` id and
    /// a field map for `s.field` access lowering.
    struct_types: Vec<StructTypeInfo>,
    /// Struct tag name → its registry index. Lets `Foo(args)`
    /// constructor calls resolve in `lower_expr`'s `Call`
    /// branch in O(1).
    struct_name_to_idx: HashMap<String, u32>,
}

#[derive(Clone)]
struct StructTypeInfo {
    type_id: Word,
    /// Field name → (zero-based member index, ESSL type).
    fields: HashMap<String, (u32, TypeKind)>,
}

struct InputBinding {
    /// SPIR-V `OpVariable` Words for this input. A non-matrix
    /// input has one entry; a matrix input is column-split into
    /// N entries (one per column), each a vec_n Input variable
    /// at sequential Locations. The Ident-lookup site loads each
    /// column and composite-constructs the matrix.
    vars: Vec<Word>,
    /// SPIR-V type Word the `OpLoad` returns. For non-matrix
    /// inputs that's the value type; for matrix inputs it's the
    /// per-column vec_n type.
    pointee_type: Word,
    /// ESSL value type of the assembled binding. For a matrix
    /// input this is `Mat_n`, not the column type.
    kind: TypeKind,
}

#[derive(Clone)]
struct OutputBinding {
    /// SPIR-V `OpVariable` Words for this output. One entry for
    /// a non-matrix output; N entries for a column-split matrix
    /// output (one per column, sequential Locations).
    vars: Vec<Word>,
    /// ESSL value type of the assembled binding (matches the
    /// declared `varying`/`out` type).
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

#[derive(Clone, Copy)]
struct SamplerBinding {
    /// `OpVariable` in `UniformConstant` storage for the texture
    /// (an `OpTypeImage` pointee).
    image_var: Word,
    /// `OpVariable` in `UniformConstant` storage for the sampler
    /// state (an `OpTypeSampler` pointee).
    sampler_var: Word,
    /// SPIR-V `OpTypeImage` id — the pointee type of `image_var`.
    image_type: Word,
    /// SPIR-V `OpTypeSampledImage` id wrapping `image_type`. The
    /// `OpSampledImage` instruction at the call site produces a
    /// value of this type.
    sampled_image_type: Word,
    /// ESSL value type (`Sampler2D` or `SamplerCube`). Tracked
    /// so `texture2D` / `textureCube` dispatch can verify their
    /// first arg matches.
    kind: TypeKind,
}

#[derive(Clone)]
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

#[derive(Clone, Copy)]
struct LocalBinding {
    /// SPIR-V Word for the `OpVariable` allocating Function-scope
    /// storage for this local.
    var: Word,
    /// SPIR-V Word for the pointee (value) type. Used when emitting
    /// `OpLoad` to read the local.
    pointee_type: Word,
    /// ESSL value type. Mirror of `InputBinding::kind`. Used by
    /// LHS-swizzle lowering to dispatch on component count.
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
    let type_bool = b.type_bool();
    let type_bvec2 = b.type_vector(type_bool, 2);
    let type_bvec3 = b.type_vector(type_bool, 3);
    let type_bvec4 = b.type_vector(type_bool, 4);
    let type_ivec2 = b.type_vector(type_int, 2);
    let type_ivec3 = b.type_vector(type_int, 3);
    let type_ivec4 = b.type_vector(type_int, 4);
    let type_vec2 = b.type_vector(type_float, 2);
    let type_vec3 = b.type_vector(type_float, 3);
    let type_vec4 = b.type_vector(type_float, 4);
    let type_mat2 = b.type_matrix(type_vec2, 2);
    let type_mat3 = b.type_matrix(type_vec3, 3);
    let type_mat4 = b.type_matrix(type_vec4, 4);

    // Texture + sampler types. Naga's spv-in requires the
    // SPIR-V to declare separate `OpTypeImage` + `OpTypeSampler`
    // variables (Vulkan-style) and combine them at the call
    // site via `OpSampledImage`, rather than a single combined
    // `OpTypeSampledImage` variable.
    let type_image_2d = b.type_image(
        type_float,
        Dim::Dim2D,
        0,
        0,
        0,
        1,
        ImageFormat::Unknown,
        None,
    );
    let type_image_cube = b.type_image(
        type_float,
        Dim::DimCube,
        0,
        0,
        0,
        1,
        ImageFormat::Unknown,
        None,
    );
    let type_sampler = b.type_sampler();
    let type_sampled_image_2d = b.type_sampled_image(type_image_2d);
    let type_sampled_image_cube = b.type_sampled_image(type_image_cube);

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
        type_int,
        type_vec2,
        type_vec3,
        type_vec4,
        type_ivec2,
        type_ivec3,
        type_ivec4,
        type_mat2,
        type_mat3,
        type_mat4,
    );

    // Sampler uniforms live in their own `UniformConstant`
    // storage class — they cannot go inside the Block-decorated
    // struct. Each ESSL sampler becomes a pair of SPIR-V
    // variables (image + sampler) decorated with consecutive
    // bindings starting at 1 (Binding 0 is the uniform block
    // when present).
    let samplers = register_samplers(
        &mut b,
        tu,
        type_image_2d,
        type_image_cube,
        type_sampler,
    );

    // Detect ESSL 3.00 fragment shaders that declare their own
    // `out` outputs. In that mode the `gl_FragColor` builtin is
    // not used (the spec doesn't expose it) and the user's
    // `out` decls fill `@location(0)..` instead, so allocating
    // `gl_FragColor` would collide on Location 0.
    let has_user_fragment_outs = stage == ShaderStage::Fragment
        && tu.decls.iter().any(|d| {
            matches!(
                d,
                ExternalDecl::Global(g) if g.storage == StorageQualifier::Out
            )
        });

    // Primary output variable (gl_FragColor / gl_Position; always
    // vec4 in the cases this module handles). For ESSL 3.00
    // fragments with user-declared `out` outputs the primary is
    // skipped entirely.
    let primary_output: Option<Word> = if stage == ShaderStage::Vertex
        || !has_user_fragment_outs
    {
        let ptr_output = b.type_pointer(None, StorageClass::Output, type_vec4);
        let var = b.variable(ptr_output, None, StorageClass::Output, None);
        match stage {
            ShaderStage::Vertex => {
                b.decorate(var, Decoration::BuiltIn, [Operand::BuiltIn(BuiltIn::Position)]);
            },
            ShaderStage::Fragment => {
                b.decorate(var, Decoration::Location, [Operand::LiteralBit32(0)]);
            },
        }
        Some(var)
    } else {
        None
    };

    let mut ctx = Ctx {
        b,
        type_void,
        type_float,
        type_int,
        type_bool,
        type_bvec2,
        type_bvec3,
        type_bvec4,
        type_ivec2,
        type_ivec3,
        type_ivec4,
        type_image_2d,
        type_image_cube,
        type_sampler,
        type_sampled_image_2d,
        type_sampled_image_cube,
        type_vec2,
        type_vec3,
        type_vec4,
        type_mat2,
        type_mat3,
        type_mat4,
        inputs,
        outputs,
        uniforms,
        samplers,
        uniform_block_var,
        int_constants: HashMap::new(),
        user_fns: HashMap::new(),
        fn_params: HashMap::new(),
        locals: HashMap::new(),
        scope_stack: Vec::new(),
        function_ptr_types: HashMap::new(),
        break_targets: Vec::new(),
        continue_targets: Vec::new(),
        types,
        glsl_std_450: None,
        struct_types: Vec::new(),
        struct_name_to_idx: HashMap::new(),
    };

    // Allocate `OpTypeStruct` for each user struct, in source
    // order so the registry index matches the parser's
    // `TypeKind::Struct(i)` assignment. Fields fall back to
    // their member types via `spv_type_for_kind`.
    for d in &tu.decls {
        let ExternalDecl::Struct(s) = d else { continue };
        let mut field_types: Vec<Word> = Vec::with_capacity(s.fields.len());
        let mut fields: HashMap<String, (u32, TypeKind)> = HashMap::new();
        let mut ok = true;
        for (i, f) in s.fields.iter().enumerate() {
            match spv_type_for_kind(&ctx, f.ty.kind) {
                Some(ty) => {
                    field_types.push(ty);
                    fields.insert(f.name.clone(), (i as u32, f.ty.kind));
                },
                None => {
                    ok = false;
                    break;
                },
            }
        }
        if !ok {
            return Err(LoweringError::UnsupportedShape {
                what: format!(
                    "struct `{}` has an unlowered field type",
                    s.name.as_deref().unwrap_or("<anonymous>")
                ),
            });
        }
        let type_id = ctx.b.type_struct(field_types);
        let idx = ctx.struct_types.len() as u32;
        if let Some(n) = &s.name {
            ctx.struct_name_to_idx.insert(n.clone(), idx);
        }
        ctx.struct_types.push(StructTypeInfo { type_id, fields });
    }

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
    let mut interface: Vec<Word> = Vec::new();
    for b in ctx.inputs.values() {
        interface.extend(b.vars.iter().copied());
    }
    for b in ctx.outputs.values() {
        interface.extend(b.vars.iter().copied());
    }
    for s in ctx.samplers.values() {
        interface.push(s.image_var);
        interface.push(s.sampler_var);
    }
    if let Some(p) = primary_output {
        interface.push(p);
    }
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
    // Output storage qualifiers per stage:
    //   Vertex   stage: `varying` (ESSL 1.00) + `out` (ESSL 3.00).
    //   Fragment stage: `out` only (ESSL 3.00); `gl_FragColor`
    //                   is the implicit ESSL 1.00 output and is
    //                   handled separately as the primary.
    let mut location: u32 = 0;
    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        let qualifies = match stage {
            ShaderStage::Vertex => matches!(
                g.storage,
                StorageQualifier::Varying | StorageQualifier::Out
            ),
            ShaderStage::Fragment => g.storage == StorageQualifier::Out,
        };
        if !qualifies {
            continue;
        }
        let (column_type, column_count, kind) = match g.ty.kind {
            TypeKind::Float => (type_float, 1u32, TypeKind::Float),
            TypeKind::Vec2 => (type_vec2, 1, TypeKind::Vec2),
            TypeKind::Vec3 => (type_vec3, 1, TypeKind::Vec3),
            TypeKind::Vec4 => (type_vec4, 1, TypeKind::Vec4),
            TypeKind::Mat2 => (type_vec2, 2, TypeKind::Mat2),
            TypeKind::Mat3 => (type_vec3, 3, TypeKind::Mat3),
            TypeKind::Mat4 => (type_vec4, 4, TypeKind::Mat4),
            _ => continue,
        };
        let ptr_ty = b.type_pointer(None, StorageClass::Output, column_type);
        let mut vars = Vec::with_capacity(column_count as usize);
        for _ in 0..column_count {
            let var = b.variable(ptr_ty, None, StorageClass::Output, None);
            b.decorate(var, Decoration::Location, [Operand::LiteralBit32(location)]);
            location += 1;
            vars.push(var);
        }
        outputs.insert(g.name.clone(), OutputBinding { vars, kind });
    }
    outputs
}

/// Context describing main's special output target. Threaded
/// through `lower_stmt` so an assignment to `gl_Position` /
/// `gl_FragColor` resolves to `primary_output` instead of an
/// undefined identifier.
struct MainCtx {
    /// `(name, var)` of the implicit primary output:
    /// `("gl_Position", var)` for Vertex, `("gl_FragColor", var)`
    /// for Fragment under ESSL 1.00. `None` when the shader is
    /// an ESSL 3.00 fragment that declares its own `out`
    /// variables — assignments then route through `ctx.outputs`.
    primary: Option<(&'static str, Word)>,
    stage: ShaderStage,
}

fn lower_main_body(
    ctx: &mut Ctx,
    main: &FunctionDef,
    stage: ShaderStage,
    primary_output: Option<Word>,
) -> Result<(), LoweringError> {
    let main_ctx = MainCtx {
        primary: match (stage, primary_output) {
            (ShaderStage::Vertex, Some(v)) => Some(("gl_Position", v)),
            (ShaderStage::Fragment, Some(v)) => Some(("gl_FragColor", v)),
            _ => None,
        },
        stage,
    };
    // Hoist all locals declared anywhere in the body (including
    // nested If branches) into the entry block. SPIR-V requires
    // Function-storage OpVariables to live in the function's
    // first block.
    pre_allocate_locals(ctx, &main.body.stmts)?;
    // Push main's outermost lexical scope so top-level locals
    // declared directly in the function body (not in a nested
    // Block) have somewhere to register.
    ctx.scope_stack.push(HashMap::new());
    let mut walk_result = Ok(());
    for stmt in &main.body.stmts {
        if let Err(e) = lower_stmt(ctx, stmt, Some(&main_ctx)) {
            walk_result = Err(e);
            break;
        }
    }
    ctx.scope_stack.pop();
    walk_result
}

/// Pre-pass over a function body: walk every nested `Stmt::Decl`
/// (including those inside If branches, Block bodies, and For
/// inits) and allocate the matching `OpVariable` with Function
/// storage in the current (entry) block. Subsequent `lower_stmt`
/// calls then only emit the initializer `OpStore`.
fn pre_allocate_locals(ctx: &mut Ctx, stmts: &[Stmt]) -> Result<(), LoweringError> {
    for s in stmts {
        scan_stmt_for_decls(ctx, s)?;
    }
    Ok(())
}

fn scan_stmt_for_decls(ctx: &mut Ctx, s: &Stmt) -> Result<(), LoweringError> {
    match s {
        Stmt::Decl(d) => allocate_local(ctx, d),
        Stmt::Block(b) => pre_allocate_locals(ctx, &b.stmts),
        Stmt::If { then, else_, .. } => {
            scan_stmt_for_decls(ctx, then)?;
            if let Some(e) = else_ {
                scan_stmt_for_decls(ctx, e)?;
            }
            Ok(())
        },
        Stmt::While { body, .. } => scan_stmt_for_decls(ctx, body),
        Stmt::Do { body, .. } => scan_stmt_for_decls(ctx, body),
        Stmt::For { init, body, .. } => {
            if let ForInit::Decl(d) = init {
                allocate_local(ctx, d)?;
            }
            scan_stmt_for_decls(ctx, body)
        },
        _ => Ok(()),
    }
}

/// Allocate one `OpVariable` per source-level declaration. Keyed
/// by `d.span` (every Decl has a unique span), so two `int i`
/// declarations in nested scopes each get their own variable and
/// never alias.
fn allocate_local(ctx: &mut Ctx, d: &crate::ast::LocalDecl) -> Result<(), LoweringError> {
    if ctx.locals.contains_key(&d.span) {
        return Ok(());
    }
    let ptr_ty = function_ptr_for(ctx, d.ty.kind)?;
    let pointee_type = spv_type_for_kind(ctx, d.ty.kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("local `{}` type {:?} is not lowered", d.name, d.ty.kind),
        }
    })?;
    let var = ctx.b.variable(ptr_ty, None, StorageClass::Function, None);
    ctx.locals.insert(
        d.span,
        LocalBinding { var, pointee_type, kind: d.ty.kind },
    );
    Ok(())
}

/// Resolve `name` against the active lexical scope stack, walking
/// inner-out so a shadowing inner declaration hides any outer one.
fn lookup_local(ctx: &Ctx, name: &str) -> Option<LocalBinding> {
    for scope in ctx.scope_stack.iter().rev() {
        if let Some(span) = scope.get(name) {
            if let Some(binding) = ctx.locals.get(span).copied() {
                return Some(binding);
            }
        }
    }
    None
}

/// Lower a single statement. `main_ctx` is `Some` when lowering the
/// entry-point body, providing the special-case routing for
/// `gl_Position` / `gl_FragColor` writes. `None` when lowering a
/// user function body.
fn lower_stmt(
    ctx: &mut Ctx,
    stmt: &Stmt,
    main_ctx: Option<&MainCtx>,
) -> Result<(), LoweringError> {
    match stmt {
        Stmt::Decl(d) => {
            // Pre-pass allocated the OpVariable; register the
            // name -> decl-span mapping in the current lexical
            // scope so subsequent Ident lookups in this scope
            // resolve here, then emit the initializer OpStore.
            let binding = *ctx.locals.get(&d.span).ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: format!("local `{}` was not pre-allocated", d.name),
                }
            })?;
            ctx.scope_stack
                .last_mut()
                .ok_or_else(|| LoweringError::UnsupportedShape {
                    what: format!(
                        "local `{}` declared outside any lexical scope",
                        d.name
                    ),
                })?
                .insert(d.name.clone(), d.span);
            if let Some(init) = &d.init {
                let value = lower_expr(ctx, init)?;
                ctx.b
                    .store(binding.var, value, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            }
            Ok(())
        },
        Stmt::Expr(Expr::Assign { op, lhs, rhs, span }) => {
            // LHS matrix indexing: `m[i] op= vec_n`. Plain
            // assign stores directly to the column variable;
            // compound assigns load the column, fold in via
            // `compound_via_chain`, and store back.
            if let Expr::Index { base, index, .. } = lhs.as_ref() {
                if *op == AssignOp::Assign {
                    return lower_lhs_matrix_index_assignment(ctx, base, index, rhs);
                }
                let bin_op = compound_op_to_bin(op);
                return lower_lhs_matrix_index_compound_assignment(
                    ctx, base, index, bin_op, rhs,
                );
            }
            // LHS Member: struct base → field access via
            // OpAccessChain; vector base → swizzle. Compound
            // assigns (`s.x += rhs`, `v.x *= rhs`) use the same
            // chain through `compound_via_chain` to do
            // load-modify-store.
            if let Expr::Member { base, field, .. } = lhs.as_ref() {
                if let Some(TypeKind::Struct(idx)) = classify_arg_kind(ctx, base) {
                    if *op == AssignOp::Assign {
                        return lower_struct_field_assignment(ctx, base, field, idx, rhs);
                    }
                    let bin_op = compound_op_to_bin(op);
                    return lower_struct_field_compound_assignment(
                        ctx, base, field, idx, bin_op, rhs,
                    );
                }
                if *op == AssignOp::Assign {
                    return lower_lhs_swizzle_assignment(ctx, base, field, rhs, main_ctx);
                }
                let bin_op = compound_op_to_bin(op);
                return lower_lhs_swizzle_compound_assignment(
                    ctx, base, field, bin_op, rhs, main_ctx,
                );
            }
            // Compound assigns on an Ident LHS desugar to the
            // matching binary op then store back. ESSL §5.8 forbids
            // compound writes to write-only outputs / varyings, so
            // the target must be a function-scope local.
            if *op != AssignOp::Assign {
                let bin_op = match op {
                    AssignOp::AddAssign => BinOp::Add,
                    AssignOp::SubAssign => BinOp::Sub,
                    AssignOp::MulAssign => BinOp::Mul,
                    AssignOp::DivAssign => BinOp::Div,
                    AssignOp::Assign => unreachable!(),
                };
                let new_value = lower_binary(ctx, bin_op, lhs, rhs, *span)?;
                let name = match lhs.as_ref() {
                    Expr::Ident { name, .. } => name.as_str(),
                    _ => {
                        return Err(LoweringError::UnsupportedShape {
                            what: "compound assignment lhs must be an identifier".into(),
                        });
                    },
                };
                if let Some(local) = lookup_local(ctx, name) {
                    ctx.b
                        .store(local.var, new_value, None, [])
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                    return Ok(());
                }
                // Compound assign on a varying output (vertex
                // stage). The read step inside `lower_binary`
                // already loaded the current value via the
                // matrix-input assemble path; we now store the
                // new value back, splitting it across the
                // column variables for matrix outputs.
                if let Some(out) = ctx.outputs.get(name).cloned() {
                    return store_to_output(ctx, &out, name, new_value);
                }
                return Err(LoweringError::UnsupportedShape {
                    what: format!(
                        "compound assignment target `{name}` is not a writable local or varying"
                    ),
                });
            }
            let target_name = match lhs.as_ref() {
                Expr::Ident { name, .. } => name.as_str(),
                _ => {
                    return Err(LoweringError::UnsupportedShape {
                        what: "assignment lhs is not an identifier".into(),
                    });
                },
            };
            let value = lower_expr(ctx, rhs)?;
            // Locals shadow outputs and primary. Outputs and primary
            // only exist inside main.
            if let Some(local) = lookup_local(ctx, target_name) {
                ctx.b
                    .store(local.var, value, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                return Ok(());
            }
            if let Some(mc) = main_ctx {
                if let Some((pname, pvar)) = mc.primary {
                    if target_name == pname {
                        ctx.b
                            .store(pvar, value, None, [])
                            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                        return Ok(());
                    }
                }
                if let Some(out) = ctx.outputs.get(target_name).cloned() {
                    return store_to_output(ctx, &out, target_name, value);
                }
                let expected = mc
                    .primary
                    .map(|(n, _)| n.to_string())
                    .unwrap_or_else(|| "<user-declared out>".into());
                return Err(LoweringError::UnsupportedShape {
                    what: format!(
                        "main body assigns to `{target_name}`, expected `{expected}` for {:?}",
                        mc.stage
                    ),
                });
            }
            Err(LoweringError::UnsupportedShape {
                what: format!("user function assigns to unknown `{target_name}`"),
            })
        },
        Stmt::Expr(call_expr @ Expr::Call { .. }) => {
            let _ = lower_expr(ctx, call_expr)?;
            Ok(())
        },
        Stmt::Expr(expr) => {
            // Side-effecting expressions as statements: increment /
            // decrement on a local (`++i;`), or any other expression
            // whose value is discarded. lower_expr emits the side
            // effects; we drop the result.
            let _ = lower_expr(ctx, expr)?;
            Ok(())
        },
        Stmt::Return { value: Some(e), .. } => {
            if main_ctx.is_some() {
                return Err(LoweringError::UnsupportedShape {
                    what: "`return <expr>;` in main is not lowered".into(),
                });
            }
            let v = lower_expr(ctx, e)?;
            ctx.b
                .ret_value(v)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            Ok(())
        },
        Stmt::Return { value: None, .. } => {
            ctx.b
                .ret()
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            Ok(())
        },
        Stmt::Discard { .. } => {
            // ESSL fragment-only. The validator's R2 already
            // rejects discard in vertex stage, so we don't gate
            // on `main_ctx.stage` here. OpKill is a block
            // terminator.
            ctx.b
                .kill()
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            Ok(())
        },
        Stmt::Break { .. } => {
            let target = *ctx.break_targets.last().ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: "`break;` outside of a loop or switch is not lowered".into(),
                }
            })?;
            ctx.b
                .branch(target)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            Ok(())
        },
        Stmt::Continue { .. } => {
            let target = *ctx.continue_targets.last().ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: "`continue;` outside of a loop is not lowered".into(),
                }
            })?;
            ctx.b
                .branch(target)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            Ok(())
        },
        Stmt::Block(b) => {
            // Push a fresh lexical scope so locals declared inside
            // the block don't leak. The Function-storage
            // `OpVariable`s still live for the entire function (per
            // SPIR-V rules), but their name resolution is now
            // scoped — a shadowed inner `int i` resolves to its
            // own variable, distinct from any outer `i`.
            ctx.scope_stack.push(HashMap::new());
            let mut walk_result = Ok(());
            for s in &b.stmts {
                if let Err(e) = lower_stmt(ctx, s, main_ctx) {
                    walk_result = Err(e);
                    break;
                }
            }
            ctx.scope_stack.pop();
            walk_result
        },
        Stmt::If { cond, then, else_, .. } => {
            let cond_id = lower_expr(ctx, cond)?;
            let merge_label = ctx.b.id();
            let then_label = ctx.b.id();
            let else_label = if else_.is_some() { ctx.b.id() } else { merge_label };
            ctx.b
                .selection_merge(merge_label, SelectionControl::NONE)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            ctx.b
                .branch_conditional(cond_id, then_label, else_label, [])
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

            // SPIR-V allows exactly one terminator per block. If
            // the then- or else-stmt already emitted its own
            // (Return / Discard), skip the unconditional branch
            // to `merge_label`; otherwise the block would carry
            // two terminators and naga's spv-in rejects on parse.
            let then_terminates = stmt_definitely_terminates(then);
            ctx.b
                .begin_block(Some(then_label))
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            lower_stmt(ctx, then, main_ctx)?;
            if !then_terminates {
                ctx.b
                    .branch(merge_label)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            }

            if let Some(else_stmt) = else_ {
                let else_terminates = stmt_definitely_terminates(else_stmt);
                ctx.b
                    .begin_block(Some(else_label))
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                lower_stmt(ctx, else_stmt, main_ctx)?;
                if !else_terminates {
                    ctx.b
                        .branch(merge_label)
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                }
            }

            ctx.b
                .begin_block(Some(merge_label))
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            Ok(())
        },
        Stmt::For { init, cond, step, body, .. } => {
            // For-init introduces a new lexical scope: the loop
            // variable is visible in cond / step / body but not
            // outside the loop. Two nested loops can each declare
            // `int i` without aliasing.
            ctx.scope_stack.push(HashMap::new());
            let result = (|| -> Result<(), LoweringError> {
                match init {
                    ForInit::Decl(d) => {
                        let binding = *ctx.locals.get(&d.span).ok_or_else(|| {
                            LoweringError::UnsupportedShape {
                                what: format!(
                                    "for-init local `{}` not pre-allocated",
                                    d.name
                                ),
                            }
                        })?;
                        ctx.scope_stack
                            .last_mut()
                            .expect("for-init scope was just pushed")
                            .insert(d.name.clone(), d.span);
                        if let Some(init_expr) = &d.init {
                            let value = lower_expr(ctx, init_expr)?;
                            ctx.b
                                .store(binding.var, value, None, [])
                                .map_err(|e| {
                                    LoweringError::SpirvBuild(format!("{e:?}"))
                                })?;
                        }
                    },
                    ForInit::Expr(e) => {
                        let _ = lower_expr(ctx, e)?;
                    },
                    ForInit::Empty => {},
                }
                emit_loop_cfg(ctx, cond.as_ref(), step.as_ref(), body, main_ctx)
            })();
            ctx.scope_stack.pop();
            result
        },
        Stmt::While { cond, body, .. } => {
            emit_loop_cfg(ctx, Some(cond), None, body, main_ctx)
        },
        Stmt::Do { body, cond, .. } => emit_do_while_cfg(ctx, body, cond, main_ctx),
        Stmt::Switch { discriminant, body, .. } => emit_switch_cfg(ctx, discriminant, body, main_ctx),
        _ => Err(LoweringError::UnsupportedShape {
            what: "stmt shape not lowered".into(),
        }),
    }
}

/// Emit the SPIR-V CFG for a `switch` statement. The body's
/// stmts are scanned and split into per-case segments at each
/// `Stmt::Case` / `Stmt::Default` label. Each segment becomes a
/// block; segments that don't end in a terminator fall through
/// to the next segment's block (matching ESSL semantics).
///
/// First-pass restriction: case values must be `IntLit`s (the
/// validator's R10 already enforces this).
fn emit_switch_cfg(
    ctx: &mut Ctx,
    discriminant: &Expr,
    body: &crate::ast::Block,
    main_ctx: Option<&MainCtx>,
) -> Result<(), LoweringError> {
    let disc_id = lower_expr(ctx, discriminant)?;

    // Segment the body stmts into [(label_value, label, stmts)]
    // groups at each Case / Default. `label_value == None` is the
    // default segment.
    struct Segment<'a> {
        value: Option<i64>,
        label: Word,
        stmts: Vec<&'a Stmt>,
    }
    let mut segments: Vec<Segment> = Vec::new();
    for stmt in &body.stmts {
        match stmt {
            Stmt::Case { value, .. } => {
                let v = match value {
                    Expr::IntLit { value, .. } => *value,
                    _ => {
                        return Err(LoweringError::UnsupportedShape {
                            what: "switch case value must be a literal integer".into(),
                        });
                    },
                };
                segments.push(Segment { value: Some(v), label: ctx.b.id(), stmts: Vec::new() });
            },
            Stmt::Default { .. } => {
                segments.push(Segment { value: None, label: ctx.b.id(), stmts: Vec::new() });
            },
            other => {
                if let Some(seg) = segments.last_mut() {
                    seg.stmts.push(other);
                }
            },
        }
    }

    let merge_label = ctx.b.id();
    let default_label = segments
        .iter()
        .find(|s| s.value.is_none())
        .map(|s| s.label)
        .unwrap_or(merge_label);
    let case_pairs: Vec<(Operand, Word)> = segments
        .iter()
        .filter_map(|s| {
            s.value
                .map(|v| (Operand::LiteralBit32(v as u32), s.label))
        })
        .collect();

    ctx.b
        .selection_merge(merge_label, SelectionControl::NONE)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b
        .switch(disc_id, default_label, case_pairs)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // A switch only adds a break target — `continue;` falls
    // through to whatever loop (if any) encloses the switch.
    ctx.break_targets.push(merge_label);
    let mut result = Ok(());
    for (idx, seg) in segments.iter().enumerate() {
        if let Err(e) = ctx.b.begin_block(Some(seg.label)) {
            result = Err(LoweringError::SpirvBuild(format!("{e:?}")));
            break;
        }
        let mut terminated = false;
        for stmt in &seg.stmts {
            if let Err(e) = lower_stmt(ctx, stmt, main_ctx) {
                result = Err(e);
                break;
            }
            if stmt_definitely_terminates(stmt) {
                terminated = true;
            }
        }
        if result.is_err() {
            break;
        }
        if !terminated {
            // Fall through to the next segment's block, or to
            // the merge if this was the last segment.
            let next = segments
                .get(idx + 1)
                .map(|s| s.label)
                .unwrap_or(merge_label);
            if let Err(e) = ctx.b.branch(next) {
                result = Err(LoweringError::SpirvBuild(format!("{e:?}")));
                break;
            }
        }
    }
    ctx.break_targets.pop();
    result?;

    ctx.b
        .begin_block(Some(merge_label))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Emit the SPIR-V CFG for a `do { body } while (cond);` loop.
/// Differs from `for`/`while` in that the body always runs at
/// least once and the condition is evaluated at the continue
/// block (after each iteration of the body) rather than the
/// header. The loop header is a structural-only block that
/// unconditionally branches into the body so naga's structured-
/// flow recognition keeps working.
fn emit_do_while_cfg(
    ctx: &mut Ctx,
    body: &Stmt,
    cond: &Expr,
    main_ctx: Option<&MainCtx>,
) -> Result<(), LoweringError> {
    let header = ctx.b.id();
    let body_label = ctx.b.id();
    let cont = ctx.b.id();
    let merge = ctx.b.id();

    ctx.b
        .branch(header)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    ctx.b
        .begin_block(Some(header))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b
        .loop_merge(merge, cont, LoopControl::NONE, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b
        .branch(body_label)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    let body_terminates = stmt_definitely_terminates(body);
    ctx.break_targets.push(merge);
    ctx.continue_targets.push(cont);
    ctx.b
        .begin_block(Some(body_label))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    let body_result = lower_stmt(ctx, body, main_ctx);
    ctx.break_targets.pop();
    ctx.continue_targets.pop();
    body_result?;
    if !body_terminates {
        ctx.b
            .branch(cont)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    }

    ctx.b
        .begin_block(Some(cont))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    let cond_id = lower_expr(ctx, cond)?;
    ctx.b
        .branch_conditional(cond_id, header, merge, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    ctx.b
        .begin_block(Some(merge))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Emit the SPIR-V CFG for a `for` or `while` loop body. Init has
/// already been emitted in the predecessor block. The CFG shape is
/// the SPIR-V canonical four-block loop: header → body → continue
/// → header, with `merge` as the exit target.
fn emit_loop_cfg(
    ctx: &mut Ctx,
    cond: Option<&Expr>,
    step: Option<&Expr>,
    body: &Stmt,
    main_ctx: Option<&MainCtx>,
) -> Result<(), LoweringError> {
    let header = ctx.b.id();
    let body_label = ctx.b.id();
    let cont = ctx.b.id();
    let merge = ctx.b.id();

    // Predecessor (current block) → header.
    ctx.b
        .branch(header)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // Header: OpLoopMerge + conditional branch on the loop cond.
    ctx.b
        .begin_block(Some(header))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b
        .loop_merge(merge, cont, LoopControl::NONE, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    match cond {
        Some(c) => {
            let cond_id = lower_expr(ctx, c)?;
            ctx.b
                .branch_conditional(cond_id, body_label, merge, [])
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        },
        None => {
            ctx.b
                .branch(body_label)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        },
    }

    // Body: lower the body stmt, then jump to the continue block.
    // Push the (break -> merge, continue -> cont) labels so any
    // `break;` / `continue;` inside the body resolves to these.
    let body_terminates = stmt_definitely_terminates(body);
    ctx.break_targets.push(merge);
    ctx.continue_targets.push(cont);
    ctx.b
        .begin_block(Some(body_label))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    let body_result = lower_stmt(ctx, body, main_ctx);
    ctx.break_targets.pop();
    ctx.continue_targets.pop();
    body_result?;
    if !body_terminates {
        ctx.b
            .branch(cont)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    }

    // Continue: emit the for-loop step (if any), then jump back
    // to the header.
    ctx.b
        .begin_block(Some(cont))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    if let Some(s) = step {
        let _ = lower_expr(ctx, s)?;
    }
    ctx.b
        .branch(header)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    // Merge: lowering continues here after the loop exits.
    ctx.b
        .begin_block(Some(merge))
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Two-pass emit: every non-main user function's id and signature
/// is allocated and recorded in `ctx.user_fns` before any body is
/// emitted, so a body can `OpFunctionCall` any other function
/// regardless of source order. ESSL §6.1 permits forward
/// references at file scope; this pre-pass makes that work.
fn emit_user_functions(ctx: &mut Ctx, tu: &TranslationUnit) -> Result<(), LoweringError> {
    let mut prototypes: Vec<(&FunctionDef, FnPrototype)> = Vec::new();
    for d in &tu.decls {
        let ExternalDecl::Function(f) = d else { continue };
        if f.name == "main" {
            continue;
        }
        let proto = build_user_fn_prototype(ctx, f)?;
        ctx.user_fns
            .entry(f.name.clone())
            .or_default()
            .push(UserFnBinding {
                func_id: proto.func_id,
                param_types: proto.param_kinds.clone(),
                result: proto.return_kind,
            });
        prototypes.push((f, proto));
    }
    for (f, proto) in prototypes {
        emit_user_function_body(ctx, f, &proto)?;
    }
    Ok(())
}

/// Pre-validated, pre-allocated state for a user function. Phase 1
/// of [`emit_user_functions`] builds these; phase 2 emits the
/// matching body using `func_id` so forward calls resolve.
struct FnPrototype {
    func_id: Word,
    fn_type: Word,
    return_ty: Word,
    return_kind: TypeKind,
    param_kinds: Vec<TypeKind>,
    param_types_spv: Vec<Word>,
}

fn build_user_fn_prototype(
    ctx: &mut Ctx,
    f: &FunctionDef,
) -> Result<FnPrototype, LoweringError> {
    let return_kind = f.return_ty.kind;
    let return_ty = if return_kind == TypeKind::Void {
        ctx.type_void
    } else {
        spv_type_for_kind(ctx, return_kind).ok_or_else(|| {
            LoweringError::UnsupportedShape {
                what: format!(
                    "function `{}` return type {return_kind:?} is not lowered",
                    f.name
                ),
            }
        })?
    };
    let mut param_types_spv: Vec<Word> = Vec::new();
    let mut param_kinds: Vec<TypeKind> = Vec::new();
    for p in &f.params {
        let pt = spv_type_for_kind(ctx, p.ty.kind).ok_or_else(|| {
            LoweringError::UnsupportedShape {
                what: format!(
                    "function `{}` parameter `{}` type {:?} is not lowered",
                    f.name, p.name, p.ty.kind,
                ),
            }
        })?;
        param_types_spv.push(pt);
        param_kinds.push(p.ty.kind);
    }
    let fn_type = ctx.b.type_function(return_ty, param_types_spv.clone());
    let func_id = ctx.b.id();
    Ok(FnPrototype {
        func_id,
        fn_type,
        return_ty,
        return_kind,
        param_kinds,
        param_types_spv,
    })
}

fn emit_user_function_body(
    ctx: &mut Ctx,
    f: &FunctionDef,
    proto: &FnPrototype,
) -> Result<(), LoweringError> {
    ctx.b
        .begin_function(
            proto.return_ty,
            Some(proto.func_id),
            FunctionControl::NONE,
            proto.fn_type,
        )
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    let mut fn_params: HashMap<String, FnParamBinding> = HashMap::new();
    for (p, pt) in f.params.iter().zip(proto.param_types_spv.iter()) {
        let pid = ctx
            .b
            .function_parameter(*pt)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        fn_params.insert(p.name.clone(), FnParamBinding { value_id: pid, kind: p.ty.kind });
    }

    ctx.b
        .begin_block(None)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;

    let saved_params = std::mem::take(&mut ctx.fn_params);
    let saved_locals = std::mem::take(&mut ctx.locals);
    let saved_scopes = std::mem::take(&mut ctx.scope_stack);
    ctx.fn_params = fn_params;
    ctx.scope_stack = vec![HashMap::new()];
    let body_result = lower_user_body(ctx, f, proto.return_kind);
    ctx.fn_params = saved_params;
    ctx.locals = saved_locals;
    ctx.scope_stack = saved_scopes;
    body_result?;

    ctx.b
        .end_function()
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Walk a user function's body statements via `lower_stmt`. Adds
/// the implicit `OpReturn` terminator for void functions whose
/// body did not explicitly return.
fn lower_user_body(
    ctx: &mut Ctx,
    f: &FunctionDef,
    return_kind: TypeKind,
) -> Result<(), LoweringError> {
    pre_allocate_locals(ctx, &f.body.stmts)?;
    for s in &f.body.stmts {
        lower_stmt(ctx, s, None)?;
    }
    if !last_stmt_is_return(&f.body.stmts) {
        if return_kind == TypeKind::Void {
            ctx.b
                .ret()
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        } else {
            return Err(LoweringError::UnsupportedShape {
                what: format!(
                    "function `{}` returns {return_kind:?} but body has no terminating return",
                    f.name
                ),
            });
        }
    }
    Ok(())
}

fn last_stmt_is_return(stmts: &[Stmt]) -> bool {
    match stmts.last() {
        Some(Stmt::Return { .. }) => true,
        Some(Stmt::Block(b)) => last_stmt_is_return(&b.stmts),
        _ => false,
    }
}

/// True when `s` is guaranteed to leave its block via a SPIR-V
/// terminator. Used by the `If` arm and loop bodies to decide
/// whether to emit a trailing branch (which would otherwise
/// double-terminate the block).
///
/// Counts as terminating: `return`, `discard` (OpKill),
/// `break` and `continue` (each emits an OpBranch), and a Block
/// whose last stmt terminates, and an If whose then- and
/// else-branches both terminate.
fn stmt_definitely_terminates(s: &Stmt) -> bool {
    match s {
        Stmt::Return { .. }
        | Stmt::Discard { .. }
        | Stmt::Break { .. }
        | Stmt::Continue { .. } => true,
        Stmt::Block(b) => b.stmts.last().is_some_and(stmt_definitely_terminates),
        Stmt::If { then, else_: Some(else_), .. } => {
            stmt_definitely_terminates(then) && stmt_definitely_terminates(else_)
        },
        _ => false,
    }
}

fn register_samplers(
    b: &mut Builder,
    tu: &TranslationUnit,
    type_image_2d: Word,
    type_image_cube: Word,
    type_sampler: Word,
) -> HashMap<String, SamplerBinding> {
    let mut samplers = HashMap::new();
    // Binding 0 is reserved for the uniform Block. Each ESSL
    // sampler becomes two SPIR-V variables (image + sampler)
    // with consecutive bindings starting at 1.
    let mut binding: u32 = 1;
    let type_sampled_image_2d = b.type_sampled_image(type_image_2d);
    let type_sampled_image_cube = b.type_sampled_image(type_image_cube);
    for d in &tu.decls {
        let ExternalDecl::Global(g) = d else { continue };
        if g.storage != StorageQualifier::Uniform {
            continue;
        }
        let (image_type, sampled_image_type, kind) = match g.ty.kind {
            TypeKind::Sampler2D => {
                (type_image_2d, type_sampled_image_2d, TypeKind::Sampler2D)
            },
            TypeKind::SamplerCube => {
                (type_image_cube, type_sampled_image_cube, TypeKind::SamplerCube)
            },
            _ => continue,
        };
        let ptr_image = b.type_pointer(None, StorageClass::UniformConstant, image_type);
        let image_var = b.variable(ptr_image, None, StorageClass::UniformConstant, None);
        b.decorate(image_var, Decoration::DescriptorSet, [Operand::LiteralBit32(0)]);
        b.decorate(image_var, Decoration::Binding, [Operand::LiteralBit32(binding)]);
        binding += 1;
        let ptr_sampler =
            b.type_pointer(None, StorageClass::UniformConstant, type_sampler);
        let sampler_var =
            b.variable(ptr_sampler, None, StorageClass::UniformConstant, None);
        b.decorate(sampler_var, Decoration::DescriptorSet, [Operand::LiteralBit32(0)]);
        b.decorate(sampler_var, Decoration::Binding, [Operand::LiteralBit32(binding)]);
        binding += 1;
        samplers.insert(
            g.name.clone(),
            SamplerBinding {
                image_var,
                sampler_var,
                image_type,
                sampled_image_type,
                kind,
            },
        );
    }
    samplers
}

#[allow(clippy::too_many_arguments)]
fn register_uniforms(
    b: &mut Builder,
    tu: &TranslationUnit,
    type_float: Word,
    type_int: Word,
    type_vec2: Word,
    type_vec3: Word,
    type_vec4: Word,
    type_ivec2: Word,
    type_ivec3: Word,
    type_ivec4: Word,
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
            TypeKind::Int => (type_int, TypeKind::Int, 4u32, None),
            TypeKind::Vec2 => (type_vec2, TypeKind::Vec2, 8u32, None),
            TypeKind::Vec3 => (type_vec3, TypeKind::Vec3, 16u32, None),
            TypeKind::Vec4 => (type_vec4, TypeKind::Vec4, 16u32, None),
            TypeKind::Ivec2 => (type_ivec2, TypeKind::Ivec2, 8u32, None),
            TypeKind::Ivec3 => (type_ivec3, TypeKind::Ivec3, 16u32, None),
            TypeKind::Ivec4 => (type_ivec4, TypeKind::Ivec4, 16u32, None),
            TypeKind::Mat2 => (type_mat2, TypeKind::Mat2, 16u32, Some(8u32)),
            TypeKind::Mat3 => (type_mat3, TypeKind::Mat3, 48u32, Some(16u32)),
            TypeKind::Mat4 => (type_mat4, TypeKind::Mat4, 64u32, Some(16u32)),
            // Bool uniforms aren't allowed by SPIR-V (no defined
            // memory layout). Sampler uniforms have a different
            // storage class (UniformConstant). Skip both.
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
        // Matrix inputs are column-split: a `mat_n` is emitted as
        // N separate vec_n Input variables at sequential
        // Locations. The Ident-lookup site loads each column and
        // composite-constructs the matrix.
        let (column_type, column_count, kind) = match g.ty.kind {
            TypeKind::Float => (type_float, 1u32, TypeKind::Float),
            TypeKind::Vec2 => (type_vec2, 1, TypeKind::Vec2),
            TypeKind::Vec3 => (type_vec3, 1, TypeKind::Vec3),
            TypeKind::Vec4 => (type_vec4, 1, TypeKind::Vec4),
            TypeKind::Mat2 => (type_vec2, 2, TypeKind::Mat2),
            TypeKind::Mat3 => (type_vec3, 3, TypeKind::Mat3),
            TypeKind::Mat4 => (type_vec4, 4, TypeKind::Mat4),
            // Other input types (int / ivec etc.) aren't exercised
            // by today's spike corpus; emit nothing so the
            // expression emitter will error if they are referenced.
            _ => continue,
        };
        let ptr_ty = b.type_pointer(None, StorageClass::Input, column_type);
        let mut vars = Vec::with_capacity(column_count as usize);
        for _ in 0..column_count {
            let var = b.variable(ptr_ty, None, StorageClass::Input, None);
            b.decorate(var, Decoration::Location, [Operand::LiteralBit32(location)]);
            location += 1;
            vars.push(var);
        }
        inputs.insert(
            g.name.clone(),
            InputBinding { vars, pointee_type: column_type, kind },
        );
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

/// GLSL.std.450 instruction-set numbers for ESSL §8 built-ins that
/// the lowering supports today. Coverage skips textures (samplers
/// not yet plumbed) and matrix built-ins (matrixCompMult etc.);
/// `dot` is the one ESSL §8 built-in that maps to a core SPIR-V
/// opcode (OpDot) rather than a GLSL.std.450 extended instruction,
/// so it is dispatched separately in `lower_expr`.
fn builtin_glsl450_opcode(name: &str) -> Option<u32> {
    Some(match name {
        // §8.1 Angle and trigonometry.
        "radians" => 11,
        "degrees" => 12,
        "sin" => 13,
        "cos" => 14,
        "tan" => 15,
        "asin" => 16,
        "acos" => 17,
        "atan" => 18,
        // §8.2 Exponential.
        "pow" => 26,
        "exp" => 27,
        "log" => 28,
        "exp2" => 29,
        "log2" => 30,
        "sqrt" => 31,
        "inversesqrt" => 32,
        // §8.3 Common.
        "abs" => 4,
        "sign" => 6,
        "floor" => 8,
        "ceil" => 9,
        "fract" => 10,
        // `mod` deliberately omitted: naga's spv-in rejects
        // GLSL.std.450 FMod (35) with `UnsupportedExtInst(35)`.
        // Future widening: inline as `x - y * floor(x/y)`.
        "min" => 37,
        "max" => 40,
        "clamp" => 43,
        "mix" => 46,
        "step" => 48,
        "smoothstep" => 49,
        // §8.4 Geometric (length / distance return scalar — that
        // shape is handled by the call's typecheck result kind).
        "length" => 66,
        "distance" => 67,
        "cross" => 68,
        "normalize" => 69,
        "faceforward" => 70,
        "reflect" => 71,
        "refract" => 72,
        _ => return None,
    })
}

/// For ESSL built-ins with mixed scalar / vector overloads
/// (clamp / mix / step / smoothstep / min / max), returns true if
/// the argument at `arg_idx` is one of the positions where ESSL
/// permits a scalar arg that broadcasts to the result type. The
/// lowering then splats a scalar arg into a vector before passing
/// it to GLSL.std.450 FClamp / FMix / Step / etc., which require
/// homogeneous operands.
///
/// Positions for the splat-eligible built-ins are taken from the
/// ESSL 1.00 §8.3 registry:
/// - clamp(T, T, T)      and clamp(T, float, float)
/// - mix(T, T, T)        and mix(T, T, float)
/// - step(T, T)          and step(float, T)
/// - smoothstep(T,T,T)   and smoothstep(float, float, T)
/// - min / max(T, T)     and min / max(T, float)
///
/// `refract(T, T, float)` is NOT splat-eligible at position 2; its
/// trailing float (eta) is genuinely scalar.
fn should_splat_scalar_for_builtin(name: &str, arg_idx: usize) -> bool {
    match name {
        "clamp" => arg_idx == 1 || arg_idx == 2,
        "mix" => arg_idx == 2,
        "step" => arg_idx == 0,
        "smoothstep" => arg_idx == 0 || arg_idx == 1,
        "min" | "max" => arg_idx == 1,
        _ => false,
    }
}

fn compound_op_to_bin(op: &AssignOp) -> BinOp {
    match op {
        AssignOp::AddAssign => BinOp::Add,
        AssignOp::SubAssign => BinOp::Sub,
        AssignOp::MulAssign => BinOp::Mul,
        AssignOp::DivAssign => BinOp::Div,
        AssignOp::Assign => unreachable!("plain assign handled separately"),
    }
}

/// Emit a binary op on two SSA values (no `Expr` plumbing).
/// Handles the common scalar / vector / int cases that compound
/// assigns need: same-kind arithmetic for float / vec / int, and
/// vector-times-scalar for `vec_n * float` (a common
/// `v.color *= 0.5` shape). Mixed combinations beyond these
/// surface as `UnsupportedShape`.
fn emit_binary_on_values(
    ctx: &mut Ctx,
    op: BinOp,
    lhs_id: Word,
    lhs_kind: TypeKind,
    rhs_id: Word,
    rhs_kind: TypeKind,
    result_ty: Word,
) -> Result<Word, LoweringError> {
    let is_float = matches!(lhs_kind, TypeKind::Float);
    let is_vec = matches!(lhs_kind, TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4);
    let is_int = matches!(lhs_kind, TypeKind::Int);
    let r = match (op, is_float, is_vec, is_int, lhs_kind == rhs_kind) {
        (BinOp::Add, true, _, _, true) | (BinOp::Add, _, true, _, true) => {
            ctx.b.f_add(result_ty, None, lhs_id, rhs_id)
        },
        (BinOp::Sub, true, _, _, true) | (BinOp::Sub, _, true, _, true) => {
            ctx.b.f_sub(result_ty, None, lhs_id, rhs_id)
        },
        (BinOp::Mul, true, _, _, true) | (BinOp::Mul, _, true, _, true) => {
            ctx.b.f_mul(result_ty, None, lhs_id, rhs_id)
        },
        (BinOp::Div, true, _, _, true) | (BinOp::Div, _, true, _, true) => {
            ctx.b.f_div(result_ty, None, lhs_id, rhs_id)
        },
        (BinOp::Mul, _, true, _, false) if rhs_kind == TypeKind::Float => {
            ctx.b.vector_times_scalar(result_ty, None, lhs_id, rhs_id)
        },
        (BinOp::Add, _, _, true, true) => ctx.b.i_add(result_ty, None, lhs_id, rhs_id),
        (BinOp::Sub, _, _, true, true) => ctx.b.i_sub(result_ty, None, lhs_id, rhs_id),
        (BinOp::Mul, _, _, true, true) => ctx.b.i_mul(result_ty, None, lhs_id, rhs_id),
        (BinOp::Div, _, _, true, true) => ctx.b.s_div(result_ty, None, lhs_id, rhs_id),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: format!(
                    "compound binary `{op:?}` on {lhs_kind:?} and {rhs_kind:?} is not lowered"
                ),
            });
        },
    };
    r.map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}

/// Load via the given access-chain pointer, fold in `rhs` via
/// the binary op, store the result back. Shared by struct-field,
/// LHS-swizzle, and matrix-index compound assigns.
fn compound_via_chain(
    ctx: &mut Ctx,
    chain: Word,
    chain_pointee: Word,
    leaf_kind: TypeKind,
    bin_op: BinOp,
    rhs: &Expr,
) -> Result<(), LoweringError> {
    let current = ctx
        .b
        .load(chain_pointee, None, chain, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    let rhs_kind = classify_arg_kind(ctx, rhs).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: "could not classify rhs of compound assign".into(),
        }
    })?;
    let rhs_val = lower_expr(ctx, rhs)?;
    let result = emit_binary_on_values(
        ctx, bin_op, current, leaf_kind, rhs_val, rhs_kind, chain_pointee,
    )?;
    ctx.b
        .store(chain, result, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Compound assign on a struct field LHS (`s.x op= rhs` or
/// nested). Same access-chain plumbing as `lower_struct_field_*`
/// — only the load-modify-store pattern differs.
fn lower_struct_field_compound_assignment(
    ctx: &mut Ctx,
    base: &Expr,
    field: &str,
    struct_idx: u32,
    bin_op: BinOp,
    rhs: &Expr,
) -> Result<(), LoweringError> {
    let (chain, leaf_kind) = build_struct_access_chain(ctx, base, field, struct_idx)?;
    let leaf_ty = spv_type_for_kind(ctx, leaf_kind).expect("checked in helper");
    compound_via_chain(ctx, chain, leaf_ty, leaf_kind, bin_op, rhs)
}

/// Compound assign on a single-component LHS swizzle
/// (`v.x op= rhs`, `gl_FragColor.r += rhs`, etc.). Mirrors the
/// plain-assign `lower_lhs_swizzle_assignment` path but does
/// load-modify-store at the component pointer.
fn lower_lhs_swizzle_compound_assignment(
    ctx: &mut Ctx,
    base: &Expr,
    field: &str,
    bin_op: BinOp,
    rhs: &Expr,
    main_ctx: Option<&MainCtx>,
) -> Result<(), LoweringError> {
    let base_name = match base {
        Expr::Ident { name, .. } => name.as_str(),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "compound LHS-swizzle base must be a plain identifier".into(),
            });
        },
    };
    let target = resolve_lhs_target(ctx, base_name, main_ctx).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("compound LHS-swizzle target `{base_name}` is not a writable variable"),
        }
    })?;
    let base_size = vector_size_of(target.kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("compound LHS-swizzle target type {:?} is not a vector", target.kind),
        }
    })?;
    let indices = parse_swizzle_indices(field, base_size).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("invalid compound LHS swizzle `.{field}` on {:?}", target.kind),
        }
    })?;
    if indices.len() != 1 {
        return Err(LoweringError::UnsupportedShape {
            what: format!(
                "multi-component compound LHS swizzle `.{field}` is not yet lowered"
            ),
        });
    }
    let component_idx = indices[0] as i32;
    let ptr_to_float = ctx
        .b
        .type_pointer(None, target.storage, ctx.type_float);
    let idx_const = int_constant(ctx, component_idx);
    let chain = ctx
        .b
        .access_chain(ptr_to_float, None, target.var, [idx_const])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    compound_via_chain(ctx, chain, ctx.type_float, TypeKind::Float, bin_op, rhs)
}

/// Compound assign on a matrix index LHS (`m[i] op= rhs`) where
/// `m` is a column-split matrix output. Loads the column,
/// folds in via the binary op, stores back.
fn lower_lhs_matrix_index_compound_assignment(
    ctx: &mut Ctx,
    base: &Expr,
    index: &Expr,
    bin_op: BinOp,
    rhs: &Expr,
) -> Result<(), LoweringError> {
    let name = match base {
        Expr::Ident { name, .. } => name.as_str(),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "compound matrix-index LHS base must be a plain identifier".into(),
            });
        },
    };
    let idx_value = match index {
        Expr::IntLit { value, .. } => *value as usize,
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "compound matrix-index LHS index must be a literal integer".into(),
            });
        },
    };
    let out = ctx.outputs.get(name).cloned().ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!(
                "compound matrix-index LHS target `{name}` is not a column-split output"
            ),
        }
    })?;
    if out.vars.len() <= 1 || idx_value >= out.vars.len() {
        return Err(LoweringError::UnsupportedShape {
            what: format!(
                "compound matrix-index LHS `{name}[{idx_value}]` out of bounds or not a matrix"
            ),
        });
    }
    let column_kind = match out.kind {
        TypeKind::Mat2 => TypeKind::Vec2,
        TypeKind::Mat3 => TypeKind::Vec3,
        TypeKind::Mat4 => TypeKind::Vec4,
        other => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("compound matrix-index on {other:?} not lowered"),
            });
        },
    };
    let column_ty = spv_type_for_kind(ctx, column_kind).expect("vec kind");
    let col_var = out.vars[idx_value];
    compound_via_chain(ctx, col_var, column_ty, column_kind, bin_op, rhs)
}

/// Build an N×N identity-shaped matrix with `scalar` on the
/// diagonal and zeros elsewhere — the ESSL §5.4.2 single-scalar
/// matrix constructor.
fn lower_diagonal_matrix(
    ctx: &mut Ctx,
    scalar: Word,
    n: usize,
) -> Result<Word, LoweringError> {
    let (col_ty, mat_ty) = match n {
        2 => (ctx.type_vec2, ctx.type_mat2),
        3 => (ctx.type_vec3, ctx.type_mat3),
        4 => (ctx.type_vec4, ctx.type_mat4),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("mat{n} constructor not lowered"),
            });
        },
    };
    let zero = ctx.b.constant_bit32(ctx.type_float, 0.0f32.to_bits());
    let mut columns: Vec<Word> = Vec::with_capacity(n);
    for col_idx in 0..n {
        let mut components: Vec<Word> = Vec::with_capacity(n);
        for row_idx in 0..n {
            components.push(if row_idx == col_idx { scalar } else { zero });
        }
        let col = ctx
            .b
            .composite_construct(col_ty, None, components)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        columns.push(col);
    }
    ctx.b
        .composite_construct(mat_ty, None, columns)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}

/// Walk a (possibly nested) `Expr::Member` access into a
/// struct-typed local, collecting member indices outermost-first
/// so an `OpAccessChain` can dive straight to the leaf. Returns
/// the access-chain `Word` and the leaf's ESSL kind.
///
/// `field` and `struct_idx` describe the *outermost* member
/// access (the one currently being lowered); the function walks
/// inward through any `Expr::Member` bases to find the root
/// Ident.
fn build_struct_access_chain(
    ctx: &mut Ctx,
    base: &Expr,
    field: &str,
    struct_idx: u32,
) -> Result<(Word, TypeKind), LoweringError> {
    // Collect (field_name, parent_struct_idx) starting from the
    // outermost member. Walk the base inward; each Member node
    // adds another entry until we hit an Ident.
    let mut path: Vec<(String, u32)> = vec![(field.to_string(), struct_idx)];
    let mut current = base;
    let root_name = loop {
        match current {
            Expr::Member { base: inner_base, field: inner_field, .. } => {
                let inner_kind = classify_arg_kind(ctx, inner_base).ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: "nested struct access base has unknown type".into(),
                    }
                })?;
                let inner_idx = match inner_kind {
                    TypeKind::Struct(i) => i,
                    other => {
                        return Err(LoweringError::UnsupportedShape {
                            what: format!(
                                "nested member access on non-struct base type {other:?}"
                            ),
                        });
                    },
                };
                path.push((inner_field.clone(), inner_idx));
                current = inner_base;
            },
            Expr::Ident { name, .. } => break name.as_str(),
            _ => {
                return Err(LoweringError::UnsupportedShape {
                    what: "struct access root must be a plain identifier".into(),
                });
            },
        }
    };
    let local = lookup_local(ctx, root_name).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("struct base `{root_name}` is not a function-scope local"),
        }
    })?;
    // Path is inside-out; reverse to outermost-first for the
    // access chain (`s.inner.x` → indices [inner_idx, x_idx]).
    path.reverse();
    let mut indices: Vec<Word> = Vec::with_capacity(path.len());
    let mut leaf_kind: TypeKind = TypeKind::Void;
    for (field_name, parent_struct_idx) in &path {
        let info = ctx
            .struct_types
            .get(*parent_struct_idx as usize)
            .cloned()
            .ok_or_else(|| LoweringError::UnsupportedShape {
                what: format!("struct index {parent_struct_idx} not in registry"),
            })?;
        let (member_idx, field_kind) =
            info.fields.get(field_name).copied().ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: format!("struct field `{field_name}` not found"),
                }
            })?;
        indices.push(int_constant(ctx, member_idx as i32));
        leaf_kind = field_kind;
    }
    let leaf_ty = spv_type_for_kind(ctx, leaf_kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("leaf field type {leaf_kind:?} not lowered"),
        }
    })?;
    let ptr_to_leaf = ctx
        .b
        .type_pointer(None, StorageClass::Function, leaf_ty);
    let chain = ctx
        .b
        .access_chain(ptr_to_leaf, None, local.var, indices)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok((chain, leaf_kind))
}

/// Lower a struct field read: `s.x` (or nested `s.inner.x`).
/// Emits `OpAccessChain` to the leaf member then `OpLoad`.
fn lower_struct_field_read(
    ctx: &mut Ctx,
    base: &Expr,
    field: &str,
    struct_idx: u32,
) -> Result<Word, LoweringError> {
    let (chain, leaf_kind) = build_struct_access_chain(ctx, base, field, struct_idx)?;
    let leaf_ty = spv_type_for_kind(ctx, leaf_kind).expect("checked in helper");
    ctx.b
        .load(leaf_ty, None, chain, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}

/// Lower a struct field write: `s.x = rhs` (or nested
/// `s.inner.x = rhs`). `OpAccessChain` to the leaf member then
/// `OpStore`.
fn lower_struct_field_assignment(
    ctx: &mut Ctx,
    base: &Expr,
    field: &str,
    struct_idx: u32,
    rhs: &Expr,
) -> Result<(), LoweringError> {
    let value = lower_expr(ctx, rhs)?;
    let (chain, _leaf_kind) = build_struct_access_chain(ctx, base, field, struct_idx)?;
    ctx.b
        .store(chain, value, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Resolve an assignment LHS to its `OpVariable`, ESSL value
/// type, and SPIR-V storage class. Used by the write-side
/// swizzle path to locate the variable so an `OpAccessChain` can
/// be built into one of its components.
struct LhsTarget {
    var: Word,
    kind: TypeKind,
    storage: StorageClass,
}

fn resolve_lhs_target(
    ctx: &Ctx,
    name: &str,
    main_ctx: Option<&MainCtx>,
) -> Option<LhsTarget> {
    if let Some(local) = lookup_local(ctx, name) {
        return Some(LhsTarget {
            var: local.var,
            kind: local.kind,
            storage: StorageClass::Function,
        });
    }
    if let Some(out) = ctx.outputs.get(name) {
        // LHS-swizzle on matrix outputs would need per-column
        // OpAccessChain plumbing; not yet supported. Fall back to
        // the single-var path only for non-matrix outputs.
        if out.vars.len() == 1 {
            return Some(LhsTarget {
                var: out.vars[0],
                kind: out.kind,
                storage: StorageClass::Output,
            });
        }
    }
    if let Some(mc) = main_ctx {
        if let Some((pname, pvar)) = mc.primary {
            if name == pname {
                return Some(LhsTarget {
                    var: pvar,
                    kind: TypeKind::Vec4,
                    storage: StorageClass::Output,
                });
            }
        }
    }
    None
}

fn vector_size_of(kind: TypeKind) -> Option<u32> {
    Some(match kind {
        TypeKind::Vec2 => 2,
        TypeKind::Vec3 => 3,
        TypeKind::Vec4 => 4,
        _ => return None,
    })
}

/// Lower `target.field = rhs;` where `target` is an identifier
/// and `field` is a single-component swizzle. Emits
/// `OpAccessChain` to the component followed by `OpStore`.
/// Multi-component LHS swizzles (`.xy`, etc.) are queued.
fn lower_lhs_swizzle_assignment(
    ctx: &mut Ctx,
    base: &Expr,
    field: &str,
    rhs: &Expr,
    main_ctx: Option<&MainCtx>,
) -> Result<(), LoweringError> {
    let base_name = match base {
        Expr::Ident { name, .. } => name.as_str(),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "LHS-swizzle base must be a plain identifier".into(),
            });
        },
    };
    let target = resolve_lhs_target(ctx, base_name, main_ctx).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("LHS-swizzle target `{base_name}` is not a writable variable"),
        }
    })?;
    let base_size = vector_size_of(target.kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("LHS-swizzle target type {:?} is not a vector", target.kind),
        }
    })?;
    let indices = parse_swizzle_indices(field, base_size).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("invalid LHS swizzle `.{field}` on {:?}", target.kind),
        }
    })?;
    // ESSL §5.5 forbids repeated components on the LHS — each
    // target component can only be assigned once.
    let mut seen = [false; 4];
    for &i in &indices {
        if i >= 4 || seen[i as usize] {
            return Err(LoweringError::UnsupportedShape {
                what: format!("LHS swizzle `.{field}` repeats a component"),
            });
        }
        seen[i as usize] = true;
    }
    let value = lower_expr(ctx, rhs)?;
    if indices.len() == 1 {
        // Single-component LHS: cheaper via OpAccessChain to the
        // component pointer + OpStore.
        let component_idx = indices[0] as i32;
        let ptr_to_float = ctx
            .b
            .type_pointer(None, target.storage, ctx.type_float);
        let idx_const = int_constant(ctx, component_idx);
        let chain = ctx
            .b
            .access_chain(ptr_to_float, None, target.var, [idx_const])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        ctx.b
            .store(chain, value, None, [])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        return Ok(());
    }
    // Multi-component LHS: load the existing value, splice in the
    // new components via OpVectorShuffle, store back.
    // OpVectorShuffle component indices in [0, base_size) refer
    // to vector_1 (old); indices in [base_size, base_size + rhs)
    // refer to vector_2 (the new values from rhs).
    let base_value_ty = spv_type_for_kind(ctx, target.kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("LHS-swizzle target {:?} has no SPIR-V type", target.kind),
        }
    })?;
    let old = ctx
        .b
        .load(base_value_ty, None, target.var, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    let mut shuffle: Vec<u32> = (0..base_size).collect();
    for (rhs_component, &target_component) in indices.iter().enumerate() {
        shuffle[target_component as usize] = base_size + rhs_component as u32;
    }
    let new_value = ctx
        .b
        .vector_shuffle(base_value_ty, None, old, value, shuffle)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b
        .store(target.var, new_value, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Lower ESSL §8.6 vector relational built-ins. Two-arg
/// comparison ops emit an ordered-float compare with a `bvec`
/// result; `any` / `all` reduce a `bvec` to a `bool`; `not`
/// negates a `bvec` component-wise.
fn lower_vector_relational(
    ctx: &mut Ctx,
    callee: &str,
    args: &[Expr],
    call_span: Span,
) -> Result<Word, LoweringError> {
    let result_kind = ctx.types.get(&call_span).copied().ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("§8.6 `{callee}` has no typecheck result"),
        }
    })?;
    let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("§8.6 `{callee}` returns {result_kind:?} which is not lowered"),
        }
    })?;
    match callee {
        "lessThan"
        | "lessThanEqual"
        | "greaterThan"
        | "greaterThanEqual"
        | "equal"
        | "notEqual" => {
            if args.len() != 2 {
                return Err(LoweringError::UnsupportedShape {
                    what: format!("§8.6 `{callee}` expects 2 args, got {}", args.len()),
                });
            }
            let arg_kind = classify_arg_kind(ctx, &args[0]).ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: format!("§8.6 `{callee}` could not classify first arg"),
                }
            })?;
            let lhs = lower_expr(ctx, &args[0])?;
            let rhs = lower_expr(ctx, &args[1])?;
            // ivec inputs use the signed-integer compare ops; vec
            // inputs use the ordered-float ops.
            let is_int_vec = matches!(
                arg_kind,
                TypeKind::Ivec2 | TypeKind::Ivec3 | TypeKind::Ivec4
            );
            let r = match (callee, is_int_vec) {
                ("lessThan", false) => ctx.b.f_ord_less_than(result_ty, None, lhs, rhs),
                ("lessThan", true) => ctx.b.s_less_than(result_ty, None, lhs, rhs),
                ("lessThanEqual", false) => {
                    ctx.b.f_ord_less_than_equal(result_ty, None, lhs, rhs)
                },
                ("lessThanEqual", true) => {
                    ctx.b.s_less_than_equal(result_ty, None, lhs, rhs)
                },
                ("greaterThan", false) => {
                    ctx.b.f_ord_greater_than(result_ty, None, lhs, rhs)
                },
                ("greaterThan", true) => {
                    ctx.b.s_greater_than(result_ty, None, lhs, rhs)
                },
                ("greaterThanEqual", false) => {
                    ctx.b.f_ord_greater_than_equal(result_ty, None, lhs, rhs)
                },
                ("greaterThanEqual", true) => {
                    ctx.b.s_greater_than_equal(result_ty, None, lhs, rhs)
                },
                ("equal", false) => ctx.b.f_ord_equal(result_ty, None, lhs, rhs),
                ("equal", true) => ctx.b.i_equal(result_ty, None, lhs, rhs),
                ("notEqual", false) => ctx.b.f_ord_not_equal(result_ty, None, lhs, rhs),
                ("notEqual", true) => ctx.b.i_not_equal(result_ty, None, lhs, rhs),
                _ => unreachable!(),
            };
            r.map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
        },
        "any" | "all" => {
            if args.len() != 1 {
                return Err(LoweringError::UnsupportedShape {
                    what: format!("§8.6 `{callee}` expects 1 arg, got {}", args.len()),
                });
            }
            let v = lower_expr(ctx, &args[0])?;
            let r = if callee == "any" {
                ctx.b.any(result_ty, None, v)
            } else {
                ctx.b.all(result_ty, None, v)
            };
            r.map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
        },
        "not" => {
            if args.len() != 1 {
                return Err(LoweringError::UnsupportedShape {
                    what: format!("`not` expects 1 arg, got {}", args.len()),
                });
            }
            let v = lower_expr(ctx, &args[0])?;
            ctx.b
                .logical_not(result_ty, None, v)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
        },
        _ => unreachable!(),
    }
}

/// Lower `outerProduct(c, r)` by hand: column j of the result
/// is `c * r[j]`, computed via `OpVectorTimesScalar`. Avoids
/// `OpOuterProduct` because naga's WGSL backend rejects it.
fn lower_outer_product_expansion(
    ctx: &mut Ctx,
    args: &[Expr],
    call_span: Span,
) -> Result<Word, LoweringError> {
    let c = lower_expr(ctx, &args[0])?;
    let r = lower_expr(ctx, &args[1])?;
    let result_kind = ctx.types.get(&call_span).copied().ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: "outerProduct has no typecheck result".into(),
        }
    })?;
    let (column_kind, n) = match result_kind {
        TypeKind::Mat2 => (TypeKind::Vec2, 2usize),
        TypeKind::Mat3 => (TypeKind::Vec3, 3),
        TypeKind::Mat4 => (TypeKind::Vec4, 4),
        other => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("outerProduct result {other:?} is not a matrix"),
            });
        },
    };
    let col_ty = spv_type_for_kind(ctx, column_kind).expect("matrix column");
    let mat_ty = spv_type_for_kind(ctx, result_kind).expect("matrix");
    let mut columns: Vec<Word> = Vec::with_capacity(n);
    for j in 0..n {
        let r_j = ctx
            .b
            .composite_extract(ctx.type_float, None, r, [j as u32])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        let col = ctx
            .b
            .vector_times_scalar(col_ty, None, c, r_j)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        columns.push(col);
    }
    ctx.b
        .composite_construct(mat_ty, None, columns)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}

/// Lower `matrixCompMult(a, b)` as a column-by-column
/// element-wise multiply: extract each column from both
/// matrices, `OpFMul` them, then `OpCompositeConstruct` the
/// result matrix. SPIR-V does not have a single opcode for
/// matrix component-wise multiplication; GLSL.std.450 has no
/// equivalent either.
fn lower_matrix_comp_mult(
    ctx: &mut Ctx,
    args: &[Expr],
    call_span: Span,
) -> Result<Word, LoweringError> {
    let a = lower_expr(ctx, &args[0])?;
    let b = lower_expr(ctx, &args[1])?;
    let result_kind = ctx.types.get(&call_span).copied().ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: "matrixCompMult has no typecheck result".into(),
        }
    })?;
    let (column_kind, n) = match result_kind {
        TypeKind::Mat2 => (TypeKind::Vec2, 2usize),
        TypeKind::Mat3 => (TypeKind::Vec3, 3),
        TypeKind::Mat4 => (TypeKind::Vec4, 4),
        other => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("matrixCompMult result {other:?} is not a matrix"),
            });
        },
    };
    let col_ty = spv_type_for_kind(ctx, column_kind).expect("matrix column");
    let mat_ty = spv_type_for_kind(ctx, result_kind).expect("matrix");
    let mut columns: Vec<Word> = Vec::with_capacity(n);
    for i in 0..n {
        let a_col = ctx
            .b
            .composite_extract(col_ty, None, a, [i as u32])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        let b_col = ctx
            .b
            .composite_extract(col_ty, None, b, [i as u32])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        let prod = ctx
            .b
            .f_mul(col_ty, None, a_col, b_col)
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        columns.push(prod);
    }
    ctx.b
        .composite_construct(mat_ty, None, columns)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}

/// Inline ESSL `mod(x, y)` as `x - y * floor(x / y)`. The
/// GLSL.std.450 `FMod` opcode would be cleaner, but naga's
/// `spv-in` rejects it with `UnsupportedExtInst(35)`. The
/// expansion runs over float and vec_n; the `mod(vec_n, float)`
/// overload splats `y` to the vector width first.
fn lower_mod_expansion(
    ctx: &mut Ctx,
    args: &[Expr],
    call_span: Span,
) -> Result<Word, LoweringError> {
    if args.len() != 2 {
        return Err(LoweringError::UnsupportedShape {
            what: format!("`mod` expects 2 args, got {}", args.len()),
        });
    }
    let result_kind = ctx.types.get(&call_span).copied().ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: "`mod` has no typecheck result kind".into(),
        }
    })?;
    let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("`mod` returns {result_kind:?} which is not lowered"),
        }
    })?;
    let x = lower_expr(ctx, &args[0])?;
    let mut y = lower_expr(ctx, &args[1])?;
    // Splat scalar y to vector result.
    if matches!(
        result_kind,
        TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4
    ) {
        if classify_arg_kind(ctx, &args[1]) == Some(TypeKind::Float) {
            y = splat_scalar_to_vector(ctx, y, result_kind)?;
        }
    }
    let div = ctx
        .b
        .f_div(result_ty, None, x, y)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    let set_id = glsl_std_450_id(ctx);
    let floor = ctx
        .b
        .ext_inst(result_ty, None, set_id, 8, vec![Operand::IdRef(div)])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    let prod = ctx
        .b
        .f_mul(result_ty, None, y, floor)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    ctx.b
        .f_sub(result_ty, None, x, prod)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}

/// Emit an `OpCompositeConstruct` that broadcasts `scalar_id` to
/// every component of `target_kind`. `target_kind` must be one of
/// Vec2 / Vec3 / Vec4; other kinds error.
fn splat_scalar_to_vector(
    ctx: &mut Ctx,
    scalar_id: Word,
    target_kind: TypeKind,
) -> Result<Word, LoweringError> {
    let (target_ty, count) = match target_kind {
        TypeKind::Vec2 => (ctx.type_vec2, 2usize),
        TypeKind::Vec3 => (ctx.type_vec3, 3),
        TypeKind::Vec4 => (ctx.type_vec4, 4),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("scalar splat target {target_kind:?} is not lowered"),
            });
        },
    };
    let constituents: Vec<Word> = std::iter::repeat(scalar_id).take(count).collect();
    ctx.b
        .composite_construct(target_ty, None, constituents)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}

/// Lazily allocate the `GLSL.std.450` `OpExtInstImport` and cache
/// its id on `ctx`. The first built-in call that needs it pays the
/// allocation; subsequent calls reuse the cached id.
fn glsl_std_450_id(ctx: &mut Ctx) -> Word {
    if let Some(id) = ctx.glsl_std_450 {
        return id;
    }
    let id = ctx.b.ext_inst_import("GLSL.std.450");
    ctx.glsl_std_450 = Some(id);
    id
}

fn spv_type_for_kind(ctx: &Ctx, kind: TypeKind) -> Option<Word> {
    Some(match kind {
        TypeKind::Float => ctx.type_float,
        TypeKind::Int => ctx.type_int,
        TypeKind::Bool => ctx.type_bool,
        TypeKind::Vec2 => ctx.type_vec2,
        TypeKind::Vec3 => ctx.type_vec3,
        TypeKind::Vec4 => ctx.type_vec4,
        TypeKind::Bvec2 => ctx.type_bvec2,
        TypeKind::Bvec3 => ctx.type_bvec3,
        TypeKind::Bvec4 => ctx.type_bvec4,
        TypeKind::Ivec2 => ctx.type_ivec2,
        TypeKind::Ivec3 => ctx.type_ivec3,
        TypeKind::Ivec4 => ctx.type_ivec4,
        TypeKind::Mat2 => ctx.type_mat2,
        TypeKind::Mat3 => ctx.type_mat3,
        TypeKind::Mat4 => ctx.type_mat4,
        TypeKind::Struct(idx) => {
            return ctx.struct_types.get(idx as usize).map(|s| s.type_id);
        },
        _ => return None,
    })
}

/// Get (or lazily allocate) the SPIR-V `OpTypePointer Function T`
/// id for `kind`. Used when emitting `OpVariable` for a local. Each
/// pointer type is allocated at most once per `Ctx`.
fn function_ptr_for(ctx: &mut Ctx, kind: TypeKind) -> Result<Word, LoweringError> {
    if let Some(&w) = ctx.function_ptr_types.get(&kind) {
        return Ok(w);
    }
    let value_ty = spv_type_for_kind(ctx, kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("function-scope pointer to {kind:?} is not lowered"),
        }
    })?;
    let ptr = ctx.b.type_pointer(None, StorageClass::Function, value_ty);
    ctx.function_ptr_types.insert(kind, ptr);
    Ok(ptr)
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
        Expr::IntLit { value, span } => {
            // Choose the constant's type from the typecheck. ESSL
            // assigns Int to IntLit; the historical Float-promotion
            // hack misfired for loop counters and is now gated to
            // contexts that explicitly want Float (which we don't
            // produce today, but the branch leaves the door open).
            let kind = ctx.types.get(span).copied().unwrap_or(TypeKind::Int);
            match kind {
                TypeKind::Int => Ok(ctx.b.constant_bit32(ctx.type_int, *value as u32)),
                TypeKind::Float => {
                    Ok(ctx.b.constant_bit32(ctx.type_float, (*value as f32).to_bits()))
                },
                _ => Err(LoweringError::UnsupportedShape {
                    what: format!("int literal in {kind:?} context is not lowered"),
                }),
            }
        },
        Expr::BoolLit { value, .. } => {
            let bool_ty = ctx.type_bool;
            Ok(if *value {
                ctx.b.constant_true(bool_ty)
            } else {
                ctx.b.constant_false(bool_ty)
            })
        },
        Expr::Ident { name, .. } => {
            // Function-parameter SSA values shadow everything else
            // while lowering a user function body.
            if let Some(p) = ctx.fn_params.get(name) {
                return Ok(p.value_id);
            }
            // Function-scope locals are next. `OpLoad` the value
            // out of the local's variable.
            if let Some(local) = lookup_local(ctx, name) {
                return ctx
                    .b
                    .load(local.pointee_type, None, local.var, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if let Some(binding) = ctx.inputs.get(name) {
                let pointee = binding.pointee_type;
                if binding.vars.len() == 1 {
                    let var = binding.vars[0];
                    return ctx
                        .b
                        .load(pointee, None, var, None, [])
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
                }
                // Matrix input: load each column, composite-
                // construct the matrix.
                let kind = binding.kind;
                let vars: Vec<Word> = binding.vars.clone();
                let mut columns: Vec<Word> = Vec::with_capacity(vars.len());
                for v in vars {
                    let col = ctx
                        .b
                        .load(pointee, None, v, None, [])
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                    columns.push(col);
                }
                let result_ty = spv_type_for_kind(ctx, kind).ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: format!(
                            "input `{name}` of type {kind:?} has no SPIR-V matrix type"
                        ),
                    }
                })?;
                return ctx
                    .b
                    .composite_construct(result_ty, None, columns)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // Reading from a varying output in the vertex stage
            // is spec-legal (ESSL 1.00 / 3.00: vertex outputs
            // are mutable, not write-only). The compound-assign
            // path needs this to compose `v += rhs` as
            // `load(v); add; store`. For column-split matrix
            // outputs, the load assembles the matrix from its
            // column variables.
            if let Some(out) = ctx.outputs.get(name) {
                if out.vars.len() == 1 {
                    let kind = out.kind;
                    let ty = spv_type_for_kind(ctx, kind).ok_or_else(|| {
                        LoweringError::UnsupportedShape {
                            what: format!("output `{name}` type {kind:?} not lowered"),
                        }
                    })?;
                    let var = out.vars[0];
                    return ctx
                        .b
                        .load(ty, None, var, None, [])
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
                }
                let kind = out.kind;
                let column_kind = match kind {
                    TypeKind::Mat2 => TypeKind::Vec2,
                    TypeKind::Mat3 => TypeKind::Vec3,
                    TypeKind::Mat4 => TypeKind::Vec4,
                    _ => {
                        return Err(LoweringError::UnsupportedShape {
                            what: format!(
                                "output `{name}` of kind {kind:?} cannot be read as a column-split"
                            ),
                        });
                    },
                };
                let col_ty = spv_type_for_kind(ctx, column_kind).expect("vec kind");
                let vars: Vec<Word> = out.vars.clone();
                let mut columns: Vec<Word> = Vec::with_capacity(vars.len());
                for v in vars {
                    let col = ctx
                        .b
                        .load(col_ty, None, v, None, [])
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                    columns.push(col);
                }
                let result_ty = spv_type_for_kind(ctx, kind).ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: format!("output `{name}` of type {kind:?} has no SPIR-V matrix type"),
                    }
                })?;
                return ctx
                    .b
                    .composite_construct(result_ty, None, columns)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // Sampler uniforms: load both the image and the
            // sampler variables, then `OpSampledImage` combines
            // them into a SampledImage SSA value the texture
            // built-ins can sample from. Naga's spv-in requires
            // the combine to happen at the call site rather than
            // through a combined-image variable load.
            if let Some(sampler) = ctx.samplers.get(name).copied() {
                let image_val = ctx
                    .b
                    .load(sampler.image_type, None, sampler.image_var, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                let sampler_val = ctx
                    .b
                    .load(ctx.type_sampler, None, sampler.sampler_var, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
                return ctx
                    .b
                    .sampled_image(
                        sampler.sampled_image_type,
                        None,
                        image_val,
                        sampler_val,
                    )
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
        Expr::Call { callee, args, span, .. } => {
            // ESSL §8 built-ins (sin/cos/dot/...) dispatch first.
            // Most map to GLSL.std.450 OpExtInst; `dot` is the one
            // that uses a core SPIR-V opcode (OpDot). User-defined
            // function calls go through OpFunctionCall next.
            // Constructor calls (vec2 / vec3 / vec4) fall through
            // to the composite-construct path below.
            if callee == "dot" {
                if args.len() != 2 {
                    return Err(LoweringError::UnsupportedShape {
                        what: format!("`dot` expects 2 args, got {}", args.len()),
                    });
                }
                let a = lower_expr(ctx, &args[0])?;
                let b = lower_expr(ctx, &args[1])?;
                return ctx
                    .b
                    .dot(ctx.type_float, None, a, b)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if callee == "mod" {
                // naga's spv-in rejects GLSL.std.450 FMod (35). Inline
                // as `x - y * floor(x / y)`, splatting y to vector
                // width when the overload is `mod(vec_n, float)`.
                return lower_mod_expansion(ctx, args, *span);
            }
            // ESSL §8.5 matrix built-ins.
            if callee == "transpose" && args.len() == 1 {
                let m = lower_expr(ctx, &args[0])?;
                let result_kind = ctx.types.get(span).copied().ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: "transpose has no typecheck result".into(),
                    }
                })?;
                let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: format!("transpose returns {result_kind:?} which is not lowered"),
                    }
                })?;
                return ctx
                    .b
                    .transpose(result_ty, None, m)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if callee == "outerProduct" && args.len() == 2 {
                // naga rejects OpOuterProduct's WGSL emit. Inline
                // as N columns where column j = c * r[j] (i.e.
                // OpVectorTimesScalar of the first vector by each
                // component of the second).
                return lower_outer_product_expansion(ctx, args, *span);
            }
            if callee == "inverse" && args.len() == 1 {
                // GLSL.std.450 MatrixInverse (34).
                let m = lower_expr(ctx, &args[0])?;
                let result_kind = ctx.types.get(span).copied().ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: "inverse has no typecheck result".into(),
                    }
                })?;
                let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: format!("inverse returns {result_kind:?} which is not lowered"),
                    }
                })?;
                let set_id = glsl_std_450_id(ctx);
                return ctx
                    .b
                    .ext_inst(result_ty, None, set_id, 34, [Operand::IdRef(m)])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if callee == "matrixCompMult" && args.len() == 2 {
                // No single SPIR-V opcode — component-wise mul
                // means OpFMul on each column, then
                // OpCompositeConstruct the result matrix.
                return lower_matrix_comp_mult(ctx, args, *span);
            }
            // ESSL §8.7 texture lookup. `texture2D` /
            // `textureCube` 2-arg form is implicit Lod; 3-arg
            // form is implicit Lod with a bias. `texture2DLod` /
            // `textureCubeLod` use explicit Lod.
            if matches!(callee.as_str(), "texture2D" | "textureCube") {
                if args.len() == 2 || args.len() == 3 {
                    let sampled_image = lower_expr(ctx, &args[0])?;
                    let coord = lower_expr(ctx, &args[1])?;
                    let (image_operands, operands) = if args.len() == 3 {
                        let bias = lower_expr(ctx, &args[2])?;
                        (Some(ImageOperands::BIAS), vec![Operand::IdRef(bias)])
                    } else {
                        (None, Vec::new())
                    };
                    return ctx
                        .b
                        .image_sample_implicit_lod(
                            ctx.type_vec4,
                            None,
                            sampled_image,
                            coord,
                            image_operands,
                            operands,
                        )
                        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
                }
            }
            if matches!(callee.as_str(), "texture2DLod" | "textureCubeLod") && args.len() == 3 {
                let sampled_image = lower_expr(ctx, &args[0])?;
                let coord = lower_expr(ctx, &args[1])?;
                let lod = lower_expr(ctx, &args[2])?;
                return ctx
                    .b
                    .image_sample_explicit_lod(
                        ctx.type_vec4,
                        None,
                        sampled_image,
                        coord,
                        ImageOperands::LOD,
                        [Operand::IdRef(lod)],
                    )
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // ESSL §8.6 vector relational. lessThan / equal / etc.
            // map to ordered-float compares with a bvec result;
            // any / all reduce a bvec to a bool; not negates a bvec
            // component-wise.
            if matches!(
                callee.as_str(),
                "lessThan"
                    | "lessThanEqual"
                    | "greaterThan"
                    | "greaterThanEqual"
                    | "equal"
                    | "notEqual"
                    | "any"
                    | "all"
                    | "not"
            ) {
                return lower_vector_relational(ctx, callee, args, *span);
            }
            if let Some(mut opcode) = builtin_glsl450_opcode(callee) {
                // `atan(y, x)` (2-arg form) maps to GLSL.std.450
                // Atan2 (25), not the 1-arg Atan (18) the table
                // returns by name. Disambiguate by arity here.
                if callee == "atan" && args.len() == 2 {
                    opcode = 25;
                }
                let result_kind = ctx.types.get(span).copied().ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: format!("built-in `{callee}` has no typecheck result"),
                    }
                })?;
                let result_ty = spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
                    LoweringError::UnsupportedShape {
                        what: format!("built-in `{callee}` returns {result_kind:?} which is not lowered"),
                    }
                })?;
                // Lower each arg, splatting scalars into the result
                // type when the built-in's signature admits scalar
                // broadcast at this position. GLSL.std.450
                // FClamp / FMix / Step / etc. require homogeneous
                // operands, so the ESSL `clamp(vec3, float, float)`
                // overload has to splat the bounds to vec3 first.
                let mut arg_ids = Vec::with_capacity(args.len());
                let result_is_vector = matches!(
                    result_kind,
                    TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4
                );
                for (idx, a) in args.iter().enumerate() {
                    let mut id = lower_expr(ctx, a)?;
                    if result_is_vector && should_splat_scalar_for_builtin(callee, idx) {
                        let arg_kind = classify_arg_kind(ctx, a);
                        if arg_kind == Some(TypeKind::Float) {
                            id = splat_scalar_to_vector(ctx, id, result_kind)?;
                        }
                    }
                    arg_ids.push(id);
                }
                let set_id = glsl_std_450_id(ctx);
                let operands: Vec<Operand> =
                    arg_ids.iter().map(|w| Operand::IdRef(*w)).collect();
                return ctx
                    .b
                    .ext_inst(result_ty, None, set_id, opcode, operands)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            if ctx.user_fns.contains_key(callee) {
                // Classify args first so we can pick the matching
                // overload. The typecheck already validated the
                // call shape; classify_arg_kind reads its types
                // map.
                let arg_kinds: Vec<TypeKind> = args
                    .iter()
                    .map(|a| classify_arg_kind(ctx, a))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| LoweringError::UnsupportedShape {
                        what: format!("could not classify arguments to `{callee}`"),
                    })?;
                let overload = ctx
                    .user_fns
                    .get(callee)
                    .and_then(|sigs| {
                        sigs.iter().find(|s| s.param_types == arg_kinds).cloned()
                    })
                    .ok_or_else(|| LoweringError::UnsupportedShape {
                        what: format!(
                            "no overload of `{callee}` matches argument types {arg_kinds:?}"
                        ),
                    })?;
                let mut arg_ids = Vec::with_capacity(args.len());
                for a in args {
                    arg_ids.push(lower_expr(ctx, a)?);
                }
                let result_kind = overload.result;
                let result_ty = if result_kind == TypeKind::Void {
                    ctx.type_void
                } else {
                    spv_type_for_kind(ctx, result_kind).ok_or_else(|| {
                        LoweringError::UnsupportedShape {
                            what: format!("call `{callee}` returns {result_kind:?} which is not lowered"),
                        }
                    })?
                };
                return ctx
                    .b
                    .function_call(result_ty, None, overload.func_id, arg_ids)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // Struct constructor `Foo(args)`. Each arg is
            // lowered in order and `OpCompositeConstruct`
            // packages them into the struct value. The
            // typechecker has already verified the arg types
            // against the declared field order.
            if let Some(&struct_idx) = ctx.struct_name_to_idx.get(callee.as_str()) {
                let type_id = ctx.struct_types[struct_idx as usize].type_id;
                let mut constituents: Vec<Word> = Vec::with_capacity(args.len());
                for a in args {
                    constituents.push(lower_expr(ctx, a)?);
                }
                return ctx
                    .b
                    .composite_construct(type_id, None, constituents)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
            // ESSL §5.4.2 matrix constructors. The single-scalar
            // form `mat_n(s)` builds a matrix with `s` on the
            // diagonal and zeros elsewhere (the identity-like
            // pattern). The full N×N form is queued.
            if matches!(callee.as_str(), "mat2" | "mat3" | "mat4") && args.len() == 1 {
                let n = match callee.as_str() {
                    "mat2" => 2usize,
                    "mat3" => 3,
                    "mat4" => 4,
                    _ => unreachable!(),
                };
                let scalar_kind = classify_arg_kind(ctx, &args[0]);
                if scalar_kind == Some(TypeKind::Float)
                    || matches!(&args[0], Expr::IntLit { .. })
                {
                    let scalar = lower_expr(ctx, &args[0])?;
                    return lower_diagonal_matrix(ctx, scalar, n);
                }
            }
            let (result_ty, component_count) = match callee.as_str() {
                "vec2" => (ctx.type_vec2, 2usize),
                "vec3" => (ctx.type_vec3, 3usize),
                "vec4" => (ctx.type_vec4, 4usize),
                "ivec2" => (ctx.type_ivec2, 2usize),
                "ivec3" => (ctx.type_ivec3, 3usize),
                "ivec4" => (ctx.type_ivec4, 4usize),
                "bvec2" => (ctx.type_bvec2, 2usize),
                "bvec3" => (ctx.type_bvec3, 3usize),
                "bvec4" => (ctx.type_bvec4, 4usize),
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
        Expr::Member { base, field, span, .. } => {
            // Dispatch on the base kind: struct base → field
            // lookup via OpAccessChain + OpLoad; vector base →
            // swizzle (existing path).
            if let Some(TypeKind::Struct(idx)) = classify_arg_kind(ctx, base) {
                return lower_struct_field_read(ctx, base, field, idx);
            }
            lower_swizzle(ctx, base, field, *span)
        },
        Expr::Index { base, index, .. } => lower_index_rhs(ctx, base, index),
        Expr::Unary { op, expr, .. } => lower_unary(ctx, *op, expr),
        Expr::Assign { op, lhs, rhs, span } => {
            // Expression-context assignment (e.g., a for-step
            // `i += 1`). The new value is both stored back to the
            // local and returned as the expression's result. Only
            // Ident-LHS is supported here; the Stmt::Expr(Assign)
            // arm handles assignments to outputs and the primary.
            let new_value = if *op == AssignOp::Assign {
                lower_expr(ctx, rhs)?
            } else {
                let bin_op = match op {
                    AssignOp::AddAssign => BinOp::Add,
                    AssignOp::SubAssign => BinOp::Sub,
                    AssignOp::MulAssign => BinOp::Mul,
                    AssignOp::DivAssign => BinOp::Div,
                    AssignOp::Assign => unreachable!(),
                };
                lower_binary(ctx, bin_op, lhs, rhs, *span)?
            };
            let name = match lhs.as_ref() {
                Expr::Ident { name, .. } => name.as_str(),
                _ => {
                    return Err(LoweringError::UnsupportedShape {
                        what: "expression-context assignment lhs must be an identifier".into(),
                    });
                },
            };
            let local = lookup_local(ctx, name).ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: format!(
                        "expression-context assignment target `{name}` is not a writable local"
                    ),
                }
            })?;
            ctx.b
                .store(local.var, new_value, None, [])
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            Ok(new_value)
        },
        other => Err(LoweringError::UnsupportedShape {
            what: format!("expression shape not lowered: {other:?}"),
        }),
    }
}

/// Lower a unary expression. First-pass coverage: arithmetic
/// negation on float/int (OpFNegate / OpSNegate), pre/post
/// increment / decrement on a local variable of int or float
/// type, and logical NOT on bool (OpLogicalNot). Other shapes
/// (BitNot, vector negation on a non-local) are queued.
fn lower_unary(ctx: &mut Ctx, op: UnaryOp, expr: &Expr) -> Result<Word, LoweringError> {
    match op {
        UnaryOp::Pos => lower_expr(ctx, expr),
        UnaryOp::Neg => {
            let kind = classify_arg_kind(ctx, expr).ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: "could not classify unary `-` operand".into(),
                }
            })?;
            let id = lower_expr(ctx, expr)?;
            let ty = spv_type_for_kind(ctx, kind).ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: format!("unary `-` result type {kind:?} is not lowered"),
                }
            })?;
            match kind {
                TypeKind::Int
                | TypeKind::Ivec2
                | TypeKind::Ivec3
                | TypeKind::Ivec4 => ctx
                    .b
                    .s_negate(ty, None, id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}"))),
                TypeKind::Float | TypeKind::Vec2 | TypeKind::Vec3 | TypeKind::Vec4 => ctx
                    .b
                    .f_negate(ty, None, id)
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}"))),
                _ => Err(LoweringError::UnsupportedShape {
                    what: format!("unary `-` on {kind:?} is not lowered"),
                }),
            }
        },
        UnaryOp::Not => {
            let id = lower_expr(ctx, expr)?;
            let bool_ty = ctx.type_bool;
            ctx.b
                .logical_not(bool_ty, None, id)
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
        },
        UnaryOp::PreInc | UnaryOp::PreDec | UnaryOp::PostInc | UnaryOp::PostDec => {
            let name = match expr {
                Expr::Ident { name, .. } => name.as_str(),
                _ => {
                    return Err(LoweringError::UnsupportedShape {
                        what: "++/-- target must be a local identifier".into(),
                    });
                },
            };
            let local = lookup_local(ctx, name).ok_or_else(|| {
                LoweringError::UnsupportedShape {
                    what: format!("++/-- on non-local `{name}` is not lowered"),
                }
            })?;
            let pointee = local.pointee_type;
            let var = local.var;
            let is_int = pointee == ctx.type_int;
            let one_id = if is_int {
                ctx.b.constant_bit32(ctx.type_int, 1)
            } else {
                ctx.b.constant_bit32(ctx.type_float, 1.0_f32.to_bits())
            };
            let old = ctx
                .b
                .load(pointee, None, var, None, [])
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            let inc = matches!(op, UnaryOp::PreInc | UnaryOp::PostInc);
            let new = if is_int {
                if inc {
                    ctx.b.i_add(pointee, None, old, one_id)
                } else {
                    ctx.b.i_sub(pointee, None, old, one_id)
                }
            } else if inc {
                ctx.b.f_add(pointee, None, old, one_id)
            } else {
                ctx.b.f_sub(pointee, None, old, one_id)
            }
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            ctx.b
                .store(var, new, None, [])
                .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
            // Pre-form returns the new value, post-form returns the old.
            Ok(match op {
                UnaryOp::PreInc | UnaryOp::PreDec => new,
                _ => old,
            })
        },
        UnaryOp::BitNot => Err(LoweringError::UnsupportedShape {
            what: "bitwise `~` is not lowered".into(),
        }),
    }
}

/// Store `value` to an output binding. For a single-var
/// output, this is a plain `OpStore`. For a column-split
/// matrix output, this `OpCompositeExtract`s each column from
/// `value` and `OpStore`s it to the matching column variable.
fn store_to_output(
    ctx: &mut Ctx,
    out: &OutputBinding,
    name: &str,
    value: Word,
) -> Result<(), LoweringError> {
    if out.vars.len() == 1 {
        ctx.b
            .store(out.vars[0], value, None, [])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        return Ok(());
    }
    let column_kind = match out.kind {
        TypeKind::Mat2 => TypeKind::Vec2,
        TypeKind::Mat3 => TypeKind::Vec3,
        TypeKind::Mat4 => TypeKind::Vec4,
        other => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("output `{name}` of type {other:?} has no column kind"),
            });
        },
    };
    let column_ty = spv_type_for_kind(ctx, column_kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("column type {column_kind:?} for output `{name}` is not lowered"),
        }
    })?;
    for (idx, col_var) in out.vars.iter().enumerate() {
        let col_val = ctx
            .b
            .composite_extract(column_ty, None, value, [idx as u32])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
        ctx.b
            .store(*col_var, col_val, None, [])
            .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    }
    Ok(())
}

/// Lower `m[i] = rhs` where `m` is a column-split matrix
/// output and `i` is a literal integer. Stores `rhs` directly
/// to the matching column variable. Locals / uniforms (which
/// keep the matrix as a single variable) and non-constant
/// indices are queued.
fn lower_lhs_matrix_index_assignment(
    ctx: &mut Ctx,
    base: &Expr,
    index: &Expr,
    rhs: &Expr,
) -> Result<(), LoweringError> {
    let name = match base {
        Expr::Ident { name, .. } => name.as_str(),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "matrix-index LHS base must be a plain identifier".into(),
            });
        },
    };
    let idx_value = match index {
        Expr::IntLit { value, .. } => *value as usize,
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "matrix-index LHS index must be a literal integer".into(),
            });
        },
    };
    let out = ctx.outputs.get(name).cloned().ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!(
                "matrix-index LHS target `{name}` is not a column-split output"
            ),
        }
    })?;
    if out.vars.len() <= 1 {
        return Err(LoweringError::UnsupportedShape {
            what: format!(
                "matrix-index LHS target `{name}` is not a column-split matrix"
            ),
        });
    }
    if idx_value >= out.vars.len() {
        return Err(LoweringError::UnsupportedShape {
            what: format!(
                "matrix-index LHS `{name}[{idx_value}]` out of bounds (matrix has {} columns)",
                out.vars.len()
            ),
        });
    }
    let value = lower_expr(ctx, rhs)?;
    let col_var = out.vars[idx_value];
    ctx.b
        .store(col_var, value, None, [])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))?;
    Ok(())
}

/// Right-hand-side matrix indexing: `m[i]` where `m` is a
/// `mat_n` returns a `vec_n` column. First-pass support is
/// restricted to literal-integer indices (the validator's R12
/// already enforces this for ESSL 1.00 array indexing). For an
/// Ident base that refers to a column-split matrix
/// input/output, the lookup skips the assemble step and loads
/// the column variable directly. Otherwise the matrix value is
/// produced first and `OpCompositeExtract` picks the column.
fn lower_index_rhs(
    ctx: &mut Ctx,
    base: &Expr,
    index: &Expr,
) -> Result<Word, LoweringError> {
    let base_kind = classify_arg_kind(ctx, base).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: "matrix index base type unknown".into(),
        }
    })?;
    let column_kind = match base_kind {
        TypeKind::Mat2 => TypeKind::Vec2,
        TypeKind::Mat3 => TypeKind::Vec3,
        TypeKind::Mat4 => TypeKind::Vec4,
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("indexing on non-matrix base type {base_kind:?}"),
            });
        },
    };
    let col_ty = spv_type_for_kind(ctx, column_kind).ok_or_else(|| {
        LoweringError::UnsupportedShape {
            what: format!("column type {column_kind:?} for matrix index is not lowered"),
        }
    })?;
    let idx_value = match index {
        Expr::IntLit { value, .. } => *value as u32,
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: "matrix index must be a literal integer".into(),
            });
        },
    };
    // Fast path: Ident base referring to a column-split input
    // or output. The columns are already separate variables, so
    // we can OpLoad the right one directly without assembling
    // the matrix first.
    if let Expr::Ident { name, .. } = base {
        if let Some(input) = ctx.inputs.get(name) {
            if input.vars.len() > 1 {
                let col_var = input.vars[idx_value as usize];
                let pointee = input.pointee_type;
                return ctx
                    .b
                    .load(pointee, None, col_var, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
        }
        if let Some(out) = ctx.outputs.get(name) {
            if out.vars.len() > 1 {
                let col_var = out.vars[idx_value as usize];
                return ctx
                    .b
                    .load(col_ty, None, col_var, None, [])
                    .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
            }
        }
    }
    // Slow path: lower the base to a matrix SSA value, then
    // composite-extract the column.
    let base_value = lower_expr(ctx, base)?;
    ctx.b
        .composite_extract(col_ty, None, base_value, [idx_value])
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
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
        TypeKind::Vec2 | TypeKind::Bvec2 | TypeKind::Ivec2 => 2u32,
        TypeKind::Vec3 | TypeKind::Bvec3 | TypeKind::Ivec3 => 3,
        TypeKind::Vec4 | TypeKind::Bvec4 | TypeKind::Ivec4 => 4,
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
    let mat_lhs = matches!(lhs_kind, TypeKind::Mat2 | TypeKind::Mat3 | TypeKind::Mat4);
    let mat_rhs = matches!(rhs_kind, TypeKind::Mat2 | TypeKind::Mat3 | TypeKind::Mat4);

    match op {
        BinOp::Add | BinOp::Sub => {
            if (scalar_lhs && scalar_rhs)
                || (vec_lhs && vec_rhs && lhs_kind == rhs_kind)
                || (mat_lhs && mat_rhs && lhs_kind == rhs_kind)
            {
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
        // Scalar float comparisons return Bool. ESSL also has
        // vector comparison builtins (lessThan, etc.); those go
        // through the §8.6 registry, not the binary-op path.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne
            if scalar_lhs && scalar_rhs =>
        {
            let bool_ty = ctx.type_bool;
            let r = match op {
                BinOp::Lt => ctx.b.f_ord_less_than(bool_ty, None, lhs_id, rhs_id),
                BinOp::Le => ctx.b.f_ord_less_than_equal(bool_ty, None, lhs_id, rhs_id),
                BinOp::Gt => ctx.b.f_ord_greater_than(bool_ty, None, lhs_id, rhs_id),
                BinOp::Ge => ctx.b.f_ord_greater_than_equal(bool_ty, None, lhs_id, rhs_id),
                BinOp::Eq => ctx.b.f_ord_equal(bool_ty, None, lhs_id, rhs_id),
                BinOp::Ne => ctx.b.f_ord_not_equal(bool_ty, None, lhs_id, rhs_id),
                _ => unreachable!(),
            };
            return r.map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
        },
        _ => {},
    }
    // Integer arithmetic and comparison. Driven by the typecheck
    // (lhs and rhs both classified as Int); needed for loop
    // counters and the for-loop `i < N` pattern.
    let int_lhs = matches!(lhs_kind, TypeKind::Int);
    let int_rhs = matches!(rhs_kind, TypeKind::Int);
    if int_lhs && int_rhs {
        let r = match op {
            BinOp::Add => ctx.b.i_add(result_ty, None, lhs_id, rhs_id),
            BinOp::Sub => ctx.b.i_sub(result_ty, None, lhs_id, rhs_id),
            BinOp::Mul => ctx.b.i_mul(result_ty, None, lhs_id, rhs_id),
            BinOp::Div => ctx.b.s_div(result_ty, None, lhs_id, rhs_id),
            BinOp::Lt => ctx.b.s_less_than(ctx.type_bool, None, lhs_id, rhs_id),
            BinOp::Le => ctx.b.s_less_than_equal(ctx.type_bool, None, lhs_id, rhs_id),
            BinOp::Gt => ctx.b.s_greater_than(ctx.type_bool, None, lhs_id, rhs_id),
            BinOp::Ge => ctx.b.s_greater_than_equal(ctx.type_bool, None, lhs_id, rhs_id),
            BinOp::Eq => ctx.b.i_equal(ctx.type_bool, None, lhs_id, rhs_id),
            BinOp::Ne => ctx.b.i_not_equal(ctx.type_bool, None, lhs_id, rhs_id),
            _ => {
                return Err(LoweringError::UnsupportedShape {
                    what: format!("integer binary `{op:?}` is not lowered"),
                });
            },
        };
        return r.map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
    }
    // Integer-vector arithmetic. ivec_n + / - / * / / ivec_n
    // emits component-wise i_add / i_sub / i_mul / s_div on the
    // vector operands. ivec_n * int splats the scalar.
    let ivec_lhs = matches!(lhs_kind, TypeKind::Ivec2 | TypeKind::Ivec3 | TypeKind::Ivec4);
    let ivec_rhs = matches!(rhs_kind, TypeKind::Ivec2 | TypeKind::Ivec3 | TypeKind::Ivec4);
    if (ivec_lhs && ivec_rhs && lhs_kind == rhs_kind)
        || (ivec_lhs && rhs_kind == TypeKind::Int)
        || (ivec_rhs && lhs_kind == TypeKind::Int)
    {
        // Splat the scalar side when needed so both operands are
        // the same ivec_n width.
        let (lhs_id, rhs_id) = if ivec_lhs && rhs_kind == TypeKind::Int {
            let splat = splat_int_to_ivec(ctx, rhs_id, lhs_kind)?;
            (lhs_id, splat)
        } else if ivec_rhs && lhs_kind == TypeKind::Int {
            let splat = splat_int_to_ivec(ctx, lhs_id, rhs_kind)?;
            (splat, rhs_id)
        } else {
            (lhs_id, rhs_id)
        };
        let r = match op {
            BinOp::Add => ctx.b.i_add(result_ty, None, lhs_id, rhs_id),
            BinOp::Sub => ctx.b.i_sub(result_ty, None, lhs_id, rhs_id),
            BinOp::Mul => ctx.b.i_mul(result_ty, None, lhs_id, rhs_id),
            BinOp::Div => ctx.b.s_div(result_ty, None, lhs_id, rhs_id),
            _ => {
                return Err(LoweringError::UnsupportedShape {
                    what: format!("ivec binary `{op:?}` is not lowered"),
                });
            },
        };
        return r.map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")));
    }
    Err(LoweringError::UnsupportedShape {
        what: format!("binary `{op:?}` on {lhs_kind:?} and {rhs_kind:?} not lowered"),
    })
}

/// Splat a scalar int to an ivec of `target_kind` width.
fn splat_int_to_ivec(
    ctx: &mut Ctx,
    scalar_id: Word,
    target_kind: TypeKind,
) -> Result<Word, LoweringError> {
    let (target_ty, count) = match target_kind {
        TypeKind::Ivec2 => (ctx.type_ivec2, 2usize),
        TypeKind::Ivec3 => (ctx.type_ivec3, 3),
        TypeKind::Ivec4 => (ctx.type_ivec4, 4),
        _ => {
            return Err(LoweringError::UnsupportedShape {
                what: format!("int splat target {target_kind:?} is not lowered"),
            });
        },
    };
    let constituents: Vec<Word> = std::iter::repeat(scalar_id).take(count).collect();
    ctx.b
        .composite_construct(target_ty, None, constituents)
        .map_err(|e| LoweringError::SpirvBuild(format!("{e:?}")))
}
