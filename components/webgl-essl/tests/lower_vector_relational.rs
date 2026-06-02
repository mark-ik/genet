/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! ESSL §8.6 vector relational built-ins. Each receipt
//! compiles a fragment shader exercising one of the §8.6
//! family and confirms the round-trip through naga to WGSL.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;

fn frag_wgsl(body: &str) -> String {
    let src = format!(
        "precision mediump float;\nuniform vec3 u_a;\nuniform vec3 u_b;\nvoid main() {{\n    {body}\n}}\n"
    );
    compile(&src, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("compile: {e:?}\n--- src ---\n{src}"))
        .wgsl
}

// ---------- comparisons return bvec ---------------------------------

#[test]
fn less_than_vec3_lowers() {
    let wgsl = frag_wgsl(
        "if (lessThan(u_a, u_b).x) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

#[test]
fn less_than_equal_vec3_lowers() {
    let wgsl = frag_wgsl(
        "if (lessThanEqual(u_a, u_b).y) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

#[test]
fn greater_than_vec3_lowers() {
    let wgsl = frag_wgsl(
        "if (greaterThan(u_a, u_b).z) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

#[test]
fn greater_than_equal_vec3_lowers() {
    let wgsl = frag_wgsl(
        "if (greaterThanEqual(u_a, u_b).x) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

#[test]
fn equal_vec3_lowers() {
    let wgsl = frag_wgsl(
        "if (equal(u_a, u_b).x) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

#[test]
fn not_equal_vec3_lowers() {
    let wgsl = frag_wgsl(
        "if (notEqual(u_a, u_b).x) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

// ---------- reductions: any / all reduce bvec to bool ---------------

#[test]
fn any_of_less_than_lowers() {
    let wgsl = frag_wgsl(
        "if (any(lessThan(u_a, u_b))) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

#[test]
fn all_of_less_than_lowers() {
    let wgsl = frag_wgsl(
        "if (all(lessThan(u_a, u_b))) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

// ---------- component-wise negation ---------------------------------

#[test]
fn not_of_less_than_lowers() {
    let wgsl = frag_wgsl(
        "if (not(lessThan(u_a, u_b)).x) gl_FragColor = vec4(1.0); else gl_FragColor = vec4(0.0);",
    );
    assert!(wgsl.contains("if"));
}

// ---------- vec2 width ------------------------------------------------

#[test]
fn less_than_vec2_lowers() {
    let src = r#"
precision mediump float;
uniform vec2 a;
uniform vec2 b;
void main() {
    if (any(lessThan(a, b))) {
        gl_FragColor = vec4(1.0);
    } else {
        gl_FragColor = vec4(0.0);
    }
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("if"));
}

// ---------- vec4 width ------------------------------------------------

#[test]
fn less_than_vec4_with_all_reduction_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 a;
uniform vec4 b;
void main() {
    if (all(lessThan(a, b))) {
        gl_FragColor = vec4(1.0);
    } else {
        gl_FragColor = vec4(0.0);
    }
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("if"));
}
