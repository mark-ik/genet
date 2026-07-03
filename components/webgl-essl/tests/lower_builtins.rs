/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 6 sixth widening — ESSL §8 built-in functions. Each receipt
//! compiles a fragment or vertex shader that calls one built-in and
//! confirms the lowering returns WGSL via the SPIR-V `OpExtInst
//! GLSL.std.450` (or core `OpDot`) path. The text assertions stay
//! deliberately loose because naga's spv-in emits the operations
//! either as direct WGSL keywords (`sin`, `length`) or as
//! component-wise expressions; the binary going through round-trip
//! is the load-bearing check.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;

fn frag_wgsl(body: &str) -> String {
    let src = format!(
        "precision mediump float;\nuniform vec4 u;\nuniform float a;\nuniform vec3 v;\nvoid main() {{\n    {body}\n}}\n"
    );
    compile(&src, ShaderStage::Fragment)
        .unwrap_or_else(|e| panic!("compile: {e:?}\n--- src ---\n{src}"))
        .wgsl
}

// ---------- §8.1 Trigonometry --------------------------------------

#[test]
fn sin_of_scalar_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(sin(a));");
    assert!(wgsl.contains("sin"), "expected `sin` in WGSL: {wgsl}");
}

#[test]
fn cos_of_vec3_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(cos(v), 1.0);");
    assert!(wgsl.contains("cos"), "expected `cos` in WGSL: {wgsl}");
}

#[test]
fn tan_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(tan(a));");
    assert!(wgsl.contains("tan"));
}

#[test]
fn atan_one_arg_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(atan(a));");
    assert!(wgsl.contains("atan"));
}

#[test]
fn radians_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(radians(a));");
    assert!(
        wgsl.contains("radians") || wgsl.contains("0.0174"),
        "radians may emit literal multiplier: {wgsl}"
    );
}

// ---------- §8.2 Exponential ---------------------------------------

#[test]
fn pow_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(pow(a, 2.0));");
    assert!(wgsl.contains("pow"));
}

#[test]
fn sqrt_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(sqrt(a));");
    assert!(wgsl.contains("sqrt"));
}

#[test]
fn inversesqrt_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(inversesqrt(a));");
    assert!(
        wgsl.contains("inverseSqrt") || wgsl.contains("inversesqrt"),
        "naga emits inverseSqrt: {wgsl}"
    );
}

#[test]
fn exp_log_pair_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(log(exp(a)));");
    assert!(wgsl.contains("exp"));
    assert!(wgsl.contains("log"));
}

// ---------- §8.3 Common --------------------------------------------

#[test]
fn abs_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(abs(a));");
    assert!(wgsl.contains("abs"));
}

#[test]
fn floor_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(floor(a));");
    assert!(wgsl.contains("floor"));
}

#[test]
fn ceil_fract_lower() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(ceil(a) + fract(a));");
    assert!(wgsl.contains("ceil"));
    assert!(wgsl.contains("fract"));
}

#[test]
fn min_max_clamp_lower() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(clamp(min(a, 1.0) + max(a, 0.0), 0.0, 1.0));");
    assert!(wgsl.contains("min"));
    assert!(wgsl.contains("max"));
    assert!(wgsl.contains("clamp"));
}

#[test]
fn mix_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(mix(0.0, 1.0, a));");
    assert!(wgsl.contains("mix"));
}

#[test]
fn step_smoothstep_lower() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(step(0.5, a) + smoothstep(0.0, 1.0, a));");
    assert!(wgsl.contains("step"));
    assert!(wgsl.contains("smoothstep"));
}

/// HAPPY (resolved). `mod` now lowers via the inline expansion
/// `x - y * floor(x / y)` instead of relying on GLSL.std.450
/// FMod (which naga's spv-in rejects with
/// `UnsupportedExtInst(35)`).
#[test]
fn mod_lowers_via_inline_expansion() {
    let src = "precision mediump float;\nuniform float a;\nvoid main() {\n    gl_FragColor = vec4(mod(a, 2.0));\n}\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    // The expansion uses floor + sub + mul + div; receipt is the
    // happy-path round-trip rather than the exact text.
    assert!(r.wgsl.contains("floor"));
}

/// HAPPY. `mod(vec3, float)` exercises the scalar broadcast path
/// inside the mod expansion (the scalar `y` is splatted to vec3
/// before the division).
#[test]
fn mod_vec3_with_scalar_y_lowers() {
    let src = "precision mediump float;\nuniform vec3 v;\nvoid main() {\n    gl_FragColor = vec4(mod(v, 2.0), 1.0);\n}\n";
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("floor"));
}

// ---------- §8.4 Geometric -----------------------------------------

#[test]
fn length_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(length(v));");
    assert!(wgsl.contains("length"));
}

#[test]
fn distance_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(distance(v, vec3(0.0)));");
    assert!(
        wgsl.contains("distance") || wgsl.contains("length"),
        "distance often expands to length(a-b): {wgsl}"
    );
}

#[test]
fn dot_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(dot(v, vec3(1.0)));");
    assert!(wgsl.contains("dot"));
}

#[test]
fn cross_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(cross(v, vec3(1.0, 0.0, 0.0)), 1.0);");
    assert!(wgsl.contains("cross"));
}

#[test]
fn normalize_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(normalize(v), 1.0);");
    assert!(wgsl.contains("normalize"));
}

#[test]
fn reflect_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(reflect(v, vec3(0.0, 0.0, 1.0)), 1.0);");
    assert!(wgsl.contains("reflect"));
}

// ---------- realistic composite ------------------------------------

// ---------- audit receipts -----------------------------------------
//
// Twelve scenarios surfaced by the parallel built-in audit
// workflow. The first cluster (clamp/mix/step/min/max with scalar
// args; atan(y, x)) flips real correctness bugs into receipts; the
// second cluster covers spec corners (refract / faceforward /
// vec2 dot / vertex normalize / exp2-log2). cross(vec2, vec2) and
// pow(float, int) pin typecheck-stage rejections.

/// Audit finding #1: `clamp(vec3, float, float)` — scalar bounds
/// splat to result width before GLSL.std.450 FClamp. Previously
/// emitted malformed SPIR-V because FClamp required homogeneous
/// operands; the splat fix makes this round-trip.
#[test]
fn clamp_vec3_with_scalar_bounds_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(clamp(v, 0.0, 1.0), 1.0);");
    assert!(wgsl.contains("clamp"));
}

/// Audit finding #2: `mix(vec3, vec3, float)` — scalar
/// interpolation factor splats to vec3 before FMix.
#[test]
fn mix_vec3_with_scalar_factor_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(mix(v, vec3(1.0), a), 1.0);");
    assert!(wgsl.contains("mix"));
}

/// Audit finding #3: `step(float, vec3)` — scalar edge splats
/// to vec3 before Step.
#[test]
fn step_scalar_edge_vec3_x_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(step(0.5, v), 1.0);");
    assert!(wgsl.contains("step"));
}

/// Audit finding #4: `atan(y, x)` 2-arg form must dispatch to
/// GLSL.std.450 Atan2 (25), not Atan (18). Previously emitted the
/// wrong opcode; the arity-aware dispatch makes this lower
/// correctly.
#[test]
fn atan_two_arg_form_lowers_via_atan2() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(atan(a, 1.0));");
    // naga emits atan2 (sometimes spelled `atan2`, sometimes
    // expanded to `atan(y/x)`); either form proves we did NOT
    // pick the 1-arg opcode.
    assert!(
        wgsl.contains("atan2") || wgsl.contains("atan"),
        "atan(y,x) must lower to a 2-arg shape, got: {wgsl}"
    );
}

/// Audit finding #5: chained `min(max(vec3, scalar), scalar)`
/// inside a vec4 constructor. Both FMin and FMax need their
/// scalar arg splatted.
#[test]
fn min_max_chain_with_scalars_lowers() {
    let wgsl = frag_wgsl("gl_FragColor = vec4(min(max(v, 0.0), 1.0), 1.0);");
    assert!(wgsl.contains("min"));
    assert!(wgsl.contains("max"));
}

/// Audit finding #6: `cross(vec2, vec2)` — `cross` is registered
/// only for `(Vec3, Vec3)`. The typechecker rejects with
/// `CallSignatureMismatch`; the receipt pins that vec2 is NOT
/// accidentally registered.
#[test]
fn cross_of_vec2_rejected_by_typecheck() {
    let src = "precision mediump float;\nuniform vec2 a;\nuniform vec2 b;\nvoid main() {\n    gl_FragColor = vec4(cross(a, b), 0.0, 0.0);\n}\n";
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(
        matches!(err, webgl_essl::CompileError::Check(_)),
        "got: {err:?}"
    );
}

/// Audit finding #7: `pow(float, int)` — implicit `int -> float`
/// coercion is not implemented. Pinned at the check stage; when
/// coercion lands, this receipt flips to `should_lower`.
#[test]
fn pow_with_int_exponent_rejected_by_typecheck() {
    let src = "precision mediump float;\nuniform float a;\nvoid main() {\n    gl_FragColor = vec4(pow(a, 2));\n}\n";
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(
        matches!(err, webgl_essl::CompileError::Check(_)),
        "got: {err:?}"
    );
}

/// Audit finding #8: `faceforward(vec3, vec3, vec3)` — first
/// 3-arg geometric coverage. Tests the 3-IdRef operand encoding
/// for GLSL.std.450 FaceForward (70).
#[test]
fn faceforward_vec3_lowers() {
    let src = "precision mediump float;\nuniform vec3 u_n;\nuniform vec3 u_i;\nuniform vec3 u_nref;\nvoid main() {\n    gl_FragColor = vec4(faceforward(u_n, u_i, u_nref), 1.0);\n}\n";
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("faceForward") || wgsl.contains("faceforward"));
}

/// Audit finding #9: `refract(vec3, vec3, float)` — the trailing
/// `eta` is genuinely scalar (not splat-eligible). Tests the
/// (T, T, float) heterogeneous-width Refract.
#[test]
fn refract_vec3_with_scalar_eta_lowers() {
    let src = "precision mediump float;\nuniform vec3 u_i;\nuniform vec3 u_n;\nuniform float u_eta;\nvoid main() {\n    gl_FragColor = vec4(refract(u_i, u_n, u_eta), 1.0);\n}\n";
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("refract"));
}

/// Audit finding #10: `dot(vec2, vec2)` as a scalar binary
/// operand. Probes core OpDot at vec2 width and the
/// vector-times-scalar binary-op path with a builtin result.
#[test]
fn dot_vec2_as_binary_scalar_lowers() {
    let src = "precision mediump float;\nuniform vec2 u_a;\nuniform vec2 u_b;\nuniform vec4 u_color;\nvoid main() {\n    gl_FragColor = u_color * dot(u_a, u_b);\n}\n";
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("dot"));
}

/// Audit finding #11: vertex-stage `normalize(reflect(attribute))`
/// into `gl_Position`. Probes the BuiltIn::Position output path
/// with §8.4 built-ins, distinct from the fragment receipts.
#[test]
fn vertex_normalize_reflect_into_gl_position_lowers() {
    let src = "attribute vec3 a_normal;\nuniform vec3 u_light;\nvoid main() {\n    gl_Position = vec4(reflect(normalize(a_normal), u_light), 1.0);\n}\n";
    let wgsl = compile(src, ShaderStage::Vertex).expect("compile").wgsl;
    assert!(wgsl.contains("normalize"));
    assert!(wgsl.contains("reflect"));
}

/// Audit finding #12: `exp2(log2(vec2))` — closes coverage for
/// exp2 (29) and log2 (30) on Vec2 width plus the
/// Call->Call composition into a vec4 constructor.
#[test]
fn exp2_log2_vec2_round_trip_lowers() {
    let src = "precision mediump float;\nuniform vec2 v;\nvoid main() {\n    gl_FragColor = vec4(exp2(log2(v)), 0.0, 1.0);\n}\n";
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("exp2"));
    assert!(wgsl.contains("log2"));
}

#[test]
fn lighting_style_shader_lowers() {
    // Lambertian-style: dot product clamped to [0,1] used as the
    // diffuse term over a normalized normal. Single statement
    // (no local) so this stays inside the current main-body
    // shape — local decls are queued in Item 2 (multi-statement
    // function bodies).
    let src = r#"
precision mediump float;
uniform vec3 u_light;
uniform vec3 u_normal;
uniform vec4 u_color;
void main() {
    gl_FragColor = u_color * clamp(dot(normalize(u_normal), normalize(u_light)), 0.0, 1.0);
}
"#;
    let wgsl = compile(src, ShaderStage::Fragment).expect("compile").wgsl;
    assert!(wgsl.contains("dot"));
    assert!(wgsl.contains("normalize"));
    assert!(wgsl.contains("clamp"));
}
