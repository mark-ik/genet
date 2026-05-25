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
//! ## Scope (v1, 2026-05-18)
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
/// background layer). Gradients and additional layers are deferred.
pub struct BackgroundImagePlane<NodeId: Copy + Eq + Hash> {
    images: FxHashMap<NodeId, DecodedImage>,
}

impl<NodeId: Copy + Eq + Hash> Default for BackgroundImagePlane<NodeId> {
    fn default() -> Self {
        Self {
            images: FxHashMap::default(),
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
        let mut images = FxHashMap::default();
        let mut queue = vec![dom.document()];
        while let Some(id) = queue.pop() {
            if let Some(src) = background_image_url(styles, id) {
                let decoded = if src.starts_with("data:") {
                    decode_data_uri(&src)
                } else {
                    loader.load(&src).and_then(|bytes| decode_image_bytes(&bytes))
                };
                if let Some(decoded) = decoded {
                    images.insert(id, decoded);
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
}

/// Read an element's first `url()` `background-image` layer as a URL
/// string. `None` when the cascade hasn't run, there's no background
/// image, or the first layer isn't a `url()` (e.g. a gradient).
fn background_image_url<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<String> {
    use style::values::generics::image::Image;

    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let first = primary.get_background().background_image.0.iter().next()?;
    match first {
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

/// Decode raw image-file bytes (PNG / JPEG / etc.) into RGBA8 pixels.
/// Returns `None` for formats the `image` crate can't decode with its
/// enabled features.
fn decode_image_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    let dynamic = image::load_from_memory(bytes).ok()?;
    let rgba = dynamic.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(DecodedImage {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}
