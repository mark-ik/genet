/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! User-defined struct types: declaration at file scope, locals
//! of struct type, and `.field` read/write access.
//!
//! Not in this first cut (each pinned separately or queued):
//!   - struct constructors (`Foo(...)`) — call the constructor
//!     form with positional args. Workaround: declare uninit and
//!     assign fields one by one.
//!   - struct globals / uniforms of struct type. Workaround:
//!     individual uniform fields.
//!   - nested struct member access (`s.inner.x`).
//!   - struct equality (`s == t`).
//!   - struct const initializers.

use webgl_essl::compile;
use webgl_essl::validate::ShaderStage;
use webgl_essl::CompileError;

// ---------- declaration + field-by-field initialization -------------

#[test]
fn struct_decl_and_field_assign_then_read_lowers() {
    let src = r#"
precision mediump float;
struct Foo {
    float x;
    vec3 y;
};
void main() {
    Foo s;
    s.x = 1.0;
    s.y = vec3(0.25, 0.5, 0.75);
    gl_FragColor = vec4(s.y, s.x);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    // Naga's WGSL emitter typically lowers OpTypeStruct as a
    // named struct.
    assert!(r.wgsl.contains("vec4") || r.wgsl.contains("struct"));
}

#[test]
fn two_structs_with_distinct_indices_lower() {
    let src = r#"
precision mediump float;
struct A { float x; };
struct B { vec3 y; };
void main() {
    A a;
    B b;
    a.x = 1.0;
    b.y = vec3(0.5);
    gl_FragColor = vec4(b.y, a.x);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

// ---------- typecheck errors ----------------------------------------

#[test]
fn unknown_struct_field_is_a_typecheck_error() {
    let src = r#"
precision mediump float;
struct Foo { float x; };
void main() {
    Foo s;
    s.y = 1.0;
    gl_FragColor = vec4(s.x);
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        matches!(err, CompileError::Check(_)) && msg.contains("UnknownStructField"),
        "expected UnknownStructField: {msg}"
    );
}

#[test]
fn assigning_wrong_type_to_struct_field_is_a_typecheck_error() {
    let src = r#"
precision mediump float;
struct Foo { float x; };
void main() {
    Foo s;
    s.x = vec3(0.0);
    gl_FragColor = vec4(s.x);
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Check(_)), "got: {err:?}");
}

#[test]
fn undeclared_struct_type_at_local_decl_fails_at_parse() {
    // `Bar bar;` where `Bar` was never declared — parser doesn't
    // know it as a type, so parsing fails with "expected type".
    let src = r#"
precision mediump float;
void main() {
    Bar bar;
    gl_FragColor = vec4(0.0);
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    assert!(matches!(err, CompileError::Parse(_)), "got: {err:?}");
}

// ---------- struct field used in arithmetic --------------------------

#[test]
fn struct_field_used_in_binary_op_lowers() {
    let src = r#"
precision mediump float;
struct Material {
    vec3 albedo;
    float roughness;
};
uniform float u_light;
void main() {
    Material mat;
    mat.albedo = vec3(0.5);
    mat.roughness = 0.75;
    vec3 c = mat.albedo * u_light * mat.roughness;
    gl_FragColor = vec4(c, 1.0);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

// ---------- queued-as-error receipts --------------------------------

/// HAPPY (resolved). `Foo(args)` constructs a struct value by
/// `OpCompositeConstruct` over the lowered arg ids. The
/// typecheck verifies the args match the declared field types
/// in order; the lowering emits the composite construct.
#[test]
fn struct_constructor_call_lowers() {
    let src = r#"
precision mediump float;
struct Foo { float x; vec3 y; };
void main() {
    Foo s = Foo(1.0, vec3(0.25, 0.5, 0.75));
    gl_FragColor = vec4(s.y, s.x);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// HAPPY. Struct constructor as an init for a local, then
/// read via member access (the canonical use shape).
#[test]
fn struct_constructor_with_uniform_args_lowers() {
    let src = r#"
precision mediump float;
struct Material { vec3 albedo; float roughness; };
uniform vec3 u_color;
uniform float u_r;
void main() {
    Material mat = Material(u_color, u_r);
    gl_FragColor = vec4(mat.albedo, mat.roughness);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// SPEC-CONFORMANCE. Wrong arg count or types on the struct
/// constructor surface as `CallSignatureMismatch` (reusing the
/// existing diagnostic shape with a synthetic single candidate).
#[test]
fn struct_constructor_with_wrong_args_is_a_typecheck_error() {
    let src = r#"
precision mediump float;
struct Foo { float x; vec3 y; };
void main() {
    Foo s = Foo(1.0);
    gl_FragColor = vec4(s.x);
}
"#;
    let err = compile(src, ShaderStage::Fragment).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        matches!(err, CompileError::Check(_)) && msg.contains("CallSignatureMismatch"),
        "expected CallSignatureMismatch: {msg}"
    );
}

/// HAPPY (resolved). Nested struct member access `s.inner.x`
/// now lowers: `build_struct_access_chain` walks the
/// member-of-member chain inward to find the root Ident, then
/// emits a single `OpAccessChain` with the collected member
/// indices. Same path covers both read and write.
#[test]
fn nested_struct_member_access_lowers() {
    let src = r#"
precision mediump float;
struct Inner { float x; };
struct Outer { Inner inner; };
void main() {
    Outer o;
    o.inner.x = 1.0;
    gl_FragColor = vec4(o.inner.x);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}

/// HAPPY. Three-level nesting exercises the loop in
/// `build_struct_access_chain` past the inner-base sanity
/// check. `s.mid.inner.v` produces three indices.
#[test]
fn three_level_nested_struct_member_access_lowers() {
    let src = r#"
precision mediump float;
struct Leaf { float v; };
struct Mid { Leaf inner; };
struct Outer { Mid mid; };
void main() {
    Outer o;
    o.mid.inner.v = 0.42;
    gl_FragColor = vec4(o.mid.inner.v);
}
"#;
    let r = compile(src, ShaderStage::Fragment).expect("compile");
    assert!(r.wgsl.contains("vec4"));
}
