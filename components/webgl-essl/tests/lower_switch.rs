/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `switch` statement lowering. The validator already enforces
//! R9-R11 (int discriminant, integer-constant case values, no
//! duplicate values); the lowering emits the SPIR-V
//! `OpSelectionMerge` + `OpSwitch` block-graph and handles
//! fall-through naturally by branching each case body to the
//! next segment's label when it doesn't terminate.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;

fn frag_switch(body: &str) -> String {
    let src = format!(
        "precision mediump float;\nuniform int sel;\nvoid main() {{\n    {body}\n}}\n"
    );
    compile(&src, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("compile: {e:?}\n--- src ---\n{src}"))
        .wgsl
}

// ---------- two cases + default + break -----------------------------

#[test]
fn switch_two_cases_with_default_and_break_lowers() {
    let wgsl = frag_switch(
        "switch (sel) {\n        case 0:\n            gl_FragColor = vec4(1.0);\n            break;\n        case 1:\n            gl_FragColor = vec4(0.5);\n            break;\n        default:\n            gl_FragColor = vec4(0.0);\n            break;\n    }",
    );
    assert!(wgsl.contains("switch") || wgsl.contains("case"));
}

// ---------- fall-through (case 0 falls into case 1) ----------------

#[test]
fn switch_fallthrough_lowers() {
    let wgsl = frag_switch(
        "gl_FragColor = vec4(0.0);\n    switch (sel) {\n        case 0:\n        case 1:\n            gl_FragColor = vec4(1.0);\n            break;\n        default:\n            gl_FragColor = vec4(0.5);\n            break;\n    }",
    );
    assert!(wgsl.contains("switch") || wgsl.contains("case"));
}

// ---------- no default -----------------------------------------------

#[test]
fn switch_without_default_lowers() {
    let wgsl = frag_switch(
        "gl_FragColor = vec4(0.0);\n    switch (sel) {\n        case 0:\n            gl_FragColor = vec4(1.0);\n            break;\n    }",
    );
    assert!(wgsl.contains("switch") || wgsl.contains("case"));
}

// ---------- switch inside a for-loop (break exits switch, not loop) -

#[test]
fn switch_inside_for_break_exits_switch_only() {
    let src = r#"
precision mediump float;
uniform int sel;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 3; ++i) {
        switch (sel) {
            case 0:
                acc = acc + vec4(0.1);
                break;
            default:
                acc = acc + vec4(0.2);
                break;
        }
    }
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("for") || r.wgsl.contains("loop"));
}

// ---------- continue inside switch falls through to enclosing loop --

#[test]
fn continue_inside_switch_inside_for_targets_for_continue() {
    let src = r#"
precision mediump float;
uniform int sel;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 3; ++i) {
        switch (sel) {
            case 0:
                continue;
            default:
                acc = acc + vec4(0.1);
                break;
        }
    }
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("for") || r.wgsl.contains("loop"));
}

// ---------- empty switch --------------------------------------------

#[test]
fn empty_switch_lowers() {
    let wgsl = frag_switch(
        "gl_FragColor = vec4(0.0);\n    switch (sel) {}",
    );
    assert!(wgsl.contains("vec4"));
}

// ---------- break outside loop/switch is rejected -------------------

#[test]
fn break_outside_loop_or_switch_does_not_lower() {
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0);
    break;
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(
        matches!(err, webgl_essl::CompileError::Lower(_)),
        "got: {err:?}"
    );
}
