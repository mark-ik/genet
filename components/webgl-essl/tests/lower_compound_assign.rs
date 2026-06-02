/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Compound assignment operators on function-scope locals.
//! `v += e` desugars to `v = v + e`; same for `-=`, `*=`, `/=`.
//! The target must be a local (writable); compound assigns on a
//! swizzled LHS or a write-only output are queued.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;
use webgl_essl::CompileError;

fn frag_wgsl(body: &str) -> String {
    let src = format!(
        "precision mediump float;\nuniform float a;\nuniform vec3 v;\nvoid main() {{\n    {body}\n}}\n"
    );
    compile(&src, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("compile: {e:?}\n--- src ---\n{src}"))
        .wgsl
}

// ---------- float locals -------------------------------------------

#[test]
fn float_add_assign_lowers() {
    let wgsl = frag_wgsl("float t = a; t += 0.5; gl_FragColor = vec4(t);");
    assert!(wgsl.contains("vec4"));
}

#[test]
fn float_sub_assign_lowers() {
    let wgsl = frag_wgsl("float t = a; t -= 0.5; gl_FragColor = vec4(t);");
    assert!(wgsl.contains("vec4"));
}

#[test]
fn float_mul_assign_lowers() {
    let wgsl = frag_wgsl("float t = a; t *= 2.0; gl_FragColor = vec4(t);");
    assert!(wgsl.contains("vec4"));
}

#[test]
fn float_div_assign_lowers() {
    let wgsl = frag_wgsl("float t = a; t /= 2.0; gl_FragColor = vec4(t);");
    assert!(wgsl.contains("vec4"));
}

// ---------- int locals ---------------------------------------------

#[test]
fn int_add_assign_in_for_loop_lowers() {
    // Appendix A requires the loop var to be declared in the
    // for-init, so the receipt threads `i += 1` through the
    // step slot rather than recovering `i` from outside.
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 4; i += 1) {
        acc = acc + u_color;
    }
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("for") || r.wgsl.contains("loop"));
}

// ---------- vec3 locals --------------------------------------------

#[test]
fn vec3_add_assign_lowers() {
    let wgsl = frag_wgsl("vec3 acc = v; acc += vec3(0.1); gl_FragColor = vec4(acc, 1.0);");
    assert!(wgsl.contains("vec4"));
}

#[test]
fn vec3_mul_assign_by_vec3_lowers() {
    let wgsl =
        frag_wgsl("vec3 acc = v; acc *= vec3(0.5); gl_FragColor = vec4(acc, 1.0);");
    assert!(wgsl.contains("vec4"));
}

#[test]
fn vec3_mul_assign_by_scalar_lowers() {
    let wgsl = frag_wgsl("vec3 acc = v; acc *= 2.0; gl_FragColor = vec4(acc, 1.0);");
    assert!(wgsl.contains("vec4"));
}

// ---------- rejection: compound on non-local LHS -------------------

/// HAPPY (corrected). Earlier assumption: vertex varyings are
/// write-only and `v += rhs` must error. ESSL 1.00 / 3.00
/// actually permit reading vertex outputs (reading returns the
/// current value). With the lowering's matrix-output read path
/// added (and the existing vec3 store path), the compound
/// assign now lowers.
#[test]
fn compound_assign_to_varying_lowers() {
    let src = r#"
attribute vec3 a_position;
varying vec3 v_color;
void main() {
    v_color = vec3(0.0);
    v_color += vec3(0.5);
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("location(0)"));
}

/// Compound assign on a swizzled LHS is queued — error today.
#[test]
fn compound_assign_on_swizzled_lhs_does_not_lower_today() {
    let src = r#"
precision mediump float;
uniform vec3 v;
void main() {
    vec3 acc = v;
    acc.x += 0.5;
    gl_FragColor = vec4(acc, 1.0);
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Lower(_)), "got: {err:?}");
}
