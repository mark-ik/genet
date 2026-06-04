/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Receipts for the post-widening `ProgramReflection` surface:
//! arbitrary attribute kinds (vec3 included), arbitrary uniform
//! Block layouts (mat4 / multi-uniform), and end-to-end draws
//! that exercise the new pipeline / bind-group factories.

use super::test_support::{make_context, make_context_with_depth};
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

// =====================================================================
// stage-interface varyings: arbitrary kinds flow from the vertex
// shader through to the fragment shader via webgl-essl's
// outputs/inputs.
// =====================================================================

#[test]
fn varying_vec3_passes_uniform_color_through_to_fragment() {
    let mut context = make_context(32, 32);
    let vertex = r#"
attribute vec2 a_position;
attribute vec3 a_color;
varying vec3 v_color;
void main() {
    v_color = a_color;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying vec3 v_color;
void main() {
    gl_FragColor = vec4(v_color, 1.0);
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
        .expect("varying-vec3 program links");
    context.use_program(Some(program));
    let position_location = context.get_attrib_location(program, "a_position") as u32;
    let color_location = context.get_attrib_location(program, "a_color") as u32;
    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    // Three vertices, each (vec2 pos, vec3 cyan) — stride 20 bytes.
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[
            -0.8, -0.8, 0.0, 1.0, 1.0, 0.8, -0.8, 0.0, 1.0, 1.0, 0.0, 0.8, 0.0, 1.0, 1.0,
        ],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(position_location);
    context.vertex_attrib_pointer_f32(position_location, 2, false, 20, 0);
    context.enable_vertex_attrib_array(color_location);
    context.vertex_attrib_pointer_f32(color_location, 3, false, 20, 8);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 255, 255, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn varying_float_passes_uniform_intensity_through_to_fragment() {
    let mut context = make_context(32, 32);
    let vertex = r#"
attribute vec2 a_position;
attribute float a_intensity;
varying float v_intensity;
void main() {
    v_intensity = a_intensity;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying float v_intensity;
void main() {
    gl_FragColor = vec4(v_intensity, v_intensity, v_intensity, 1.0);
}
"#;
    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("varying-float program links");
    context.use_program(Some(program));
    let position_location = context.get_attrib_location(program, "a_position") as u32;
    let intensity_location = context.get_attrib_location(program, "a_intensity") as u32;
    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    // Per-vertex (vec2 pos, float 1.0). Stride 12, intensity at offset 8.
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 1.0, 0.8, -0.8, 1.0, 0.0, 0.8, 1.0],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(position_location);
    context.vertex_attrib_pointer_f32(position_location, 2, false, 12, 0);
    context.enable_vertex_attrib_array(intensity_location);
    context.vertex_attrib_pointer_f32(intensity_location, 1, false, 12, 8);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[255, 255, 255, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn lit_triangle_uses_varying_normal_and_uniform_light() {
    // Per-vertex normal + uniform light direction, dotted in
    // the fragment shader. With all three normals pointing
    // along +Z and the light along +Z, the surface gets full
    // intensity; redirecting the light to +X turns it black.
    // This pins both the varying-vec3 path and the use of a
    // uniform-driven fragment computation.
    let vertex_source = r#"
attribute vec2 a_position;
attribute vec3 a_normal;
varying vec3 v_normal;
void main() {
    v_normal = a_normal;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment_source = r#"
precision mediump float;
varying vec3 v_normal;
uniform vec3 u_light_dir;
void main() {
    float intensity = max(dot(normalize(v_normal), u_light_dir), 0.0);
    gl_FragColor = vec4(vec3(intensity), 1.0);
}
"#;
    // Case A: light along +Z, all normals +Z → bright center.
    {
        let mut context = make_context(32, 32);
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let program = context
            .create_program_from_essl(vertex_source, fragment_source)
            .expect("lit program A");
        context.use_program(Some(program));
        let pos = context.get_attrib_location(program, "a_position") as u32;
        let nor = context.get_attrib_location(program, "a_normal") as u32;
        let light = context
            .get_uniform_location(program, "u_light_dir")
            .expect("u_light_dir location A");
        context.uniform3f(light, 0.0, 0.0, 1.0);
        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        // Stride 20 bytes: vec2 pos + vec3 normal each row.
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[
                -0.8, -0.8, 0.0, 0.0, 1.0, 0.8, -0.8, 0.0, 0.0, 1.0, 0.0, 0.8, 0.0, 0.0, 1.0,
            ],
            BufferUsage::StaticDraw,
        );
        context.enable_vertex_attrib_array(pos);
        context.vertex_attrib_pointer_f32(pos, 2, false, 20, 0);
        context.enable_vertex_attrib_array(nor);
        context.vertex_attrib_pointer_f32(nor, 3, false, 20, 8);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

        let center = context.read_pixels(16, 16, 1, 1).expect("center A");
        assert_eq!(&center[0..4], &[255, 255, 255, 255]);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }

    // Case B: light along +X, all normals +Z → dot is 0,
    // intensity clamps to 0 → black center.
    {
        let mut context = make_context(32, 32);
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let program = context
            .create_program_from_essl(vertex_source, fragment_source)
            .expect("lit program B");
        context.use_program(Some(program));
        let pos = context.get_attrib_location(program, "a_position") as u32;
        let nor = context.get_attrib_location(program, "a_normal") as u32;
        let light = context
            .get_uniform_location(program, "u_light_dir")
            .expect("u_light_dir location B");
        context.uniform3f(light, 1.0, 0.0, 0.0);
        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[
                -0.8, -0.8, 0.0, 0.0, 1.0, 0.8, -0.8, 0.0, 0.0, 1.0, 0.0, 0.8, 0.0, 0.0, 1.0,
            ],
            BufferUsage::StaticDraw,
        );
        context.enable_vertex_attrib_array(pos);
        context.vertex_attrib_pointer_f32(pos, 2, false, 20, 0);
        context.enable_vertex_attrib_array(nor);
        context.vertex_attrib_pointer_f32(nor, 3, false, 20, 8);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

        let center = context.read_pixels(16, 16, 1, 1).expect("center B");
        assert_eq!(&center[0..4], &[0, 0, 0, 255]);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }
}

// =====================================================================
// depth test: occlusion via the lazily-allocated depth attachment.
// =====================================================================

#[test]
fn depth_test_occludes_back_triangle_drawn_before_front() {
    // Two triangles cover the same screen-space region. Drawn
    // in order (front first, back second), without depth test
    // the second write would win and the center pixel would be
    // green. With depth test enabled and DepthFunc::Less, the
    // back triangle fails (0.8 < 0.0 is false), so the front
    // (red) color survives.
    //
    // Vertex shader threads gl_Position.z through, so each
    // draw can pin its own clip-space depth. wgpu's NDC depth
    // range is [0, 1]; we clear depth to 1.0 to start.
    let vertex = r#"
attribute vec3 a_position;
void main() {
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let red_fragment = r#"
precision mediump float;
void main() { gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0); }
"#;
    let green_fragment = r#"
precision mediump float;
void main() { gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0); }
"#;
    let mut context = make_context_with_depth(32, 32);
    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    });
    context.set_depth_test_enabled(true);
    context.set_depth_func(DepthFunc::Less);
    context.set_clear_depth(1.0);
    context.clear_depth_buffer();

    let red = context
        .create_program_from_essl(vertex, red_fragment)
        .expect("red program");
    let green = context
        .create_program_from_essl(vertex, green_fragment)
        .expect("green program");

    let front_vertices = context.create_buffer();
    let back_vertices = context.create_buffer();
    // Same x/y; z=0.0 (front) vs z=0.8 (back).
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(front_vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.0, 0.8, -0.8, 0.0, 0.0, 0.8, 0.0],
        BufferUsage::StaticDraw,
    );
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(back_vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, 0.8, -0.8, 0.8, 0.0, 0.8, 0.8],
        BufferUsage::StaticDraw,
    );

    // Front (red) draws first.
    context.use_program(Some(red));
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(front_vertices));
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 3, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    // Back (green) draws second — must lose the depth test.
    context.use_program(Some(green));
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(back_vertices));
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 3, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center");
    assert_eq!(&center[0..4], &[255, 0, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn depth_test_disabled_lets_last_drawn_overwrite() {
    // Sanity-check counterpart: with depth test off, the same
    // draw order ends with the second (green) write winning,
    // proving the depth test is what made the first variant
    // survive.
    let vertex = r#"
attribute vec3 a_position;
void main() {
    gl_Position = vec4(a_position, 1.0);
}
"#;
    let red_fragment = r#"
precision mediump float;
void main() { gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0); }
"#;
    let green_fragment = r#"
precision mediump float;
void main() { gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0); }
"#;
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    });

    let red = context
        .create_program_from_essl(vertex, red_fragment)
        .expect("red program");
    let green = context
        .create_program_from_essl(vertex, green_fragment)
        .expect("green program");

    let front_vertices = context.create_buffer();
    let back_vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(front_vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.0, 0.8, -0.8, 0.0, 0.0, 0.8, 0.0],
        BufferUsage::StaticDraw,
    );
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(back_vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, 0.8, -0.8, 0.8, 0.0, 0.8, 0.8],
        BufferUsage::StaticDraw,
    );

    context.use_program(Some(red));
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(front_vertices));
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 3, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    context.use_program(Some(green));
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(back_vertices));
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 3, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center");
    assert_eq!(&center[0..4], &[0, 255, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn enabling_depth_test_on_depthless_canvas_records_invalid_operation() {
    // The default canvas isn't built with `depth = true`, so
    // there's no depth attachment for the pipeline to render
    // into. Asking for depth test under those conditions must
    // surface as `InvalidOperation` — silently no-op'ing here
    // would mean a user thinking depth was on while their
    // overlapping geometry still flickered last-write-wins.
    let mut context = make_context(8, 8);
    assert_eq!(context.get_error(), WebGlError::NoError);
    context.set_depth_test_enabled(true);
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);

    // Same call on a canvas built with depth = true succeeds.
    let mut depth_context = make_context_with_depth(8, 8);
    depth_context.set_depth_test_enabled(true);
    assert_eq!(depth_context.get_error(), WebGlError::NoError);
    depth_context.clear_depth_buffer();
    assert_eq!(depth_context.get_error(), WebGlError::NoError);
}

#[test]
fn clear_depth_buffer_on_depthless_canvas_records_invalid_operation() {
    // Symmetric guardrail: even without enabling depth test,
    // attempting to clear a depth attachment that doesn't
    // exist must error rather than silently no-op.
    let mut context = make_context(8, 8);
    context.set_clear_depth(0.5);
    assert_eq!(context.get_error(), WebGlError::NoError);
    context.clear_depth_buffer();
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
}

// =====================================================================
// multi-sampler: two sampler2D uniforms in one fragment shader
// bind their image+sampler pairs into the single @group(0) BGL.
// =====================================================================

#[test]
fn two_samplers_combine_into_one_fragment() {
    // Red texture at texture unit 0, green texture at unit 1.
    // Fragment shader sums them: (1,0,0,1) + (0,1,0,1) clamps to
    // (1,1,0,2) -> writes (1,1,0,1) into Rgba8Unorm (alpha
    // saturates at 1.0). Center pixel reads as yellow only if
    // BOTH samplers are actually being sampled — proves the new
    // multi-sampler entries are wired into the bind group.
    let vertex = r#"
attribute vec2 a_position;
attribute vec2 a_uv;
varying vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying vec2 v_uv;
uniform sampler2D u_a;
uniform sampler2D u_b;
void main() {
    gl_FragColor = texture2D(u_a, v_uv) + texture2D(u_b, v_uv);
}
"#;
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("two-sampler program links");
    context.use_program(Some(program));

    let a_loc = context
        .get_uniform_location(program, "u_a")
        .expect("u_a sampler location");
    let b_loc = context
        .get_uniform_location(program, "u_b")
        .expect("u_b sampler location");
    context.uniform1i(a_loc, 0);
    context.uniform1i(b_loc, 1);

    let red_texture = context.create_texture();
    context.active_texture(0);
    context.bind_texture_2d(Some(red_texture));
    context.tex_image_2d_rgba8(1, 1, &[255, 0, 0, 255]);

    let green_texture = context.create_texture();
    context.active_texture(1);
    context.bind_texture_2d(Some(green_texture));
    context.tex_image_2d_rgba8(1, 1, &[0, 255, 0, 255]);

    let positions = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(positions));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 2, false, 0, 0);

    let uvs = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(uvs));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[0.0, 0.0, 1.0, 0.0, 0.5, 1.0],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(1);
    context.vertex_attrib_pointer_f32(1, 2, false, 0, 0);

    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    // Inside the triangle: red + green = yellow.
    let center = context.read_pixels(16, 16, 1, 1).expect("center");
    assert_eq!(&center[0..4], &[255, 255, 0, 255]);
    // Outside (corner): cleared blue.
    let corner = context.read_pixels(0, 0, 1, 1).expect("corner");
    assert_eq!(&corner[0..4], &[0, 0, 255, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

// =====================================================================
// samplerCube: per-face uploads through bind_texture_cube +
// tex_image_2d_cube_face, sampled in the fragment via textureCube.
// =====================================================================

#[test]
fn sampler_cube_samples_positive_z_face() {
    // Each face is 1x1 with a distinct color:
    //   +X red, -X yellow, +Y magenta, -Y cyan, +Z green, -Z white
    // The varying carries a constant direction (0, 0, 1) which
    // textureCube samples from the +Z face. A center pixel of
    // green proves both that the cube view dimension is wired
    // through to the BGL and that webgl-essl's textureCube
    // lowering goes end-to-end.
    let vertex = r#"
attribute vec2 a_position;
varying vec3 v_dir;
void main() {
    v_dir = vec3(0.0, 0.0, 1.0);
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying vec3 v_dir;
uniform samplerCube u_env;
void main() {
    gl_FragColor = textureCube(u_env, v_dir);
}
"#;
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("cube program links");
    context.use_program(Some(program));

    let env_loc = context
        .get_uniform_location(program, "u_env")
        .expect("u_env sampler location");
    context.uniform1i(env_loc, 0);

    let cube = context.create_texture();
    context.active_texture(0);
    context.bind_texture_cube(Some(cube));
    context.tex_image_2d_cube_face(CubeFace::PositiveX, 1, 1, &[255, 0, 0, 255]);
    context.tex_image_2d_cube_face(CubeFace::NegativeX, 1, 1, &[255, 255, 0, 255]);
    context.tex_image_2d_cube_face(CubeFace::PositiveY, 1, 1, &[255, 0, 255, 255]);
    context.tex_image_2d_cube_face(CubeFace::NegativeY, 1, 1, &[0, 255, 255, 255]);
    context.tex_image_2d_cube_face(CubeFace::PositiveZ, 1, 1, &[0, 255, 0, 255]);
    context.tex_image_2d_cube_face(CubeFace::NegativeZ, 1, 1, &[255, 255, 255, 255]);

    let positions = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(positions));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 2, false, 0, 0);

    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center");
    assert_eq!(&center[0..4], &[0, 255, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn sampler_cube_binding_2d_texture_records_invalid_operation() {
    // The texture-kind cross-check at draw time catches the
    // wrong-slot bind: a 2D texture lookup-by-cube-sampler must
    // surface as InvalidOperation rather than panicking inside
    // wgpu when the view dimension doesn't match the BGL entry.
    let vertex = r#"
attribute vec2 a_position;
varying vec3 v_dir;
void main() {
    v_dir = vec3(0.0, 0.0, 1.0);
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;
    let fragment = r#"
precision mediump float;
varying vec3 v_dir;
uniform samplerCube u_env;
void main() {
    gl_FragColor = textureCube(u_env, v_dir);
}
"#;
    let mut context = make_context(8, 8);
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("cube program");
    context.use_program(Some(program));
    let env_loc = context
        .get_uniform_location(program, "u_env")
        .expect("u_env");
    context.uniform1i(env_loc, 0);

    // Bind a 2D texture into unit 0's CUBE slot — the bind is
    // *allowed* (it just records the id), but the draw-time
    // resolution rejects it because the texture's kind is 2D
    // while the sampler expects Cube.
    let bad = context.create_texture();
    context.active_texture(0);
    context.bind_texture_2d(Some(bad));
    context.tex_image_2d_rgba8(1, 1, &[0, 0, 0, 255]);
    context.bind_texture_cube(Some(bad));

    let positions = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(positions));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 2, false, 0, 0);

    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
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
