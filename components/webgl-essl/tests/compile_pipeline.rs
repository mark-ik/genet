/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Audit-response receipt: the production-shaped `compile(source,
//! stage)` entry point routes source through parse → check →
//! validate → lower and stops at the first failing stage. Also
//! confirms the validator's info-log lines now carry real 1-based
//! line numbers (the previous `source_text = ""` bug rendered every
//! line as `0:0:`).

use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind};
use webgl_essl::{CompileError, compile};

// ---------- happy path -------------------------------------------------

#[test]
fn const_color_fragment_compiles_to_wgsl() {
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
    assert!(
        r.info_log.is_empty(),
        "no errors expected, got: {}",
        r.info_log
    );
}

#[test]
fn vertex_attribute_passthrough_compiles_to_wgsl() {
    let src = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("@vertex"));
    assert!(r.wgsl.contains("location(0)"));
}

#[test]
fn uniform_fragment_compiles_to_wgsl() {
    let src = r#"
precision mediump float;
uniform mediump vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("@group(0)") && r.wgsl.contains("@binding(0)"));
}

// ---------- stage gating -----------------------------------------------

#[test]
fn parse_failure_routed_through_CompileError_Parse() {
    let src = "void main() { this is not valid GLSL ;;; }";
    match compile(src, ShaderStage::Fragment) {
        Err(CompileError::Parse(_)) => {},
        other => panic!("expected CompileError::Parse, got {other:?}"),
    }
}

#[test]
fn typecheck_failure_routed_through_CompileError_Check() {
    // Reference an undeclared identifier; typecheck flags it before
    // validation or lowering can run.
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(unknown_var);
}
"#;
    match compile(src, ShaderStage::Fragment) {
        Err(CompileError::Check(diags)) => {
            assert!(!diags.is_empty());
        },
        other => panic!("expected CompileError::Check, got {other:?}"),
    }
}

#[test]
fn validate_failure_routed_through_CompileError_Validate() {
    // Recursive function; validator R1 flags it.
    let src = r#"
precision mediump float;
void f() { f(); }
void main() {
    f();
    gl_FragColor = vec4(0.0);
}
"#;
    match compile(src, ShaderStage::Fragment) {
        Err(CompileError::Validate(r)) => {
            assert!(
                r.errors
                    .iter()
                    .any(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. }))
            );
        },
        other => panic!("expected CompileError::Validate, got {other:?}"),
    }
}

#[test]
fn lowering_failure_routed_through_CompileError_Lower() {
    // A shape the lowering does not handle: a write-side
    // (LHS) swizzle. `lower_stmt` only accepts `Expr::Ident`
    // as the assignment LHS today; `Expr::Member` is rejected.
    let src = r#"
precision mediump float;
varying vec4 v_color;
void main() {
    v_color.x = 1.0;
    gl_FragColor = v_color;
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Lower(_)), "got: {err:?}");
}

// ---------- info_log carries real line numbers ------------------------

#[test]
fn validate_error_info_log_uses_one_based_line_numbers() {
    // R2 discard in a vertex shader. The discard is on line 3 of the
    // source. The pre-fix renderer would have emitted `ERROR: 0:0:`.
    let src = "void main() {\n    if (true) discard;\n    gl_Position = vec4(0.0);\n}\n";
    match compile(src, ShaderStage::Vertex) {
        Err(CompileError::Validate(r)) => {
            let line: &str = r
                .info_log
                .lines()
                .find(|l| l.contains("discard"))
                .expect("a line");
            // Should be `ERROR: 0:2: ...` (the discard is on line 2,
            // not line 0).
            assert!(
                line.starts_with("ERROR: 0:") && !line.starts_with("ERROR: 0:0:"),
                "line should not have placeholder 0:0; got: {line}"
            );
        },
        other => panic!("expected CompileError::Validate, got {other:?}"),
    }
}

#[test]
fn validate_error_line_number_advances_with_actual_lines() {
    // Same diagnostic but two extra blank lines before the discard.
    let src = "void main() {\n\n\n    discard;\n}\n";
    match compile(src, ShaderStage::Vertex) {
        Err(CompileError::Validate(r)) => {
            let line: &str = r
                .info_log
                .lines()
                .find(|l| l.contains("discard"))
                .expect("a line");
            // The discard is on line 4 now.
            assert!(line.contains("0:4:"), "expected 0:4:, got: {line}");
        },
        other => panic!("expected CompileError::Validate, got {other:?}"),
    }
}
