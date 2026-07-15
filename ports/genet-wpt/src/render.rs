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

use std::path::Path;
use std::rc::Rc;

use dpi::PhysicalSize;
use embedder_traits::ViewportDetails;
use euclid::{Scale, Size2D};
use genet_layout::{
    BackgroundImagePlane, ImageLoader, ImagePlane, LocalFileImageLoader, ResourceResolver,
    StylePlane, emit_paint_list_with_layouts, inline_stylesheets, layout, linked_stylesheets,
    run_cascade,
};
use genet_livery::{Device as LiveryDevice, LiveryDocument, StyleSet as LiveryStyleSet};
use genet_static_dom::StaticDocument;
use netrender::{NetrenderOptions, boot, create_netrender_instance};
use paint::Paint;
use paint_api::display_list::{AxesScrollSensitivity, PaintDisplayListInfo, ScrollType};
use paint_api::wgpu_readback::read_texture_to_image;
use paint_list_api::{DeviceIntSize, PaintEnvelope};
use paint_types::PipelineId;
use paint_types::units::{DeviceIntRect, LayoutSize};
use servo_base::id::{PainterId, PipelineNamespace, PipelineNamespaceId, WebViewId};

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

        Ok(Self {
            device,
            queue,
            paint,
            painter_id,
            webview_id,
        })
    }

    /// Render `html` to an image at `width` x `height`, resolving the
    /// page's inline + linked CSS and local images relative to `base_dir`
    /// (and `tests_root` for `/`-absolute URLs).
    pub fn render_html(
        &self,
        html: &str,
        base_dir: &Path,
        tests_root: &Path,
        width: u32,
        height: u32,
        is_xml: bool,
    ) -> Image {
        let envelope = html_to_envelope(html, base_dir, tests_root, width, height, is_xml);
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

    /// Render through the clean-room Livery lane. This first WPT bridge is
    /// intentionally bounded: it extracts inline and local linked stylesheets,
    /// supplies host-resolved local image bytes, and lets Livery handle its own
    /// declarations and data-URI image subset.
    pub fn render_html_livery(
        &self,
        html: &str,
        base_dir: &Path,
        tests_root: &Path,
        width: u32,
        height: u32,
        is_xml: bool,
    ) -> Image {
        let document = if is_xml {
            StaticDocument::parse_xml(html)
        } else {
            StaticDocument::parse(html)
        };
        let resolver = ResourceResolver {
            base_dir: Some(base_dir.to_path_buf()),
            tests_root: Some(tests_root.to_path_buf()),
        };
        let mut sheets = inline_stylesheets(&document);
        sheets.extend(linked_stylesheets(&document, &resolver));
        let sheet_refs = sheets.iter().map(String::as_str).collect::<Vec<_>>();
        let mut session = LiveryDocument::new(
            document,
            LiveryStyleSet::cambium(&sheet_refs),
            LiveryDevice::screen(width as f32, height as f32),
        );
        let image_loader = LocalFileImageLoader::new(resolver);
        for url in livery_image_urls(&sheets) {
            if let Some(bytes) = image_loader.load(&url) {
                session.set_image_resource(url, bytes);
            }
        }
        let list = session
            .frame(width, height)
            .expect("Livery WPT reftest layout");
        let envelope = PaintEnvelope::from_list(&list);
        let paint = self.paint.borrow();
        paint.handle_messages(vec![paint_api::PaintMessage::SendPaintList {
            webview_id: self.webview_id,
            envelope,
            paint_info: paint_info_for(PipelineId::default(), width, height),
        }]);
        paint.render(self.webview_id);
        let master = paint
            .composite_texture(self.painter_id)
            .expect("composite_texture after Livery render");
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
        .expect("Livery master readback")
    }
}

fn livery_image_urls(stylesheets: &[String]) -> Vec<String> {
    let mut urls = Vec::new();
    for stylesheet in stylesheets {
        let lower = stylesheet.to_ascii_lowercase();
        let mut cursor = 0;
        while let Some(offset) = lower[cursor..].find("url(") {
            let start = cursor + offset + 4;
            let Some(close) = stylesheet[start..].find(')') else {
                break;
            };
            let raw = stylesheet[start..start + close].trim();
            let url = raw
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| {
                    raw.strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                })
                .unwrap_or(raw)
                .trim();
            if !url.is_empty() && !urls.iter().any(|seen| seen == url) {
                urls.push(url.to_owned());
            }
            cursor = start + close + 1;
        }
    }
    urls
}

#[cfg(test)]
mod tests {
    use super::livery_image_urls;

    #[test]
    fn livery_image_urls_deduplicates_css_sources() {
        let sheets = vec![
            ".a { background-image: url(\"a.png\"); }".to_owned(),
            ".b { background-image: url(a.png); background: url(b.png); }".to_owned(),
        ];
        assert_eq!(
            livery_image_urls(&sheets),
            vec!["a.png".to_owned(), "b.png".to_owned()]
        );
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
/// `html_to_envelope`, plus author sheets from inline `<style>` + linked
/// `<link rel="stylesheet">`, and a file-backed image loader. data-URI
/// images decode inline; remote (`http(s)://`) resources are not fetched.
fn html_to_envelope(
    html: &str,
    base_dir: &Path,
    tests_root: &Path,
    width: u32,
    height: u32,
    is_xml: bool,
) -> PaintEnvelope {
    // Route by the caller's explicit format (from the file extension), not a
    // content sniff — sniffing misroutes HTML files that merely mention "xhtml".
    let document = if is_xml {
        StaticDocument::parse_xml(html)
    } else {
        StaticDocument::parse(html)
    };

    let resolver = ResourceResolver {
        base_dir: Some(base_dir.to_path_buf()),
        tests_root: Some(tests_root.to_path_buf()),
    };
    let mut sheets = inline_stylesheets(&document);
    sheets.extend(linked_stylesheets(&document, &resolver));
    let sheet_refs: Vec<&str> = sheets.iter().map(String::as_str).collect();

    // The document's file:// base URL, so relative CSS url() refs
    // (e.g. background-image: url(support/x.png)) resolve to real files.
    let base_url = resolver.base_url();

    let mut styles: StylePlane<_> = StylePlane::new();
    run_cascade(
        &document,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        &sheet_refs,
        base_url.as_deref(),
    );

    let loader = LocalFileImageLoader::new(resolver);
    let images = ImagePlane::decode_from_dom_with_loader(&document, &loader);
    let bg_images = BackgroundImagePlane::decode_from_cascade(&document, &styles, &loader);

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
        // Static reftest render has no scrolling, so pass empty
        // scroll offsets (mirrors emit_paint_list's no_scroll).
        &Default::default(),
        DeviceIntSize::new(width as i32, height as i32),
    );
    PaintEnvelope::from_list(&plist)
}
