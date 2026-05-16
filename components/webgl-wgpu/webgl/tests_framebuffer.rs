/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::test_support::make_context;
use super::*;
use crate::{CANONICAL_TRIANGLE_FRAGMENT_SHADER, CANONICAL_TRIANGLE_VERTEX_SHADER};

#[test]
fn webgl_context_draws_into_texture_framebuffer() {
    let mut context = make_context(8, 8);
    let texture = context.create_texture();
    context.bind_texture_2d(Some(texture));
    context.tex_image_2d_rgba8(8, 8, &vec![0; 8 * 8 * 4]);
    let framebuffer = context.create_framebuffer();
    context.bind_framebuffer(Some(framebuffer));
    context.framebuffer_texture_2d(Some(texture));
    context.viewport(0, 0, 8, 8);
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
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

    assert_eq!(
        &context.read_pixels(4, 4, 1, 1).expect("fbo center")[0..4],
        &[0, 255, 0, 255]
    );
    assert_eq!(
        &context.read_pixels(0, 0, 1, 1).expect("fbo corner")[0..4],
        &[255, 0, 0, 255]
    );
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_clears_renderbuffer_framebuffer() {
    let mut context = make_context(8, 8);
    let renderbuffer = context.create_renderbuffer();
    context.bind_renderbuffer(Some(renderbuffer));
    context.renderbuffer_storage_rgba8(8, 8);
    let framebuffer = context.create_framebuffer();
    context.bind_framebuffer(Some(framebuffer));
    context.framebuffer_renderbuffer(Some(renderbuffer));
    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    });

    assert_eq!(
        &context
            .read_pixels(4, 4, 1, 1)
            .expect("renderbuffer center")[0..4],
        &[0, 0, 255, 255]
    );
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_reports_framebuffer_completeness_status() {
    let mut context = make_context(8, 8);
    assert_eq!(
        context.check_framebuffer_status(),
        WebGlFramebufferStatus::Complete
    );

    let framebuffer = context.create_framebuffer();
    context.bind_framebuffer(Some(framebuffer));
    assert_eq!(
        context.check_framebuffer_status(),
        WebGlFramebufferStatus::IncompleteMissingAttachment
    );

    let texture = context.create_texture();
    context.bind_texture_2d(Some(texture));
    context.tex_image_2d_rgba8(8, 8, &vec![0; 8 * 8 * 4]);
    context.framebuffer_texture_2d(Some(texture));
    assert_eq!(
        context.check_framebuffer_status(),
        WebGlFramebufferStatus::Complete
    );

    context.framebuffer_texture_2d(None);
    assert_eq!(
        context.check_framebuffer_status(),
        WebGlFramebufferStatus::IncompleteMissingAttachment
    );

    let renderbuffer = context.create_renderbuffer();
    context.bind_renderbuffer(Some(renderbuffer));
    context.renderbuffer_storage_rgba8(8, 8);
    context.framebuffer_renderbuffer(Some(renderbuffer));
    assert_eq!(
        context.check_framebuffer_status(),
        WebGlFramebufferStatus::Complete
    );

    context.framebuffer_renderbuffer(None);
    assert_eq!(
        context.check_framebuffer_status(),
        WebGlFramebufferStatus::IncompleteMissingAttachment
    );
    assert_eq!(context.get_error(), WebGlError::NoError);
}

#[test]
fn webgl_context_rejects_incomplete_framebuffer_operations() {
    let mut context = make_context(8, 8);
    let framebuffer = context.create_framebuffer();
    context.bind_framebuffer(Some(framebuffer));
    context.clear(wgpu::Color::BLACK);
    assert_eq!(context.get_error(), WebGlError::InvalidFramebufferOperation);

    let readback = context.read_pixels(0, 0, 1, 1).expect("empty readback");
    assert!(readback.is_empty());
    assert_eq!(context.get_error(), WebGlError::InvalidFramebufferOperation);

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
    context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
    assert_eq!(context.get_error(), WebGlError::InvalidFramebufferOperation);
}

#[test]
fn webgl_context_loss_and_restore_recreate_default_framebuffer() {
    let mut context = make_context(4, 4);
    let initial_generation = context.texture().generation;
    context.lose_context();
    context.clear(wgpu::Color::BLACK);
    assert_eq!(context.get_error(), WebGlError::ContextLostWebgl);

    context.restore_context().expect("restore");
    assert_eq!(context.texture().generation, initial_generation + 1);
    context.clear(wgpu::Color::BLACK);
    assert_eq!(context.get_error(), WebGlError::NoError);
}
