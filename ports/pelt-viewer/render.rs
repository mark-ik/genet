/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! serval HTML → `peniko::ImageData`.
//!
//! Runs the serval engine pipeline (parse → cascade → layout → emit),
//! translates the resulting paint list to a netrender `Scene`, renders
//! it to an offscreen texture, reads the pixels back, and wraps them as
//! a `peniko::ImageData` for a Xilem `image_view`.
//!
//! v1 boots a fresh netrender device per call — wasteful but simple;
//! fine for static content with infrequent re-renders. A held renderer
//! is a follow-up.

use std::sync::Arc;

use masonry::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
use paint_list_api::DeviceIntSize;
use paint_types::units::{DeviceIntPoint, DeviceIntRect};
use serval_layout::{
    BackgroundImagePlane, ImagePlane, NoImageLoader, StylePlane, emit_paint_list_with_layouts,
    layout, run_cascade,
};
use serval_static_dom::StaticDocument;

/// Render `html` (with optional `stylesheets`) at `width`×`height` to an
/// RGBA8 `ImageData`. Returns `Err` if the GPU boot or readback fails.
pub fn render_html(
    html: &str,
    stylesheets: &[&str],
    width: u32,
    height: u32,
) -> Result<ImageData, String> {
    // 1. serval pipeline: parse → cascade → refresh Taffy → decode
    //    images → layout → emit paint list.
    let document = StaticDocument::parse(html);
    let mut styles: StylePlane<_> = StylePlane::new();
    run_cascade(
        &document,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        stylesheets,
    );
    styles.refresh_taffy_from_cascade();

    let images = ImagePlane::decode_from_dom(&document);
    styles.apply_intrinsic_image_sizes(&images);
    let bg_images = BackgroundImagePlane::decode_from_cascade(&document, &styles, &NoImageLoader);

    let viewport = taffy::Size {
        width: taffy::AvailableSpace::Definite(width as f32),
        height: taffy::AvailableSpace::Definite(height as f32),
    };
    let (fragments, built, text_ctx) = layout(&document, &styles, viewport);
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

    // 2. Translate to a netrender Scene (public seam in servo-paint).
    let scene = paint::translate_paint_list(&plist);

    // 3. Render the Scene to an offscreen texture.
    let handles = netrender::boot().map_err(|e| format!("wgpu boot failed: {e}"))?;
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = netrender::create_netrender_instance(
        handles,
        netrender::NetrenderOptions {
            tile_cache_size: Some(64),
            enable_vello: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("netrender init failed: {e:?}"))?;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pelt-viewer content target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        // vello renders via compute → storage texture; COPY_SRC for readback.
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_vello(&scene, &view, netrender::ColorLoad::default());

    // 4. Read the pixels back to CPU.
    let rgba = paint_api::wgpu_readback::read_texture_to_image(
        &device,
        &queue,
        &texture,
        texture.format(),
        dpi::PhysicalSize::new(width, height),
        DeviceIntRect::new(
            DeviceIntPoint::new(0, 0),
            DeviceIntPoint::new(width as i32, height as i32),
        ),
    )
    .ok_or_else(|| "content texture readback failed".to_string())?;

    // 5. Wrap as peniko ImageData for the Xilem image view.
    Ok(ImageData {
        data: Blob::new(Arc::new(rgba.into_raw())),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}
