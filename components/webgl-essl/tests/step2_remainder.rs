/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 2 remainder: function definitions with non-empty parameter
//! lists (the code path exists, this checks it actually works),
//! ternary `?:` expressions, and struct declarations. Each test is
//! isolated on one new construct so a regression points at the right
//! grammar rule.

use webgl_essl::ast::*;
use webgl_essl::parse_source;

// ---------- function definitions with parameters -----------------------

#[test]
fn function_def_with_three_params() {
    let src = r#"
vec4 lerp(vec4 a, vec4 b, float t) {
    return a + (b - a) * t;
}
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let func = match &tu.decls[0] {
        ExternalDecl::Function(f) if f.name == "lerp" => f,
        other => panic!("expected lerp function, got {other:?}"),
    };
    assert_eq!(func.return_ty.kind, TypeKind::Vec4);
    assert_eq!(func.params.len(), 3);
    assert_eq!(func.params[0].ty.kind, TypeKind::Vec4);
    assert_eq!(func.params[0].name, "a");
    assert_eq!(func.params[1].ty.kind, TypeKind::Vec4);
    assert_eq!(func.params[1].name, "b");
    assert_eq!(func.params[2].ty.kind, TypeKind::Float);
    assert_eq!(func.params[2].name, "t");
    // Body should hold one return statement.
    assert_eq!(func.body.stmts.len(), 1);
    assert!(matches!(func.body.stmts[0], Stmt::Return { value: Some(_), .. }));
}

#[test]
fn function_def_with_explicit_void_params() {
    let src = r#"void noop(void) { }"#;
    let tu = parse_source(src).expect("parse void-params");
    let func = match &tu.decls[0] {
        ExternalDecl::Function(f) => f,
        _ => panic!("expected function"),
    };
    assert_eq!(func.name, "noop");
    assert_eq!(func.params.len(), 0);
    assert_eq!(func.body.stmts.len(), 0);
}

#[test]
fn two_function_defs_in_one_translation_unit() {
    let src = r#"
float square(float x) { return x * x; }
void main() { gl_FragColor = vec4(square(0.5)); }
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    assert_eq!(tu.decls.len(), 2);
    match &tu.decls[0] {
        ExternalDecl::Function(f) => assert_eq!(f.name, "square"),
        _ => panic!("expected square fn"),
    }
    match &tu.decls[1] {
        ExternalDecl::Function(f) => assert_eq!(f.name, "main"),
        _ => panic!("expected main fn"),
    }
}

// ---------- ternary `?:` -----------------------------------------------

#[test]
fn ternary_in_assign_rhs() {
    let src = r#"
precision mediump float;
uniform bool use_red;
void main() {
    gl_FragColor = use_red ? vec4(1.0, 0.0, 0.0, 1.0) : vec4(0.0, 1.0, 0.0, 1.0);
}
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let main = match &tu.decls[2] {
        ExternalDecl::Function(f) => f,
        _ => panic!("expected main fn"),
    };
    let rhs = match &main.body.stmts[0] {
        Stmt::Expr(Expr::Assign { rhs, .. }) => rhs.as_ref(),
        other => panic!("expected assign stmt, got {other:?}"),
    };
    match rhs {
        Expr::Ternary { cond, then, else_, .. } => {
            assert!(matches!(cond.as_ref(), Expr::Ident { .. }));
            assert!(matches!(then.as_ref(), Expr::Call { .. }));
            assert!(matches!(else_.as_ref(), Expr::Call { .. }));
        },
        other => panic!("expected ternary, got {other:?}"),
    }
}

#[test]
fn ternary_is_right_associative() {
    let src = r#"
void main() {
    float x = true ? 1.0 : false ? 2.0 : 3.0;
}
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let main = match &tu.decls[0] {
        ExternalDecl::Function(f) => f,
        _ => panic!("expected main fn"),
    };
    let init = match &main.body.stmts[0] {
        Stmt::Decl(d) => d.init.as_ref().expect("init present"),
        other => panic!("expected decl, got {other:?}"),
    };
    // Should parse as `true ? 1.0 : (false ? 2.0 : 3.0)` (right-assoc).
    let outer_else = match init {
        Expr::Ternary { else_, .. } => else_.as_ref(),
        other => panic!("expected outer ternary, got {other:?}"),
    };
    match outer_else {
        Expr::Ternary { .. } => {},
        other => panic!(
            "expected nested ternary on else_ side (right-assoc), got {other:?}"
        ),
    }
}

#[test]
fn ternary_binds_looser_than_logical_or() {
    let src = r#"
void main() {
    float x = true || false ? 1.0 : 0.0;
}
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let main = match &tu.decls[0] {
        ExternalDecl::Function(f) => f,
        _ => panic!("expected main fn"),
    };
    let init = match &main.body.stmts[0] {
        Stmt::Decl(d) => d.init.as_ref().expect("init"),
        _ => panic!("expected decl"),
    };
    // Should parse as `(true || false) ? 1.0 : 0.0` — log-or binds tighter.
    match init {
        Expr::Ternary { cond, .. } => match cond.as_ref() {
            Expr::Binary { op: BinOp::LogOr, .. } => {},
            other => panic!("expected log-or as ternary cond, got {other:?}"),
        },
        other => panic!("expected ternary, got {other:?}"),
    }
}

// ---------- struct declarations ----------------------------------------

#[test]
fn struct_decl_with_three_typed_fields() {
    let src = r#"
struct Light {
    vec3 position;
    vec3 color;
    float intensity;
};
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let s = match &tu.decls[0] {
        ExternalDecl::Struct(s) => s,
        other => panic!("expected struct decl, got {other:?}"),
    };
    assert_eq!(s.name.as_deref(), Some("Light"));
    assert_eq!(s.fields.len(), 3);
    assert_eq!(s.fields[0].ty.kind, TypeKind::Vec3);
    assert_eq!(s.fields[0].name, "position");
    assert_eq!(s.fields[1].ty.kind, TypeKind::Vec3);
    assert_eq!(s.fields[1].name, "color");
    assert_eq!(s.fields[2].ty.kind, TypeKind::Float);
    assert_eq!(s.fields[2].name, "intensity");
}

#[test]
fn struct_decl_multi_field_per_line() {
    let src = r#"
struct Pair {
    vec3 a, b;
};
"#;
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let s = match &tu.decls[0] {
        ExternalDecl::Struct(s) => s,
        _ => panic!("expected struct decl"),
    };
    assert_eq!(s.fields.len(), 2);
    assert_eq!(s.fields[0].ty.kind, TypeKind::Vec3);
    assert_eq!(s.fields[0].name, "a");
    assert_eq!(s.fields[1].ty.kind, TypeKind::Vec3);
    assert_eq!(s.fields[1].name, "b");
}
