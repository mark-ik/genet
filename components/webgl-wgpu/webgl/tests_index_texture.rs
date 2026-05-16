/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::test_support::make_context;
use super::*;
use crate::{CANONICAL_TRIANGLE_FRAGMENT_SHADER, CANONICAL_TRIANGLE_VERTEX_SHADER};

#[test]
fn webgl_context_draws_indexed_triangle_elements() {
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical program");
    context.use_program(Some(program));

    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 2, false, 0, 0);

    let indices = context.create_buffer();
    context.bind_buffer(BufferTarget::ElementArrayBuffer, Some(indices));
    context.buffer_data_u16(
        BufferTarget::ElementArrayBuffer,
        &[0, 1, 2],
        BufferUsage::StaticDraw,
    );
    context.draw_elements(PrimitiveMode::Triangles, 3, IndexType::UnsignedShort, 0);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 255, 0, 255]);
    let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
    assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_draw_elements_rejects_out_of_range_indices() {
    let mut context = make_context(8, 8);
    let program = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical program");
    context.use_program(Some(program));

    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 2, false, 0, 0);

    let indices = context.create_buffer();
    context.bind_buffer(BufferTarget::ElementArrayBuffer, Some(indices));
    context.buffer_data_u16(
        BufferTarget::ElementArrayBuffer,
        &[0, 1, 9],
        BufferUsage::StaticDraw,
    );
    context.draw_elements(PrimitiveMode::Triangles, 3, IndexType::UnsignedShort, 0);
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
}

#[test]
fn webgl_context_viewport_and_scissor_clip_draws() {
    let mut context = make_context(32, 32);
    let program = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical program");
    context.use_program(Some(program));
    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(0);
    context.vertex_attrib_pointer_f32(0, 2, false, 0, 0);

    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    context.viewport(0, 0, 16, 32);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
    assert_eq!(
        &context.read_pixels(8, 16, 1, 1).expect("viewport hit")[0..4],
        &[0, 255, 0, 255]
    );
    assert_eq!(
        &context.read_pixels(24, 16, 1, 1).expect("viewport miss")[0..4],
        &[255, 0, 0, 255]
    );

    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    context.viewport(0, 0, 32, 32);
    context.scissor(0, 0, 20, 32);
    context.set_scissor_test_enabled(true);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
    assert_eq!(
        &context.read_pixels(16, 16, 1, 1).expect("scissor hit")[0..4],
        &[0, 255, 0, 255]
    );
    assert_eq!(
        &context.read_pixels(24, 16, 1, 1).expect("scissor miss")[0..4],
        &[255, 0, 0, 255]
    );
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_draws_texture_sampler_fragment() {
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
            uniform sampler2D u_texture;
            void main() {
                gl_FragColor = texture2D(u_texture, v_uv);
            }
        "#;
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("texture sampler program");
    context.use_program(Some(program));
    let sampler_location = context
        .get_uniform_location(program, "u_texture")
        .expect("sampler location");
    context.uniform1i(sampler_location, 0);

    let texture = context.create_texture();
    context.bind_texture_2d(Some(texture));
    context.tex_image_2d_rgba8(1, 1, &[0, 0, 255, 255]);

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
    assert_eq!(
        &context.read_pixels(16, 16, 1, 1).expect("center read")[0..4],
        &[0, 0, 255, 255]
    );
    assert_eq!(
        &context.read_pixels(0, 0, 1, 1).expect("corner read")[0..4],
        &[255, 0, 0, 255]
    );
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_sampler_uniform_selects_texture_unit() {
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
            uniform sampler2D u_texture;
            void main() {
                gl_FragColor = texture2D(u_texture, v_uv);
            }
        "#;
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("texture sampler program");
    context.use_program(Some(program));
    let sampler_location = context
        .get_uniform_location(program, "u_texture")
        .expect("sampler location");
    context.uniform1i(sampler_location, 1);

    let unit_zero_texture = context.create_texture();
    context.active_texture(0);
    context.bind_texture_2d(Some(unit_zero_texture));
    context.tex_image_2d_rgba8(1, 1, &[0, 255, 0, 255]);

    let unit_one_texture = context.create_texture();
    context.active_texture(1);
    context.bind_texture_2d(Some(unit_one_texture));
    context.tex_image_2d_rgba8(1, 1, &[0, 0, 255, 255]);

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
    assert_eq!(
        &context.read_pixels(16, 16, 1, 1).expect("center read")[0..4],
        &[0, 0, 255, 255]
    );
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_rejects_invalid_texture_unit_state() {
    let mut context = make_context(4, 4);
    context.active_texture(MAX_TEXTURE_IMAGE_UNITS as u32);
    assert_eq!(context.get_error(), WebGlError::InvalidEnum);

    let program = context
        .create_program_from_essl(
            r#"
                    attribute vec2 a_position;
                    attribute vec2 a_uv;
                    varying vec2 v_uv;
                    void main() {
                        v_uv = a_uv;
                        gl_Position = vec4(a_position, 0.0, 1.0);
                    }
                "#,
            r#"
                    precision mediump float;
                    varying vec2 v_uv;
                    uniform sampler2D u_texture;
                    void main() {
                        gl_FragColor = texture2D(u_texture, v_uv);
                    }
                "#,
        )
        .expect("texture sampler program");
    context.use_program(Some(program));
    let sampler_location = context
        .get_uniform_location(program, "u_texture")
        .expect("sampler location");
    context.uniform1i(sampler_location, MAX_TEXTURE_IMAGE_UNITS as i32);
    assert_eq!(context.get_error(), WebGlError::InvalidValue);
}

#[test]
fn webgl_context_rejects_texture_sampler_draw_without_texture() {
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
            uniform sampler2D u_texture;
            void main() {
                gl_FragColor = texture2D(u_texture, v_uv);
            }
        "#;
    let mut context = make_context(8, 8);
    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("texture sampler program");
    context.use_program(Some(program));
    let sampler_location = context
        .get_uniform_location(program, "u_texture")
        .expect("sampler location");
    context.uniform1i(sampler_location, 0);

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
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
}
