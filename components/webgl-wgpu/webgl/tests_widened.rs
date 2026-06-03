/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Receipts for the post-widening `ProgramReflection` surface:
//! arbitrary attribute kinds (vec3 included), arbitrary uniform
//! Block layouts (mat4 / multi-uniform), and end-to-end draws
//! that exercise the new pipeline / bind-group factories.

use super::test_support::make_context;
use super::*;

#[test]
fn reflection_carries_full_attribute_list_in_source_order() {
    let mut context = make_context(8, 8);
    let vertex = r#"
attribute vec3 a_position;
attribute vec3 a_normal;
attribute vec2 a_uv;
varying vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = vec4(a_position + a_normal * 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying vec2 v_uv;
void main() {
    gl_FragColor = vec4(v_uv, 0.0, 1.0);
}
"#;
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("widened-attribute program links");

    // Three attributes at locations 0, 1, 2 — vec3, vec3, vec2.
    assert_eq!(context.get_attrib_location(program, "a_position"), 0);
    assert_eq!(context.get_attrib_location(program, "a_normal"), 1);
    assert_eq!(context.get_attrib_location(program, "a_uv"), 2);
    assert_eq!(context.get_attrib_location(program, "missing"), -1);
}

#[test]
fn mat4_uniform_block_layout_reserves_64_bytes() {
    let mut context = make_context(8, 8);
    let vertex = r#"
attribute vec3 a_position;
uniform mat4 u_mvp;
void main() {
    gl_Position = u_mvp * vec4(a_position, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(1.0);
}
"#;
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("mat4 uniform program links");
    context.use_program(Some(program));

    // The Block is one mat4 → 64 bytes (aligned up to 16, no
    // change). `getUniformLocation` resolves to the BlockMember
    // slot for `u_mvp`; `uniformMatrix4fv` writes through it.
    let location = context
        .get_uniform_location(program, "u_mvp")
        .expect("u_mvp uniform location");
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];
    context.uniform_matrix4fv(location, &identity);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn multi_uniform_block_packs_mat4_then_vec3() {
    let mut context = make_context(8, 8);
    let vertex = r#"
attribute vec3 a_position;
uniform mat4 u_mvp;
void main() {
    gl_Position = u_mvp * vec4(a_position, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
uniform vec3 u_tint;
void main() {
    gl_FragColor = vec4(u_tint, 1.0);
}
"#;
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("multi-uniform program links");
    context.use_program(Some(program));

    // Two uniforms in the union list — mat4 (offset 0, size 64)
    // then vec3 (offset 64, size 12, padded to 16-byte buffer).
    let mvp_loc = context
        .get_uniform_location(program, "u_mvp")
        .expect("u_mvp location");
    let tint_loc = context
        .get_uniform_location(program, "u_tint")
        .expect("u_tint location");
    context.uniform_matrix4fv(
        mvp_loc,
        &[
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ],
    );
    context.uniform3f(tint_loc, 0.25, 0.5, 0.75);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn vec3_attribute_draws_into_render_target() {
    let mut context = make_context(32, 32);
    let vertex = r#"
attribute vec3 a_position;
void main() {
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 1.0, 1.0);
}
"#;
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("vec3-attribute program links");
    context.use_program(Some(program));
    let location = context.get_attrib_location(program, "a_position");
    assert_eq!(location, 0);

    // Three vertices of a triangle, z=0.0 in clip space.
    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.0, 0.8, -0.8, 0.0, 0.0, 0.8, 0.0],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 3, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 255, 255, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn mvp_matrix_drives_3d_triangle() {
    // The classic 3D draw: vec3 position + mat4 MVP uniform.
    // The MVP here is an identity scale that brings each vertex
    // into the visible -1..1 clip range; the fragment paints
    // green so the center pixel test confirms the triangle hit
    // the target through the new uniform path.
    let mut context = make_context(32, 32);
    let vertex = r#"
attribute vec3 a_position;
uniform mat4 u_mvp;
void main() {
    gl_Position = u_mvp * vec4(a_position, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("MVP program links");
    context.use_program(Some(program));

    let mvp_loc = context
        .get_uniform_location(program, "u_mvp")
        .expect("u_mvp location");
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];
    context.uniform_matrix4fv(mvp_loc, &identity);
    assert_eq!(context.get_error(), WebGlError::NoError);

    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.0, 0.8, -0.8, 0.0, 0.0, 0.8, 0.0],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 3, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 255, 0, 255]);
    let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
    assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn uniform_setters_reject_kind_mismatch() {
    let mut context = make_context(8, 8);
    let vertex = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
uniform vec3 u_color;
void main() {
    gl_FragColor = vec4(u_color, 1.0);
}
"#;
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("vec3-uniform program links");
    context.use_program(Some(program));
    let location = context
        .get_uniform_location(program, "u_color")
        .expect("u_color location");

    // The uniform is vec3; calling uniformMatrix4fv on it must
    // record InvalidOperation (the kind tag doesn't match).
    context.uniform_matrix4fv(
        location,
        &[
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ],
    );
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);

    // The right setter passes.
    context.uniform3f(location, 0.5, 0.25, 0.75);
    assert_eq!(context.get_error(), WebGlError::NoError);
}
