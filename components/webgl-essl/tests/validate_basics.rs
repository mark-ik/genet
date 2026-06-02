/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 first chunk: WebGL validator restrictions plus the
//! `getShaderInfoLog`-shaped diagnostic rendering. Each test isolates
//! one restriction (R1 recursion, R2 discard-in-vertex, R3 main
//! signature) so a regression points at the right ANGLE-borrowed rule.

use webgl_essl::ast::TypeKind;
use webgl_essl::parse_source;
use webgl_essl::validate::{
    ShaderStage, Severity, WebGlDiagnosticKind, validate,
};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

// ---------- R1: no recursion -------------------------------------------

#[test]
fn direct_self_recursion_emits_error() {
    let src = r#"
void f() { f(); }
void main() { f(); }
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let recursion: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. }))
        .collect();
    assert_eq!(recursion.len(), 1, "direct self-recursion `f -> f`");
    match &recursion[0].kind {
        WebGlDiagnosticKind::Recursion { cycle } => {
            assert!(cycle.iter().any(|n| n == "f"), "cycle should contain f: {cycle:?}");
        },
        _ => unreachable!(),
    }
}

#[test]
fn mutual_recursion_emits_error() {
    let src = r#"
void a() { b(); }
void b() { a(); }
void main() { a(); }
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let recursion: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. }))
        .collect();
    assert_eq!(recursion.len(), 1, "mutual a <-> b: one cycle reported");
    match &recursion[0].kind {
        WebGlDiagnosticKind::Recursion { cycle } => {
            assert!(cycle.contains(&"a".to_string()));
            assert!(cycle.contains(&"b".to_string()));
        },
        _ => unreachable!(),
    }
}

#[test]
fn three_function_cycle_emits_error() {
    let src = r#"
void a() { b(); }
void b() { c(); }
void c() { a(); }
void main() { a(); }
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let recursion: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. }))
        .collect();
    assert_eq!(recursion.len(), 1, "three-way cycle a -> b -> c -> a");
}

#[test]
fn non_recursive_helper_chain_is_clean() {
    let src = r#"
float square(float x) { return x * x; }
float quad(float x) { return square(square(x)); }
void main() {
    float y = quad(0.5);
    gl_FragColor = vec4(y, y, y, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r.errors.iter().filter(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. })).count(),
        0
    );
}

#[test]
fn calling_builtins_does_not_count_as_recursion() {
    let src = r#"
void main() {
    float x = sin(cos(tan(0.5)));
    gl_FragColor = vec4(x, x, x, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r.errors.iter().filter(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. })).count(),
        0
    );
}

// ---------- R2: discard only in fragment ------------------------------

#[test]
fn discard_in_fragment_main_is_clean() {
    let src = r#"
precision mediump float;
void main() {
    if (true) discard;
    gl_FragColor = vec4(1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r.errors.iter().filter(|d| matches!(d.kind, WebGlDiagnosticKind::DiscardOutsideFragment { .. })).count(),
        0
    );
}

#[test]
fn discard_in_vertex_main_emits_error() {
    let src = r#"
void main() {
    if (true) discard;
    gl_Position = vec4(0.0, 0.0, 0.0, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let bad: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::DiscardOutsideFragment { .. }))
        .collect();
    assert_eq!(bad.len(), 1);
    match &bad[0].kind {
        WebGlDiagnosticKind::DiscardOutsideFragment { function } => {
            assert_eq!(function, "main");
        },
        _ => unreachable!(),
    }
}

#[test]
fn discard_in_helper_called_from_vertex_emits_error_with_helper_function_name() {
    // discard usage is rejected wherever it appears in a vertex shader,
    // not just in `main`.
    let src = r#"
void abort_path() { discard; }
void main() {
    abort_path();
    gl_Position = vec4(0.0, 0.0, 0.0, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let bad: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::DiscardOutsideFragment { .. }))
        .collect();
    assert_eq!(bad.len(), 1);
    match &bad[0].kind {
        WebGlDiagnosticKind::DiscardOutsideFragment { function } => {
            assert_eq!(function, "abort_path", "the function holding the discard, not main");
        },
        _ => unreachable!(),
    }
}

// ---------- R3: main signature ----------------------------------------

#[test]
fn missing_main_emits_error() {
    let src = "void helper() {}";
    let r = validate_src(src, ShaderStage::Fragment);
    let bad: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::MainNotDefined))
        .collect();
    assert_eq!(bad.len(), 1);
}

#[test]
fn main_with_non_void_return_emits_error() {
    let src = "float main() { return 0.0; }";
    let r = validate_src(src, ShaderStage::Fragment);
    let bad: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::MainBadSignature { .. }))
        .collect();
    assert_eq!(bad.len(), 1);
    match &bad[0].kind {
        WebGlDiagnosticKind::MainBadSignature { return_ty, param_count } => {
            assert_eq!(*return_ty, TypeKind::Float);
            assert_eq!(*param_count, 0);
        },
        _ => unreachable!(),
    }
}

#[test]
fn main_with_params_emits_error() {
    let src = "void main(float x) {}";
    let r = validate_src(src, ShaderStage::Fragment);
    let bad: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::MainBadSignature { .. }))
        .collect();
    assert_eq!(bad.len(), 1);
    match &bad[0].kind {
        WebGlDiagnosticKind::MainBadSignature { return_ty, param_count } => {
            assert_eq!(*return_ty, TypeKind::Void);
            assert_eq!(*param_count, 1);
        },
        _ => unreachable!(),
    }
}

#[test]
fn void_main_void_is_accepted() {
    let src = "void main(void) {}";
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r.errors.iter().filter(|d| matches!(d.kind, WebGlDiagnosticKind::MainBadSignature { .. })).count(),
        0,
        "`void main(void)` should be accepted alongside `void main()`"
    );
    assert_eq!(
        r.errors.iter().filter(|d| matches!(d.kind, WebGlDiagnosticKind::MainNotDefined)).count(),
        0
    );
}

// ---------- info_log shape --------------------------------------------

#[test]
fn info_log_renders_errors_as_angle_shaped_lines() {
    let src = r#"
void main() {
    discard;
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert!(r.num_errors() >= 1);
    // ANGLE / Chrome shape: each line begins with `ERROR: 0:LINE:`.
    let lines: Vec<&str> = r.info_log.lines().collect();
    let first = lines[0];
    assert!(first.starts_with("ERROR: 0:"), "got: {first}");
    assert!(first.contains("discard"), "message should mention discard: {first}");
}

#[test]
fn empty_info_log_when_clean() {
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r.errors.len(), 0);
    assert_eq!(r.warnings.len(), 0);
    assert!(r.info_log.is_empty(), "no diagnostics, empty log");
}

#[test]
fn num_errors_and_num_warnings_accessors() {
    let src = "void helper() {}"; // missing main
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(r.num_errors(), 1);
    assert_eq!(r.num_warnings(), 0);
}

#[test]
fn severity_is_error_for_r1_r2_r3() {
    // None of these are spec-warnings; all are spec errors.
    let src = r#"
void f() { f(); }
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    for d in &r.errors {
        assert_eq!(d.severity, Severity::Error);
    }
}

// ---------- multi-error rendering -------------------------------------

#[test]
fn multi_error_log_has_one_line_per_diagnostic() {
    let src = r#"
void f() { f(); }
"#;
    // No main + recursion in f. Two errors expected.
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(r.num_errors() >= 2);
    let non_empty_lines: Vec<&str> = r.info_log.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(non_empty_lines.len(), r.num_errors());
}
