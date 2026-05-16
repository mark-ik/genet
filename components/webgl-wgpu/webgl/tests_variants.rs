/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::test_support::make_context;
use super::*;
use crate::{CANONICAL_TRIANGLE_FRAGMENT_SHADER, CANONICAL_TRIANGLE_VERTEX_SHADER};

#[test]
fn webgl_context_draws_literal_fragment_color() {
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let blue_fragment = r#"
            precision mediump float;
            void main() {
                gl_FragColor = vec4(0.0, 0.0, 1.0, 1.0);
            }
        "#;

    let program = context
        .create_program_from_essl(CANONICAL_TRIANGLE_VERTEX_SHADER, blue_fragment)
        .expect("literal color program");
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
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 0, 255, 255]);
    let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
    assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_draws_uniform_fragment_color() {
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let uniform_fragment = r#"
            precision mediump float;
            uniform vec4 u_color;
            void main() {
                gl_FragColor = u_color;
            }
        "#;

    let program = context
        .create_program_from_essl(CANONICAL_TRIANGLE_VERTEX_SHADER, uniform_fragment)
        .expect("uniform color program");
    context.use_program(Some(program));
    let location = context
        .get_uniform_location(program, "u_color")
        .expect("uniform location");
    assert!(context.get_uniform_location(program, "missing").is_none());
    context.uniform4f(location, 0.0, 0.0, 1.0, 1.0);

    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    let position_location = context.get_attrib_location(program, "a_position") as u32;
    context.enable_vertex_attrib_array(position_location);
    context.vertex_attrib_pointer_f32(position_location, 2, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 0, 255, 255]);
    let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
    assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_draws_varying_vertex_color() {
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let vertex = r#"
            attribute vec2 a_position;
            attribute vec4 a_color;
            varying vec4 v_color;
            void main() {
                v_color = a_color;
                gl_Position = vec4(a_position, 0.0, 1.0);
            }
        "#;
    let fragment = r#"
            precision mediump float;
            varying vec4 v_color;
            void main() {
                gl_FragColor = v_color;
            }
        "#;

    let program = context
        .create_program_from_essl(vertex, fragment)
        .expect("varying color program");
    let position_location = context.get_attrib_location(program, "a_position");
    let color_location = context.get_attrib_location(program, "a_color");
    assert_eq!(position_location, 0);
    assert_eq!(color_location, 1);
    context.use_program(Some(program));

    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[
            -0.8, -0.8, 0.0, 0.0, 1.0, 1.0, 0.8, -0.8, 0.0, 0.0, 1.0, 1.0, 0.0, 0.8, 0.0, 0.0, 1.0,
            1.0,
        ],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(position_location as u32);
    context.vertex_attrib_pointer_f32(position_location as u32, 2, false, 24, 0);
    context.enable_vertex_attrib_array(color_location as u32);
    context.vertex_attrib_pointer_f32(color_location as u32, 4, false, 24, 8);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 0, 255, 255]);
    let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
    assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_uniform4f_requires_current_program() {
    let mut context = make_context(4, 4);
    let uniform_fragment = r#"
            precision mediump float;
            uniform vec4 u_color;
            void main() { gl_FragColor = u_color; }
        "#;
    let program = context
        .create_program_from_essl(CANONICAL_TRIANGLE_VERTEX_SHADER, uniform_fragment)
        .expect("uniform color program");
    let location = context
        .get_uniform_location(program, "u_color")
        .expect("uniform location");

    context.uniform4f(location, 0.0, 0.0, 1.0, 1.0);

    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
}

#[test]
fn webgl_context_draws_with_renamed_position_attribute() {
    let mut context = make_context(32, 32);
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let vertex = r#"
            attribute vec2 position;
            void main() {
                gl_Position = vec4(position, 0.0, 1.0);
            }
        "#;

    let program = context
        .create_program_from_essl(vertex, CANONICAL_TRIANGLE_FRAGMENT_SHADER)
        .expect("renamed attribute program");
    let position_location = context.get_attrib_location(program, "position");
    assert_eq!(position_location, 0);
    assert_eq!(context.get_attrib_location(program, "a_position"), -1);
    context.use_program(Some(program));
    let vertices = context.create_buffer();
    context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
    context.buffer_data_f32(
        BufferTarget::ArrayBuffer,
        &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
        BufferUsage::StaticDraw,
    );
    context.enable_vertex_attrib_array(position_location as u32);
    context.vertex_attrib_pointer_f32(position_location as u32, 2, false, 0, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 255, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_draws_interleaved_vertex_stride() {
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
        &[
            -0.8, -0.8, 9.0, 9.0, 0.8, -0.8, 9.0, 9.0, 0.0, 0.8, 9.0, 9.0,
        ],
        BufferUsage::StaticDraw,
    );
    let position_location = context.get_attrib_location(program, "a_position") as u32;
    context.enable_vertex_attrib_array(position_location);
    context.vertex_attrib_pointer_f32(position_location, 2, false, 16, 0);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 255, 0, 255]);
    assert_eq!(context.get_error(), WebGlError::NoError);
    assert_eq!(
        context
            .programs
            .get(&program)
            .expect("program")
            .pipelines
            .len(),
        1
    );
}

#[test]
fn webgl_first_gate_triangle_error_resize_receipt() {
    let mut context = make_context(32, 32);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);

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
    context.clear(wgpu::Color::BLACK);
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
    assert_eq!(
        &context.read_pixels(16, 16, 1, 1).expect("center read")[0..4],
        &[0, 255, 0, 255]
    );
    context.resize(16, 16).expect("resize");
    assert_eq!(context.texture().size, (16, 16));
    assert_eq!(context.texture().generation, 1);
    assert_eq!(context.get_error(), WebGlError::NoError);
}
