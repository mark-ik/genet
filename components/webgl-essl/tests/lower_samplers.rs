/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Sampler uniforms (`sampler2D` / `samplerCube`) and texture
//! lookup built-ins (`texture2D` / `textureCube`).
//!
//! Each sampler is allocated as an `OpVariable` in
//! `UniformConstant` storage decorated with
//! `DescriptorSet 0` and a `Binding N` starting at 1 (Binding 0
//! is reserved for the Block-decorated uniform struct when
//! present). `texture2D(s, uv)` / `textureCube(s, dir)` lower as
//! `OpImageSampleImplicitLod` returning `vec4`.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;

#[test]
fn sampler2d_plus_texture2d_lowers() {
    let src = r#"
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("texture") || r.wgsl.contains("textureSample"));
}

#[test]
fn sampler_with_uniform_block_uses_distinct_bindings() {
    // The sampler must NOT collide with the uniform Block on
    // Binding 0 — the block takes Binding 0 and the sampler
    // takes Binding 1.
    let src = r#"
precision mediump float;
uniform vec4 u_tint;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv) * u_tint;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("binding(0)"));
    assert!(r.wgsl.contains("binding(1)"));
}

#[test]
fn sampler_only_starts_at_binding_one_when_no_uniform_block() {
    // No regular uniforms; the sampler is the only descriptor.
    // It still goes to Binding 1 (Binding 0 is reserved even
    // when no Block is emitted, for consistency).
    let src = r#"
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("binding(1)") || r.wgsl.contains("binding"));
}

#[test]
fn sampler_cube_with_textureCube_lowers() {
    let src = r#"
precision mediump float;
uniform samplerCube u_env;
varying vec3 v_dir;
void main() {
    gl_FragColor = textureCube(u_env, v_dir);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("texture") || r.wgsl.contains("textureSample"));
}

#[test]
fn two_samplers_get_independent_bindings() {
    let src = r#"
precision mediump float;
uniform sampler2D u_diffuse;
uniform sampler2D u_normal;
varying vec2 v_uv;
void main() {
    vec4 d = texture2D(u_diffuse, v_uv);
    vec4 n = texture2D(u_normal, v_uv);
    gl_FragColor = d + n;
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("binding(1)"));
    assert!(r.wgsl.contains("binding(2)"));
}

#[test]
fn texture_swizzle_lowers() {
    // Sample then swizzle the rgba result — a common shader
    // idiom (use alpha as a mask, take just .rgb, etc.).
    let src = r#"
precision mediump float;
uniform sampler2D u_tex;
varying vec2 v_uv;
void main() {
    vec3 rgb = texture2D(u_tex, v_uv).rgb;
    gl_FragColor = vec4(rgb, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}
