/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Built-in function registry and the [`Signature`] vocabulary the
//! typecheck visitor uses for callable lookup.
//!
//! Sourced from the ESSL 1.00 spec §8 (built-in functions). Vector
//! relational built-ins (§8.6: `lessThan`, `any`, `not`, ...) are
//! deliberately skipped this pass because they return `bvec_n`, which
//! is not yet in [`crate::ast::TypeKind`]. Adding bvec / ivec is a
//! separate parser + AST change.

use std::collections::HashMap;

use crate::ast::TypeKind;

/// One overload of a callable: a parameter type list and a result.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub params: Vec<TypeKind>,
    pub result: TypeKind,
}

impl Signature {
    pub fn matches(&self, args: &[TypeKind]) -> bool {
        self.params.len() == args.len()
            && self.params.iter().zip(args).all(|(p, a)| p == a)
    }
}

/// Multi-signature lookup table. Built-ins are populated once at
/// construction; user-defined functions live in the scope stack
/// (so each carries its own [`Signature`] via [`super::ScopeEntry`]).
#[derive(Debug, Default)]
pub struct Registry {
    signatures: HashMap<String, Vec<Signature>>,
}

impl Registry {
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        populate(&mut r);
        r
    }

    pub fn register(&mut self, name: &str, params: Vec<TypeKind>, result: TypeKind) {
        self.signatures
            .entry(name.to_string())
            .or_default()
            .push(Signature { params, result });
    }

    /// Try to find an overload of `name` whose parameter types match
    /// `args` exactly. Returns the matching signature on hit; returns
    /// `LookupOutcome::ArityOrTypeMismatch` with the candidate set if
    /// the name is known but no overload accepts these args. Returns
    /// `LookupOutcome::Unknown` if the name is not registered.
    pub fn lookup(&self, name: &str, args: &[TypeKind]) -> LookupOutcome<'_> {
        match self.signatures.get(name) {
            Some(sigs) => match sigs.iter().find(|s| s.matches(args)) {
                Some(sig) => LookupOutcome::Match(sig),
                None => LookupOutcome::Mismatch(sigs.as_slice()),
            },
            None => LookupOutcome::Unknown,
        }
    }
}

#[derive(Debug)]
pub enum LookupOutcome<'a> {
    Match(&'a Signature),
    Mismatch(&'a [Signature]),
    Unknown,
}

// ---------- ESSL 1.00 §8 ---------------------------------------------

fn populate(r: &mut Registry) {
    use TypeKind::*;
    let vec_t: &[TypeKind] = &[Float, Vec2, Vec3, Vec4];

    // §8.1 Angle and Trigonometry. Each is T -> T over vec_t.
    for name in ["radians", "degrees", "sin", "cos", "tan", "asin", "acos", "atan"] {
        for &t in vec_t {
            r.register(name, vec![t], t);
        }
    }
    // atan(T, T) -> T overload.
    for &t in vec_t {
        r.register("atan", vec![t, t], t);
    }

    // §8.2 Exponential.
    for name in ["exp", "log", "exp2", "log2", "sqrt", "inversesqrt"] {
        for &t in vec_t {
            r.register(name, vec![t], t);
        }
    }
    for &t in vec_t {
        r.register("pow", vec![t, t], t);
    }

    // §8.3 Common.
    for name in ["abs", "sign", "floor", "ceil", "fract"] {
        for &t in vec_t {
            r.register(name, vec![t], t);
        }
    }
    // mod / min / max: (T, T) and (T, float) for vec T.
    for name in ["mod", "min", "max"] {
        for &t in vec_t {
            r.register(name, vec![t, t], t);
            if t != Float {
                r.register(name, vec![t, Float], t);
            }
        }
    }
    for &t in vec_t {
        r.register("clamp", vec![t, t, t], t);
        if t != Float {
            r.register("clamp", vec![t, Float, Float], t);
        }
        r.register("mix", vec![t, t, t], t);
        if t != Float {
            r.register("mix", vec![t, t, Float], t);
        }
        r.register("step", vec![t, t], t);
        if t != Float {
            r.register("step", vec![Float, t], t);
        }
        r.register("smoothstep", vec![t, t, t], t);
        if t != Float {
            r.register("smoothstep", vec![Float, Float, t], t);
        }
    }

    // §8.4 Geometric. length / distance / dot collapse to float.
    for &t in vec_t {
        r.register("length", vec![t], Float);
        r.register("distance", vec![t, t], Float);
        r.register("dot", vec![t, t], Float);
        r.register("normalize", vec![t], t);
        r.register("faceforward", vec![t, t, t], t);
        r.register("reflect", vec![t, t], t);
        r.register("refract", vec![t, t, Float], t);
    }
    r.register("cross", vec![Vec3, Vec3], Vec3);

    // §8.5 Matrix.
    for t in [Mat2, Mat3, Mat4] {
        r.register("matrixCompMult", vec![t, t], t);
    }

    // §8.6 Vector relational. Per-component compare/test on
    // vectors; the result has matching width but element type
    // Bool (bvec_n).
    let vec_to_bvec = [(Vec2, Bvec2), (Vec3, Bvec3), (Vec4, Bvec4)];
    for &(v, bv) in &vec_to_bvec {
        for name in [
            "lessThan",
            "lessThanEqual",
            "greaterThan",
            "greaterThanEqual",
            "equal",
            "notEqual",
        ] {
            r.register(name, vec![v, v], bv);
        }
    }
    // Reduction: any/all collapse a bvec to a scalar bool; not
    // negates component-wise.
    for &(_, bv) in &vec_to_bvec {
        r.register("any", vec![bv], Bool);
        r.register("all", vec![bv], Bool);
        r.register("not", vec![bv], bv);
    }

    // §8.7 Texture lookup.
    r.register("texture2D", vec![Sampler2D, Vec2], Vec4);
    r.register("texture2D", vec![Sampler2D, Vec2, Float], Vec4);
    r.register("texture2DProj", vec![Sampler2D, Vec3], Vec4);
    r.register("texture2DProj", vec![Sampler2D, Vec4], Vec4);
    r.register("texture2DLod", vec![Sampler2D, Vec2, Float], Vec4);
    r.register("texture2DProjLod", vec![Sampler2D, Vec3, Float], Vec4);
    r.register("texture2DProjLod", vec![Sampler2D, Vec4, Float], Vec4);
    r.register("textureCube", vec![SamplerCube, Vec3], Vec4);
    r.register("textureCube", vec![SamplerCube, Vec3, Float], Vec4);
    r.register("textureCubeLod", vec![SamplerCube, Vec3, Float], Vec4);
}
