/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! First typecheck pass: symbol resolution, literal types, identifier
//! types. Each test isolates one rule so a regression points at the
//! right line in `check.rs`.

use webgl_essl::ast::*;
use webgl_essl::check::{TypeDiagnosticKind, check};
use webgl_essl::parse_source;

fn check_clean(src: &str) -> webgl_essl::check::CheckResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let r = check(&tu);
    if !r.diagnostics.is_empty() {
        let rendered: Vec<String> =
            r.diagnostics.iter().map(|d| format!("{}", d.display(src))).collect();
        panic!("expected zero diagnostics, got: {}\n--- source ---\n{src}", rendered.join("; "));
    }
    r
}

// ---------- literal types ----------------------------------------------

#[test]
fn literal_types_are_annotated() {
    let src = r#"
void main() {
    float f = 3.14;
    int i = 42;
    bool b = true;
}
"#;
    let r = check_clean(src);
    let mut int_count = 0;
    let mut float_count = 0;
    let mut bool_count = 0;
    for &ty in r.types.values() {
        match ty {
            TypeKind::Int => int_count += 1,
            TypeKind::Float => float_count += 1,
            TypeKind::Bool => bool_count += 1,
            _ => {},
        }
    }
    assert_eq!(int_count, 1, "one IntLit (42)");
    assert_eq!(float_count, 1, "one FloatLit (3.14)");
    assert_eq!(bool_count, 1, "one BoolLit (true)");
}

// ---------- local variable resolution ----------------------------------

#[test]
fn local_var_resolves_in_same_block() {
    let src = r#"
void main() {
    float x = 1.0;
    float y = x;
}
"#;
    check_clean(src);
}

#[test]
fn local_var_does_not_leak_into_sibling_function() {
    let src = r#"
void a() {
    float x = 1.0;
}
void b() {
    float y = x;
}
"#;
    let tu = parse_source(src).unwrap();
    let r = check(&tu);
    assert_eq!(r.diagnostics.len(), 1, "x should be unknown in b()");
    match &r.diagnostics[0].kind {
        TypeDiagnosticKind::UnknownIdentifier { name } => assert_eq!(name, "x"),
    }
}

// ---------- global variable resolution ---------------------------------

#[test]
fn global_uniform_resolves_inside_function() {
    let src = r#"
uniform float u_amount;
void main() {
    float y = u_amount;
}
"#;
    check_clean(src);
}

#[test]
fn attribute_resolves_in_vertex_shader() {
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    check_clean(src);
}

// ---------- function parameter resolution ------------------------------

#[test]
fn param_resolves_inside_body() {
    let src = r#"
float square(float x) {
    return x * x;
}
void main() {}
"#;
    check_clean(src);
}

#[test]
fn param_does_not_leak_outside_function() {
    let src = r#"
void inner(float x) {}
void main() {
    float y = x;
}
"#;
    let r = check(&parse_source(src).unwrap());
    assert_eq!(r.diagnostics.len(), 1);
    match &r.diagnostics[0].kind {
        TypeDiagnosticKind::UnknownIdentifier { name } => assert_eq!(name, "x"),
    }
}

// ---------- function name + recursion ----------------------------------

#[test]
fn function_name_visible_to_callers() {
    let src = r#"
float helper(float x) {
    return x + x;
}
void main() {
    float y = helper(1.0);
}
"#;
    // `helper` reaches the Ident lookup path? Actually no, because the
    // parser folds `helper(1.0)` into a Call with `callee: "helper"`,
    // not Ident("helper") followed by call. The Call itself is not yet
    // typechecked, so this is silently clean.
    check_clean(src);
}

// ---------- built-ins ---------------------------------------------------

#[test]
fn gl_fragcolor_is_seeded_as_builtin() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    check_clean(src);
}

#[test]
fn gl_position_is_seeded_as_builtin() {
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    check_clean(src);
}

// ---------- nested resolution through call args ------------------------

#[test]
fn ident_inside_call_args_resolves() {
    let src = r#"
uniform vec4 u_color;
void main() {
    gl_FragColor = vec4(u_color.r, 0.0, 0.0, 1.0);
}
"#;
    // u_color.r is a Member { base: Ident(u_color) }. The base ident
    // gets visited and resolved.
    check_clean(src);
}

#[test]
fn ident_inside_binary_op_resolves() {
    let src = r#"
uniform float u_scale;
attribute vec2 a_position;
void main() {
    vec2 scaled = a_position * u_scale;
    gl_Position = vec4(scaled, 0.0, 1.0);
}
"#;
    check_clean(src);
}

// ---------- block scope -------------------------------------------------

#[test]
fn block_local_does_not_escape_its_block() {
    let src = r#"
void main() {
    if (true) {
        float inside = 1.0;
    }
    float y = inside;
}
"#;
    let r = check(&parse_source(src).unwrap());
    assert_eq!(r.diagnostics.len(), 1, "inside should be unknown after the if block");
    match &r.diagnostics[0].kind {
        TypeDiagnosticKind::UnknownIdentifier { name } => assert_eq!(name, "inside"),
    }
}

#[test]
fn outer_local_visible_in_nested_block() {
    let src = r#"
void main() {
    float outer = 1.0;
    if (true) {
        float inner = outer;
    }
}
"#;
    check_clean(src);
}

// ---------- unknown identifier diagnostic ------------------------------

#[test]
fn unknown_ident_diagnostic_carries_name_and_span() {
    let src = "void main() { float y = unknown_var; }";
    let r = check(&parse_source(src).unwrap());
    assert_eq!(r.diagnostics.len(), 1);
    let d = &r.diagnostics[0];
    match &d.kind {
        TypeDiagnosticKind::UnknownIdentifier { name } => assert_eq!(name, "unknown_var"),
    }
    // Span points at the identifier reference, not the declaration.
    let rendered = format!("{}", d.display(src));
    assert!(rendered.contains("unknown_var"), "rendered: {rendered}");
    assert!(rendered.contains("unknown identifier"), "rendered: {rendered}");
}

// ---------- scope teardown ---------------------------------------------

#[test]
fn scopes_pop_clean_after_walk() {
    // If the typechecker leaks scopes, subsequent unresolved idents
    // would still find symbols from prior functions. This test parses
    // two functions and looks up an ident in the second that was only
    // declared in the first.
    let src = r#"
void a() {
    float only_in_a = 1.0;
    float use_a = only_in_a;
}
void b() {
    float misuse = only_in_a;
}
"#;
    let r = check(&parse_source(src).unwrap());
    assert_eq!(r.diagnostics.len(), 1, "only_in_a should not leak into b()");
}
