/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::test_support::make_context;
use super::*;
use crate::{CANONICAL_TRIANGLE_FRAGMENT_SHADER, CANONICAL_TRIANGLE_VERTEX_SHADER};

#[test]
fn webgl_context_clear_triangle_read_pixels_and_get_error() {
    let mut context = make_context(32, 32);
    assert_eq!(
        context.context_attributes(),
        WebGlContextAttributes::default()
    );
    context.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });

    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
    assert_eq!(context.get_error(), WebGlError::NoError);

    let program = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical program");
    let position_location = context.get_attrib_location(program, "a_position");
    assert_eq!(position_location, 0);
    assert_eq!(context.get_attrib_location(program, "missing"), -1);
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
    assert_eq!(context.get_error(), WebGlError::NoError);

    let center = context.read_pixels(16, 16, 1, 1).expect("center read");
    assert_eq!(&center[0..4], &[0, 255, 0, 255]);
    let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
    assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
}

#[test]
fn webgl_context_rejects_invalid_shader_pair() {
    let mut context = make_context(4, 4);
    // Reserved `gl_*` identifier — webgl-essl validator R5
    // rejects this at compile time.
    let program = context.create_program_from_essl(
        "attribute vec2 a_position; uniform vec4 gl_userColor; \
         void main() { gl_Position = vec4(a_position, 0.0, 1.0) * gl_userColor; }",
        CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    );
    assert!(program.is_none());
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
}

#[test]
fn webgl_context_get_attrib_location_reports_invalid_program() {
    let mut context = make_context(4, 4);

    assert_eq!(
        context.get_attrib_location(WebGlProgramId(99), "a_position"),
        -1
    );
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
}

#[test]
fn webgl_context_caches_translation_across_identical_source_pairs() {
    // Cache key is now source-derived. Two programs built from
    // the *same* source pair share the same translated entry;
    // differently-formatted sources (whitespace-only deltas
    // included) are treated as distinct keys. A future
    // canonicalization layer could merge them, but it's not a
    // correctness requirement — webgl-essl re-parses each
    // unique key cheaply.
    let mut context = make_context(4, 4);
    let first = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("first canonical program");
    let second = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("second canonical program");

    assert_ne!(first, second);
    assert_eq!(context.translated_programs.len(), 1);
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_explicit_compile_link_pipeline_renders() {
    let mut context = make_context(32, 32);
    let vertex = context.create_shader(ShaderStage::Vertex);
    context.shader_source(vertex, CANONICAL_TRIANGLE_VERTEX_SHADER);
    context.compile_shader(vertex);
    assert!(context.get_shader_compile_status(vertex));
    assert_eq!(context.get_shader_info_log(vertex).as_deref(), Some(""));

    let fragment = context.create_shader(ShaderStage::Fragment);
    context.shader_source(fragment, CANONICAL_TRIANGLE_FRAGMENT_SHADER);
    context.compile_shader(fragment);
    assert!(context.get_shader_compile_status(fragment));
    assert_eq!(context.get_shader_info_log(fragment).as_deref(), Some(""));

    let program = context.create_program();
    context.attach_shader(program, vertex);
    context.attach_shader(program, fragment);
    context.link_program(program);
    assert!(context.get_program_link_status(program));
    assert_eq!(context.get_program_info_log(program).as_deref(), Some(""));
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
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_compile_failure_reports_shader_info_log() {
    let mut context = make_context(4, 4);
    let fragment = context.create_shader(ShaderStage::Fragment);
    // Reserved `gl_*` identifier — R5 — surfaces at compile
    // time with a non-empty info log.
    context.shader_source(
        fragment,
        "precision mediump float; uniform vec4 gl_userColor; \
         void main() { gl_FragColor = gl_userColor; }",
    );
    context.compile_shader(fragment);
    assert!(!context.get_shader_compile_status(fragment));
    let log = context
        .get_shader_info_log(fragment)
        .expect("shader info log");
    assert!(log.contains("Fragment shader compile failed"));
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_link_failure_reports_program_info_log() {
    let vertex_source = r#"
            attribute vec2 a_position;
            attribute vec4 a_color;
            varying vec4 v_color;
            void main() {
                v_color = a_color;
                gl_Position = vec4(a_position, 0.0, 1.0);
            }
        "#;
    let fragment_source = r#"
            precision mediump float;
            varying vec4 other_color;
            void main() {
                gl_FragColor = other_color;
            }
        "#;
    let mut context = make_context(4, 4);
    let vertex = context.create_shader(ShaderStage::Vertex);
    context.shader_source(vertex, vertex_source);
    context.compile_shader(vertex);
    assert!(context.get_shader_compile_status(vertex));
    let fragment = context.create_shader(ShaderStage::Fragment);
    context.shader_source(fragment, fragment_source);
    context.compile_shader(fragment);
    assert!(context.get_shader_compile_status(fragment));

    let program = context.create_program();
    context.attach_shader(program, vertex);
    context.attach_shader(program, fragment);
    context.link_program(program);
    assert!(!context.get_program_link_status(program));
    let log = context
        .get_program_info_log(program)
        .expect("program info log");
    assert!(log.contains("interface validation"));
    context.use_program(Some(program));
    assert_eq!(context.get_error(), WebGlError::InvalidOperation);
}
