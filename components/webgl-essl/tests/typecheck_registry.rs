/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 4b second chunk: ESSL 1.00 §8 built-in function registry and
//! user-defined function call typechecking. Each test isolates one
//! piece of the Call resolution stack (constructor → registry →
//! scope) so a regression points at the right call site.

use webgl_essl::ast::*;
use webgl_essl::check::{TypeDiagnosticKind, check};
use webgl_essl::parse_source;

fn check_clean(src: &str) -> webgl_essl::check::CheckResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    let r = check(&tu);
    if !r.diagnostics.is_empty() {
        let rendered: Vec<String> =
            r.diagnostics.iter().map(|d| format!("{}", d.display(src))).collect();
        panic!("expected zero diagnostics, got: {}\n--- source ---\n{src}", rendered.join("; "));
    }
    r
}

fn count_of(r: &webgl_essl::check::CheckResult, ty: TypeKind) -> usize {
    r.types.values().filter(|&&t| t == ty).count()
}

// ---------- §8.1 trig ---------------------------------------------------

#[test]
fn sin_float_resolves_to_float() {
    let src = "void main() { float x = sin(1.0); }";
    let r = check_clean(src);
    // 1.0 (Float), sin(1.0) (Float). Decl init annotation is Float
    // both at the literal and at the call expression.
    assert!(count_of(&r, TypeKind::Float) >= 2);
}

#[test]
fn sin_vec3_resolves_to_vec3() {
    let src = r#"
uniform vec3 u_freq;
void main() {
    vec3 phase = sin(u_freq);
    gl_FragColor = vec4(phase, 1.0);
}
"#;
    let r = check_clean(src);
    assert!(count_of(&r, TypeKind::Vec3) >= 2);
}

#[test]
fn atan_two_arg_overload_resolves() {
    let src = "void main() { float a = atan(1.0, 0.5); }";
    check_clean(src);
}

// ---------- §8.2 exp ---------------------------------------------------

#[test]
fn pow_vec_vec_resolves_to_vec() {
    let src = r#"
varying vec3 v_color;
uniform float u_gamma;
void main() {
    vec3 corrected = pow(v_color, vec3(1.0 / u_gamma));
    gl_FragColor = vec4(corrected, 1.0);
}
"#;
    check_clean(src);
}

#[test]
fn sqrt_float_resolves_to_float() {
    let src = "void main() { float r = sqrt(2.0); }";
    check_clean(src);
}

// ---------- §8.3 common -----------------------------------------------

#[test]
fn mix_vec_vec_float_overload_resolves() {
    let src = r#"
uniform vec3 a;
uniform vec3 b;
uniform float t;
void main() {
    vec3 c = mix(a, b, t);
    gl_FragColor = vec4(c, 1.0);
}
"#;
    check_clean(src);
}

#[test]
fn mix_vec_vec_vec_overload_resolves() {
    let src = r#"
uniform vec3 a;
uniform vec3 b;
uniform vec3 t;
void main() {
    vec3 c = mix(a, b, t);
    gl_FragColor = vec4(c, 1.0);
}
"#;
    check_clean(src);
}

#[test]
fn clamp_vec_float_float_overload_resolves() {
    let src = "uniform vec4 c; void main() { gl_FragColor = clamp(c, 0.0, 1.0); }";
    check_clean(src);
}

#[test]
fn smoothstep_float_float_vec_overload_resolves() {
    let src = r#"
varying vec3 v_dist;
void main() {
    vec3 r = smoothstep(0.0, 1.0, v_dist);
    gl_FragColor = vec4(r, 1.0);
}
"#;
    check_clean(src);
}

// ---------- §8.4 geometric --------------------------------------------

#[test]
fn length_vec3_resolves_to_float() {
    let src = r#"
varying vec3 v_pos;
void main() {
    float d = length(v_pos);
    gl_FragColor = vec4(d, d, d, 1.0);
}
"#;
    check_clean(src);
}

#[test]
fn dot_vec3_vec3_resolves_to_float() {
    let src = r#"
varying vec3 v_normal;
uniform vec3 u_light_dir;
void main() {
    float lambert = dot(v_normal, u_light_dir);
    gl_FragColor = vec4(lambert, lambert, lambert, 1.0);
}
"#;
    check_clean(src);
}

#[test]
fn cross_vec3_vec3_resolves_to_vec3() {
    let src = r#"
uniform vec3 a;
uniform vec3 b;
void main() {
    vec3 c = cross(a, b);
    gl_FragColor = vec4(c, 1.0);
}
"#;
    check_clean(src);
}

#[test]
fn normalize_vec3_resolves_to_vec3() {
    let src = r#"
varying vec3 v_normal;
void main() {
    vec3 n = normalize(v_normal);
    gl_FragColor = vec4(n, 1.0);
}
"#;
    check_clean(src);
}

// ---------- §8.7 texture lookup ---------------------------------------

#[test]
fn texture2d_sampler_vec2_resolves_to_vec4() {
    let src = r#"
precision mediump float;
uniform sampler2D u_albedo;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_albedo, v_uv);
}
"#;
    check_clean(src);
}

#[test]
fn texture2d_with_bias_overload_resolves() {
    let src = r#"
precision mediump float;
uniform sampler2D u_albedo;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_albedo, v_uv, 0.5);
}
"#;
    check_clean(src);
}

#[test]
fn texture_cube_resolves_to_vec4() {
    let src = r#"
precision mediump float;
uniform samplerCube u_env;
varying vec3 v_dir;
void main() {
    gl_FragColor = textureCube(u_env, v_dir);
}
"#;
    check_clean(src);
}

// ---------- mismatch diagnostics --------------------------------------

#[test]
fn sin_int_emits_signature_mismatch() {
    let src = "void main() { float x = sin(1); }";
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::CallSignatureMismatch { .. }))
        .collect();
    assert_eq!(mismatches.len(), 1, "sin(int) should not match any overload");
}

#[test]
fn cross_vec2_emits_signature_mismatch() {
    // cross is vec3-only in ESSL 1.00.
    let src = r#"
uniform vec2 a;
uniform vec2 b;
void main() {
    vec2 c = cross(a, b);
}
"#;
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::CallSignatureMismatch { .. }))
        .collect();
    assert_eq!(mismatches.len(), 1);
}

// ---------- user-defined functions ------------------------------------

#[test]
fn user_function_call_resolves_with_signature() {
    let src = r#"
float square(float x) {
    return x * x;
}
void main() {
    float y = square(0.5);
    gl_FragColor = vec4(y, y, y, 1.0);
}
"#;
    let r = check_clean(src);
    // square call should annotate the call expression as Float.
    assert!(count_of(&r, TypeKind::Float) >= 3);
}

#[test]
fn forward_reference_to_function_resolves() {
    // helper is defined AFTER main, but the forward-ref pre-pass
    // registers it before the body walk.
    let src = r#"
void main() {
    float y = helper(0.5);
    gl_FragColor = vec4(y, y, y, 1.0);
}
float helper(float x) {
    return x + 1.0;
}
"#;
    check_clean(src);
}

#[test]
fn user_function_signature_mismatch_emits_diagnostic() {
    let src = r#"
float helper(float x) {
    return x * 2.0;
}
void main() {
    float y = helper(1.0, 2.0);
}
"#;
    let r = check(&parse_source(src).unwrap());
    let mismatches: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::CallSignatureMismatch { .. }))
        .collect();
    assert_eq!(mismatches.len(), 1, "user helper called with 2 args, accepts 1");
}

#[test]
fn unknown_function_emits_diagnostic() {
    let src = "void main() { float x = totally_unknown(1.0); }";
    let r = check(&parse_source(src).unwrap());
    let unknowns: Vec<_> = r
        .diagnostics
        .iter()
        .filter(|d| matches!(d.kind, TypeDiagnosticKind::UnknownFunction { .. }))
        .collect();
    assert_eq!(unknowns.len(), 1);
}

#[test]
fn unknown_function_carries_the_callee_name() {
    let src = "void main() { float x = totally_unknown(1.0); }";
    let r = check(&parse_source(src).unwrap());
    let kind = &r.diagnostics[0].kind;
    match kind {
        TypeDiagnosticKind::UnknownFunction { name } => {
            assert_eq!(name, "totally_unknown");
        },
        other => panic!("expected UnknownFunction, got {other:?}"),
    }
}

// ---------- constructor still wins over registry-shaped lookup --------

#[test]
fn vec4_constructor_resolves_via_constructor_not_registry() {
    // vec4 is not in the built-in registry; it's a constructor.
    // Make sure that path still works after the Call refactor.
    let src = "void main() { vec4 v = vec4(1.0, 0.0, 0.0, 1.0); }";
    let r = check_clean(src);
    assert!(count_of(&r, TypeKind::Vec4) >= 1);
}

#[test]
fn mat4_constructor_still_resolves() {
    let src = "void main() { mat4 m = mat4(1.0); }";
    check_clean(src);
}

// ---------- realistic lighting shader uses many built-ins ------------

#[test]
fn realistic_lighting_fragment_full_check_clean() {
    let src = r#"
precision mediump float;
uniform vec3 u_light_dir;
uniform vec3 u_light_color;
uniform vec3 u_base_color;
varying vec3 v_normal;
varying vec3 v_view_dir;
void main() {
    vec3 n = normalize(v_normal);
    vec3 l = normalize(-u_light_dir);
    vec3 v = normalize(v_view_dir);
    vec3 h = normalize(l + v);
    float diffuse = max(dot(n, l), 0.0);
    float spec = pow(max(dot(n, h), 0.0), 32.0);
    vec3 lit = u_base_color * (diffuse + spec) * u_light_color;
    gl_FragColor = vec4(lit, 1.0);
}
"#;
    check_clean(src);
}
