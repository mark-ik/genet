/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! W4 WebGL-over-wgpu bridge receipt.
//!
//! Drives a synthetic WebGL canvas texture from `servo-webgl-wgpu`
//! through the Serval paint path:
//!
//! `WebGlContext` -> painter external texture registry ->
//! `PaintCmd::DrawExternalTexture` (carried inside a `PaintEnvelope`) ->
//! `Paint::render` ->
//! `Renderer::render_with_compositor_and_external_textures` ->
//! `Paint::composite_texture`.

use dpi::PhysicalSize;
use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use netrender::{NetrenderOptions, boot, create_netrender_instance};
use paint::Paint;
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_api::wgpu_readback::read_texture_to_image;
use paint_list_api::{
    ColorF, CommonPlacement, DeviceIntSize, EngineId, ExternalTextureItem, LayoutPoint, LayoutRect,
    PaintCmd, PaintEnvelope, PrimitiveFlags, RectItem,
};
use paint_types::PipelineId;
use paint_types::units::{DeviceIntRect, LayoutSize};
use servo_base::id::{PainterId, PipelineNamespace, PipelineNamespaceId, WebViewId};
use webgl_wgpu::{
    BufferTarget, BufferUsage, CANONICAL_TRIANGLE_FRAGMENT_SHADER,
    CANONICAL_TRIANGLE_VERTEX_SHADER, PrimitiveMode, WebGlCanvasDescriptor, WebGlContext,
    WebGlError,
};

const VIEWPORT: u32 = 64;
const WEBGL_TEXTURE_KEY: u64 = 7;

fn ensure_pipeline_namespace() {
    PipelineNamespace::install(PipelineNamespaceId(1));
}

fn paint_info_for(pipeline_id: PipelineId) -> PaintDisplayListInfo {
    PaintDisplayListInfo::new(
        ViewportDetails {
            size: Size2D::new(VIEWPORT as f32, VIEWPORT as f32),
            hidpi_scale_factor: Scale::new(1.0),
        },
        LayoutSize::new(VIEWPORT as f32, VIEWPORT as f32),
        pipeline_id,
        servo_base::Epoch(0),
        AxesScrollSensitivity {
            x: ScrollType::InputEvents | ScrollType::Script,
            y: ScrollType::InputEvents | ScrollType::Script,
        },
        true,
    )
}

fn placement_at(bounds: LayoutRect) -> CommonPlacement {
    CommonPlacement {
        bounds,
        flags: PrimitiveFlags::empty(),
    }
}

fn envelope_with_webgl_canvas() -> PaintEnvelope {
    let full = LayoutRect::new(
        LayoutPoint::new(0.0, 0.0),
        LayoutPoint::new(VIEWPORT as f32, VIEWPORT as f32),
    );
    let canvas = LayoutRect::new(LayoutPoint::new(16.0, 16.0), LayoutPoint::new(48.0, 48.0));
    let overlay = LayoutRect::new(LayoutPoint::new(28.0, 28.0), LayoutPoint::new(36.0, 36.0));

    PaintEnvelope {
        engine: EngineId::SERVAL,
        viewport: DeviceIntSize::new(VIEWPORT as i32, VIEWPORT as i32),
        generation: 0,
        commands: vec![
            PaintCmd::DrawRect(RectItem {
                placement: placement_at(full),
                color: ColorF {
                    r: 1.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
            }),
            PaintCmd::DrawExternalTexture(ExternalTextureItem {
                placement: placement_at(canvas),
                texture_key: WEBGL_TEXTURE_KEY,
                opacity: 1.0,
                content_generation: None,
            }),
            PaintCmd::DrawRect(RectItem {
                placement: placement_at(overlay),
                color: ColorF {
                    r: 0.0,
                    g: 0.0,
                    b: 1.0,
                    a: 1.0,
                },
            }),
        ],
        fonts: Vec::new(),
        images: Vec::new(),
    }
}

fn draw_webgl_triangle(device: wgpu::Device, queue: wgpu::Queue) -> WebGlContext {
    let mut context =
        WebGlContext::from_wgpu_handles(device, queue, WebGlCanvasDescriptor::new(32, 32))
            .expect("WebGL context");

    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    });

    let program = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical WebGL program");
    context.use_program(Some(program));
    let position_location = context.get_attrib_location(program, "a_position");
    assert_eq!(position_location, 0);

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
    context
}

#[test]
fn webgl_canvas_texture_composes_through_paint_render_path() {
    let handles = boot().expect("wgpu boot");
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size: Some(32),
            enable_vello: true,
            ..Default::default()
        },
    )
    .expect("create_netrender_instance");

    let webgl = draw_webgl_triangle(device.clone(), queue.clone());

    let paint_rc = Paint::new_for_test();
    let paint = paint_rc.borrow();
    ensure_pipeline_namespace();
    let painter_id = PainterId::next();
    paint.install_renderer(painter_id, renderer);
    paint.install_external_texture(WEBGL_TEXTURE_KEY, webgl.texture().texture.clone());

    let webview_id = WebViewId::new(painter_id);
    let pipeline_id = PipelineId::default();
    paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
        webview_id,
        envelope: envelope_with_webgl_canvas(),
        paint_info: paint_info_for(pipeline_id),
    }]);
    paint.render(webview_id);

    let master = paint
        .composite_texture(painter_id)
        .expect("composite texture after render");
    let image = read_texture_to_image(
        &device,
        &queue,
        &master,
        master.format(),
        PhysicalSize::new(VIEWPORT, VIEWPORT),
        DeviceIntRect::new(
            paint_types::units::DeviceIntPoint::new(0, 0),
            paint_types::units::DeviceIntPoint::new(VIEWPORT as i32, VIEWPORT as i32),
        ),
    )
    .expect("master readback");

    assert_eq!(image.get_pixel(4, 4).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(32, 26).0, [0, 255, 0, 255]);
    assert_eq!(image.get_pixel(32, 32).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(17, 17).0, [255, 0, 0, 255]);
}
