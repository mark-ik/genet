/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 6 spike: lower a typechecked ESSL translation unit to WGSL via
//! path A (ESSL → SPIR-V (rspirv) → naga IR (`spv-in`) → WGSL
//! (`wgsl-out`)).
//!
//! This is the smallest end-to-end proof of the pipeline. The lowering
//! today accepts only the narrow shape of the canonical shaders:
//!
//! * `void main() { gl_FragColor = vec4(C, C, C, C); }`
//! * `void main() { gl_Position = vec4(C, C, C, C); }`
//!
//! Anything else returns `LoweringError::UnsupportedShape`. Each
//! follow-up sub-step (attributes, uniforms, binary ops, function
//! calls, swizzles) is a localized extension; the SPIR-V emission
//! shape and the naga seam stay constant.

use rspirv::binary::Assemble;
use rspirv::dr::{Builder, Module, Operand};
use rspirv::spirv::{
    AddressingModel, BuiltIn, Capability, Decoration, ExecutionMode, ExecutionModel,
    FunctionControl, MemoryModel, StorageClass,
};

use crate::ast::{Expr, ExternalDecl, FunctionDef, Stmt, TranslationUnit};
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
    let main = find_main(tu)?;
    let constants = extract_const_color_assign(main, stage)?;

    let spirv = build_spirv_const_color(&constants, stage);
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

// ---------- AST analysis ----------------------------------------------

fn find_main(tu: &TranslationUnit) -> Result<&FunctionDef, LoweringError> {
    tu.decls
        .iter()
        .find_map(|d| match d {
            ExternalDecl::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .ok_or(LoweringError::NoMain)
}

fn extract_const_color_assign(
    main: &FunctionDef,
    stage: ShaderStage,
) -> Result<[f32; 4], LoweringError> {
    let expected_output = match stage {
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

    if lhs_name != expected_output {
        return Err(LoweringError::UnsupportedShape {
            what: format!(
                "main body assigns to `{lhs_name}`, expected `{expected_output}` for {stage:?}"
            ),
        });
    }

    match rhs {
        Expr::Call { callee, args, .. } if callee == "vec4" && args.len() == 4 => {
            let mut out = [0.0f32; 4];
            for (i, a) in args.iter().enumerate() {
                out[i] = match a {
                    Expr::FloatLit { value, .. } => *value as f32,
                    Expr::IntLit { value, .. } => *value as f32,
                    _ => {
                        return Err(LoweringError::UnsupportedShape {
                            what: format!("vec4 arg {i} is not a numeric literal"),
                        });
                    },
                };
            }
            Ok(out)
        },
        _ => Err(LoweringError::UnsupportedShape {
            what: "main body rhs is not vec4(C, C, C, C)".into(),
        }),
    }
}

// ---------- SPIR-V emission via rspirv --------------------------------

fn build_spirv_const_color(constants: &[f32; 4], stage: ShaderStage) -> Module {
    let mut b = Builder::new();
    b.set_version(1, 0);
    b.capability(Capability::Shader);
    b.memory_model(AddressingModel::Logical, MemoryModel::GLSL450);

    // Types.
    let type_void = b.type_void();
    let type_float = b.type_float(32, None);
    let type_vec4 = b.type_vector(type_float, 4);

    // Output variable in the Output storage class.
    let ptr_output = b.type_pointer(None, StorageClass::Output, type_vec4);
    let output_var = b.variable(ptr_output, None, StorageClass::Output, None);

    // Decorate per shader stage:
    //   * Vertex: gl_Position is built-in Position.
    //   * Fragment: gl_FragColor is a Location 0 output (the WebGL 1
    //     convention for the single color buffer).
    match stage {
        ShaderStage::Vertex => {
            b.decorate(output_var, Decoration::BuiltIn, [Operand::BuiltIn(BuiltIn::Position)]);
        },
        ShaderStage::Fragment => {
            b.decorate(output_var, Decoration::Location, [Operand::LiteralBit32(0)]);
        },
    }

    // Constants. rspirv 0.13 takes the bit pattern via `constant_bit32`.
    let c0 = b.constant_bit32(type_float, constants[0].to_bits());
    let c1 = b.constant_bit32(type_float, constants[1].to_bits());
    let c2 = b.constant_bit32(type_float, constants[2].to_bits());
    let c3 = b.constant_bit32(type_float, constants[3].to_bits());
    let color = b.constant_composite(type_vec4, [c0, c1, c2, c3]);

    // void main() function.
    let fn_type = b.type_function(type_void, []);
    let main_fn = b
        .begin_function(type_void, None, FunctionControl::NONE, fn_type)
        .expect("begin_function");
    b.begin_block(None).expect("begin_block");
    b.store(output_var, color, None, []).expect("store");
    b.ret().expect("ret");
    b.end_function().expect("end_function");

    // Entry point + execution mode.
    let execution_model = match stage {
        ShaderStage::Vertex => ExecutionModel::Vertex,
        ShaderStage::Fragment => ExecutionModel::Fragment,
    };
    b.entry_point(execution_model, main_fn, "main", [output_var]);
    if stage == ShaderStage::Fragment {
        b.execution_mode(main_fn, ExecutionMode::OriginUpperLeft, []);
    }

    b.module()
}
