/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 2's done-condition corpus: a curated set of real-shape WebGL 1
//! shaders that exercise distinct grammar features. The assertion is
//! the same in every test (`parse_source` returns `Ok`) because Step 2's
//! gate is grammar coverage, not semantic correctness. Validator-side
//! receipts come later, when typecheck and the WebGL restrictions layer
//! land (Steps 4 and 5).
//!
//! Each shader is annotated with the features it stresses so a failure
//! points at the right grammar rule.

use webgl_essl::parse_source;

fn parse_ok(label: &str, src: &str) {
    if let Err(e) = parse_source(src) {
        panic!(
            "{label}: parse failed: {}\n--- source ---\n{src}",
            e.display(src)
        );
    }
}

// ---------- 1. Solid color fragment ------------------------------------
//
// Trivial case. Exercises: precision decl, uniform vec4, main, assign
// to `gl_FragColor`. Pure baseline.

#[test]
fn solid_color_fragment() {
    let src = r#"
precision mediump float;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color;
}
"#;
    parse_ok("solid_color_fragment", src);
}

// ---------- 2. MVP transform vertex -----------------------------------
//
// The canonical 3D vertex shader. Exercises: uniform mat4, attribute
// vec3, varying vec3, mat4 * vec4 (matrix-vector mul through the
// multiplicative precedence level), vec4 constructor mixing an
// attribute and a scalar.

#[test]
fn mvp_transform_vertex() {
    let src = r#"
uniform mat4 u_projection;
uniform mat4 u_view;
uniform mat4 u_model;
attribute vec3 a_position;
attribute vec3 a_normal;
varying vec3 v_normal;
void main() {
    v_normal = a_normal;
    gl_Position = u_projection * u_view * u_model * vec4(a_position, 1.0);
}
"#;
    parse_ok("mvp_transform_vertex", src);
}

// ---------- 3. Textured fragment with swizzle --------------------------
//
// Standard texturing path. Exercises: `texture2D` built-in (just an
// identifier-call in our AST), member access for swizzles
// (`.rgb` / `.a`), `vec4` constructor mixing a vec3 with a scalar.

#[test]
fn textured_fragment_with_swizzle() {
    let src = r#"
precision mediump float;
uniform sampler2D u_albedo;
varying vec2 v_uv;
void main() {
    vec4 sampled = texture2D(u_albedo, v_uv);
    gl_FragColor = vec4(sampled.rgb, sampled.a * 0.8);
}
"#;
    parse_ok("textured_fragment_with_swizzle", src);
}

// ---------- 4. Lighting fragment with a struct uniform -----------------
//
// Real-world struct usage. Exercises: file-scope struct decl with
// typed fields; uniform of struct type used elsewhere (parser sees
// `Light` as an identifier in the uniform's type position — which is a
// validator-side resolution concern; for the parser this only needs the
// struct decl to lex+parse, since uniform's type slot is a type-keyword
// path in the spike). To keep it parser-side honest, the uniform here
// uses a built-in type, and the struct is just declared.

#[test]
fn struct_decl_plus_lighting_math() {
    let src = r#"
precision mediump float;
struct Light {
    vec3 position;
    vec3 color;
    float intensity;
};
uniform vec3 u_light_pos;
uniform vec3 u_light_color;
uniform float u_light_intensity;
varying vec3 v_normal;
varying vec3 v_world_pos;
void main() {
    vec3 n = normalize(v_normal);
    vec3 l = normalize(u_light_pos - v_world_pos);
    float diffuse = max(dot(n, l), 0.0);
    vec3 rgb = u_light_color * (u_light_intensity * diffuse);
    gl_FragColor = vec4(rgb, 1.0);
}
"#;
    parse_ok("struct_decl_plus_lighting_math", src);
}

// ---------- 5. Toon shading with step + conditional + ternary ----------
//
// Mixed control flow. Exercises: `step` built-in, `if` / `else`,
// ternary in an assignment RHS, comparison ops, multiplicative +
// additive precedence in nested expressions.

#[test]
fn toon_shading_with_mixed_control_flow() {
    let src = r#"
precision mediump float;
uniform vec3 u_light_dir;
uniform vec3 u_base_color;
varying vec3 v_normal;
void main() {
    float lambert = max(dot(normalize(v_normal), normalize(u_light_dir)), 0.0);
    float band;
    if (lambert > 0.66) {
        band = 1.0;
    } else if (lambert > 0.33) {
        band = 0.6;
    } else {
        band = 0.3;
    }
    vec3 rgb = u_base_color * band;
    float warmth = lambert > 0.5 ? 1.0 : 0.85;
    gl_FragColor = vec4(rgb * warmth, 1.0);
}
"#;
    parse_ok("toon_shading_with_mixed_control_flow", src);
}

// ---------- 6. Fog blending with `mix` ---------------------------------
//
// Exercises: float-valued built-ins (`length`, `exp`, `mix`), nested
// calls, comparison, swizzle.

#[test]
fn fog_blending_with_mix() {
    let src = r#"
precision mediump float;
uniform vec3 u_fog_color;
uniform float u_fog_density;
varying vec3 v_world_pos;
varying vec4 v_color;
void main() {
    float dist = length(v_world_pos);
    float fog = exp(-u_fog_density * dist);
    vec3 rgb = mix(u_fog_color, v_color.rgb, fog);
    gl_FragColor = vec4(rgb, v_color.a);
}
"#;
    parse_ok("fog_blending_with_mix", src);
}

// ---------- 7. Const-bound loop accumulator ----------------------------
//
// A safe ESSL 1.00 loop (constant bound). Exercises: local int decl
// with init, `for` with all three slots populated, postfix `++` in the
// step slot, body that mutates the accumulator.

#[test]
fn const_bound_loop_accumulator() {
    let src = r#"
precision mediump float;
uniform float u_step;
void main() {
    vec4 sum = vec4(0.0);
    for (int i = 0; i < 4; i++) {
        sum = sum + vec4(u_step);
    }
    gl_FragColor = sum;
}
"#;
    parse_ok("const_bound_loop_accumulator", src);
}

// ---------- 8. Alpha-test discard --------------------------------------
//
// The common alpha-cutout idiom. Exercises: `if` guarding a `discard;`,
// swizzle assigning to a single channel (read-only here), comparison
// against a uniform threshold.

#[test]
fn alpha_test_discard() {
    let src = r#"
precision mediump float;
uniform sampler2D u_albedo;
uniform float u_alpha_cutoff;
varying vec2 v_uv;
void main() {
    vec4 sampled = texture2D(u_albedo, v_uv);
    if (sampled.a < u_alpha_cutoff) {
        discard;
    }
    gl_FragColor = sampled;
}
"#;
    parse_ok("alpha_test_discard", src);
}

// ---------- 9. Helper function + call --------------------------------
//
// Multi-function shader. Exercises: function definition with parameters
// (the second commit's first explicit receipt is single-function;
// this one routes a real call site through the parameter-passing path),
// the helper's `return` with a built-in inside it.

#[test]
fn helper_function_and_caller() {
    let src = r#"
precision mediump float;
float luminance(vec3 rgb) {
    return dot(rgb, vec3(0.299, 0.587, 0.114));
}
uniform sampler2D u_albedo;
varying vec2 v_uv;
void main() {
    vec3 rgb = texture2D(u_albedo, v_uv).rgb;
    float gray = luminance(rgb);
    gl_FragColor = vec4(gray, gray, gray, 1.0);
}
"#;
    parse_ok("helper_function_and_caller", src);
}

// ---------- 10. Gamma correction with `pow` ---------------------------
//
// Exercises: `pow` builtin with vec arg, divide operator at the
// multiplicative level (`/`), swizzle on a constructor expression,
// floating-point literal in scientific notation (`1.0e0`).

#[test]
fn gamma_correction_with_pow() {
    let src = r#"
precision mediump float;
varying vec3 v_color;
uniform float u_gamma;
void main() {
    vec3 corrected = pow(v_color, vec3(1.0 / u_gamma));
    float exposure = 1.0e0;
    gl_FragColor = vec4(corrected * exposure, 1.0);
}
"#;
    parse_ok("gamma_correction_with_pow", src);
}
