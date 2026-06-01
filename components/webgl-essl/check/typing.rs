/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pure type rules consumed by the typecheck visitor: binary-op
//! result types, unary-op result types, constructor signatures, and
//! swizzle field rules. Each function is `Option<TypeKind>` returning
//! so the visitor can route a `None` result into a diagnostic.

use crate::ast::{BinOp, TypeKind, UnaryOp};

/// Result type for a binary operator on already-typed operands.
/// Returns None when the operand pair does not match any ESSL rule.
pub(super) fn binary_result(op: BinOp, lhs: TypeKind, rhs: TypeKind) -> Option<TypeKind> {
    use BinOp::*;
    use TypeKind::*;
    match op {
        Add | Sub | Mul | Div | Rem => arithmetic_result(op, lhs, rhs),
        Lt | Le | Gt | Ge => {
            // Scalar numeric only: int < int, float < float.
            if matches!(lhs, Int | Float) && lhs == rhs {
                Some(Bool)
            } else {
                None
            }
        },
        Eq | Ne => {
            // Any same-type comparison; result is bool. Void is the
            // only type that can never participate.
            if lhs == rhs && !matches!(lhs, Void) {
                Some(Bool)
            } else {
                None
            }
        },
        LogAnd | LogOr => {
            if lhs == Bool && rhs == Bool {
                Some(Bool)
            } else {
                None
            }
        },
    }
}

fn arithmetic_result(op: BinOp, lhs: TypeKind, rhs: TypeKind) -> Option<TypeKind> {
    use BinOp::*;
    use TypeKind::*;
    // Same type: result is that type, modulo non-arithmetic kinds.
    if lhs == rhs {
        return match lhs {
            Int | Float | Vec2 | Vec3 | Vec4 | Mat2 | Mat3 | Mat4 => Some(lhs),
            _ => None,
        };
    }
    // Scalar broadcast: float-vec or float-mat works for +, -, *, /.
    // ESSL 1.00 has no ivec / imat, so int broadcast does not apply.
    let (scalar, vector_or_matrix) = if lhs == Float {
        (lhs, rhs)
    } else if rhs == Float {
        (rhs, lhs)
    } else {
        // No scalar; check matrix-vector mul for `*` only.
        return if matches!(op, Mul) { matrix_mul(lhs, rhs) } else { None };
    };
    let _ = scalar;
    match vector_or_matrix {
        Vec2 | Vec3 | Vec4 | Mat2 | Mat3 | Mat4 => Some(vector_or_matrix),
        _ => None,
    }
}

fn matrix_mul(lhs: TypeKind, rhs: TypeKind) -> Option<TypeKind> {
    use TypeKind::*;
    match (lhs, rhs) {
        (Mat2, Vec2) => Some(Vec2),
        (Mat3, Vec3) => Some(Vec3),
        (Mat4, Vec4) => Some(Vec4),
        (Vec2, Mat2) => Some(Vec2),
        (Vec3, Mat3) => Some(Vec3),
        (Vec4, Mat4) => Some(Vec4),
        _ => None,
    }
}

/// Result type for a prefix or postfix unary operator. Returns None
/// when the operator does not accept the operand type.
pub(super) fn unary_result(op: UnaryOp, operand: TypeKind) -> Option<TypeKind> {
    use TypeKind::*;
    use UnaryOp::*;
    match op {
        Neg | Pos => match operand {
            Int | Float | Vec2 | Vec3 | Vec4 | Mat2 | Mat3 | Mat4 => Some(operand),
            _ => None,
        },
        Not => {
            if operand == Bool {
                Some(Bool)
            } else {
                None
            }
        },
        PreInc | PreDec | PostInc | PostDec => match operand {
            Int | Float | Vec2 | Vec3 | Vec4 | Mat2 | Mat3 | Mat4 => Some(operand),
            _ => None,
        },
    }
}

/// Result type for `vec_n(args)` / `mat_n(args)` / `float(arg)` etc.
/// Constructor rules follow ESSL 1.00 §5.4.2: scalar promotes to all
/// components; vectors and scalars combine to fill the component count;
/// matrices truncate / promote per the spec.
pub(super) fn constructor_result(name: &str, args: &[TypeKind]) -> Option<TypeKind> {
    use TypeKind::*;
    let target = match name {
        "float" => CtorTarget::Scalar(Float),
        "int" => CtorTarget::Scalar(Int),
        "bool" => CtorTarget::Scalar(Bool),
        "vec2" => CtorTarget::Vec(2, Vec2),
        "vec3" => CtorTarget::Vec(3, Vec3),
        "vec4" => CtorTarget::Vec(4, Vec4),
        "mat2" => CtorTarget::Mat(2, Mat2),
        "mat3" => CtorTarget::Mat(3, Mat3),
        "mat4" => CtorTarget::Mat(4, Mat4),
        _ => return None,
    };
    match target {
        CtorTarget::Scalar(ty) => {
            // Scalar constructors take exactly one scalar arg.
            if args.len() == 1 && is_scalar(args[0]) {
                Some(ty)
            } else {
                None
            }
        },
        CtorTarget::Vec(n, ty) => {
            if args.len() == 1 {
                // Single scalar: broadcast. Single vector: copy or
                // truncate from a wider vector.
                match args[0] {
                    Int | Float | Bool => Some(ty),
                    Vec2 | Vec3 | Vec4 => {
                        let m = vec_size(args[0])?;
                        if m >= n {
                            Some(ty)
                        } else {
                            None
                        }
                    },
                    _ => None,
                }
            } else {
                let total: u32 = args.iter().map(|t| component_count(*t).unwrap_or(0)).sum();
                if total == n {
                    Some(ty)
                } else {
                    None
                }
            }
        },
        CtorTarget::Mat(n, ty) => {
            if args.len() == 1 {
                match args[0] {
                    Float => Some(ty),
                    Mat2 | Mat3 | Mat4 => Some(ty),
                    _ => None,
                }
            } else {
                let total: u32 = args.iter().map(|t| component_count(*t).unwrap_or(0)).sum();
                if total == n * n {
                    Some(ty)
                } else {
                    None
                }
            }
        },
    }
}

enum CtorTarget {
    Scalar(TypeKind),
    Vec(u32, TypeKind),
    Mat(u32, TypeKind),
}

fn is_scalar(ty: TypeKind) -> bool {
    matches!(ty, TypeKind::Float | TypeKind::Int | TypeKind::Bool)
}

fn vec_size(ty: TypeKind) -> Option<u32> {
    match ty {
        TypeKind::Vec2 => Some(2),
        TypeKind::Vec3 => Some(3),
        TypeKind::Vec4 => Some(4),
        _ => None,
    }
}

fn component_count(ty: TypeKind) -> Option<u32> {
    use TypeKind::*;
    match ty {
        Float | Int | Bool => Some(1),
        Vec2 => Some(2),
        Vec3 => Some(3),
        Vec4 => Some(4),
        Mat2 => Some(4),
        Mat3 => Some(9),
        Mat4 => Some(16),
        _ => None,
    }
}

/// Swizzle result: `<vec>.field` where field is 1-4 chars from one of
/// the three swizzle sets (xyzw / rgba / stpq), each indexing within
/// the base vector's size.
pub(super) fn swizzle_result(base: TypeKind, field: &str) -> Option<TypeKind> {
    let base_size = vec_size(base)?;
    let chars: Vec<char> = field.chars().collect();
    if chars.is_empty() || chars.len() > 4 {
        return None;
    }
    const SWIZZLE_SETS: [&[char]; 3] =
        [&['x', 'y', 'z', 'w'], &['r', 'g', 'b', 'a'], &['s', 't', 'p', 'q']];
    let set = SWIZZLE_SETS.iter().find(|s| chars.iter().all(|c| s.contains(c)))?;
    for c in &chars {
        let idx = set.iter().position(|sc| sc == c)? as u32;
        if idx >= base_size {
            return None;
        }
    }
    match chars.len() {
        1 => Some(TypeKind::Float),
        2 => Some(TypeKind::Vec2),
        3 => Some(TypeKind::Vec3),
        4 => Some(TypeKind::Vec4),
        _ => None,
    }
}
