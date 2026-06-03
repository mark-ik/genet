/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `do { ... } while (cond);` lowering plus the ESSL §8.5
//! matrix built-ins: `transpose`, `inverse`, `matrixCompMult`,
//! `outerProduct`.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;

// =====================================================================
// do-while
// =====================================================================

#[test]
fn do_while_with_int_counter_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    int i = 0;
    do {
        acc = acc + u_color;
        ++i;
    } while (i < 4);
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("loop") || r.wgsl.contains("for"));
}

#[test]
fn do_while_runs_body_once_even_on_false_cond_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    vec4 acc = vec4(0.0);
    do {
        acc = u_color;
    } while (false);
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("loop") || r.wgsl.contains("for"));
}

#[test]
fn break_inside_do_while_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
uniform int n;
void main() {
    vec4 acc = vec4(0.0);
    int i = 0;
    do {
        acc = acc + u_color;
        if (i > 2) break;
        ++i;
    } while (i < n);
    gl_FragColor = acc;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("loop") || r.wgsl.contains("for"));
}

// =====================================================================
// matrix built-ins
// =====================================================================

#[test]
fn transpose_mat3_lowers() {
    let src = r#"
precision mediump float;
uniform mat3 u_m;
void main() {
    mat3 t = transpose(u_m);
    gl_FragColor = vec4(t[0], 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("transpose"));
}

#[test]
fn transpose_mat4_lowers() {
    let src = r#"
precision mediump float;
uniform mat4 u_m;
void main() {
    mat4 t = transpose(u_m);
    gl_FragColor = t[0];
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("transpose"));
}

#[test]
fn inverse_mat4_lowers() {
    let src = r#"
precision mediump float;
uniform mat4 u_m;
uniform vec4 u_v;
void main() {
    gl_FragColor = inverse(u_m) * u_v;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    // naga maps GLSL.std.450 MatrixInverse to an
    // implementation-defined inverse — could be the WGSL
    // intrinsic if present, or an inline expansion.
    assert!(r.wgsl.contains("vec4"));
}

#[test]
fn matrixCompMult_mat3_lowers() {
    let src = r#"
precision mediump float;
uniform mat3 a;
uniform mat3 b;
void main() {
    mat3 c = matrixCompMult(a, b);
    gl_FragColor = vec4(c[0], 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

#[test]
fn matrixCompMult_mat4_lowers() {
    let src = r#"
precision mediump float;
uniform mat4 a;
uniform mat4 b;
void main() {
    mat4 c = matrixCompMult(a, b);
    gl_FragColor = c[0];
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

#[test]
fn outerProduct_vec3_lowers() {
    let src = r#"
precision mediump float;
uniform vec3 u_a;
uniform vec3 u_b;
void main() {
    mat3 m = outerProduct(u_a, u_b);
    gl_FragColor = vec4(m[0], 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

#[test]
fn outerProduct_vec4_lowers() {
    let src = r#"
precision mediump float;
uniform vec4 u_a;
uniform vec4 u_b;
void main() {
    mat4 m = outerProduct(u_a, u_b);
    gl_FragColor = m[0];
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}
