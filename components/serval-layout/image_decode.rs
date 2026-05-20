/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `<img>` decode pass — DOM → decoded RGBA8 pixels keyed by `NodeId`.
//!
//! Walks the DOM for `<img>` elements, reads their `src`, and decodes
//! the referenced image to RGBA8. The result feeds two consumers:
//! - **Layout** reads intrinsic dimensions to size `<img>` boxes
//!   whose CSS leaves width/height `auto`
//!   (`StylePlane::apply_intrinsic_image_sizes`).
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
