/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `<img>` decode pass — DOM → decoded RGBA8 pixels keyed by `NodeId`.
//!
//! Walks the DOM for `<img>` elements, reads their `src`, and decodes
//! the referenced image to RGBA8. The result feeds two consumers:
//! - **Layout** reads intrinsic dimensions to size `<img>` boxes
//!   whose CSS leaves width/height `auto` (the box tree's
//!   replaced-leaf sizing, `construct::replaced_px_size`).
//! - **Paint emission** reads the pixels to emit `DrawImage` +
//!   `ImageResource` (`paint_emit`).
//!
//! ## Fetch boundary
//!
//! `data:` URIs are decoded inline (self-contained, no fetch): the
//! `src` is parsed via the `data-url` crate (base64 / percent
//! encoding) and decoded via the `image` crate (PNG / JPEG / etc. per
//! its enabled features).
//!
//! Non-`data:` URLs (`http(s):`, relative) are **not fetched by
//! serval** — per the Hekate lanes doc, fetching is the host /
//! network-adapter's job; serval consumes bytes. The seam is an
//! [`ImageLoader`]: callers that have already fetched a URL hand its
//! bytes back through the loader, and [`ImagePlane::decode_from_dom_with_loader`]
//! decodes them. The default [`ImagePlane::decode_from_dom`] uses a
//! no-op loader, so unfetched remote `<img>`s lay out at 0×0 with no
//! paint (same as a broken image).

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use rustc_hash::FxHashMap;

use crate::box_tree::PseudoKind;
use crate::style::StylePlane;

/// Supplies raw (undecoded) image bytes for non-`data:` URLs that
/// serval itself doesn't fetch. The host (or Hekate's network
/// adapter) implements this over its resource cache; `data:` URIs are
/// handled internally and never reach the loader.
pub trait ImageLoader {
    /// Return the raw image-file bytes for `url`, or `None` if not
    /// available (not fetched, failed, or unsupported scheme).
    fn load(&self, url: &str) -> Option<Vec<u8>>;
}

/// No-op loader: every non-`data:` URL is unavailable. Used by
/// [`ImagePlane::decode_from_dom`] — only `data:` URIs decode.
pub struct NoImageLoader;

impl ImageLoader for NoImageLoader {
    fn load(&self, _url: &str) -> Option<Vec<u8>> {
        None
    }
}

/// Decoded image pixels + intrinsic dimensions.
#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major, tightly packed (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
}

impl DecodedImage {
    /// A sub-image of the `(sx, sy, sw, sh)` source rect (clamped to bounds), as
    /// its own tightly-packed `DecodedImage`. Used by border-image's 9-slice to
    /// carve the source into corner / edge / center regions, each then drawn
    /// (stretched or tiled) to its destination rect via the existing image
    /// commands. `None` if the requested rect has zero area after clamping.
    pub fn crop(&self, sx: u32, sy: u32, sw: u32, sh: u32) -> Option<DecodedImage> {
        let sx = sx.min(self.width);
        let sy = sy.min(self.height);
        let sw = sw.min(self.width - sx);
        let sh = sh.min(self.height - sy);
        if sw == 0 || sh == 0 {
            return None;
        }
        let mut rgba = Vec::with_capacity((sw * sh * 4) as usize);
        for row in 0..sh {
            let src_y = sy + row;
            let start = (((src_y * self.width) + sx) * 4) as usize;
            let end = start + (sw * 4) as usize;
            rgba.extend_from_slice(&self.rgba[start..end]);
        }
        Some(DecodedImage { width: sw, height: sh, rgba })
    }
}

/// Decoded `<img>` images keyed by their DOM `NodeId`. Built by
/// [`ImagePlane::decode_from_dom`] before layout.
pub struct ImagePlane<NodeId: Copy + Eq + Hash> {
    images: FxHashMap<NodeId, DecodedImage>,
}

impl<NodeId: Copy + Eq + Hash> Default for ImagePlane<NodeId> {
    fn default() -> Self {
        Self {
            images: FxHashMap::default(),
        }
    }
}

impl<NodeId: Copy + Eq + Hash> ImagePlane<NodeId> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk `dom`, decode every `<img>` whose `src` is a `data:` URI,
    /// keyed by the `<img>` element's `NodeId`. Remote URLs are
    /// skipped (use [`Self::decode_from_dom_with_loader`] to supply
    /// their bytes).
    pub fn decode_from_dom<D>(dom: &D) -> Self
    where
        D: LayoutDom<NodeId = NodeId>,
    {
        Self::decode_from_dom_with_loader(dom, &NoImageLoader)
    }

    /// Like [`Self::decode_from_dom`], but resolves non-`data:` `src`
    /// URLs through `loader` (the host's resource cache / fetcher).
    /// `data:` URIs are still decoded inline.
    pub fn decode_from_dom_with_loader<D, L>(dom: &D, loader: &L) -> Self
    where
        D: LayoutDom<NodeId = NodeId>,
        L: ImageLoader,
    {
        let mut images = FxHashMap::default();
        let no_ns = markup5ever::Namespace::default();
        let src_local = markup5ever::LocalName::from("src");

        let mut queue = vec![dom.document()];
        while let Some(id) = queue.pop() {
            if dom
                .element_name(id)
                .is_some_and(|q| q.local == html5ever::local_name!("img"))
            {
                if let Some(src) = dom.attribute(id, &no_ns, &src_local) {
                    let decoded = if src.starts_with("data:") {
                        decode_data_uri(src)
                    } else {
                        loader.load(src).and_then(|bytes| decode_image_bytes(&bytes))
                    };
                    if let Some(decoded) = decoded {
                        images.insert(id, decoded);
                    }
                }
            }
            queue.extend(dom.dom_children(id));
        }
        Self { images }
    }

    pub fn get(&self, id: NodeId) -> Option<&DecodedImage> {
        self.images.get(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &DecodedImage)> {
        self.images.iter()
    }
}

/// Decoded CSS `background-image` images keyed by their element's DOM
/// `NodeId`. Distinct from [`ImagePlane`] (the `<img>` replaced-content
/// plane) on purpose: background images must **not** size their box
/// (only `<img>` replaced content does), so they never feed the box
/// tree's replaced-leaf sizing. Built by
/// [`BackgroundImagePlane::decode_from_cascade`] after
/// the cascade has run (it reads `background-image` from
/// `ComputedValues`).
///
/// v1 decodes the first `url()` layer per element (the topmost CSS
/// background layer). Gradients (linear / radial / conic) are emitted
/// directly by `paint_emit`, not decoded here; additional `url()` layers
/// are deferred.
pub struct BackgroundImagePlane<NodeId: Copy + Eq + Hash> {
    images: FxHashMap<NodeId, DecodedImage>,
    /// `url()` background-images for block-display `::before` / `::after` pseudo
    /// boxes, keyed by `(originating element, kind)` — the pseudo box has no DOM
    /// id of its own, so it cannot share the element's `images` slot.
    pseudo_images: FxHashMap<(NodeId, PseudoKind), DecodedImage>,
    /// Decoded `border-image-source` `url()` images, keyed by element. A separate
    /// slot from `images` because an element can carry both a background-image and
    /// a border-image; paint's 9-slice carves this source into edge/corner regions.
    border_images: FxHashMap<NodeId, DecodedImage>,
}

impl<NodeId: Copy + Eq + Hash> Default for BackgroundImagePlane<NodeId> {
    fn default() -> Self {
        Self {
            images: FxHashMap::default(),
            pseudo_images: FxHashMap::default(),
            border_images: FxHashMap::default(),
        }
    }
}

impl<NodeId: Copy + Eq + Hash> BackgroundImagePlane<NodeId> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk every element with cascade data, decode its first `url()`
    /// `background-image` layer (a `data:` URI inline, a remote URL via
    /// `loader`), keyed by the element's `NodeId`.
    pub fn decode_from_cascade<D, L>(dom: &D, styles: &StylePlane<NodeId>, loader: &L) -> Self
    where
        D: LayoutDom<NodeId = NodeId>,
        L: ImageLoader,
    {
        let decode = |src: String| -> Option<DecodedImage> {
            if src.starts_with("data:") {
                decode_data_uri(&src)
            } else {
                loader.load(&src).and_then(|bytes| decode_image_bytes(&bytes))
            }
        };
        let mut images = FxHashMap::default();
        let mut pseudo_images = FxHashMap::default();
        let mut border_images = FxHashMap::default();
        let mut queue = vec![dom.document()];
        while let Some(id) = queue.pop() {
            if let Some(decoded) = background_image_url(styles, id).and_then(decode) {
                images.insert(id, decoded);
            }
            // Block `::before` / `::after` boxes carry their own background, keyed
            // by `(element, kind)` since they have no DOM id of their own.
            for kind in [PseudoKind::Before, PseudoKind::After] {
                if let Some(decoded) = pseudo_background_image_url(styles, id, kind).and_then(decode)
                {
                    pseudo_images.insert((id, kind), decoded);
                }
            }
            if let Some(decoded) = border_image_url(styles, id).and_then(decode) {
                border_images.insert(id, decoded);
            }
            queue.extend(dom.dom_children(id));
        }
        Self { images, pseudo_images, border_images }
    }

    pub fn get(&self, id: NodeId) -> Option<&DecodedImage> {
        self.images.get(&id)
    }

    /// The decoded `url()` background-image for `element`'s block `kind` pseudo
    /// box, if any.
    pub fn get_pseudo(&self, element: NodeId, kind: PseudoKind) -> Option<&DecodedImage> {
        self.pseudo_images.get(&(element, kind))
    }

    /// The decoded `border-image-source` `url()` image for `element`, if any.
    pub fn get_border_image(&self, id: NodeId) -> Option<&DecodedImage> {
        self.border_images.get(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty() && self.pseudo_images.is_empty() && self.border_images.is_empty()
    }

    pub fn len(&self) -> usize {
        self.images.len() + self.pseudo_images.len() + self.border_images.len()
    }
}

/// Read an element's first `url()` `background-image` layer as a URL
/// string. `None` when the cascade hasn't run, there's no background
/// image, or the first layer isn't a `url()` (e.g. a gradient).
fn background_image_url<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<String> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    first_url_background(data.styles.primary())
}

/// Read the first `url()` `background-image` layer of `id`'s block-level
/// `::before` / `::after` pseudo (the one that becomes a paintable box). `None`
/// when the pseudo is absent, inline-level, or its first layer isn't a `url()`.
fn pseudo_background_image_url<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    kind: PseudoKind,
) -> Option<String> {
    use style::selector_parser::PseudoElement;
    use style::values::specified::box_::DisplayOutside;

    let pseudo = match kind {
        PseudoKind::Before => PseudoElement::Before,
        PseudoKind::After => PseudoElement::After,
    };
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let cv = data.styles.pseudos.get(&pseudo)?;
    if !matches!(cv.get_box().display.outside(), DisplayOutside::Block) {
        return None;
    }
    first_url_background(cv)
}

/// The first `url()` `background-image` layer of a `ComputedValues` as a URL
/// string, or `None` when the first layer is not a `url()` (e.g. a gradient).
fn first_url_background(cv: &style::properties::ComputedValues) -> Option<String> {
    use style::values::generics::image::Image;
    match cv.get_background().background_image.0.iter().next()? {
        Image::Url(url) => url.url().map(|u| u.as_str().to_string()),
        _ => None,
    }
}

/// An element's `border-image-source` as a `url()` string. `None` when there is
/// no border-image, or the source is not a `url()` (a gradient border-image is a
/// later slice — gradients emit directly, not through this decode pass).
fn border_image_url<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<String> {
    use style::values::generics::image::Image;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    match &data.styles.primary().get_border().border_image_source {
        Image::Url(url) => url.url().map(|u| u.as_str().to_string()),
        _ => None,
    }
}

/// Decode a `data:` URI into RGBA8 pixels. Returns `None` for
/// malformed data or formats the `image` crate can't decode.
fn decode_data_uri(src: &str) -> Option<DecodedImage> {
    let url = data_url::DataUrl::process(src).ok()?;
    let (bytes, _fragment) = url.decode_to_vec().ok()?;
    decode_image_bytes(&bytes)
}

/// Decode raw image-file bytes into RGBA8 pixels. Tries the `image`
/// crate first (PNG / JPEG / GIF / etc. per its enabled features); on
/// failure, falls back to SVG (`image` is raster-only). Returns `None`
/// when neither can decode the bytes.
fn decode_image_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    if let Ok(dynamic) = image::load_from_memory(bytes) {
        let rgba = dynamic.to_rgba8();
        let (width, height) = rgba.dimensions();
        return Some(DecodedImage { width, height, rgba: rgba.into_raw() });
    }
    decode_svg_bytes(bytes)
}

/// Rasterize SVG bytes into RGBA8 at the document's intrinsic size via
/// `resvg` (parse with `usvg`, render into a `tiny_skia` pixmap).
/// `background-size` / replaced-box sizing scale the result downstream,
/// so this rasterizes once at the SVG's own size. Returns `None` for a
/// parse error or a zero/over-large intrinsic size.
fn decode_svg_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    use resvg::{tiny_skia, usvg};

    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let width = size.width().ceil() as u32;
    let height = size.height().ceil() as u32;
    // Guard against degenerate / pathologically large intrinsic sizes
    // (a 0-dim SVG, or one that would allocate gigabytes).
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    // tiny_skia pixmaps are premultiplied RGBA8; the paint path wants
    // straight (un-premultiplied) RGBA8, so un-premultiply per pixel.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3];
        if a != 0 && a != 255 {
            let unmul = |c: u8| ((c as u32 * 255 + (a as u32 / 2)) / a as u32).min(255) as u8;
            px[0] = unmul(px[0]);
            px[1] = unmul(px[1]);
            px[2] = unmul(px[2]);
        }
    }
    Some(DecodedImage { width, height, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SVG bytes rasterize at the document's intrinsic size, through the same
    /// `decode_image_bytes` entry the raster path uses (the `image` crate fails
    /// on SVG, so the SVG fallback runs). A 20x20 green-rect SVG must decode to
    /// 20x20 with an opaque green center pixel.
    #[test]
    fn decodes_svg_to_rgba_at_intrinsic_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20">
            <rect width="20" height="20" fill="#008000"/></svg>"##;
        let decoded = decode_image_bytes(svg).expect("SVG decodes via the resvg fallback");
        assert_eq!((decoded.width, decoded.height), (20, 20), "intrinsic 20x20");
        // Center pixel (10, 10): straight RGBA8, opaque mid-green (#008000).
        let i = ((10 * decoded.width + 10) * 4) as usize;
        let (r, g, b, a) = (decoded.rgba[i], decoded.rgba[i + 1], decoded.rgba[i + 2], decoded.rgba[i + 3]);
        assert_eq!(a, 255, "opaque");
        assert!(r < 40 && g > 100 && b < 40, "green center, got ({r},{g},{b})");
    }

    /// `DecodedImage::crop` carves a sub-rect (border-image's 9-slice primitive):
    /// it copies the right pixels and clamps an out-of-bounds rect to `None`.
    #[test]
    fn crop_extracts_subimage_and_clamps() {
        // 4×2: left half red, right half blue.
        let mut rgba = Vec::new();
        for _y in 0..2 {
            for x in 0..4 {
                rgba.extend_from_slice(if x < 2 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let img = DecodedImage { width: 4, height: 2, rgba };

        let right = img.crop(2, 0, 2, 2).expect("non-empty crop");
        assert_eq!((right.width, right.height), (2, 2));
        assert_eq!(&right.rgba[0..4], &[0, 0, 255, 255], "crop top-left is blue");
        assert_eq!(right.rgba.len(), 2 * 2 * 4, "tightly packed");

        assert!(img.crop(4, 0, 2, 2).is_none(), "rect at the right edge clamps to zero width → None");
    }

    #[test]
    fn debug_real_vector_svg() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg"
     width="8px" height="32px" viewBox="0 0 4 64" preserveAspectRatio="none">
  <rect y="0" width="100%" height="50%" fill="lime"/>
  <rect y="50%" width="100%" height="50%" fill="aqua"/></svg>"##;
        let d = decode_image_bytes(svg).expect("decodes");
        eprintln!("DBG dims={}x{}", d.width, d.height);
        // sample: top band center, bottom band center, full-width at mid-top row
        let px = |x: u32, y: u32| {
            let i = ((y * d.width + x) * 4) as usize;
            (d.rgba[i], d.rgba[i + 1], d.rgba[i + 2], d.rgba[i + 3])
        };
        eprintln!("DBG top-center(4,8)={:?}", px(4, 8));
        eprintln!("DBG bot-center(4,24)={:?}", px(4, 24));
        eprintln!("DBG top-left(0,8)={:?} top-right(7,8)={:?}", px(0, 8), px(7, 8));
    }

    /// A `data:image/svg+xml` URI decodes through `decode_data_uri` → the same
    /// SVG fallback (the reftest + live paths both reach SVG this way).
    #[test]
    fn decodes_svg_data_uri() {
        let uri = "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' \
                   width='8' height='8'><rect width='8' height='8' fill='blue'/></svg>";
        let decoded = decode_data_uri(uri).expect("SVG data-URI decodes");
        assert_eq!((decoded.width, decoded.height), (8, 8));
    }
}
