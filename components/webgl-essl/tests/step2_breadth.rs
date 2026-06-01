/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 2 parser breadth: control flow (`if` / `while` / `for` / `do`),
//! local declarations, jump statements, postfix `.field` / `[i]` / `++`,
//! and unary prefix operators. Each test is an isolated probe on a
//! minimum-sized shader that exercises exactly one new construct, so a
//! regression points at the right grammar rule.

use webgl_essl::ast::*;
use webgl_essl::parse_source;

/// Pull `main()`'s body out of a shader that has a single function.
fn main_body(src: &str) -> Block {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let func = tu
        .decls
        .iter()
        .find_map(|d| match d {
            ExternalDecl::Function(f) if f.name == "main" => Some(f.body.clone()),
            _ => None,
        })
        .expect("main() function in shader");
    func
}

fn expect_block_stmt(s: &Stmt) -> &Block {
    match s {
        Stmt::Block(b) => b,
        _ => panic!("expected block stmt, got {s:?}"),
    }
}

// ---------- local declarations -----------------------------------------

#[test]
fn local_decl_with_init() {
    let src = r#"void main() { float x = 3.14; }"#;
    let body = main_body(src);
    assert_eq!(body.stmts.len(), 1);
    let d = match &body.stmts[0] {
        Stmt::Decl(d) => d,
        other => panic!("expected decl, got {other:?}"),
    };
    assert_eq!(d.ty.kind, TypeKind::Float);
    assert_eq!(d.name, "x");
    assert!(!d.is_const);
    let init = d.init.as_ref().expect("init present");
    match init {
        Expr::FloatLit { value, .. } => assert!((value - 3.14).abs() < 1e-6),
        other => panic!("expected float lit init, got {other:?}"),
    }
}

#[test]
fn local_decl_without_init() {
    let src = r#"void main() { vec4 c; }"#;
    let body = main_body(src);
    let d = match &body.stmts[0] {
        Stmt::Decl(d) => d,
        other => panic!("expected decl, got {other:?}"),
    };
    assert_eq!(d.ty.kind, TypeKind::Vec4);
    assert_eq!(d.name, "c");
    assert!(d.init.is_none());
}

#[test]
fn const_local_decl_carries_flag() {
    let src = r#"void main() { const float k = 1.0; }"#;
    let body = main_body(src);
    let d = match &body.stmts[0] {
        Stmt::Decl(d) => d,
        other => panic!("expected decl, got {other:?}"),
    };
    assert!(d.is_const);
    assert_eq!(d.ty.kind, TypeKind::Float);
}

// ---------- if / else --------------------------------------------------

#[test]
fn if_without_else_parses_then_only() {
    let src = r#"
void main() {
    if (true) {
        gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0);
    }
}
"#;
    let body = main_body(src);
    match &body.stmts[0] {
        Stmt::If { else_: None, then, .. } => {
            // then is a block stmt holding one expr stmt
            let block = expect_block_stmt(then);
            assert_eq!(block.stmts.len(), 1);
        },
        other => panic!("expected if stmt without else, got {other:?}"),
    }
}

#[test]
fn if_else_branches_both_present() {
    let src = r#"
void main() {
    if (false) {
        gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0);
    } else {
        gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
    }
}
"#;
    let body = main_body(src);
    match &body.stmts[0] {
        Stmt::If { else_: Some(_), .. } => {},
        other => panic!("expected if/else, got {other:?}"),
    }
}

#[test]
fn nested_if_else_chains() {
    let src = r#"
void main() {
    if (false) gl_FragColor = vec4(1.0);
    else if (true) gl_FragColor = vec4(0.5);
    else gl_FragColor = vec4(0.0);
}
"#;
    let body = main_body(src);
    // Outer: if/else where else is itself an if/else.
    let outer_else = match &body.stmts[0] {
        Stmt::If { else_: Some(e), .. } => e.as_ref(),
        other => panic!("expected outer if/else, got {other:?}"),
    };
    match outer_else {
        Stmt::If { else_: Some(_), .. } => {},
        other => panic!("expected nested if/else, got {other:?}"),
    }
}

// ---------- while / do / for -------------------------------------------

#[test]
fn while_loop_with_body_block() {
    let src = r#"
void main() {
    int i = 0;
    while (i < 10) {
        i = i + 1;
    }
}
"#;
    let body = main_body(src);
    assert_eq!(body.stmts.len(), 2);
    match &body.stmts[1] {
        Stmt::While { cond, body, .. } => {
            match cond {
                Expr::Binary { op: BinOp::Lt, .. } => {},
                other => panic!("expected `<` cond, got {other:?}"),
            }
            assert!(matches!(body.as_ref(), Stmt::Block(_)));
        },
        other => panic!("expected while, got {other:?}"),
    }
}

#[test]
fn do_while_loop_carries_body_and_cond() {
    let src = r#"
void main() {
    int i = 0;
    do {
        i = i + 1;
    } while (i < 3);
}
"#;
    let body = main_body(src);
    match &body.stmts[1] {
        Stmt::Do { body, cond, .. } => {
            assert!(matches!(body.as_ref(), Stmt::Block(_)));
            match cond {
                Expr::Binary { op: BinOp::Lt, .. } => {},
                other => panic!("expected `<` cond, got {other:?}"),
            }
        },
        other => panic!("expected do/while, got {other:?}"),
    }
}

#[test]
fn for_loop_with_decl_init_cond_and_step() {
    let src = r#"
void main() {
    vec4 sum = vec4(0.0);
    for (int i = 0; i < 4; i = i + 1) {
        sum = sum + vec4(0.25);
    }
}
"#;
    let body = main_body(src);
    match &body.stmts[1] {
        Stmt::For { init: ForInit::Decl(d), cond: Some(_), step: Some(_), .. } => {
            assert_eq!(d.ty.kind, TypeKind::Int);
            assert_eq!(d.name, "i");
            assert!(d.init.is_some(), "for-init decl has its own initializer");
        },
        other => panic!("expected for with decl init, got {other:?}"),
    }
}

#[test]
fn for_loop_with_empty_slots() {
    let src = r#"void main() { for (;;) { discard; } }"#;
    let body = main_body(src);
    match &body.stmts[0] {
        Stmt::For { init: ForInit::Empty, cond: None, step: None, .. } => {},
        other => panic!("expected for with all-empty slots, got {other:?}"),
    }
}

// ---------- jump statements --------------------------------------------

#[test]
fn break_continue_discard_in_loop() {
    let src = r#"
void main() {
    while (true) {
        if (false) break;
        if (false) continue;
        if (false) discard;
    }
}
"#;
    let body = main_body(src);
    let outer = match &body.stmts[0] {
        Stmt::While { body, .. } => body.as_ref(),
        other => panic!("expected while, got {other:?}"),
    };
    let inner_block = expect_block_stmt(outer);
    let kinds: Vec<&'static str> = inner_block
        .stmts
        .iter()
        .map(|s| match s {
            Stmt::If { then, .. } => match then.as_ref() {
                Stmt::Break { .. } => "break",
                Stmt::Continue { .. } => "continue",
                Stmt::Discard { .. } => "discard",
                _ => "other",
            },
            _ => "non-if",
        })
        .collect();
    assert_eq!(kinds, vec!["break", "continue", "discard"]);
}

// ---------- swizzle / member access ------------------------------------

#[test]
fn swizzle_member_access_dot_three_components() {
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
void main() {
    gl_FragColor = vec4(u_tint.rgb, 1.0);
}
"#;
    let body = main_body(src);
    let assign_rhs = match &body.stmts[0] {
        Stmt::Expr(Expr::Assign { rhs, .. }) => rhs.as_ref(),
        other => panic!("expected assign, got {other:?}"),
    };
    let args = match assign_rhs {
        Expr::Call { args, .. } => args,
        other => panic!("expected call, got {other:?}"),
    };
    match &args[0] {
        Expr::Member { base, field, .. } => {
            match base.as_ref() {
                Expr::Ident { name, .. } => assert_eq!(name, "u_tint"),
                other => panic!("expected ident base, got {other:?}"),
            }
            assert_eq!(field, "rgb");
        },
        other => panic!("expected member access, got {other:?}"),
    }
}

// ---------- unary prefix -----------------------------------------------

#[test]
fn unary_minus_on_float_lit() {
    let src = r#"void main() { gl_Position = vec4(-1.0, -1.0, 0.0, 1.0); }"#;
    let body = main_body(src);
    let args = match &body.stmts[0] {
        Stmt::Expr(Expr::Assign { rhs, .. }) => match rhs.as_ref() {
            Expr::Call { args, .. } => args,
            _ => panic!("expected call"),
        },
        _ => panic!("expected assign stmt"),
    };
    match &args[0] {
        Expr::Unary { op: UnaryOp::Neg, expr, .. } => match expr.as_ref() {
            Expr::FloatLit { value, .. } => assert_eq!(*value, 1.0),
            other => panic!("expected float lit under unary, got {other:?}"),
        },
        other => panic!("expected unary neg, got {other:?}"),
    }
}

#[test]
fn unary_not_on_bool_lit() {
    let src = r#"void main() { if (!true) discard; }"#;
    let body = main_body(src);
    let cond = match &body.stmts[0] {
        Stmt::If { cond, .. } => cond,
        _ => panic!("expected if stmt"),
    };
    match cond {
        Expr::Unary { op: UnaryOp::Not, expr, .. } => match expr.as_ref() {
            Expr::BoolLit { value: true, .. } => {},
            other => panic!("expected true under !, got {other:?}"),
        },
        other => panic!("expected unary not, got {other:?}"),
    }
}

// ---------- postfix ----------------------------------------------------

#[test]
fn postfix_inc_inside_for_step() {
    let src = r#"
void main() {
    for (int i = 0; i < 4; i++) {
        gl_FragColor = vec4(0.0);
    }
}
"#;
    let body = main_body(src);
    let step = match &body.stmts[0] {
        Stmt::For { step: Some(s), .. } => s,
        _ => panic!("expected for with step"),
    };
    match step {
        Expr::Unary { op: UnaryOp::PostInc, expr, .. } => match expr.as_ref() {
            Expr::Ident { name, .. } => assert_eq!(name, "i"),
            other => panic!("expected ident under postinc, got {other:?}"),
        },
        other => panic!("expected post-inc, got {other:?}"),
    }
}

#[test]
fn nested_swizzle_after_call() {
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(1.0, 0.5, 0.25, 1.0).bgra;
}
"#;
    let body = main_body(src);
    let rhs = match &body.stmts[0] {
        Stmt::Expr(Expr::Assign { rhs, .. }) => rhs.as_ref(),
        _ => panic!("expected assign"),
    };
    match rhs {
        Expr::Member { base, field, .. } => {
            assert_eq!(field, "bgra");
            assert!(matches!(base.as_ref(), Expr::Call { .. }));
        },
        other => panic!("expected member-of-call, got {other:?}"),
    }
}

// ---------- precedence regression --------------------------------------

#[test]
fn multiplication_binds_tighter_than_addition() {
    let src = r#"
void main() {
    gl_FragColor = vec4(1.0 + 2.0 * 3.0);
}
"#;
    let body = main_body(src);
    let args = match &body.stmts[0] {
        Stmt::Expr(Expr::Assign { rhs, .. }) => match rhs.as_ref() {
            Expr::Call { args, .. } => args.clone(),
            _ => panic!("expected call"),
        },
        _ => panic!("expected assign"),
    };
    // Should parse as 1 + (2 * 3), i.e. top-level is Add with right child Mul.
    match &args[0] {
        Expr::Binary { op: BinOp::Add, rhs, .. } => match rhs.as_ref() {
            Expr::Binary { op: BinOp::Mul, .. } => {},
            other => panic!("expected mul under add.rhs, got {other:?}"),
        },
        other => panic!("expected add at top, got {other:?}"),
    }
}
