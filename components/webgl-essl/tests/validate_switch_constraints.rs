/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 fifth chunk, R9 / R10 / R11: WebGL validator restrictions
//! on `switch`. Each test isolates one rule so a regression points at
//! the right ESSL 3.00 §6.5 / WebGL2 packing constraint.
//!
//! - R9: discriminant must resolve to `int` (not float, vec, bool).
//! - R10: each `case <value>:` label's value must be a literal
//!   integer constant. First-pass impl accepts only `IntLit`; full
//!   constant-folding is queued.
//! - R11: two case labels inside the same switch must not share the
//!   same integer value.

use webgl_essl::ast::TypeKind;
use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

// ---------- R9: switch discriminant must be int ------------------------

#[test]
fn switch_on_int_passes_r9() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 0:
            break;
        case 1:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r9: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::SwitchDiscriminantNotInt { .. }))
        .collect();
    assert!(r9.is_empty(), "int discriminant should pass R9: {r9:?}");
}

#[test]
fn switch_on_float_emits_r9() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    float x = 1.0;
    switch (x) {
        case 0:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r9: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::SwitchDiscriminantNotInt { .. }))
        .collect();
    assert_eq!(r9.len(), 1, "float discriminant should fail R9");
    match &r9[0].kind {
        WebGlDiagnosticKind::SwitchDiscriminantNotInt { actual } => {
            assert_eq!(*actual, TypeKind::Float);
        },
        _ => unreachable!(),
    }
}

#[test]
fn switch_on_bool_emits_r9() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    bool b = true;
    switch (b) {
        case 0:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r9: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::SwitchDiscriminantNotInt { .. }))
        .collect();
    assert_eq!(r9.len(), 1, "bool discriminant should fail R9");
}

#[test]
fn switch_on_vec_emits_r9() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    vec2 v = vec2(0.0);
    switch (v) {
        case 0:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r9: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::SwitchDiscriminantNotInt { .. }))
        .collect();
    assert_eq!(r9.len(), 1, "vec2 discriminant should fail R9");
}

// ---------- R10: case value must be an integer constant ----------------

#[test]
fn case_with_int_literal_passes_r10() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 0:
        case 1:
        case 7:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r10: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::CaseValueNotIntegerConstant))
        .collect();
    assert!(r10.is_empty(), "int-literal cases should pass R10: {r10:?}");
}

#[test]
fn case_with_identifier_emits_r10() {
    // First-pass implementation only accepts literal IntLits; an
    // identifier (even one bound to a `const int`) is not folded
    // yet, so it must fail R10.
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    int k = 0;
    switch (x) {
        case k:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r10: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::CaseValueNotIntegerConstant))
        .collect();
    assert_eq!(r10.len(), 1, "identifier case value should fail R10");
}

#[test]
fn case_with_float_literal_emits_r10() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 0.0:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r10: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::CaseValueNotIntegerConstant))
        .collect();
    assert_eq!(r10.len(), 1, "float-literal case value should fail R10");
}

#[test]
fn case_with_binary_expression_emits_r10() {
    // `2 + 3` is constant-foldable but the first-pass impl does not
    // fold; the spec requires a constant integer expression, which a
    // future pass should accept. For now, the binary expression is
    // flagged.
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 2 + 3:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r10: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::CaseValueNotIntegerConstant))
        .collect();
    assert_eq!(r10.len(), 1, "binary-expression case value should fail R10 (first pass)");
}

// ---------- R11: duplicate case values --------------------------------

#[test]
fn duplicate_case_values_in_same_switch_emit_r11() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 1:
        case 2:
        case 1:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r11: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::DuplicateCaseValue { value: 1 }))
        .collect();
    assert_eq!(r11.len(), 1, "duplicate `case 1:` should fire once");
}

#[test]
fn distinct_case_values_in_same_switch_pass_r11() {
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 0:
        case 1:
        case 2:
        case 3:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r11: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::DuplicateCaseValue { .. }))
        .collect();
    assert!(r11.is_empty(), "distinct case values should pass R11: {r11:?}");
}

#[test]
fn same_case_value_in_distinct_switches_passes_r11() {
    // R11 is per-switch: two switches can both have `case 1:`.
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 1:
            break;
    }
    switch (x) {
        case 1:
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r11: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::DuplicateCaseValue { .. }))
        .collect();
    assert!(r11.is_empty(), "duplicates only fire within a single switch: {r11:?}");
}

#[test]
fn duplicate_case_inside_nested_switch_emits_r11() {
    // Outer and inner switches each have their own duplicate set.
    let src = r#"#version 300 es
precision mediump float;
void main() {
    int x = 1;
    switch (x) {
        case 1:
            switch (x) {
                case 2:
                case 2:
                    break;
            }
            break;
    }
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    let r11: Vec<_> = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::DuplicateCaseValue { value: 2 }))
        .collect();
    assert_eq!(r11.len(), 1, "inner switch's duplicate `case 2:` should fire");
}
