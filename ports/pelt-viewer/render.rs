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
use pelt_core::ResourceFetcher;
use serval_layout::{
    BackgroundImagePlane, ImageLoader, ImagePlane, LocalFileImageLoader, ResourceResolver,
    StylePlane, emit_paint_list_with_layouts, inline_stylesheets, layout,
    linked_stylesheets_with_loader, run_cascade,
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
    fetcher: Option<&dyn ResourceFetcher>,
) -> netrender::Scene {
    let document = StaticDocument::parse(html);

    let resolver = ResourceResolver { base_dir: base_dir.map(Path::to_path_buf), tests_root: None };

    // One loader for every external resource: remote `http(s)` URLs go through the
    // shell's `ResourceFetcher` (when supplied), local/relative ones through disk.
    // `data:` URIs are decoded inside serval and never reach it.
    let loader = HostImageLoader {
        local: LocalFileImageLoader::new(resolver),
        fetcher,
    };

    let inline_css = inline_stylesheets(&document);
    // `<link rel=stylesheet>` sheets load through the same loader, so remote
    // stylesheets resolve via the fetcher rather than being silently dropped.
    let linked_css = linked_stylesheets_with_loader(&document, &loader);
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

    // `<img>`: data: URIs decode inline; relative paths load from disk against
    // the document's directory, remote `src`s through the fetcher. The box tree
    // sizes each replaced leaf from this plane at layout time.
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

/// The host's image loader: remote `http(s)` URLs go through the shell's
/// [`ResourceFetcher`] (when one is supplied), everything else (relative / local
/// files) through serval's [`LocalFileImageLoader`]. `data:` URIs are decoded
/// inside serval and never reach a loader. With `fetcher == None` this behaves
/// exactly like the bare `LocalFileImageLoader` — remote `<img>`s just don't load.
pub(crate) struct HostImageLoader<'a> {
    pub(crate) local: LocalFileImageLoader,
    pub(crate) fetcher: Option<&'a dyn ResourceFetcher>,
}

impl ImageLoader for HostImageLoader<'_> {
    fn load(&self, url: &str) -> Option<Vec<u8>> {
        if url.starts_with("http://") || url.starts_with("https://") {
            self.fetcher.and_then(|f| f.fetch(url))
        } else {
            self.local.load(url)
        }
    }
}
