/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Parse receipt for the canonical-triangle shader pair plus the
//! extended-corpus shapes (uniform / varying / binary `*`). Asserts the
//! AST shape, not byte-for-byte equality — the spike's job is to prove the
//! parser produces a faithful tree, not to round-trip source.

use webgl_essl::ast::*;
use webgl_essl::parse_source;

const CANONICAL_VERTEX: &str = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;

const CANONICAL_FRAGMENT: &str = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;

const TINTED_VERTEX: &str = r#"
attribute vec2 a_position;
attribute vec4 a_color;
varying vec4 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position * 2.0, 0.0, 1.0);
}
"#;

const TINTED_FRAGMENT: &str = r#"
precision mediump float;
uniform vec4 u_tint;
varying vec4 v_color;
void main() {
    gl_FragColor = u_tint * v_color;
}
"#;

fn expect_function<'a>(decl: &'a ExternalDecl, name: &str) -> &'a FunctionDef {
    match decl {
        ExternalDecl::Function(f) if f.name == name => f,
        _ => panic!("expected function `{name}`, got {decl:?}"),
    }
}

fn expect_global<'a>(decl: &'a ExternalDecl) -> &'a GlobalDecl {
    match decl {
        ExternalDecl::Global(g) => g,
        _ => panic!("expected global decl, got {decl:?}"),
    }
}

fn expect_precision<'a>(decl: &'a ExternalDecl) -> &'a PrecisionDecl {
    match decl {
        ExternalDecl::Precision(p) => p,
        _ => panic!("expected precision decl, got {decl:?}"),
    }
}

fn expect_call<'a>(expr: &'a Expr) -> (&'a str, &'a [Expr]) {
    match expr {
        Expr::Call { callee, args, .. } => (callee.as_str(), args.as_slice()),
        _ => panic!("expected call expr, got {expr:?}"),
    }
}

fn expect_assign<'a>(expr: &'a Expr) -> (&'a Expr, &'a Expr) {
    match expr {
        Expr::Assign {
            op: AssignOp::Assign,
            lhs,
            rhs,
            ..
        } => (lhs.as_ref(), rhs.as_ref()),
        _ => panic!("expected `=` assign, got {expr:?}"),
    }
}

fn expect_binary<'a>(expr: &'a Expr, want: BinOp) -> (&'a Expr, &'a Expr) {
    match expr {
        Expr::Binary { op, lhs, rhs, .. } if *op == want => (lhs.as_ref(), rhs.as_ref()),
        _ => panic!("expected binary `{want:?}`, got {expr:?}"),
    }
}

fn expect_ident<'a>(expr: &'a Expr) -> &'a str {
    match expr {
        Expr::Ident { name, .. } => name.as_str(),
        _ => panic!("expected ident, got {expr:?}"),
    }
}

fn expect_float(expr: &Expr) -> f64 {
    match expr {
        Expr::FloatLit { value, .. } => *value,
        _ => panic!("expected float lit, got {expr:?}"),
    }
}

#[test]
fn canonical_vertex_parses_to_attribute_and_main() {
    let tu = parse_source(CANONICAL_VERTEX).expect("parse canonical vertex");
    assert_eq!(tu.decls.len(), 2, "decls: {:?}", tu.decls);

    let attr = expect_global(&tu.decls[0]);
    assert_eq!(attr.storage, StorageQualifier::Attribute);
    assert_eq!(attr.ty.kind, TypeKind::Vec2);
    assert_eq!(attr.name, "a_position");

    let main = expect_function(&tu.decls[1], "main");
    assert_eq!(main.return_ty.kind, TypeKind::Void);
    assert!(main.params.is_empty());
    assert_eq!(main.body.stmts.len(), 1);

    let body_expr = match &main.body.stmts[0] {
        Stmt::Expr(e) => e,
        other => panic!("expected expr stmt, got {other:?}"),
    };
    let (lhs, rhs) = expect_assign(body_expr);
    assert_eq!(expect_ident(lhs), "gl_Position");
    let (callee, args) = expect_call(rhs);
    assert_eq!(callee, "vec4");
    assert_eq!(args.len(), 3);
    assert_eq!(expect_ident(&args[0]), "a_position");
    assert_eq!(expect_float(&args[1]), 0.0);
    assert_eq!(expect_float(&args[2]), 1.0);
}

#[test]
fn canonical_fragment_parses_to_precision_and_main() {
    let tu = parse_source(CANONICAL_FRAGMENT).expect("parse canonical fragment");
    assert_eq!(tu.decls.len(), 2);

    let prec = expect_precision(&tu.decls[0]);
    assert_eq!(prec.qualifier, PrecisionQualifier::Medium);
    assert_eq!(prec.ty.kind, TypeKind::Float);

    let main = expect_function(&tu.decls[1], "main");
    let body_expr = match &main.body.stmts[0] {
        Stmt::Expr(e) => e,
        other => panic!("expected expr stmt, got {other:?}"),
    };
    let (lhs, rhs) = expect_assign(body_expr);
    assert_eq!(expect_ident(lhs), "gl_FragColor");
    let (callee, args) = expect_call(rhs);
    assert_eq!(callee, "vec4");
    assert_eq!(args.len(), 4);
    for (i, expected) in [0.0, 1.0, 0.0, 1.0].iter().enumerate() {
        assert_eq!(expect_float(&args[i]), *expected);
    }
}

#[test]
fn tinted_vertex_threads_varying_and_binary_mul_arg() {
    let tu = parse_source(TINTED_VERTEX).expect("parse tinted vertex");
    assert_eq!(tu.decls.len(), 4);

    let attr_pos = expect_global(&tu.decls[0]);
    assert_eq!(attr_pos.storage, StorageQualifier::Attribute);
    assert_eq!(attr_pos.ty.kind, TypeKind::Vec2);

    let attr_col = expect_global(&tu.decls[1]);
    assert_eq!(attr_col.storage, StorageQualifier::Attribute);
    assert_eq!(attr_col.ty.kind, TypeKind::Vec4);

    let varying = expect_global(&tu.decls[2]);
    assert_eq!(varying.storage, StorageQualifier::Varying);
    assert_eq!(varying.ty.kind, TypeKind::Vec4);
    assert_eq!(varying.name, "v_color");

    let main = expect_function(&tu.decls[3], "main");
    assert_eq!(main.body.stmts.len(), 2);

    // First stmt: v_color = a_color;
    let (lhs0, rhs0) = expect_assign(match &main.body.stmts[0] {
        Stmt::Expr(e) => e,
        _ => panic!(),
    });
    assert_eq!(expect_ident(lhs0), "v_color");
    assert_eq!(expect_ident(rhs0), "a_color");

    // Second stmt: gl_Position = vec4(a_position * 2.0, 0.0, 1.0);
    let (lhs1, rhs1) = expect_assign(match &main.body.stmts[1] {
        Stmt::Expr(e) => e,
        _ => panic!(),
    });
    assert_eq!(expect_ident(lhs1), "gl_Position");
    let (callee, args) = expect_call(rhs1);
    assert_eq!(callee, "vec4");
    assert_eq!(args.len(), 3);
    let (mul_lhs, mul_rhs) = expect_binary(&args[0], BinOp::Mul);
    assert_eq!(expect_ident(mul_lhs), "a_position");
    assert_eq!(expect_float(mul_rhs), 2.0);
}

#[test]
fn tinted_fragment_threads_uniform_and_varying_binary_mul() {
    let tu = parse_source(TINTED_FRAGMENT).expect("parse tinted fragment");
    assert_eq!(tu.decls.len(), 4);

    let prec = expect_precision(&tu.decls[0]);
    assert_eq!(prec.qualifier, PrecisionQualifier::Medium);
    assert_eq!(prec.ty.kind, TypeKind::Float);

    let uniform = expect_global(&tu.decls[1]);
    assert_eq!(uniform.storage, StorageQualifier::Uniform);
    assert_eq!(uniform.ty.kind, TypeKind::Vec4);
    assert_eq!(uniform.name, "u_tint");

    let varying = expect_global(&tu.decls[2]);
    assert_eq!(varying.storage, StorageQualifier::Varying);
    assert_eq!(varying.name, "v_color");

    let main = expect_function(&tu.decls[3], "main");
    assert_eq!(main.body.stmts.len(), 1);
    let (lhs, rhs) = expect_assign(match &main.body.stmts[0] {
        Stmt::Expr(e) => e,
        _ => panic!(),
    });
    assert_eq!(expect_ident(lhs), "gl_FragColor");
    let (mul_lhs, mul_rhs) = expect_binary(rhs, BinOp::Mul);
    assert_eq!(expect_ident(mul_lhs), "u_tint");
    assert_eq!(expect_ident(mul_rhs), "v_color");
}

#[test]
fn line_comment_and_block_comment_are_skipped() {
    let src = r#"
// leading line comment
precision mediump float; /* block in the middle */
void main() {
    // statement-line comment
    gl_FragColor = vec4(0.0, 0.0, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).expect("parse with comments");
    assert_eq!(tu.decls.len(), 2);
    assert!(matches!(tu.decls[0], ExternalDecl::Precision(_)));
    assert!(matches!(tu.decls[1], ExternalDecl::Function(_)));
}

#[test]
fn malformed_shader_reports_a_useful_error() {
    let src = "precision mediump 42;";
    let err = parse_source(src).expect_err("malformed precision should fail");
    let rendered = format!("{}", err.display(src));
    assert!(rendered.contains("type"), "rendered: {rendered}");
}
