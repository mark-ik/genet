/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Step 5 fifth chunk, R13: WebGL 1 §6.4 minimum-guaranteed packing
//! limits. The validator counts one vec4 slot per scalar / vec_n
//! declaration and n slots per mat_n; samplers do not count.
//!
//! Limits:
//! - attribute (vertex):         MAX_VERTEX_ATTRIBS         = 8
//! - varying (both stages):      MAX_VARYING_VECTORS        = 8
//! - vertex uniform:             MAX_VERTEX_UNIFORM_VECTORS = 128
//! - fragment uniform:           MAX_FRAGMENT_UNIFORM_VECTORS = 16
//!
//! Note: the validator is conservative and does not implement
//! Appendix A's full packing scheme. A shader that real
//! implementations could pack into 8 varying slots via scalar /
//! vec2 mixing may still trip R13.

use webgl_essl::parse_source;
use webgl_essl::validate::{ShaderStage, WebGlDiagnosticKind, validate};

fn validate_src(src: &str, stage: ShaderStage) -> webgl_essl::validate::ValidationResult {
    let tu = parse_source(src).unwrap_or_else(|e| panic!("parse: {}", e.display(src)));
    validate(&tu, src, stage)
}

fn r13_count(r: &webgl_essl::validate::ValidationResult, class: &str) -> usize {
    r.errors
        .iter()
        .filter(|d| matches!(&d.kind, WebGlDiagnosticKind::PackingLimitExceeded { class: c, .. } if *c == class))
        .count()
}

// ---------- attribute slot limit (vertex stage) ----------------------

#[test]
fn eight_vec4_attributes_pass_r13() {
    let src = r#"
attribute vec4 a0;
attribute vec4 a1;
attribute vec4 a2;
attribute vec4 a3;
attribute vec4 a4;
attribute vec4 a5;
attribute vec4 a6;
attribute vec4 a7;
void main() {
    gl_Position = a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7;
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert_eq!(
        r13_count(&r, "attribute"),
        0,
        "8 attributes pass: {:#?}",
        r.errors
    );
}

#[test]
fn nine_vec4_attributes_emit_r13() {
    let src = r#"
attribute vec4 a0;
attribute vec4 a1;
attribute vec4 a2;
attribute vec4 a3;
attribute vec4 a4;
attribute vec4 a5;
attribute vec4 a6;
attribute vec4 a7;
attribute vec4 a8;
void main() {
    gl_Position = a0 + a1 + a2 + a3 + a4 + a5 + a6 + a7 + a8;
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert_eq!(
        r13_count(&r, "attribute"),
        1,
        "9 attributes fail: {:#?}",
        r.errors
    );
}

#[test]
fn three_mat4_attributes_emit_r13() {
    // 3 * 4 slots = 12 > 8.
    let src = r#"
attribute mat4 a0;
attribute mat4 a1;
attribute mat4 a2;
void main() {
    gl_Position = a0[0] + a1[0] + a2[0];
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert_eq!(
        r13_count(&r, "attribute"),
        1,
        "3 mat4 (12 slots) fail: {:#?}",
        r.errors
    );
}

// ---------- varying slot limit (both stages) -------------------------

#[test]
fn eight_vec4_varyings_pass_r13_in_vertex() {
    let src = r#"
attribute vec4 a;
varying vec4 v0;
varying vec4 v1;
varying vec4 v2;
varying vec4 v3;
varying vec4 v4;
varying vec4 v5;
varying vec4 v6;
varying vec4 v7;
void main() {
    v0 = a; v1 = a; v2 = a; v3 = a; v4 = a; v5 = a; v6 = a; v7 = a;
    gl_Position = a;
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert_eq!(
        r13_count(&r, "varying"),
        0,
        "8 varyings pass: {:#?}",
        r.errors
    );
}

#[test]
fn nine_vec4_varyings_emit_r13_in_fragment() {
    let src = r#"
precision mediump float;
varying vec4 v0;
varying vec4 v1;
varying vec4 v2;
varying vec4 v3;
varying vec4 v4;
varying vec4 v5;
varying vec4 v6;
varying vec4 v7;
varying vec4 v8;
void main() {
    gl_FragColor = v0 + v1 + v2 + v3 + v4 + v5 + v6 + v7 + v8;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r13_count(&r, "varying"),
        1,
        "9 varyings fail: {:#?}",
        r.errors
    );
}

// ---------- fragment uniform slot limit ------------------------------

#[test]
fn sixteen_vec4_fragment_uniforms_pass_r13() {
    let src = r#"
precision mediump float;
uniform vec4 u00;
uniform vec4 u01;
uniform vec4 u02;
uniform vec4 u03;
uniform vec4 u04;
uniform vec4 u05;
uniform vec4 u06;
uniform vec4 u07;
uniform vec4 u08;
uniform vec4 u09;
uniform vec4 u10;
uniform vec4 u11;
uniform vec4 u12;
uniform vec4 u13;
uniform vec4 u14;
uniform vec4 u15;
void main() {
    gl_FragColor = u00;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r13_count(&r, "fragment uniform"),
        0,
        "16 uniforms pass: {:#?}",
        r.errors
    );
}

#[test]
fn seventeen_vec4_fragment_uniforms_emit_r13() {
    let src = r#"
precision mediump float;
uniform vec4 u00;
uniform vec4 u01;
uniform vec4 u02;
uniform vec4 u03;
uniform vec4 u04;
uniform vec4 u05;
uniform vec4 u06;
uniform vec4 u07;
uniform vec4 u08;
uniform vec4 u09;
uniform vec4 u10;
uniform vec4 u11;
uniform vec4 u12;
uniform vec4 u13;
uniform vec4 u14;
uniform vec4 u15;
uniform vec4 u16;
void main() {
    gl_FragColor = u00 + u16;
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r13_count(&r, "fragment uniform"),
        1,
        "17 uniforms fail: {:#?}",
        r.errors
    );
}

#[test]
fn five_mat4_fragment_uniforms_emit_r13() {
    // 5 * 4 = 20 > 16.
    let src = r#"
precision mediump float;
uniform mat4 m0;
uniform mat4 m1;
uniform mat4 m2;
uniform mat4 m3;
uniform mat4 m4;
void main() {
    gl_FragColor = m0[0] + m1[0] + m2[0] + m3[0] + m4[0];
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r13_count(&r, "fragment uniform"),
        1,
        "5 mat4 (20 slots) fail: {:#?}",
        r.errors
    );
}

// ---------- vertex uniform slot limit (much higher) -----------------

#[test]
fn many_vec4_vertex_uniforms_pass_r13() {
    // Below 128 — pick 16 to keep the test small.
    let src = r#"
attribute vec4 a;
uniform vec4 u0;
uniform vec4 u1;
uniform vec4 u2;
uniform vec4 u3;
void main() {
    gl_Position = a + u0 + u1 + u2 + u3;
}
"#;
    let r = validate_src(src, ShaderStage::Vertex);
    assert_eq!(
        r13_count(&r, "vertex uniform"),
        0,
        "few uniforms pass: {:#?}",
        r.errors
    );
}

// ---------- samplers do not count -----------------------------------

#[test]
fn many_samplers_do_not_trip_uniform_limit() {
    // 17 sampler2D would naively count as 17 uniforms, exceeding
    // MAX_FRAGMENT_UNIFORM_VECTORS=16. With samplers skipped,
    // they should not contribute.
    let src = r#"
precision mediump float;
uniform sampler2D s00;
uniform sampler2D s01;
uniform sampler2D s02;
uniform sampler2D s03;
uniform sampler2D s04;
uniform sampler2D s05;
uniform sampler2D s06;
uniform sampler2D s07;
uniform sampler2D s08;
uniform sampler2D s09;
uniform sampler2D s10;
uniform sampler2D s11;
uniform sampler2D s12;
uniform sampler2D s13;
uniform sampler2D s14;
uniform sampler2D s15;
uniform sampler2D s16;
void main() {
    gl_FragColor = vec4(0.0);
}
"#;
    let r = validate_src(src, ShaderStage::Fragment);
    assert_eq!(
        r13_count(&r, "fragment uniform"),
        0,
        "samplers do not count: {:#?}",
        r.errors
    );
}
