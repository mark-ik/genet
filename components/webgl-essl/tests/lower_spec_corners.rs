/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 6 fifth-widening verification receipts. After the
//! varyings + user-functions + swizzles widening landed, a parallel
//! audit (workflow `webgl-essl-step6-fifth-widening-verify`) returned
//! twelve high-signal scenarios across the ESSL 1.00 spec corners.
//! Each receipt pins what the current lowering actually does so a
//! future widening that fixes the spec gap will flip the assertion
//! intentionally rather than silently.
//!
//! Each test header notes the spec citation and whether the current
//! behavior matches the spec or is a known narrow-shape gap.

use webgl_essl::validate::ShaderStage;
use webgl_essl::{CompileError, compile};

// ---------- varying widening corners ----------------------------------

/// SPEC-GAP. ESSL 1.00 §4.3.5 allows `mat4` varyings, but
/// `register_varying_outputs` only handles float / vec_n. The mat4
/// silently doesn't register, so the assignment to `v_xform` then
/// fails the lowering with the "expected gl_Position" diagnostic
/// shape (the misleading-diagnostic pin from the audit).
#[test]
fn mat4_varying_in_vertex_does_not_lower_today() {
    let src = "attribute vec3 a_position;\n\
               varying mat4 v_xform;\n\
               uniform mat4 u_base;\n\
               void main() {\n\
                   v_xform = u_base;\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    assert!(matches!(err, CompileError::Lower(_)), "got: {err:?}");
}

/// HAPPY. Two varyings of different widths in one vertex shader
/// must both reach the lowered WGSL with their own `@location`s.
/// Pins that `register_varying_outputs` walks decls sequentially
/// and that naga's spv-in linker assigns the locations the
/// emit step put on each variable.
#[test]
fn two_varyings_of_different_widths_lower_with_two_locations() {
    let src = "attribute vec4 a_position;\n\
               varying vec2 v_uv;\n\
               varying vec3 v_color;\n\
               void main() {\n\
                   v_uv = vec2(0.0, 0.0);\n\
                   v_color = vec3(1.0, 1.0, 1.0);\n\
                   gl_Position = a_position;\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("location(0)"));
    assert!(r.wgsl.contains("location(1)"));
}

// ---------- user-function widening corners ----------------------------

/// SPEC-GAP. ESSL 1.00 §6.1 permits a function to be referenced
/// before its definition appears at file scope, but
/// `emit_user_functions` walks declarations top-down and only
/// inserts a binding *after* the body has been emitted. A caller
/// defined before its callee currently fails lowering.
#[test]
fn forward_user_function_reference_does_not_lower_today() {
    let src = "precision mediump float;\n\
               float caller(float x) { return callee(x); }\n\
               float callee(float x) { return x * 2.0; }\n\
               void main() {\n\
                   gl_FragColor = vec4(caller(0.5));\n\
               }\n";
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Lower(_)), "got: {err:?}");
}

/// SPEC-GAP / CORRECTNESS. ESSL 1.00 §6.1.1 allows overloading
/// user functions by parameter type. The current binding store is
/// `HashMap<String, _>`, so the second `helper` silently overwrites
/// the first; the typechecker then sees only the `vec2`-taking
/// candidate and rejects `helper(0.25)` with `CallSignatureMismatch`.
/// (Pre-lowering rejection — the silent-overwrite is at the
/// `user_fns` binding step, not at the call site.)
///
/// Pins the boundary: a future widening that admits overloads at
/// the typechecker layer would flip this assertion deliberately.
#[test]
fn overloaded_user_functions_do_not_lower_today() {
    let src = "precision mediump float;\n\
               float helper(float x) { return x * 2.0; }\n\
               float helper(vec2 v) { return v.x + v.y; }\n\
               void main() {\n\
                   gl_FragColor = vec4(helper(0.25));\n\
               }\n";
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Check(_)), "got: {err:?}");
}

/// HAPPY (resolved). `emit_user_function` now walks
/// multi-statement bodies via `lower_stmt`. Local decl then
/// return — the most common real-world function shape — lowers
/// cleanly. This receipt was the inverse-direction pin while the
/// gap existed; it is now a forward receipt.
#[test]
fn multi_statement_user_function_body_lowers() {
    let src = "precision mediump float;\n\
               float helper(float x) {\n\
                   float t = x * 2.0;\n\
                   return t;\n\
               }\n\
               void main() {\n\
                   gl_FragColor = vec4(helper(0.5));\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// SPEC-GAP. ESSL 1.00 §6.1 allows `bool` user-function parameters.
/// `spv_type_for_kind` does not map `TypeKind::Bool`, so
/// `emit_user_function` short-circuits the parameter-type lookup.
#[test]
fn bool_typed_user_function_parameter_does_not_lower_today() {
    let src = "precision mediump float;\n\
               float pick(bool b) { return 1.0; }\n\
               void main() {\n\
                   gl_FragColor = vec4(pick(true));\n\
               }\n";
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Lower(_)), "got: {err:?}");
}

/// INTERNAL-CONTRADICTION. `lower_main_body` has a dedicated
/// `Stmt::Expr(Call)` branch that suggests void-as-statement is
/// supported. But `spv_type_for_kind` returns `None` for `Void`,
/// so `emit_user_function` fails at parameter-type lookup time.
/// The two paths contradict each other; the receipt locks in the
/// current observable behavior.
#[test]
fn void_user_function_called_as_statement_does_not_lower_today() {
    let src = "attribute vec2 a_position;\n\
               void noop() {}\n\
               void main() {\n\
                   noop();\n\
                   gl_Position = vec4(a_position, 0.0, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    assert!(matches!(err, CompileError::Lower(_)), "got: {err:?}");
}

// ---------- swizzle widening corners ----------------------------------

/// HAPPY. ESSL 1.00 §5.5 lists three independent component sets:
/// xyzw / rgba / stpq. Existing receipts cover xyzw and rgba;
/// this one locks in stpq as the third lane that round-trips
/// through naga's spv-in.
#[test]
fn stpq_swizzle_set_on_vec4_uniform_lowers() {
    let src = "precision mediump float;\n\
               uniform vec4 u_coord;\n\
               void main() {\n\
                   gl_FragColor = u_coord.stpq;\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// SPEC-CONFORMANCE. ESSL 1.00 §5.5 forbids mixing component sets
/// in one swizzle. The typechecker rejects with `InvalidSwizzle`
/// before lowering ever runs, so the error surfaces as
/// `CompileError::Check`. Pins the reject path no single-set
/// happy-path test exercises.
#[test]
fn mixed_set_swizzle_xrs_does_not_lower() {
    let src = "precision mediump float;\n\
               uniform vec4 u_color;\n\
               void main() {\n\
                   gl_FragColor = vec4(u_color.xrs, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Check(_)), "got: {err:?}");
}

/// HAPPY. ESSL 1.00 §5.5 permits repeated components on the
/// read side. `.xxxx` is a classic broadcast splat; the
/// lowering emits `OpVectorShuffle [0,0,0,0]`. Pins that naga's
/// spv-in accepts the duplicated-index shuffle.
#[test]
fn repeat_component_swizzle_xxxx_lowers_as_broadcast() {
    let src = "precision mediump float;\n\
               uniform vec4 u_color;\n\
               void main() {\n\
                   gl_FragColor = u_color.xxxx;\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// SPEC-GAP. ESSL 1.00 §5.5 allows non-repeating swizzles as
/// assignment LHS. `lower_main_body` only accepts `Expr::Ident`
/// as the assignment LHS today; an `Expr::Member` (write-side
/// swizzle) falls into "main body lhs is not an identifier".
#[test]
fn write_side_lhs_swizzle_does_not_lower_today() {
    let src = "attribute vec3 a_position;\n\
               varying vec3 v_color;\n\
               void main() {\n\
                   v_color.x = 1.0;\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    assert!(matches!(err, CompileError::Lower(_)), "got: {err:?}");
}

/// SPEC-CONFORMANCE. ESSL 1.00 §5.5: a `vec2` has components
/// `x` and `y` only. `.z` is out of bounds. The typechecker
/// catches this first as `InvalidSwizzle`, so the error surfaces
/// as `CompileError::Check`. Pins the bounds-check reject.
#[test]
fn out_of_bounds_swizzle_z_on_vec2_does_not_lower() {
    let src = "precision mediump float;\n\
               uniform vec2 u_uv;\n\
               void main() {\n\
                   gl_FragColor = vec4(u_uv.z, 0.0, 0.0, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Check(_)), "got: {err:?}");
}
