/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 3 third chunk: ESSL 3.00 `switch` / `case` / `default`. The
//! parser today is permissive: case values can be any expression (the
//! spec wants integer constants; that constraint moves to the
//! validator in a follow-up).

use webgl_essl::ast::*;
use webgl_essl::parse_source;

fn parse_or_panic(src: &str) -> TranslationUnit {
    parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)))
}

fn main_body(src: &str) -> Block {
    let tu = parse_or_panic(src);
    tu.decls
        .into_iter()
        .find_map(|d| match d {
            ExternalDecl::Function(f) if f.name == "main" => Some(f.body),
            _ => None,
        })
        .expect("main")
}

// ---------- basic shapes ---------------------------------------------

#[test]
fn switch_with_two_cases_and_default_parses() {
    let src = r#"
void main() {
    int x = 0;
    switch (x) {
        case 0:
            gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0);
            break;
        case 1:
            gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
            break;
        default:
            discard;
    }
}
"#;
    let body = main_body(src);
    let switch_stmt = body
        .stmts
        .iter()
        .find_map(|s| match s {
            Stmt::Switch {
                discriminant, body, ..
            } => Some((discriminant, body)),
            _ => None,
        })
        .expect("a switch stmt in main");
    let (_disc, switch_body) = switch_stmt;
    // Three labels (two case, one default) interleaved with statements.
    let mut case_count = 0;
    let mut default_count = 0;
    for s in &switch_body.stmts {
        match s {
            Stmt::Case { .. } => case_count += 1,
            Stmt::Default { .. } => default_count += 1,
            _ => {},
        }
    }
    assert_eq!(case_count, 2);
    assert_eq!(default_count, 1);
}

#[test]
fn switch_with_int_constant_case_values_parses() {
    let src = r#"
void main() {
    int kind = 0;
    switch (kind) {
        case 0:
            gl_FragColor = vec4(0.25);
            break;
        case 1:
            gl_FragColor = vec4(0.5);
            break;
        case 2:
            gl_FragColor = vec4(0.75);
            break;
    }
}
"#;
    let body = main_body(src);
    let switch_body = match &body.stmts[1] {
        Stmt::Switch { body, .. } => body,
        other => panic!("expected switch, got {other:?}"),
    };
    let case_values: Vec<i64> = switch_body
        .stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::Case {
                value: Expr::IntLit { value, .. },
                ..
            } => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(case_values, vec![0, 1, 2]);
}

#[test]
fn switch_discriminant_can_be_complex_expression() {
    let src = r#"
void main() {
    int a = 2;
    int b = 3;
    switch (a + b) {
        case 5:
            gl_FragColor = vec4(1.0);
            break;
    }
}
"#;
    let body = main_body(src);
    let disc = match &body.stmts[2] {
        Stmt::Switch { discriminant, .. } => discriminant,
        other => panic!("expected switch, got {other:?}"),
    };
    assert!(matches!(disc, Expr::Binary { op: BinOp::Add, .. }));
}

#[test]
fn switch_with_no_default_parses() {
    let src = r#"
void main() {
    int x = 1;
    switch (x) {
        case 0:
            gl_FragColor = vec4(0.0);
            break;
        case 1:
            gl_FragColor = vec4(1.0);
            break;
    }
}
"#;
    let body = main_body(src);
    let switch_body = match &body.stmts[1] {
        Stmt::Switch { body, .. } => body,
        _ => panic!("expected switch"),
    };
    let default_count = switch_body
        .stmts
        .iter()
        .filter(|s| matches!(s, Stmt::Default { .. }))
        .count();
    assert_eq!(default_count, 0);
}

#[test]
fn empty_switch_body_parses() {
    let src = r#"
void main() {
    int x = 0;
    switch (x) {
    }
}
"#;
    let body = main_body(src);
    let switch_body = match &body.stmts[1] {
        Stmt::Switch { body, .. } => body,
        _ => panic!("expected switch"),
    };
    assert_eq!(switch_body.stmts.len(), 0);
}

#[test]
fn case_label_followed_by_block_lets_the_block_stand_alone() {
    // ESSL 3.00 allows `case 0: { ... }` where the block is a
    // separate Stmt::Block following the label. The parser's
    // statement loop handles this because Stmt::Block is just another
    // stmt kind.
    let src = r#"
void main() {
    int x = 0;
    switch (x) {
        case 0: {
            gl_FragColor = vec4(1.0);
            break;
        }
    }
}
"#;
    let body = main_body(src);
    let switch_body = match &body.stmts[1] {
        Stmt::Switch { body, .. } => body,
        _ => panic!("expected switch"),
    };
    // case label, block (with assign + break), no default.
    assert_eq!(switch_body.stmts.len(), 2);
    assert!(matches!(switch_body.stmts[0], Stmt::Case { .. }));
    assert!(matches!(switch_body.stmts[1], Stmt::Block(_)));
}

#[test]
fn nested_switch_parses() {
    let src = r#"
void main() {
    int outer = 0;
    int inner = 1;
    switch (outer) {
        case 0:
            switch (inner) {
                case 1:
                    gl_FragColor = vec4(1.0);
                    break;
            }
            break;
        default:
            discard;
    }
}
"#;
    let body = main_body(src);
    let outer_switch = match &body.stmts[2] {
        Stmt::Switch { body, .. } => body,
        _ => panic!("expected outer switch"),
    };
    // Look for a nested Switch inside the outer body.
    let nested_count = outer_switch
        .stmts
        .iter()
        .filter(|s| matches!(s, Stmt::Switch { .. }))
        .count();
    assert_eq!(nested_count, 1);
}

#[test]
fn switch_outside_function_body_at_toplevel_is_not_accepted() {
    // switch is only valid as a statement inside a function body.
    // At translation-unit scope, it should fail to parse.
    let src = r#"
switch (0) {
    case 0:
        break;
}
"#;
    assert!(parse_source(src).is_err());
}
