/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! ESSL 3.00 (`#version 300 es`) shaders declare their own
//! fragment outputs with `out vec4 ...;` rather than writing to
//! the implicit `gl_FragColor` builtin. The lowering registers
//! each user `out` as an Output variable at sequential
//! `@location(N)`, and skips allocating the `gl_FragColor`
//! primary when the shader has user outputs so the two don't
//! collide on Location 0.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;

#[test]
fn essl300_fragment_with_user_out_lowers() {
    let src = r#"#version 300 es
precision mediump float;
out vec4 out_color;
void main() {
    out_color = vec4(1.0, 0.5, 0.25, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("@location(0)"), "wgsl: {}", r.wgsl);
}

#[test]
fn essl300_fragment_with_two_user_outs_at_separate_locations() {
    let src = r#"#version 300 es
precision mediump float;
out vec4 color0;
out vec4 color1;
void main() {
    color0 = vec4(1.0, 0.0, 0.0, 1.0);
    color1 = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("@location(0)"));
    assert!(r.wgsl.contains("@location(1)"));
}

#[test]
fn essl300_fragment_with_in_varying_and_out_color_lowers() {
    let src = r#"#version 300 es
precision mediump float;
in vec3 v_color;
out vec4 out_color;
void main() {
    out_color = vec4(v_color, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("@location(0)"), "wgsl: {}", r.wgsl);
}

#[test]
fn essl300_vertex_with_in_and_out_lowers() {
    // The Vertex stage already accepted ESSL 3.00 `in` decls as
    // inputs and `out` decls as outputs; this pin confirms the
    // round-trip stays clean alongside the Fragment changes.
    let src = r#"#version 300 es
in vec3 a_position;
in vec3 a_color;
out vec3 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Vertex).expect("compile");
    assert!(r.wgsl.contains("@location(0)"));
    assert!(r.wgsl.contains("@location(1)"));
    assert!(r.wgsl.contains("@builtin(position)"));
}

#[test]
fn essl100_fragment_with_gl_fragcolor_still_lowers() {
    // ESSL 1.00 shaders that write to gl_FragColor must keep
    // working — the refactor only changes ESSL 3.00 behaviour.
    let src = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.5, 0.25, 0.0, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("@location(0)"));
}

#[test]
fn essl300_fragment_uniform_then_out_color() {
    let src = r#"#version 300 es
precision mediump float;
uniform vec4 u_tint;
in vec3 v_color;
out vec4 out_color;
void main() {
    out_color = vec4(v_color, 1.0) * u_tint;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("@location(0)"));
    assert!(r.wgsl.contains("@group(0)"));
    assert!(r.wgsl.contains("@binding(0)"));
}
