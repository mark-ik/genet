/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 4b: binary op result types, constructor signatures, swizzle
//! field access, unary, ternary, assign. The built-in function
//! registry (sin / cos / mix / texture2D / etc.) is the next chunk.

use webgl_essl::ast::*;
use webgl_essl::check::{TypeDiagnosticKind, check};
use webgl_essl::parse_source;

fn check_clean(src: &str) -> webgl_essl::check::CheckResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let r = check(&tu);
    if !r.diagnostics.is_empty() {
        let rendered: Vec<String> = r
            .diagnostics
            .iter()
            .map(|d| format!("{}", d.display(src)))
            .collect();
        panic!(
            "expected zero diagnostics, got: {}\n--- source ---\n{src}",
            rendered.join("; ")
        );
    }
    r
}

fn count_of(r: &webgl_essl::check::CheckResult, ty: TypeKind) -> usize {
    r.types.values().filter(|&&t| t == ty).count()
}

// ---------- binary arithmetic -----------------------------------------

#[test]
fn binary_float_add_resolves_to_float() {
    let src = "void main() { float x = 1.0 + 2.0; }";
    let r = check_clean(src);
    // 3 Float entries: lhs lit, rhs lit, binary result.
    assert_eq!(count_of(&r, TypeKind::Float), 3);
}

#[test]
fn binary_int_mul_resolves_to_int() {
    let src = "void main() { int x = 2 * 3; }";
    let r = check_clean(src);
    assert_eq!(count_of(&r, TypeKind::Int), 3);
}

#[test]
fn binary_vec_scalar_broadcast_resolves_to_vec() {
    let src = r#"
attribute vec2 a_position;
uniform float u_scale;
void main() {
    vec2 scaled = a_position * u_scale;
    gl_Position = vec4(scaled, 0.0, 1.0);
}
"#;
    let r = check_clean(src);
    // a_position * u_scale should resolve to Vec2.
    assert!(
        count_of(&r, TypeKind::Vec2) >= 2,
        "scaled decl + binary result"
    );
}

#[test]
fn binary_matrix_vector_mul_resolves_to_vec4() {
    let src = r#"
uniform mat4 u_mvp;
attribute vec3 a_position;
void main() {
    gl_Position = u_mvp * vec4(a_position, 1.0);
}
"#;
    let r = check_clean(src);
    // u_mvp * vec4(...) should annotate as Vec4.
    // We expect at least: u_mvp (Mat4), the vec4 constructor (Vec4),
    // the binary mul result (Vec4), gl_Position ident (Vec4), the
    // outer Assign (Vec4), a_position (Vec3), the 1.0 (Float).
    assert!(count_of(&r, TypeKind::Vec4) >= 4);
    assert_eq!(count_of(&r, TypeKind::Mat4), 1);
    assert_eq!(count_of(&r, TypeKind::Vec3), 1);
}

// ---------- binary comparison / equality / logical --------------------

#[test]
fn binary_float_lt_resolves_to_bool() {
    let src = "void main() { bool b = 1.0 < 2.0; }";
    let r = check_clean(src);
    assert_eq!(count_of(&r, TypeKind::Bool), 1);
    assert_eq!(count_of(&r, TypeKind::Float), 2);
}

#[test]
fn binary_vec_equality_resolves_to_bool() {
    let src = r#"
void main() {
    vec3 a = vec3(1.0);
    vec3 b = vec3(2.0);
    bool eq = a == b;
}
"#;
    check_clean(src);
}

#[test]
fn binary_bool_and_resolves_to_bool() {
    let src = "void main() { bool b = true && false; }";
    let r = check_clean(src);
    assert_eq!(count_of(&r, TypeKind::Bool), 3);
}

// ---------- binary mismatch diagnostics ------------------------------

#[test]
fn binary_bool_add_emits_mismatch_diagnostic() {
    let src = "void main() { bool a = true; bool b = false; bool x = a + b; }";
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::BinaryOpMismatch { .. }))
        .collect();
    assert_eq!(
        mismatches.len(),
        1,
        "exactly one binary mismatch on `a + b`"
    );
}

#[test]
fn binary_mat3_mat4_mul_emits_mismatch() {
    // mat3 * mat4 is shape-incompatible.
    let src = r#"
uniform mat3 a;
uniform mat4 b;
void main() {
    mat3 c = mat3(1.0);
    c = a * b;
}
"#;
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::BinaryOpMismatch { .. }))
        .collect();
    assert_eq!(mismatches.len(), 1);
}

// ---------- constructor signatures ------------------------------------

#[test]
fn constructor_vec4_from_scalar_broadcast() {
    let src = "void main() { vec4 v = vec4(1.0); }";
    let r = check_clean(src);
    assert!(count_of(&r, TypeKind::Vec4) >= 1);
}

#[test]
fn constructor_vec4_from_four_floats() {
    let src = "void main() { vec4 v = vec4(0.0, 0.0, 0.0, 1.0); }";
    check_clean(src);
}

#[test]
fn constructor_vec4_from_vec3_plus_float() {
    let src = "void main() { vec4 v = vec4(vec3(1.0), 1.0); }";
    check_clean(src);
}

#[test]
fn constructor_vec3_truncating_vec4() {
    let src = r#"
void main() {
    vec4 src = vec4(1.0, 2.0, 3.0, 4.0);
    vec3 dst = vec3(src);
}
"#;
    check_clean(src);
}

#[test]
fn constructor_mat4_from_scalar_diagonal_broadcast() {
    let src = "void main() { mat4 m = mat4(1.0); }";
    check_clean(src);
}

#[test]
fn constructor_float_from_int() {
    let src = "void main() { float f = float(42); }";
    check_clean(src);
}

// ---------- swizzles --------------------------------------------------

#[test]
fn swizzle_rgb_on_vec4_resolves_to_vec3() {
    let src = r#"
uniform vec4 u_tint;
void main() {
    vec3 rgb = u_tint.rgb;
    gl_FragColor = vec4(rgb, 1.0);
}
"#;
    let r = check_clean(src);
    // The .rgb access should annotate Vec3.
    assert!(
        count_of(&r, TypeKind::Vec3) >= 1,
        "swizzle .rgb produces Vec3"
    );
}

#[test]
fn swizzle_x_on_vec3_resolves_to_float() {
    let src = r#"
uniform vec3 u_pos;
void main() {
    float x = u_pos.x;
}
"#;
    let r = check_clean(src);
    assert!(count_of(&r, TypeKind::Float) >= 1);
}

#[test]
fn swizzle_xyzw_on_vec4_resolves_to_vec4() {
    let src = r#"
uniform vec4 u_color;
void main() {
    vec4 reordered = u_color.bgra;
}
"#;
    check_clean(src);
}

#[test]
fn invalid_swizzle_xyzw_on_vec2_diagnostic() {
    let src = r#"
uniform vec2 u_uv;
void main() {
    vec4 v = vec4(u_uv.xyzw, 0.0, 1.0);
}
"#;
    let r = check(&parse_source(src).unwrap());
    let invalid: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::InvalidSwizzle { .. }))
        .collect();
    assert_eq!(invalid.len(), 1, "`.xyzw` is not valid on vec2");
}

#[test]
fn invalid_swizzle_mixing_sets_diagnostic() {
    // Mixing xyzw with rgba in one field is invalid in ESSL.
    let src = r#"
uniform vec4 u_color;
void main() {
    vec2 weird = u_color.xr;
}
"#;
    let r = check(&parse_source(src).unwrap());
    let invalid: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::InvalidSwizzle { .. }))
        .collect();
    assert_eq!(invalid.len(), 1);
}

// ---------- unary -----------------------------------------------------

#[test]
fn unary_neg_on_float_resolves_to_float() {
    let src = "void main() { float x = -1.0; }";
    let r = check_clean(src);
    assert_eq!(count_of(&r, TypeKind::Float), 2, "lit + unary result");
}

#[test]
fn unary_not_on_bool_resolves_to_bool() {
    let src = "void main() { bool b = !true; }";
    let r = check_clean(src);
    assert_eq!(count_of(&r, TypeKind::Bool), 2);
}

#[test]
fn unary_not_on_float_emits_diagnostic() {
    let src = "void main() { float f = 1.0; bool b = !f; }";
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::UnaryOpMismatch { .. }))
        .collect();
    assert_eq!(mismatches.len(), 1);
}

// ---------- ternary ---------------------------------------------------

#[test]
fn ternary_with_bool_cond_and_matching_branches() {
    let src = r#"
void main() {
    bool flag = true;
    float x = flag ? 1.0 : 2.0;
}
"#;
    check_clean(src);
}

#[test]
fn ternary_cond_not_bool_emits_diagnostic() {
    let src = r#"
void main() {
    float f = 1.0;
    float x = f ? 1.0 : 2.0;
}
"#;
    let r = check(&parse_source(src).unwrap());
    let cond_diags: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::TernaryCondNotBool { .. }))
        .collect();
    assert_eq!(cond_diags.len(), 1);
}

#[test]
fn ternary_branch_mismatch_emits_diagnostic() {
    let src = r#"
void main() {
    bool flag = true;
    float x = flag ? 1.0 : 2;
}
"#;
    let r = check(&parse_source(src).unwrap());
    let branch_diags: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::TernaryBranchMismatch { .. }))
        .collect();
    assert_eq!(branch_diags.len(), 1);
}

// ---------- assign returns lhs type -----------------------------------

#[test]
fn assign_annotates_with_lhs_type() {
    // The assign expression's type should be the LHS's type, so
    // `(gl_FragColor = u_color)` has Vec4 written into the types map
    // both at the lhs ident span AND at the assign expression's span.
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let r = check_clean(src);
    // Expect at least 3 Vec4 entries: gl_FragColor ident, u_color
    // ident, and the assign expression itself.
    assert!(count_of(&r, TypeKind::Vec4) >= 3);
}

#[test]
fn assign_type_mismatch_emits_diagnostic() {
    let src = r#"
void main() {
    float f = 1.0;
    bool b = true;
    f = b;
}
"#;
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::AssignTypeMismatch { .. }))
        .collect();
    assert_eq!(mismatches.len(), 1);
}
