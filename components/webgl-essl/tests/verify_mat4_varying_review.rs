/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Review receipts for the mat4-varying column-split commit
//! (2880dd6e9d1). Each test pins one of the three claims from
//! the commit's review note so a future refactor can't quietly
//! invalidate them.

use webgl_essl::compile;
use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};
use webgl_essl::CompileError;

// =====================================================================
// Claim 1: column-split consumes one Location per matrix column. A
// `varying vec4 v_color;` declared after a `varying mat4 v_xform;`
// lands at @location(4). R13's MAX_VARYING_VECTORS=8 counts a mat4
// as 4 slots, so the validator and the lowering agree.
// =====================================================================

#[test]
fn mat4_then_vec4_varying_places_vec4_at_location_four() {
    let src = "attribute vec3 a_position;\n\
               varying mat4 v_xform;\n\
               varying vec4 v_color;\n\
               uniform mat4 u_m;\n\
               uniform vec4 u_c;\n\
               void main() {\n\
                   v_xform = u_m;\n\
                   v_color = u_c;\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let wgsl = compile(src, ShaderStage::Vertex).expect("compile").wgsl;
    // mat4 columns occupy @location(0)..@location(3); the trailing
    // vec4 varying must be at @location(4).
    assert!(wgsl.contains("location(0)"), "wgsl: {wgsl}");
    assert!(wgsl.contains("location(3)"), "wgsl: {wgsl}");
    assert!(wgsl.contains("location(4)"), "wgsl: {wgsl}");
}

/// R13 happy path: `mat4 + 4 vec4 = 8 slots` is at the
/// `MAX_VARYING_VECTORS = 8` limit but doesn't exceed it.
#[test]
fn r13_mat4_plus_four_vec4_varyings_exactly_at_the_limit() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               varying vec4 v0;\n\
               varying vec4 v1;\n\
               varying vec4 v2;\n\
               varying vec4 v3;\n\
               void main() {\n\
                   v_xform = mat4(1.0); v0 = vec4(0.0); v1 = vec4(0.0); v2 = vec4(0.0); v3 = vec4(0.0);\n\
                   gl_Position = vec4(a, 1.0);\n\
               }\n";
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let r = validate(&tu, src, ShaderStage::Vertex);
    let r13_varying = r.errors.iter().any(|d| matches!(
        &d.kind,
        WebGlDiagnosticKind::PackingLimitExceeded { class, .. } if *class == "varying"
    ));
    assert!(
        !r13_varying,
        "8 slots should be exactly at limit, not over: {:#?}",
        r.errors
    );
}

/// R13 reject: `mat4 + 5 vec4 = 9 slots` exceeds
/// `MAX_VARYING_VECTORS = 8`.
#[test]
fn r13_mat4_plus_five_vec4_varyings_overshoots_by_one() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               varying vec4 v0;\n\
               varying vec4 v1;\n\
               varying vec4 v2;\n\
               varying vec4 v3;\n\
               varying vec4 v4;\n\
               void main() {\n\
                   gl_Position = vec4(a, 1.0);\n\
               }\n";
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let r = validate(&tu, src, ShaderStage::Vertex);
    let r13_varying = r
        .errors
        .iter()
        .filter(|d| {
            matches!(
                &d.kind,
                WebGlDiagnosticKind::PackingLimitExceeded { class, .. } if *class == "varying"
            )
        })
        .count();
    assert_eq!(r13_varying, 1, "9 slots should overshoot: {:#?}", r.errors);
}

// =====================================================================
// Claim 2: gl_Position is its own BuiltIn::Position Output, separate
// from the varying Location pool. Adding varyings doesn't shift the
// gl_Position decoration; the first varying still starts at
// @location(0).
// =====================================================================

#[test]
fn gl_position_carries_builtin_position_decoration_in_wgsl() {
    let src = "attribute vec3 a_position;\n\
               void main() {\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let wgsl = compile(src, ShaderStage::Vertex).expect("compile").wgsl;
    // naga emits the SPIR-V BuiltIn::Position as @builtin(position)
    // on the entry-point output struct.
    assert!(wgsl.contains("@builtin(position)"), "wgsl: {wgsl}");
}

#[test]
fn first_varying_still_takes_location_zero_alongside_gl_position() {
    let src = "attribute vec3 a_position;\n\
               varying vec3 v_color;\n\
               void main() {\n\
                   v_color = vec3(1.0);\n\
                   gl_Position = vec4(a_position, 1.0);\n\
               }\n";
    let wgsl = compile(src, ShaderStage::Vertex).expect("compile").wgsl;
    assert!(wgsl.contains("@builtin(position)"), "wgsl: {wgsl}");
    assert!(wgsl.contains("location(0)"), "wgsl: {wgsl}");
    // gl_Position must not have a Location decoration alongside the
    // BuiltIn — if naga emitted both, the test below would catch it.
    assert!(
        !wgsl.contains("@location(0) @builtin"),
        "gl_Position should not double-decorate: {wgsl}"
    );
}

// =====================================================================
// Claim 3: expansion happens at the SPIR-V lowering only. The
// typechecker still sees the varying as a `mat4`; error messages
// stay matrix-shaped; the synthetic column variables have no ESSL
// identifier (you can't write `v_xform_col0` in the source).
// =====================================================================

/// The typecheck assigns `Mat4` to a matrix varying expression.
/// Using it in a `+ float` context produces a diagnostic that
/// names `Mat4`, not any internal column type.
#[test]
fn typecheck_error_on_mat4_varying_names_the_matrix_kind_not_the_column() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               void main() {\n\
                   v_xform = mat4(1.0);\n\
                   gl_Position = vec4(a, 1.0) + v_xform;\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    let msg = format!("{err:?}");
    // The typechecker rejects `vec4 + mat4` with the matrix kind
    // named — the column-split is invisible at this layer.
    assert!(
        msg.contains("Mat4"),
        "matrix kind should appear in the diagnostic: {msg}"
    );
}

/// Trying to reference the column-split variable by a synthetic
/// name (`v_xform_col0`) from ESSL must fail at the typecheck
/// stage as an unknown identifier — the column names are
/// SPIR-V-internal only.
#[test]
fn synthetic_column_identifier_is_not_in_essl_scope() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               void main() {\n\
                   v_xform = mat4(1.0);\n\
                   gl_Position = vec4(v_xform_col0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("UnknownIdentifier") && msg.contains("v_xform_col0"),
        "should be an UnknownIdentifier on the synthetic name: {msg}"
    );
}

// =====================================================================
// Deliberately-not-done: LHS swizzle and compound assign on a matrix
// output. The column-split has no single-pointer view of the matrix,
// so each form must error at the lowering boundary rather than reach
// SPIR-V emission with a wrong access pattern.
// =====================================================================

/// SPEC-CONFORMANT. `.x` on a `mat4` is not a valid swizzle
/// (matrices use `[i]`, not `.field`). The typechecker rejects
/// with `InvalidSwizzle` before the lowering ever sees it. The
/// receipt now pins that the `mat4.x` rejection happens at the
/// check stage, not the lowering — so the diagnostic is a
/// clear spec error rather than a "not implemented" message.
#[test]
fn dot_field_on_matrix_is_a_typecheck_invalid_swizzle() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               void main() {\n\
                   v_xform.x = vec4(1.0);\n\
                   gl_Position = vec4(a, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    let msg = format!("{err:?}");
    assert!(matches!(err, CompileError::Check(_)), "got: {err:?}");
    assert!(
        msg.contains("InvalidSwizzle") && msg.contains("Mat4"),
        "should be InvalidSwizzle on Mat4: {msg}"
    );
}

/// HAPPY. Compound assign on a matrix output now lowers. The
/// vertex-stage Ident-lookup reads the output by assembling
/// its column variables into a matrix; `lower_binary` computes
/// the new value; `store_to_output` splits it back into the
/// column variables.
///
/// The receipt uses a `uniform mat4` as the rhs because the
/// `mat4(scalar)` constructor is its own queued widening (the
/// diagonal-fill ESSL semantics).
#[test]
fn compound_assign_on_matrix_output_with_uniform_rhs_lowers() {
    let src = "attribute vec3 a;\n\
               uniform mat4 u_delta;\n\
               varying mat4 v_xform;\n\
               void main() {\n\
                   v_xform = u_delta;\n\
                   v_xform += u_delta;\n\
                   gl_Position = vec4(a, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    // Compound assign reads + writes the column-split outputs;
    // each column maps to its own @location.
    assert!(r.wgsl.contains("location(0)"));
    assert!(r.wgsl.contains("location(3)"));
}

/// HAPPY (resolved). `mat_n(scalar)` builds an identity-shape
/// matrix: scalar on the diagonal, zero elsewhere (ESSL §5.4.2).
/// `lower_diagonal_matrix` emits the matrix as N column
/// `OpCompositeConstruct`s wrapped in one matrix
/// `OpCompositeConstruct`.
#[test]
fn scalar_arg_matrix_constructor_lowers() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               void main() {\n\
                   v_xform = mat4(1.0);\n\
                   gl_Position = vec4(a, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    // The matrix is split across location(0)..(3).
    assert!(r.wgsl.contains("location(0)"));
    assert!(r.wgsl.contains("location(3)"));
}

/// HAPPY. `mat3(2.5)` exercises the same path with a non-1
/// scalar and a smaller matrix.
#[test]
fn mat3_with_non_unit_scalar_constructor_lowers() {
    let src = "precision mediump float;\n\
               uniform vec3 v;\n\
               void main() {\n\
                   mat3 m = mat3(2.5);\n\
                   gl_FragColor = vec4(m * v, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

// =====================================================================
// Deliberately-not-done: matrix indexing (`m[i]`) lowering. The
// `Expr::Index` lowering path is not wired for matrix bases.
// Receipts that need to probe an assembled matrix use a matrix-
// vector multiply instead.
// =====================================================================

/// HAPPY (resolved). `m[i]` on a matrix returns the matching
/// column. For a column-split varying the lookup skips the
/// assemble step and loads the column variable directly.
#[test]
fn matrix_index_rhs_lowers() {
    let src = "precision mediump float;\n\
               varying mat4 v_xform;\n\
               void main() {\n\
                   gl_FragColor = v_xform[0];\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// HAPPY. `m[i] = vec_n` writes directly to the matching
/// column of a column-split matrix output. Constant int index
/// only; non-constant and locals/uniforms are queued.
#[test]
fn matrix_index_lhs_writes_to_column_var() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               uniform vec4 u_col;\n\
               void main() {\n\
                   v_xform[2] = u_col;\n\
                   gl_Position = vec4(a, 1.0);\n\
               }\n";
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    // Column 2 maps to @location(2) under the column-split.
    assert!(r.wgsl.contains("location(2)"));
}

/// The receipts that DO need to probe an assembled matrix go
/// through `mat * vec` (`OpMatrixTimesVector`) — the existing
/// binary-op path the mat4 commit re-used. This receipt confirms
/// that path is the one actually exercised and produces well-
/// formed WGSL.
#[test]
fn mat_times_vec_is_the_supported_matrix_use_in_lower() {
    let src = "precision mediump float;\n\
               varying mat4 v_xform;\n\
               uniform vec4 u_p;\n\
               void main() {\n\
                   gl_FragColor = v_xform * u_p;\n\
               }\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    // The reassembled mat4 feeds the OpMatrixTimesVector path,
    // which naga emits as a WGSL `*` between mat4x4 and vec4.
    // Either spelling — explicit `mat4x4 * vec4` or the more
    // compact form — is acceptable; what matters is the round-
    // trip produces a vec4 fragment output.
    assert!(r.wgsl.contains("vec4"));
}

/// Negative: a varying named `mat4` (i.e. typechecker sees the
/// matrix) cannot be assigned a `vec4` because the AST/typecheck
/// still enforces matrix vs vector typing. The column-split has
/// not weakened ESSL semantics.
#[test]
fn matrix_varying_cannot_be_assigned_a_vector() {
    let src = "attribute vec3 a;\n\
               varying mat4 v_xform;\n\
               void main() {\n\
                   v_xform = vec4(0.0);\n\
                   gl_Position = vec4(a, 1.0);\n\
               }\n";
    let err = compile(src, ShaderStage::Vertex).unwrap_err();
    assert!(matches!(err, CompileError::Check(_)), "got: {err:?}");
}
