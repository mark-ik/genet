/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 third chunk: R6 expression complexity cap and R7 call stack
//! depth cap, mirroring ANGLE's `limitExpressionComplexity` and
//! `limitCallStackDepth` `CompileOptions` flags.
//!
//! The caps are hardcoded today (256 expression nodes, 16 call-chain
//! depth). When `validate(tu, stage)` grows a `CompileOptions`-shaped
//! parameter, both numbers move into it.

use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

fn complexity_count_and_limit(
    r: &webgl_essl::validate::ValidationResult,
) -> Option<(usize, usize)> {
    r.errors.iter().find_map(|d| match d.kind {
        WebGlDiagnosticKind::ExpressionTooComplex { count, limit } => Some((count, limit)),
        _ => None,
    })
}

fn call_depth_and_limit(r: &webgl_essl::validate::ValidationResult) -> Option<(usize, usize)> {
    r.errors.iter().find_map(|d| match d.kind {
        WebGlDiagnosticKind::CallStackTooDeep { depth, limit } => Some((depth, limit)),
        _ => None,
    })
}

// ---------- R6: expression complexity ---------------------------------

#[test]
fn small_expression_under_complexity_cap() {
    let src = r#"
void main() {
    gl_FragColor = vec4(1.0, 2.0, 3.0, 4.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(complexity_count_and_limit(&r).is_none());
}

#[test]
fn moderate_expression_below_cap() {
    // ~30 binary additions; total node count around 60, well under 256.
    let body = std::iter::repeat("1.0")
        .take(30)
        .collect::<Vec<_>>()
        .join(" + ");
    let src = format!("void main() {{ float x = {body}; }}");
    let r = validate_src(&src, ShaderStage::Fragment);
    assert!(
        complexity_count_and_limit(&r).is_none(),
        "expected no diagnostic, got {:?}",
        complexity_count_and_limit(&r)
    );
}

#[test]
fn expression_just_over_cap_triggers_diagnostic() {
    // 130 float literals separated by `+`. Each lit is a node; each
    // `+` is a Binary node combining the running sum with the next
    // lit. 130 lits + 129 binaries = 259 nodes, just over the 256 cap.
    let body = std::iter::repeat("1.0")
        .take(130)
        .collect::<Vec<_>>()
        .join(" + ");
    let src = format!("void main() {{ float x = {body}; }}");
    let r = validate_src(&src, ShaderStage::Fragment);
    let (count, limit) = complexity_count_and_limit(&r).expect("complexity diagnostic");
    assert!(count > limit, "{count} should exceed {limit}");
    assert_eq!(limit, 256);
}

#[test]
fn deeply_nested_constructor_does_not_double_count_children() {
    // The visitor only checks at top-level expressions (when
    // expr_depth == 0). Without that gate this test would emit one
    // diagnostic per subexpression and the result would be unusable.
    // A vec4 of vec4 of vec4 ... built up from 1.0 ends up under
    // the cap if children are not double-counted at every level.
    let src = r#"
void main() {
    gl_FragColor = vec4(
        vec4(1.0, 1.0, 1.0, 1.0).x,
        vec4(2.0, 2.0, 2.0, 2.0).y,
        vec4(3.0, 3.0, 3.0, 3.0).z,
        vec4(4.0, 4.0, 4.0, 4.0).w
    );
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    // Whatever the total node count comes to, the visitor must emit
    // at most one ExpressionTooComplex per top-level expression, and
    // this expression should be well under the cap anyway.
    let count = r
        .errors
        .iter()
        .filter(|d| matches!(d.kind, WebGlDiagnosticKind::ExpressionTooComplex { .. }))
        .count();
    assert!(count <= 1, "got {count} complexity diagnostics");
}

// ---------- R7: call stack depth --------------------------------------

#[test]
fn shallow_call_chain_under_depth_cap() {
    let src = r#"
float helper(float x) { return x * 2.0; }
void main() {
    float y = helper(0.5);
    gl_FragColor = vec4(y);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(call_depth_and_limit(&r).is_none());
}

#[test]
fn moderate_call_chain_at_cap_is_clean() {
    // Build main -> f1 -> f2 -> ... -> f14 (depth 15, within cap 16).
    let mut src = String::new();
    for i in 0..14 {
        src.push_str(&format!("void f{i}() {{ f{}(); }}\n", i + 1));
    }
    src.push_str("void f14() {}\n");
    src.push_str("void main() { f0(); }\n");
    let r = validate_src(&src, ShaderStage::Fragment);
    assert!(
        call_depth_and_limit(&r).is_none(),
        "got {:?}",
        call_depth_and_limit(&r)
    );
}

#[test]
fn call_chain_over_cap_triggers_diagnostic() {
    // Build main -> f0 -> f1 -> ... -> f19 (longest chain depth 21,
    // over cap 16).
    let mut src = String::new();
    for i in 0..19 {
        src.push_str(&format!("void f{i}() {{ f{}(); }}\n", i + 1));
    }
    src.push_str("void f19() {}\n");
    src.push_str("void main() { f0(); }\n");
    let r = validate_src(&src, ShaderStage::Fragment);
    let (depth, limit) = call_depth_and_limit(&r).expect("call-depth diagnostic");
    assert!(depth > limit, "{depth} should exceed {limit}");
    assert_eq!(limit, 16);
}

#[test]
fn recursion_suppresses_call_depth_diagnostic() {
    // R1 already names the cycle; R7 should not also fire (its DAG
    // analysis would loop). The check guards on cycles being absent.
    let src = r#"
void f() { f(); }
void main() { f(); }
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert!(
        r.errors
            .iter()
            .any(|d| matches!(d.kind, WebGlDiagnosticKind::Recursion { .. })),
        "expected Recursion diagnostic"
    );
    assert!(call_depth_and_limit(&r).is_none(), "no CallStackTooDeep");
}
