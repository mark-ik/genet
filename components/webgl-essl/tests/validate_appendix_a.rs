/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 second chunk: Appendix A `for` loop restriction (R4) and
//! reserved identifier prefix check (R5). Two restrictions, two test
//! sections.

use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

fn for_loop_issues(r: &webgl_essl::validate::ValidationResult) -> Vec<&'static str> {
    r.errors
        .iter()
        .filter_map(|d| match &d.kind {
            WebGlDiagnosticKind::ForLoopAppendixA { what } => Some(*what),
            _ => None,
        })
        .collect()
}

fn reserved_diagnostics(r: &webgl_essl::validate::ValidationResult) -> Vec<(String, &'static str)> {
    r.errors
        .iter()
        .filter_map(|d| match &d.kind {
            WebGlDiagnosticKind::ReservedIdentifier { name, reason } => {
                Some((name.clone(), *reason))
            },
            _ => None,
        })
        .collect()
}

// ---------- R4: Appendix A for loop ------------------------------------

#[test]
fn canonical_int_counter_loop_passes() {
    let src = r#"
void main() {
    int sum = 0;
    for (int i = 0; i < 4; i++) {
        sum = sum + i;
    }
    gl_Position = vec4(0.0);
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert!(
        for_loop_issues(&r).is_empty(),
        "got {:?}",
        for_loop_issues(&r)
    );
}

#[test]
fn loop_using_i_assign_i_plus_one_step_passes() {
    let src = r#"
void main() {
    float t = 0.0;
    for (int i = 0; i < 4; i = i + 1) {
        t = t + 1.0;
    }
    gl_Position = vec4(t, 0.0, 0.0, 1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert!(
        for_loop_issues(&r).is_empty(),
        "got {:?}",
        for_loop_issues(&r)
    );
}

#[test]
fn empty_slot_for_loop_is_rejected() {
    let src = r#"
void main() {
    for (;;) {
        gl_Position = vec4(0.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let issues = for_loop_issues(&r);
    assert!(!issues.is_empty(), "expected an Appendix A violation");
    assert!(issues[0].contains("loop variable"), "got {issues:?}");
}

#[test]
fn for_loop_with_bool_init_is_rejected() {
    let src = r#"
void main() {
    for (bool b = false; b; b = true) {
        gl_Position = vec4(0.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let issues = for_loop_issues(&r);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("`int` or `float`"), "got {issues:?}");
}

#[test]
fn for_loop_without_init_value_is_rejected() {
    let src = r#"
void main() {
    int i;
    for (int j; j < 4; j++) {
        gl_Position = vec4(0.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let issues = for_loop_issues(&r);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("initializer"), "got {issues:?}");
}

#[test]
fn for_loop_cond_not_comparison_is_rejected() {
    // ESSL 1.00 Appendix A: condition must be a comparison.
    // We accept &&, ||, == in the parser but the validator's check
    // restricts to ordering / equality comparisons.
    let src = r#"
void main() {
    for (int i = 0; i + 1; i++) {
        gl_Position = vec4(0.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let issues = for_loop_issues(&r);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("operator"), "got {issues:?}");
}

#[test]
fn for_loop_cond_without_loop_var_is_rejected() {
    let src = r#"
uniform float u_cap;
void main() {
    for (int i = 0; u_cap < 1.0; i++) {
        gl_Position = vec4(0.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let issues = for_loop_issues(&r);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("loop variable"), "got {issues:?}");
}

#[test]
fn for_loop_step_does_not_update_loop_var_is_rejected() {
    let src = r#"
void main() {
    int j = 0;
    for (int i = 0; i < 4; j++) {
        gl_Position = vec4(0.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let issues = for_loop_issues(&r);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("loop variable"), "got {issues:?}");
}

#[test]
fn for_loop_with_missing_step_is_rejected() {
    let src = r#"
void main() {
    for (int i = 0; i < 4; ) {
        gl_Position = vec4(0.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    let issues = for_loop_issues(&r);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].contains("step"), "got {issues:?}");
}

#[test]
fn float_counter_loop_passes() {
    // ESSL allows float loop variables too.
    let src = r#"
void main() {
    for (float t = 0.0; t < 1.0; t += 0.25) {
        gl_Position = vec4(t, 0.0, 0.0, 1.0);
    }
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert!(
        for_loop_issues(&r).is_empty(),
        "got {:?}",
        for_loop_issues(&r)
    );
}

// ---------- R5: reserved identifier prefixes ---------------------------

#[test]
fn declaring_gl_prefixed_global_emits_reserved_error() {
    let src = r#"
uniform vec4 gl_MyColor;
void main() {
    gl_FragColor = gl_MyColor;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(
        reserved
            .iter()
            .any(|(n, reason)| n == "gl_MyColor" && reason.contains("gl_"))
    );
}

#[test]
fn declaring_webgl_prefixed_function_emits_reserved_error() {
    let src = r#"
float webgl_helper(float x) { return x * 2.0; }
void main() {
    gl_FragColor = vec4(webgl_helper(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(reserved.iter().any(|(n, _)| n == "webgl_helper"));
}

#[test]
fn underscore_underscore_in_identifier_is_reserved() {
    let src = r#"
uniform float my__cap;
void main() {
    gl_FragColor = vec4(my__cap);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(
        reserved
            .iter()
            .any(|(n, reason)| n == "my__cap" && reason.contains("__")),
        "got {reserved:?}"
    );
}

#[test]
fn _webgl_prefix_is_reserved() {
    let src = r#"
uniform float _webgl_internal;
void main() {
    gl_FragColor = vec4(_webgl_internal);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(
        reserved
            .iter()
            .any(|(n, reason)| n == "_webgl_internal" && reason.contains("_webgl_"))
    );
}

#[test]
fn function_parameter_with_reserved_name_emits_error() {
    let src = r#"
float helper(float gl_arg) { return gl_arg * 2.0; }
void main() {
    gl_FragColor = vec4(helper(0.5));
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(reserved.iter().any(|(n, _)| n == "gl_arg"));
}

#[test]
fn local_decl_with_reserved_name_emits_error() {
    let src = r#"
void main() {
    float gl_temp = 0.5;
    gl_FragColor = vec4(gl_temp);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(reserved.iter().any(|(n, _)| n == "gl_temp"));
}

#[test]
fn using_builtin_gl_position_or_gl_fragcolor_is_not_a_reserved_error() {
    // The check applies to DECLARATIONS, not uses of built-ins.
    let src = r#"
void main() {
    gl_FragColor = vec4(1.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(reserved.is_empty(), "got {reserved:?}");
}

#[test]
fn ordinary_underscore_prefix_is_not_reserved() {
    let src = r#"
uniform float _scale;
void main() {
    gl_FragColor = vec4(_scale);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let reserved = reserved_diagnostics(&r);
    assert!(reserved.is_empty(), "got {reserved:?}");
}
