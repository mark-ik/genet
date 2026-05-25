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

use std::path::{Path, PathBuf};

use layout_dom_api::LayoutDom;
use paint_list_api::DeviceIntSize;
use serval_layout::{
    BackgroundImagePlane, ImageLoader, ImagePlane, StylePlane, emit_paint_list_with_layouts, layout,
    run_cascade,
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

    let inline_css = extract_inline_styles(&document);
    let linked_css = extract_linked_styles(&document, base_dir);
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
    let loader = LocalFileImageLoader {
        base_dir: base_dir.map(Path::to_path_buf),
    };
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

/// Loads `<img>`/`background-image` resources from the local filesystem,
/// resolving relative `src`/`url()` against the document's directory.
/// Remote URLs are not fetched (host territory); `data:` URIs are handled
/// upstream and never reach the loader.
struct LocalFileImageLoader {
    base_dir: Option<PathBuf>,
}

impl ImageLoader for LocalFileImageLoader {
    fn load(&self, url: &str) -> Option<Vec<u8>> {
        if url.starts_with("http://") || url.starts_with("https://") {
            return None;
        }
        let base = self.base_dir.as_ref()?;
        std::fs::read(base.join(url)).ok()
    }
}

/// Collect the text of every `<style>` element in document order — the
/// document's inline author stylesheets.
fn extract_inline_styles<D: LayoutDom>(dom: &D) -> Vec<String> {
    let mut sheets = Vec::new();
    let mut stack = vec![dom.document()];
    while let Some(id) = stack.pop() {
        if dom
            .element_name(id)
            .is_some_and(|q| q.local.as_ref() == "style")
        {
            let mut css = String::new();
            for child in dom.dom_children(id) {
                if let Some(text) = dom.text(child) {
                    css.push_str(text);
                }
            }
            if !css.trim().is_empty() {
                sheets.push(css);
            }
        }
        for child in dom.dom_children(id) {
            stack.push(child);
        }
    }
    sheets
}

/// Read every `<link rel=stylesheet href=…>` whose `href` resolves to a
/// readable local file under `base_dir`. Empty when `base_dir` is `None`
/// or nothing resolves. Remote hrefs are skipped (host-fetch territory).
fn extract_linked_styles<D: LayoutDom>(dom: &D, base_dir: Option<&Path>) -> Vec<String> {
    let Some(base) = base_dir else {
        return Vec::new();
    };
    let no_ns = markup5ever::Namespace::default();
    let rel_attr = markup5ever::LocalName::from("rel");
    let href_attr = markup5ever::LocalName::from("href");

    let mut sheets = Vec::new();
    let mut stack = vec![dom.document()];
    while let Some(id) = stack.pop() {
        let is_link = dom
            .element_name(id)
            .is_some_and(|q| q.local.as_ref() == "link");
        if is_link {
            let is_stylesheet = dom
                .attribute(id, &no_ns, &rel_attr)
                .is_some_and(|rel| rel.eq_ignore_ascii_case("stylesheet"));
            if is_stylesheet {
                if let Some(href) = dom.attribute(id, &no_ns, &href_attr) {
                    if href.starts_with("http://") || href.starts_with("https://") {
                        eprintln!("[pelt-viewer] skipping remote stylesheet: {href}");
                    } else {
                        let path = base.join(href);
                        match std::fs::read_to_string(&path) {
                            Ok(css) => sheets.push(css),
                            Err(err) => eprintln!(
                                "[pelt-viewer] could not read stylesheet {}: {err}",
                                path.display()
                            ),
                        }
                    }
                }
            }
        }
        for child in dom.dom_children(id) {
            stack.push(child);
        }
    }
    sheets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_style_blocks_are_extracted() {
        let doc = StaticDocument::parse(
            "<html><head><style>p { color: red; }</style>\
             <style>h1 { color: blue; }</style></head><body></body></html>",
        );
        let sheets = extract_inline_styles(&doc);
        assert_eq!(sheets.len(), 2);
        let joined = sheets.join("\n");
        assert!(joined.contains("color: red"));
        assert!(joined.contains("color: blue"));
    }

    #[test]
    fn linked_stylesheets_resolve_against_base_dir() {
        let dir = std::env::temp_dir().join(format!("pelt_viewer_link_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("site.css"), "body { color: green; }").unwrap();

        let doc = StaticDocument::parse(
            "<html><head>\
             <link rel=\"stylesheet\" href=\"site.css\">\
             <link rel=\"icon\" href=\"favicon.ico\">\
             </head><body></body></html>",
        );

        let sheets = extract_linked_styles(&doc, Some(&dir));
        assert_eq!(sheets.len(), 1);
        assert!(sheets[0].contains("color: green"));
        assert!(extract_linked_styles(&doc, None).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
