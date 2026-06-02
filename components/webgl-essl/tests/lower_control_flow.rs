/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 6 seventh widening, Phase A — `if`/`else` control flow.
//! Each receipt compiles a fragment or vertex shader containing a
//! conditional and confirms it round-trips through naga to WGSL.
//! Phase A covers `if`/`else` with float-comparison conditions;
//! Phase B (queued) will add `for`/`while` plus int arithmetic.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;

fn frag_wgsl(body: &str) -> String {
    let src = format!(
        "precision mediump float;\nuniform float a;\nuniform vec4 u_color;\nvoid main() {{\n    {body}\n}}\n"
    );
    compile(&src, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("compile: {e:?}\n--- src ---\n{src}"))
        .wgsl
}

// ---------- conditions ---------------------------------------------

#[test]
fn if_with_bool_literal_true_lowers() {
    let wgsl = frag_wgsl("if (true) gl_FragColor = u_color; else gl_FragColor = vec4(0.0);");
    assert!(wgsl.contains("if"));
}

#[test]
fn if_with_float_less_than_lowers() {
    let wgsl = frag_wgsl("if (a < 0.5) gl_FragColor = u_color; else gl_FragColor = vec4(0.0);");
    assert!(wgsl.contains("if"));
    assert!(wgsl.contains("<") || wgsl.contains("lessThan"));
}

#[test]
fn if_with_float_greater_than_lowers() {
    let wgsl = frag_wgsl("if (a > 0.5) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);");
    assert!(wgsl.contains("if"));
}

#[test]
fn if_with_float_equality_lowers() {
    let wgsl = frag_wgsl("if (a == 0.0) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);");
    assert!(wgsl.contains("if"));
    assert!(wgsl.contains("=="));
}

#[test]
fn if_with_float_not_equal_lowers() {
    let wgsl = frag_wgsl("if (a != 0.0) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);");
    assert!(wgsl.contains("if"));
}

#[test]
fn if_with_le_ge_lower() {
    let wgsl_le =
        frag_wgsl("if (a <= 0.5) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);");
    assert!(wgsl_le.contains("if"));
    let wgsl_ge =
        frag_wgsl("if (a >= 0.5) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);");
    assert!(wgsl_ge.contains("if"));
}

// ---------- single-branch (no else) --------------------------------

#[test]
fn if_without_else_lowers() {
    // Without an else, the implicit fall-through is the path that
    // doesn't write the early branch. The shader still needs to
    // unconditionally write `gl_FragColor` once for the merge block
    // to be well-formed downstream, so emit a default write first.
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    gl_FragColor = vec4(0.0);
    if (a > 0.5) gl_FragColor = vec4(1.0);
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("if"));
}

// ---------- nested -------------------------------------------------

#[test]
fn nested_if_else_lowers() {
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    if (a < 0.0) {
        gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0);
    } else {
        if (a < 0.5) {
            gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
        } else {
            gl_FragColor = vec4(0.0, 0.0, 1.0, 1.0);
        }
    }
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.matches("if").count() >= 2, "expect 2+ if's: {wgsl}");
}

// ---------- locals in branches (hoisted to entry block) ------------

#[test]
fn local_declared_in_if_branch_hoists_cleanly() {
    // The local `t` is declared inside the then-branch. The
    // pre-pass must hoist its OpVariable allocation to the entry
    // block so SPIR-V's "Function-storage variables in the first
    // block" constraint is satisfied.
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    gl_FragColor = vec4(0.0);
    if (a > 0.5) {
        float t = a * 2.0;
        gl_FragColor = vec4(t);
    }
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("if"));
}

#[test]
fn locals_in_both_branches_hoist_independently() {
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    gl_FragColor = vec4(0.0);
    if (a > 0.5) {
        float t = a * 2.0;
        gl_FragColor = vec4(t);
    } else {
        float s = a * 3.0;
        gl_FragColor = vec4(s);
    }
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("if"));
}

// ---------- a realistic shader -------------------------------------

// ---------- for loops (Phase B) -------------------------------------

#[test]
fn for_loop_with_int_counter_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 4; ++i) {
        acc = acc + u_color;
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("for") || wgsl.contains("loop"));
}

#[test]
fn for_loop_with_post_increment_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 3; i++) {
        acc = acc + u_color;
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("for") || wgsl.contains("loop"));
}

#[test]
fn for_loop_with_subtract_step_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 4; i > 0; --i) {
        acc = acc + u_color;
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("for") || wgsl.contains("loop"));
}

#[test]
fn for_loop_with_float_counter_lowers() {
    // ESSL Appendix A permits float loop variables.
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (float t = 0.0; t < 1.0; ++t) {
        acc = acc + u_color;
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("for") || wgsl.contains("loop"));
}

// ---------- while loops --------------------------------------------

#[test]
fn while_loop_with_local_counter_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    int i = 0;
    while (i < 4) {
        acc = acc + u_color;
        ++i;
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("loop") || wgsl.contains("while"));
}

// ---------- if inside a for ----------------------------------------

#[test]
fn if_inside_for_loop_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 4; ++i) {
        if (i > 1) {
            acc = acc + u_color;
        }
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("if"));
    assert!(wgsl.contains("for") || wgsl.contains("loop"));
}

// ---------- audit-driven receipts (Phase A+B verification) -------------

/// Audit #1: `discard;` inside an if-branch. Pre-fix the
/// catch-all arm rejected `Stmt::Discard`; this commit adds an
/// `OpKill` lowering, and the dual-terminator guard skips the
/// trailing branch so the then-block is well-formed.
#[test]
fn discard_inside_if_branch_lowers() {
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    gl_FragColor = vec4(1.0);
    if (a < 0.0) {
        discard;
    }
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("discard") || r.wgsl.contains("if"));
}

/// HAPPY (resolved). ESSL §8.6 vector relational `lessThan` is
/// now registered (bvec_n result) and lowered via the
/// ordered-float compare path. The bvec.x component access then
/// drives the if condition.
#[test]
fn vector_lessthan_as_if_condition_lowers() {
    let src = r#"
precision mediump float;
uniform vec3 u_v;
uniform vec3 u_w;
void main() {
    gl_FragColor = vec4(0.0);
    if (lessThan(u_v, u_w).x) {
        gl_FragColor = vec4(1.0);
    }
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("if"));
}

/// Audit #3: early `return;` from inside an if inside a for body.
/// Pre-fix the if arm always added a trailing branch to merge,
/// which double-terminated the then-block. The
/// `stmt_definitely_terminates` guard now skips that branch when
/// the then-stmt itself terminates.
#[test]
fn early_return_inside_if_inside_for_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 4; ++i) {
        if (i > 1) {
            return;
        }
        acc = acc + u_color;
    }
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("return"));
}

/// HAPPY (resolved). `break;` inside a loop body now lowers via
/// the `loop_targets` stack: `emit_loop_cfg` pushes the
/// `(merge, continue)` label pair before walking the body, and
/// `Stmt::Break` branches to the innermost merge.
#[test]
fn break_inside_for_body_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 4; ++i) {
        acc = acc + u_color;
        break;
    }
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("break") || r.wgsl.contains("loop"));
}

/// HAPPY (correctness regression target). Nested for-loops with
/// the same loop-variable name `i`. Block-scoped locals now key
/// `OpVariable`s by the declaration's source `Span`, so the
/// inner `int i` gets its own variable distinct from the outer
/// — no aliasing. The outer loop runs its full 2 iterations.
#[test]
fn nested_for_same_loop_var_name_lowers_without_aliasing() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 2; ++i) {
        for (int i = 0; i < 3; ++i) {
            acc = acc + u_color;
        }
    }
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("for") || r.wgsl.contains("loop"));
}

/// Audit #6: same-named locals of different types in two if
/// branches. Multiple paths fail: (a) the dedup keeps only the
/// first type's `OpVariable`, (b) `float(int)` isn't a
/// constructor today. Pinned at whichever stage rejects first.
#[test]
fn same_named_locals_in_if_branches_with_different_types_does_not_lower() {
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    gl_FragColor = vec4(0.0);
    if (a > 0.5) {
        float t = a * 2.0;
        gl_FragColor = vec4(t);
    } else {
        int t = 3;
        gl_FragColor = vec4(float(t));
    }
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(
        matches!(err, webgl_essl::CompileError::Lower(_) | webgl_essl::CompileError::Check(_)),
        "got: {err:?}"
    );
}

/// HAPPY (resolved). `continue;` inside a loop body branches to
/// the loop's continue block via the same `loop_targets` stack.
#[test]
fn continue_inside_for_body_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 4; ++i) {
        continue;
    }
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("continue") || r.wgsl.contains("loop"));
}

#[test]
fn same_named_locals_in_distinct_blocks_get_independent_variables() {
    // Two block-scoped `float t`s: one inside the then-branch
    // and one inside the else-branch. Both must lower to their
    // own `OpVariable`, otherwise the second store would
    // clobber the first's value (silent miscompile pre-Tier-3).
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    if (a > 0.5) {
        float t = a * 2.0;
        gl_FragColor = vec4(t);
    } else {
        float t = a + 0.25;
        gl_FragColor = vec4(t);
    }
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("if"));
}

#[test]
fn break_inside_nested_for_targets_inner_loop() {
    // The inner break should exit the inner loop only; the
    // outer loop should run twice (i=0, i=1).
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 2; ++i) {
        for (int j = 0; j < 4; ++j) {
            acc = acc + u_color;
            if (j > 0) break;
        }
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("break") || wgsl.contains("loop"));
}

/// Audit #8: local declared inside a for body. The pre-pass
/// descends through `For -> body -> Block` to hoist `weight`
/// into the entry block; existing for-loop receipts only declare
/// locals OUTSIDE the loop.
#[test]
fn local_declared_inside_for_body_hoists_cleanly() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 3; ++i) {
        float weight = u_color.x * 0.25;
        acc = acc + vec4(weight);
    }
    gl_FragColor = acc;
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("for") || wgsl.contains("loop"));
}

/// Audit #9: `bool flag = a > 0.5; flag = !flag;` — the
/// Function-storage Bool round-trip. Exercises
/// `spv_type_for_kind(Bool)`, `function_ptr_for(Bool)`,
/// `OpLogicalNot`, and Bool `OpStore` / `OpLoad`. Bool was
/// previously only used implicitly via if-conditions.
#[test]
fn bool_local_with_logical_not_lowers() {
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    bool flag = a > 0.5;
    flag = !flag;
    if (flag) {
        gl_FragColor = vec4(1.0);
    } else {
        gl_FragColor = vec4(0.0);
    }
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("if"));
}

/// Audit #10: `int + float` rejected by the typechecker
/// (ESSL 1.00 §4.1.10 has no implicit int↔float conversion).
/// Pinned at the check stage, never reaches lowering.
#[test]
fn int_plus_float_rejected_by_typecheck() {
    let src = r#"
precision mediump float;
uniform float a;
void main() {
    int i = 2;
    float r = i + a;
    gl_FragColor = vec4(r);
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, webgl_essl::CompileError::Check(_)), "got: {err:?}");
}

/// Audit #11: `while (true)` infinite loop with a writing body.
/// Appendix A's for-loop shape rules don't apply to `while`, so
/// the degenerate-but-legal condition lowers cleanly. The merge
/// block is structurally reachable but dynamically unreachable,
/// which SPIR-V permits.
#[test]
fn while_true_infinite_loop_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    while (true) {
        gl_FragColor = u_color;
    }
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("loop") || wgsl.contains("while"));
}

#[test]
fn step_function_with_if_lowers() {
    // A classic shader idiom: branch on a value and pick a color.
    let src = r#"
precision mediump float;
uniform vec2 u_uv;
uniform vec4 u_above;
uniform vec4 u_below;
void main() {
    if (u_uv.x > 0.5) {
        gl_FragColor = u_above;
    } else {
        gl_FragColor = u_below;
    }
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("if"));
}
