/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 3 first chunk: ESSL 3.00 shift / bitwise / `~` operators.
//! Parser gets the binding-power table entries; typecheck enforces
//! `int <op> int -> int` (uint / ivec come when we add those types to
//! the AST). No `#version 300 es` directive handling yet, so the
//! validator does not gate these as 3.00-only.

use webgl_essl::ast::*;
use webgl_essl::check::check;
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

fn assign_rhs<'a>(stmt: &'a Stmt) -> &'a Expr {
    match stmt {
        Stmt::Decl(d) => d.init.as_ref().expect("decl init"),
        Stmt::Expr(Expr::Assign { rhs, .. }) => rhs,
        _ => panic!("not a decl or assign stmt: {stmt:?}"),
    }
}

fn first_body_stmt<'a>(tu: &'a TranslationUnit) -> &'a Stmt {
    let main = tu
        .decls
        .iter()
        .find_map(|d| match d {
            ExternalDecl::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .expect("main");
    &main.body.stmts[0]
}

// ---------- shift -----------------------------------------------------

#[test]
fn shift_left_int_int_resolves_to_int() {
    let src = "void main() { int x = 1 << 2; }";
    let r = check_clean(src);
    assert_eq!(count_of(&r, TypeKind::Int), 3, "lhs + rhs + binary result");
}

#[test]
fn shift_right_int_int_resolves_to_int() {
    let src = "void main() { int x = 8 >> 1; }";
    check_clean(src);
}

#[test]
fn shift_lower_precedence_than_additive() {
    let src = "void main() { int x = 1 + 2 << 3; }";
    let tu = parse_source(src).unwrap();
    let rhs = assign_rhs(first_body_stmt(&tu));
    // (1 + 2) << 3 means top-level is Shl with left child Add.
    match rhs {
        Expr::Binary {
            op: BinOp::Shl,
            lhs,
            ..
        } => match lhs.as_ref() {
            Expr::Binary { op: BinOp::Add, .. } => {},
            other => panic!("expected Add under Shl.lhs, got {other:?}"),
        },
        other => panic!("expected Shl top-level, got {other:?}"),
    }
}

// ---------- bitwise ---------------------------------------------------

#[test]
fn bitwise_and_int_int_resolves_to_int() {
    let src = "void main() { int x = 5 & 3; }";
    let r = check_clean(src);
    assert_eq!(count_of(&r, TypeKind::Int), 3);
}

#[test]
fn bitwise_or_int_int_resolves_to_int() {
    let src = "void main() { int x = 1 | 2; }";
    check_clean(src);
}

#[test]
fn bitwise_xor_int_int_resolves_to_int() {
    let src = "void main() { int x = 5 ^ 3; }";
    check_clean(src);
}

#[test]
fn bitwise_and_binds_tighter_than_or() {
    let src = "void main() { int x = 1 | 2 & 3; }";
    let tu = parse_source(src).unwrap();
    let rhs = assign_rhs(first_body_stmt(&tu));
    // 1 | (2 & 3): top-level BitOr with right child BitAnd.
    match rhs {
        Expr::Binary {
            op: BinOp::BitOr,
            rhs,
            ..
        } => match rhs.as_ref() {
            Expr::Binary {
                op: BinOp::BitAnd, ..
            } => {},
            other => panic!("expected BitAnd under BitOr.rhs, got {other:?}"),
        },
        other => panic!("expected BitOr top-level, got {other:?}"),
    }
}

#[test]
fn bitwise_xor_between_or_and_and() {
    // `1 | 2 ^ 3 & 4` parses as `1 | (2 ^ (3 & 4))`.
    let src = "void main() { int x = 1 | 2 ^ 3 & 4; }";
    let tu = parse_source(src).unwrap();
    let rhs = assign_rhs(first_body_stmt(&tu));
    match rhs {
        Expr::Binary {
            op: BinOp::BitOr,
            rhs,
            ..
        } => match rhs.as_ref() {
            Expr::Binary {
                op: BinOp::BitXor,
                rhs,
                ..
            } => match rhs.as_ref() {
                Expr::Binary {
                    op: BinOp::BitAnd, ..
                } => {},
                other => panic!("expected BitAnd under BitXor.rhs, got {other:?}"),
            },
            other => panic!("expected BitXor under BitOr.rhs, got {other:?}"),
        },
        other => panic!("expected BitOr top-level, got {other:?}"),
    }
}

// ---------- bitwise NOT (`~`) -----------------------------------------

#[test]
fn bit_not_on_int_resolves_to_int() {
    let src = "void main() { int x = ~7; }";
    let r = check_clean(src);
    // lit 7 + Unary BitNot result.
    assert_eq!(count_of(&r, TypeKind::Int), 2);
}

#[test]
fn bit_not_parses_as_unary_with_bit_not_op() {
    let src = "void main() { int x = ~5; }";
    let tu = parse_source(src).unwrap();
    let rhs = assign_rhs(first_body_stmt(&tu));
    match rhs {
        Expr::Unary {
            op: UnaryOp::BitNot,
            ..
        } => {},
        other => panic!("expected Unary BitNot, got {other:?}"),
    }
}

// ---------- typecheck mismatch ---------------------------------------

#[test]
fn shift_on_float_emits_mismatch_diagnostic() {
    // ESSL 1.00 has no shift at all; ESSL 3.00 limits to int (this
    // pass's strict rule). float << int should not type.
    let src = "void main() { float f = 1.5; int x = f << 2; }";
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.kind,
                webgl_essl::check::TypeDiagnosticKind::BinaryOpMismatch { .. }
            )
        })
        .collect();
    assert_eq!(mismatches.len(), 1);
}

#[test]
fn bit_not_on_float_emits_mismatch_diagnostic() {
    let src = "void main() { float f = 1.0; float g = ~f; }";
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.kind,
                webgl_essl::check::TypeDiagnosticKind::UnaryOpMismatch { .. }
            )
        })
        .collect();
    // f is float, ~ on float fails; the binding via assign also fails
    // (~f resolves to nothing, so Assign sees no rhs type and stays
    // quiet). Expect exactly the unary mismatch.
    assert_eq!(mismatches.len(), 1);
}

// ---------- existing 1.00 corpus still passes (regression) -----------

#[test]
fn renumbering_did_not_break_1_00_precedence() {
    // additive vs multiplicative still parses correctly with the new
    // table.
    let src = "void main() { gl_FragColor = vec4(1.0 + 2.0 * 3.0); }";
    let tu = parse_source(src).unwrap();
    let r = check(&tu);
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
}
