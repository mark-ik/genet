/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! HTML -> image, for reftest pixel comparison (phase 2).
//!
//! Replicates the public path the `html_to_pixels_e2e` test drives:
//! parse -> cascade -> layout -> emit paint list -> netrender -> readback.
//! The wgpu boot + netrender instance are created once
//! ([`Renderer::boot`]) and reused across every test in a subset.
//!
//! Slice 1 renders inline `<style>` only (no linked CSS / external
//! images); the runner skips tests that need those.

use std::rc::Rc;

use dpi::PhysicalSize;
use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use netrender::{boot, create_netrender_instance, NetrenderOptions};
use paint::Paint;
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_api::wgpu_readback::read_texture_to_image;
use paint_list_api::{DeviceIntSize, PaintEnvelope};
use paint_types::units::{DeviceIntRect, LayoutSize};
use paint_types::PipelineId;
use servo_base::id::{PainterId, PipelineNamespace, PipelineNamespaceId, WebViewId};
use serval_layout::{
    emit_paint_list_with_layouts, layout, run_cascade, BackgroundImagePlane, ImagePlane,
    NoImageLoader, StylePlane,
};
use serval_static_dom::StaticDocument;

pub type Image = image::ImageBuffer<image::Rgba<u8>, Vec<u8>>;

/// A booted renderer reused across a subset's tests.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    paint: Rc<std::cell::RefCell<Paint>>,
    painter_id: PainterId,
    webview_id: WebViewId,
}

impl Renderer {
    /// Boot wgpu + netrender once. Returns an error string if the GPU is
    /// unavailable (the runner can then report reftests as unrunnable
    /// rather than crash).
    pub fn boot() -> Result<Self, String> {
        let handles = boot().map_err(|e| format!("wgpu boot: {e:?}"))?;
        let device = handles.device.clone();
        let queue = handles.queue.clone();
        let renderer = create_netrender_instance(
            handles,
            NetrenderOptions {
                tile_cache_size: Some(64),
                enable_vello: true,
                ..Default::default()
            },
        )
        .map_err(|e| format!("create_netrender_instance: {e:?}"))?;

        let paint = Paint::new_for_test();
        PipelineNamespace::install(PipelineNamespaceId(1));
        let painter_id = PainterId::next();
        paint.borrow().install_renderer(painter_id, renderer);
        let webview_id = WebViewId::new(painter_id);

        Ok(Self { device, queue, paint, painter_id, webview_id })
    }

    /// Render `html` (with `sheets` as author stylesheets) to an image at
    /// `width` x `height`.
    pub fn render_html(&self, html: &str, sheets: &[&str], width: u32, height: u32) -> Image {
        let envelope = html_to_envelope(html, sheets, width, height);
        let paint = self.paint.borrow();
        paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
            webview_id: self.webview_id,
            envelope,
            paint_info: paint_info_for(PipelineId::default(), width, height),
        }]);
        paint.render(self.webview_id);
        let master = paint
            .composite_texture(self.painter_id)
            .expect("composite_texture after render");
        read_texture_to_image(
            &self.device,
            &self.queue,
            &master,
            master.format(),
            PhysicalSize::new(width, height),
            DeviceIntRect::new(
                paint_types::units::DeviceIntPoint::new(0, 0),
                paint_types::units::DeviceIntPoint::new(width as i32, height as i32),
            ),
        )
        .expect("master readback")
    }
}

fn paint_info_for(pid: PipelineId, width: u32, height: u32) -> PaintDisplayListInfo {
    PaintDisplayListInfo::new(
        ViewportDetails {
            size: Size2D::new(width as f32, height as f32),
            hidpi_scale_factor: Scale::new(1.0),
        },
        LayoutSize::new(width as f32, height as f32),
        pid,
        servo_base::Epoch(0),
        AxesScrollSensitivity {
            x: ScrollType::InputEvents | ScrollType::Script,
            y: ScrollType::InputEvents | ScrollType::Script,
        },
        true,
    )
}

/// HTML -> `PaintEnvelope` (the producer half). Mirrors the e2e test's
/// `html_to_envelope`. Inline `<img>`/background data-URIs decode via
/// `NoImageLoader`; remote resources are not fetched.
fn html_to_envelope(html: &str, sheets: &[&str], width: u32, height: u32) -> PaintEnvelope {
    let document = StaticDocument::parse(html);
    let mut styles: StylePlane<_> = StylePlane::new();
    run_cascade(&document, &mut styles, euclid::Size2D::new(width as f32, height as f32), sheets);

    let images = ImagePlane::decode_from_dom_with_loader(&document, &NoImageLoader);
    let bg_images = BackgroundImagePlane::decode_from_cascade(&document, &styles, &NoImageLoader);

    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(&document, &styles, &images, viewport);
    let plist = emit_paint_list_with_layouts(
        &document,
        &styles,
        &fragments,
        &built,
        &text_ctx,
        &images,
        &bg_images,
        DeviceIntSize::new(width as i32, height as i32),
    );
    PaintEnvelope::from_list(&plist)
}
