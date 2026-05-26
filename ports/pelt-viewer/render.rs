/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! serval HTML → `netrender::Scene`.
//!
//! Runs the serval engine pipeline (parse → cascade → layout → emit) and
//! translates the paint list to a `netrender::Scene`. The GPU side
//! (rendering the scene on Masonry's shared device and compositing it
//! into the content layer) lives in [`crate::app`]'s driver — this
//! module is GPU-free, so content production and presentation stay
//! separable.

use std::path::Path;

use paint_list_api::DeviceIntSize;
use serval_layout::{
    BackgroundImagePlane, ImagePlane, LocalFileImageLoader, ResourceResolver, StylePlane,
    emit_paint_list_with_layouts, inline_stylesheets, layout, linked_stylesheets, run_cascade,
};
use serval_static_dom::StaticDocument;

/// Build a `netrender::Scene` for `html` at `width`×`height`. Author CSS
/// = caller `stylesheets` + the document's inline `<style>` blocks +
/// local `<link rel=stylesheet>` files resolved against `base_dir`.
/// `<img>` `src`s (data: URIs and `base_dir`-relative local files) are
/// decoded and laid out.
pub fn build_scene(
    html: &str,
    stylesheets: &[&str],
    base_dir: Option<&Path>,
    width: u32,
    height: u32,
) -> netrender::Scene {
    let document = StaticDocument::parse(html);

    let resolver = ResourceResolver { base_dir: base_dir.map(Path::to_path_buf), tests_root: None };
    let inline_css = inline_stylesheets(&document);
    let linked_css = linked_stylesheets(&document, &resolver);
    let mut all_sheets: Vec<&str> = stylesheets.to_vec();
    all_sheets.extend(inline_css.iter().map(String::as_str));
    all_sheets.extend(linked_css.iter().map(String::as_str));

    let mut styles: StylePlane<_> = StylePlane::new();
    run_cascade(
        &document,
        &mut styles,
        euclid::Size2D::new(width as f32, height as f32),
        &all_sheets,
    );

    // `<img>`: data: URIs decode inline; relative paths load from disk
    // against the document's directory. The box tree sizes each replaced
    // leaf from this plane at layout time.
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
        DeviceIntSize::new(width as i32, height as i32),
    );

    paint::translate_paint_list(&plist)
}
