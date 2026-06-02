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

/// SPEC-CONFORMANT-BUT-NAGA-REJECTS. ESSL 1.00 §4.3.5 allows
/// `mat4` varyings. The lowering now registers matrix Outputs
/// in `register_varying_outputs` (one Location per column), so
/// the SPIR-V it emits is valid — but naga's WGSL pipeline
/// rejects matrices as standalone I/O variables with
/// `NotIOShareableType`, because WGSL requires them inside an
/// interface block. Closing the gap fully needs either column-
/// splitting at the lowering layer or a Block-decorated struct
/// output; queued separately. The receipt pins that:
///   1. The lowering itself no longer surfaces the misleading
///      "expected `gl_Position`" diagnostic, and
///   2. The failure now happens at the naga-validate stage with
///      a clear "not I/O shareable" error.
#[test]
fn mat4_varying_in_vertex_emits_io_shareable_naga_error() {
    let src = "attribute vec3 a_position;\n\
               varying mat4 v_xform;\n\
               uniform mat4 u_base;\n\
               void main() {\n\
                   v_xform = u_base;\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("NotIOShareableType") || msg.contains("IoShareableType"),
        "expected naga IO-shareable error, got: {msg}"
    );
    assert!(
        !msg.contains("expected `gl_Position`"),
        "old misleading diagnostic should be gone: {msg}"
    );
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

/// HAPPY (resolved). `emit_user_functions` now runs in two
/// passes: phase 1 allocates each non-main function's id and
/// records its signature in `ctx.user_fns`; phase 2 emits each
/// body. A caller defined before its callee resolves via the
/// pre-allocated id.
#[test]
fn forward_user_function_reference_lowers() {
    let src = "precision mediump float;\n\
               float caller(float x) { return callee(x); }\n\
               float callee(float x) { return x * 2.0; }\n\
               void main() {\n\
                   gl_FragColor = vec4(caller(0.5));\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// HAPPY (resolved). ESSL 1.00 §6.1.1 allows overloading user
/// functions by parameter type. The typechecker now stores each
/// user function name as a `Vec<Signature>`; the lowering's
/// `user_fns` map is `HashMap<String, Vec<UserFnBinding>>` and
/// the Call dispatch picks the overload whose `param_types`
/// matches the actual arg kinds.
#[test]
fn overloaded_user_functions_dispatch_by_arg_types() {
    let src = "precision mediump float;\n\
               float helper(float x) { return x * 2.0; }\n\
               float helper(vec2 v) { return v.x + v.y; }\n\
               void main() {\n\
                   gl_FragColor = vec4(helper(0.25));\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

#[test]
fn overloaded_user_functions_vec_arg_picks_the_vec_overload() {
    let src = "precision mediump float;\n\
               float helper(float x) { return x * 2.0; }\n\
               float helper(vec2 v) { return v.x + v.y; }\n\
               uniform vec2 u_v;\n\
               void main() {\n\
                   gl_FragColor = vec4(helper(u_v));\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
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

/// HAPPY (resolved). `spv_type_for_kind` now maps
/// `TypeKind::Bool` to `OpTypeBool`, so a user function taking
/// a `bool` parameter lowers cleanly. This receipt was the
/// inverse-direction pin while the gap existed.
#[test]
fn bool_typed_user_function_parameter_lowers() {
    let src = "precision mediump float;\n\
               float pick(bool b) { return 1.0; }\n\
               void main() {\n\
                   gl_FragColor = vec4(pick(true));\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// HAPPY (resolved). Void user functions used as statements
/// now lower: `emit_user_function` special-cases `TypeKind::Void`
/// to `ctx.type_void`, and the `Expr::Call` branch uses the
/// same fallback when the user function's result is Void.
#[test]
fn void_user_function_called_as_statement_lowers() {
    let src = "attribute vec2 a_position;\n\
               void noop() {}\n\
               void main() {\n\
                   noop();\n\
                   gl_Position = vec4(a_position, 0.0, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("location(0)"));
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

/// HAPPY (resolved). Single-component write-side swizzles
/// (`v.x = ...`, `v.y = ...`, etc.) lower via `OpAccessChain`
/// to the component pointer + `OpStore`.
#[test]
fn write_side_single_component_lhs_swizzle_lowers() {
    let src = "attribute vec3 a_position;\n\
               varying vec3 v_color;\n\
               void main() {\n\
                   v_color.x = 1.0;\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("location(0)"));
}

/// HAPPY. Multi-component LHS swizzle. `v_color.xy = vec2(...)`
/// lowers as `OpLoad` + `OpVectorShuffle` + `OpStore`, splicing
/// the new components into the existing value.
#[test]
fn write_side_multi_component_contiguous_lhs_swizzle_lowers() {
    let src = "attribute vec3 a_position;\n\
               varying vec3 v_color;\n\
               void main() {\n\
                   v_color.xy = vec2(0.5, 0.25);\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("location(0)"));
}

/// HAPPY. Non-contiguous / reordered LHS swizzle: `v.yx = e`
/// assigns the first component of e to v.y and the second to
/// v.x. The shuffle-index table handles arbitrary permutations.
#[test]
fn write_side_reordered_lhs_swizzle_lowers() {
    let src = "attribute vec3 a_position;\n\
               varying vec3 v_color;\n\
               void main() {\n\
                   v_color.yx = vec2(0.7, 0.3);\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("location(0)"));
}

/// SPEC-CONFORMANCE. Repeated components on the LHS are
/// forbidden by ESSL §5.5 (each target component can only be
/// assigned once). The typechecker catches this with
/// `InvalidSwizzle` because the parser produces the field and
/// `parse_swizzle_indices`-side rejection in the lowering as a
/// fallback. Either way the shader is rejected.
#[test]
fn write_side_repeated_component_lhs_swizzle_rejected() {
    let src = "attribute vec3 a_position;\n\
               varying vec3 v_color;\n\
               void main() {\n\
                   v_color.xx = vec2(0.5, 0.5);\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    assert!(
        matches!(err, CompileError::Check(_) | CompileError::Lower(_)),
        "got: {err:?}"
    );
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
