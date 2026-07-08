/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Producer-side: emit [`ServalPaintList`] from `FragmentPlane` +
//! `StylePlane` + DOM.
//!
//! Walks the DOM in paint order (pre-order traversal; normal-flow paint order
//! matches DOM order). Out-of-flow and stacking-context content is lifted to
//! `Deferred` layers and placed by the recursive painter in `paint_stacking.rs`,
//! which orders positioned / z-index descendants per CSS 2.1 Appendix E, so this
//! walk emits in-flow content and defers the rest. Reads per-node layout from
//! `FragmentPlane` and per-node style from `StylePlane`, and produces a
//! closed-set [`PaintCmd`] stream.
//!
//! ## Scope
//!
//! - Per element: background color, background gradient (linear / radial /
//!   conic) and image, border, outset box-shadow, and a rounded-rect clip for
//!   `border-radius` (via [`background_color_of`] and the sibling helpers).
//! - `DrawText` per inline-context leaf carrying shaped glyph runs, plus
//!   underline / line-through decoration. [`emit_paint_list_with_layouts`] is
//!   the live path: it reads cached parley `Layout`s from the [`TextMeasureCtx`]
//!   populated by `crate::layout::layout` (per-run color via the run brush). The
//!   cache-less [`emit_paint_list`] still exists for callers that have not run
//!   layout; it emits empty glyph runs so the command structure is still present.
//! - `PushTransform` / `PopTransform` per fragment around the node's primitives,
//!   composing the parent-relative `taffy::Layout.location` (and any CSS
//!   `transform`) onto the transform stack; absolute scene coordinates fall out
//!   of the composition.
//!
//! Cf. `docs/2026-05-17_paintlist_polyglot_renderer.md` (PM-3).

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use paint_list_api::items::{
    BorderDetails, BorderItem, ExternalTextureItem, NinePatchBorder, NinePatchSource, PathCommand,
    PathData, ShadowItem,
};
use paint_list_api::specs::{ClipKind, ClipSpec, FilterOp, LayerSpec, TransformKind};
use paint_list_api::{
    AlphaType, BorderRadius, BorderSide, BorderStyle, BoxShadowClipMode, ColorF, CommonPlacement,
    ConicGradientItem, ConicGradientPayload, DeviceIntSideOffsets, DeviceIntSize, EngineId,
    ExtendMode, FontInstanceKey, FontResource, GlyphInstance, GradientStop, IdNamespace, ImageItem,
    ImageKey, ImageRendering, ImageResource, LayoutPoint, LayoutRect, LayoutSideOffsets,
    LayoutSize, LayoutTransform, LayoutVector2D, LinearGradientItem, LinearGradientPayload,
    MixBlendMode, NormalBorder, PaintCmd, PaintList, RadialGradientItem, RadialGradientPayload,
    RectItem, RepeatMode, RepeatingImageItem, TextOptions, TextRunItem, TransformSpec,
};
use parley::PositionedLayoutItem;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use servo_arc::Arc as ServoArc;
use style::properties::ComputedValues;

use crate::box_tree::BoxTree;
use crate::fragment::FragmentPlane;
use crate::image_decode::{BackgroundImagePlane, DecodedImage, ImagePlane};
use crate::style::StylePlane;
use crate::text_measure::TextMeasureCtx;

/// A host-provided source of pre-emitted Path-A paint commands for chisel leaf
/// nodes (`<chisel-leaf key="…">`), keyed by the leaf's stable `key`. The host
/// runs each dirty leaf's `paint` into a command buffer and exposes it here; this
/// producer splices those commands at the leaf's box. serval-layout only queries,
/// so it does not depend on the chisel crate. See
/// `docs/2026-07-07_chisel_widget_leaf_design.md`.
pub trait LeafPaintSource {
    /// The Path-A commands for the leaf registered under `key`, in the leaf's
    /// local (border-box origin) coordinates, or `None` if absent.
    fn leaf_commands(&self, key: u64) -> Option<&[PaintCmd]>;
}

/// Clone the primary cascaded style (`ComputedValues`) for `id`, or `None` when
/// the cascade has no data (hand-rolled style fixtures, text nodes). A cheap
/// refcount bump on the cascade's `Arc`. The decoration helpers below read a
/// `&ComputedValues` directly (so a box-tree-driven walk can hand them
/// `node.style`); this is the DOM-keyed resolution at their call sites.
pub(crate) fn primary_cv<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<ServoArc<ComputedValues>> {
    styles
        .get(id)
        .and_then(|e| e.borrow_data().map(|d| d.styles.primary().clone()))
}

/// Per-node scroll offsets (device px), keyed by DOM node. An overflow
/// container with an entry here has its clipped content translated by
/// `-offset` during emit, so it scrolls. The host owns this map (updated from
/// wheel input); an empty map scrolls nothing.
pub type ScrollOffsets<NodeId> = FxHashMap<NodeId, (f32, f32)>;

/// Namespace for the font-instance keys this producer mints. Keys are
/// unique within one paint list; the namespace just disambiguates them
/// from other `FontInstanceKey` sources if they ever share a registry.
const SERVAL_FONT_NAMESPACE: IdNamespace = IdNamespace(0);

/// Namespace for the image keys this producer mints.
const SERVAL_IMAGE_NAMESPACE: IdNamespace = IdNamespace(1);

/// Serval's concrete [`PaintList`] impl. Built by [`emit_paint_list`].
///
/// No `MallocSizeOf` derive: the paint vocabulary (`paint_list_api`)
/// moved to the neutral netrender workspace and dropped its dependency
/// on servo's `malloc_size_of` (extraction plan 2026-05-20). This list
/// is a transient per-frame value converted straight to a
/// `PaintEnvelope` and sent; it is not retained in a size-reported
/// structure.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ServalPaintList {
    viewport: DeviceIntSize,
    commands: Vec<PaintCmd>,
    generation: u64,
    fonts: Vec<FontResource>,
    images: Vec<ImageResource>,
}

impl ServalPaintList {
    /// Construct an empty paint list. Mainly used by tests.
    pub fn new(viewport: DeviceIntSize) -> Self {
        Self {
            viewport,
            commands: Vec::new(),
            generation: 0,
            fonts: Vec::new(),
            images: Vec::new(),
        }
    }

    /// Append a filled rect at an absolute scene position. The shared primitive
    /// behind the host overlays ([`push_caret`](Self::push_caret) /
    /// [`push_selection`](Self::push_selection) / a scrollbar thumb): pushed
    /// *after* the emit walk, which balances its `PushTransform` stack back to
    /// identity, so absolute scene coordinates place it with no active transform.
    pub fn push_fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: ColorF) {
        self.commands.push(PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(LayoutRect::new(
                LayoutPoint::new(x, y),
                LayoutPoint::new(x + w, y + h),
            )),
            color,
        }));
    }

    /// Append a text caret as a filled bar at its absolute position. A host
    /// overlays the focused field's caret this way (from
    /// [`caret_rect`](crate::caret::caret_rect)).
    pub fn push_caret(&mut self, caret: crate::caret::CaretRect, color: ColorF) {
        self.push_fill(caret.x, caret.y, caret.width, caret.height, color);
    }

    /// Append selection-highlight rects (from
    /// [`selection_rects`](crate::caret::selection_rects)) as filled rects. Use a
    /// translucent `color`: appended after the emit walk, the highlight draws
    /// *over* the text, so the text shows through. Push the selection before the
    /// caret so the caret sits on top.
    pub fn push_selection(&mut self, rects: &[crate::caret::CaretRect], color: ColorF) {
        for r in rects {
            self.push_fill(r.x, r.y, r.width, r.height, color);
        }
    }

    /// Compose another paint list (a satellite subtree) at `origin` in this
    /// list's coordinate space: wrap `sub`'s commands in a single
    /// `PushTransform(origin)` / `PopTransform` so its local coordinates land at
    /// `origin`, and merge its font/image side-tables so text and images in the
    /// satellite resolve. The overlay-roots "overlay slot" primitive — pushed
    /// *after* the emit walk (identity transform), so the satellite paints in
    /// top-layer order over every page stacking context.
    ///
    /// The merge is index-free: `DrawText` / `DrawImage` reference resources by
    /// **key** (`FontInstanceKey` / `ImageKey`), not by a Vec index, so the
    /// satellite's commands stay valid verbatim; only unseen resources are
    /// appended (dedup by key, since the same face may already be in this list).
    pub fn push_sublist(&mut self, origin: LayoutPoint, sub: &ServalPaintList) {
        self.commands.push(PaintCmd::PushTransform(TransformSpec {
            origin,
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
        self.commands.extend(sub.commands.iter().cloned());
        self.commands.push(PaintCmd::PopTransform);
        for font in &sub.fonts {
            if !self.fonts.iter().any(|f| f.key == font.key) {
                self.fonts.push(font.clone());
            }
        }
        for image in &sub.images {
            if !self.images.iter().any(|i| i.key == image.key) {
                self.images.push(image.clone());
            }
        }
    }
}

/// Scrollbar thumb colour (translucent dark grey, on the container's right edge).
pub const SCROLLBAR_COLOR: ColorF = ColorF {
    r: 0.30,
    g: 0.30,
    b: 0.36,
    a: 0.65,
};
/// Scrollbar thumb width, device px.
pub const SCROLLBAR_WIDTH: f32 = 8.0;

/// Append a scrollbar thumb onto `plist` for each scrolled container in `scroll_offsets`: a bar
/// on the box's right edge, height ∝ visible/content, position ∝ offset/scrollable. The thumb
/// sits at the container's **absolute** (document-space, unscrolled) origin via the shared
/// [`accumulate_origins`](crate::accumulate_origins) walk (one O(n) pass, then a map lookup per
/// scroller), so it is placed correctly for a scroll container nested inside a positioned
/// ancestor, not only a top-level one.
///
/// The caller decides what `scroll_offsets` holds: a host folds its own host-scroll and the
/// retained `element_scroll` into one map; the engine's session path passes its offsets. Shared
/// by serval-render's chrome paths and the meerkat host, so every scrollbar is drawn from the one
/// fragment-geometry formula. (Upstreaming P2 — was a host copy plus a serval-render copy.)
pub fn push_scrollbars<D>(
    plist: &mut ServalPaintList,
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if scroll_offsets.is_empty() {
        return;
    }
    let origins = crate::serval_lane::accumulate_origins(dom, fragments);
    for (&node, &(_ox, oy)) in scroll_offsets {
        let Some(r) = fragments.rect_of(node) else {
            continue;
        };
        let inner_h =
            r.size.height - r.padding.top - r.padding.bottom - r.border.top - r.border.bottom;
        let content_h = r.content_size.height;
        let scrollable = content_h - inner_h;
        if scrollable <= 0.5 {
            continue;
        }
        // The container's absolute top-left (taffy locations are parent-relative), so a nested
        // scroller's bar lands on its real right edge, not at `container_width` from the document
        // left.
        let Some(p) = origins.get(&node) else {
            continue;
        };
        let thumb_h = (r.size.height * (inner_h / content_h)).max(24.0);
        let thumb_y = p.y + (oy / scrollable) * (r.size.height - thumb_h);
        let thumb_x = p.x + r.size.width - SCROLLBAR_WIDTH;
        plist.push_fill(thumb_x, thumb_y, SCROLLBAR_WIDTH, thumb_h, SCROLLBAR_COLOR);
    }
}

impl PaintList for ServalPaintList {
    fn engine_id(&self) -> EngineId {
        EngineId::SERVAL
    }
    fn viewport(&self) -> DeviceIntSize {
        self.viewport
    }
    fn generation_id(&self) -> u64 {
        self.generation
    }
    fn commands(&self) -> &[PaintCmd] {
        &self.commands
    }
    fn fonts(&self) -> &[FontResource] {
        &self.fonts
    }
    fn images(&self) -> &[ImageResource] {
        &self.images
    }
}

/// Dedups fonts referenced by glyph runs and assigns each a
/// [`FontInstanceKey`]. Keyed by parley's blob id (stable per font
/// file), so a font shared across many runs ships its bytes once.
#[derive(Default)]
struct FontCollector {
    fonts: Vec<FontResource>,
    by_blob: FxHashMap<u64, FontInstanceKey>,
}

/// Process-global font registry, keyed by font-file content.
///
/// Two jobs (P0 receipts, shell paint emission plan 2026-07-03): the same face
/// used to be `to_vec()`ed out of parley into EVERY emitted list — a per-frame
/// multi-megabyte memcpy for a symbol/emoji face — and keys were minted 0,1,2…
/// per list, so the renderer saw the "same" key carry different faces across
/// lists and could never cache (the images side hit the identical poisoning
/// and fixed it with globally-unique keys; this is the font twin). The registry
/// copies each face once per process, wraps it in the `Arc` the resource type
/// now carries, and mints its key from a global counter so key → bytes is
/// stable for the whole process — the invariant the renderer's blob cache and
/// any retained/spliced paint list both need.
///
/// Keys are CONTENT identity, not parley blob id: `FontContext::new()` runs
/// per layout session, so the same system face surfaces under a fresh blob id
/// after every session rebuild — a blob-id-keyed registry would pin a new copy
/// of each face per rebuild. `by_blob_id` in front memoizes the (cheap) id →
/// key step so the content hash is paid once per blob id, not once per emit.
struct FontRegistry {
    by_blob_id: FxHashMap<u64, FontInstanceKey>,
    /// ([`font_content_id`], byte length, TTC index) → the shared resource.
    by_content: FxHashMap<(u64, usize, u32), FontResource>,
    by_key: FxHashMap<FontInstanceKey, FontResource>,
}

static FONT_REGISTRY: std::sync::Mutex<Option<FontRegistry>> = std::sync::Mutex::new(None);
static NEXT_FONT_KEY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl FontCollector {
    /// Intern a parley `FontData`, returning the key the matching
    /// `TextRunItem::font_instance` should carry. Adds the registry's shared
    /// [`FontResource`] (an `Arc` bump, not a byte copy) on first sight of a
    /// blob within this list.
    fn intern(&mut self, font: &parley::FontData) -> FontInstanceKey {
        let blob_id = font.data.id();
        if let Some(k) = self.by_blob.get(&blob_id) {
            return *k;
        }
        let mut guard = FONT_REGISTRY.lock().expect("font registry poisoned");
        let registry = guard.get_or_insert_with(|| FontRegistry {
            by_blob_id: FxHashMap::default(),
            by_content: FxHashMap::default(),
            by_key: FxHashMap::default(),
        });
        let key = match registry.by_blob_id.get(&blob_id) {
            Some(k) => *k,
            None => {
                let bytes = font.data.data();
                let content = (font_content_id(bytes), bytes.len(), font.index);
                let resource = registry
                    .by_content
                    .entry(content)
                    .or_insert_with(|| FontResource {
                        key: FontInstanceKey::new(
                            SERVAL_FONT_NAMESPACE,
                            NEXT_FONT_KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                        ),
                        data: std::sync::Arc::new(bytes.to_vec()),
                        index: font.index,
                    });
                let key = resource.key;
                let resource = resource.clone();
                registry.by_key.entry(key).or_insert(resource);
                registry.by_blob_id.insert(blob_id, key);
                key
            },
        };
        let resource = registry.by_key[&key].clone();
        drop(guard);
        self.by_blob.insert(blob_id, key);
        self.fonts.push(resource);
        key
    }
}

/// Content identity for a font file: FxHash over three 16KB windows (head /
/// middle / tail), always paired with the exact byte length (and TTC index) in
/// the registry key. Paid once per new parley blob id — a session rebuild
/// re-surfaces every face under fresh blob ids, so this runs per rebuild and
/// must stay cheap (a full-file hash of a multi-MB emoji face measured ~0.6s
/// per rebuild frame in a debug build). Two real, different font files that
/// agree on length AND all three windows do not occur in practice; a false
/// merge would mis-render glyphs, never corrupt memory.
fn font_content_id(bytes: &[u8]) -> u64 {
    use std::hash::Hasher as _;
    const WINDOW: usize = 16 * 1024;
    let mut h = rustc_hash::FxHasher::default();
    if bytes.len() <= 3 * WINDOW {
        h.write(bytes);
    } else {
        let mid = bytes.len() / 2;
        h.write(&bytes[..WINDOW]);
        h.write(&bytes[mid - WINDOW / 2..mid + WINDOW / 2]);
        h.write(&bytes[bytes.len() - WINDOW..]);
    }
    h.finish()
}

/// Collects `<img>` images into the paint list's image side-table,
/// assigning each an [`ImageKey`]. One key per `<img>` element for the
/// probe (no cross-element src dedup yet).
/// Process-global image-key counter. Image keys must be unique across
/// the *renderer's* lifetime, not just within one paint list: the
/// netrender tile rasterizer caches decoded images by key across renders
/// and `debug_assert`s that a re-encountered key carries identical bytes
/// (`vello_tile_rasterizer`). A per-list counter restarting at 0 made
/// every list's first image collide on `ImageKey(_, 0)` with different
/// bytes, poisoning the rasterizer lock. Minting from a monotonic global
/// counter guarantees no cross-list collision. (Identical images across
/// lists get distinct keys — a small cache-churn cost, bounded by the
/// rasterizer's own LRU; within-list dedup is a possible follow-up.)
static NEXT_IMAGE_KEY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[derive(Default)]
struct ImageCollector {
    images: Vec<ImageResource>,
}

impl ImageCollector {
    /// Add a decoded image, returning the key the matching
    /// `ImageItem::image_key` should carry. The key is drawn from a
    /// process-global counter so it never collides with a key from a
    /// prior paint list (see [`NEXT_IMAGE_KEY`]).
    fn add(&mut self, decoded: &DecodedImage) -> ImageKey {
        let idx = NEXT_IMAGE_KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let key = ImageKey::new(SERVAL_IMAGE_NAMESPACE, idx);
        self.images.push(ImageResource {
            key,
            width: decoded.width,
            height: decoded.height,
            data: decoded.rgba.clone(),
        });
        key
    }
}

/// Bundles the per-emission mutable collectors + immutable resource
/// sources, so the recursive `walk` takes one `&mut Emitter` rather
/// than a half-dozen separate parameters.
pub(crate) struct Emitter<'a, NodeId: Copy + Eq + Hash> {
    /// Cascaded styles keyed by NodeId. Block boxes carry their own cloned
    /// style, but inline replaced boxes need to look up their source element.
    styles: &'a StylePlane<NodeId>,
    /// Decoded `<img>` images keyed by NodeId.
    images_plane: &'a ImagePlane<NodeId>,
    /// Decoded CSS `background-image`s keyed by NodeId.
    bg_images_plane: &'a BackgroundImagePlane<NodeId>,
    /// Per-node scroll offsets (device px). A clipping (overflow) container with
    /// an entry here has its clipped content translated by `-offset`, so the
    /// container scrolls. Empty ⇒ nothing scrolls.
    scroll_offsets: &'a FxHashMap<NodeId, (f32, f32)>,
    fonts: FontCollector,
    images: ImageCollector,
    /// The element whose background was propagated to the canvas (the root, or
    /// the body when the root's background is transparent — CSS Backgrounds-3
    /// §root-background). That element must *not* paint the background on its
    /// own box: it has been lifted to the canvas. `None` ⇒ no propagation.
    canvas_bg_source: Option<NodeId>,
    /// The document (viewport) scroll offset in device px (CSS Overflow §3.3,
    /// scope doc rule 2). The whole document paints translated by `-offset` inside
    /// the canvas; the canvas background ([`emit_canvas_background`], emitted before
    /// the wrap) and `position: fixed` layers (which counter it in
    /// [`crate::paint_stacking::paint_layer`]) are exempt. `(0.0, 0.0)` ⇒ the
    /// document does not scroll (the default the public entry points pass until the
    /// host wires the offset).
    viewport_scroll: (f32, f32),
    /// Whole subtrees keyed by their source DOM id that this emit pass must skip.
    /// Used by shell partitioning: a host can paint the base document without the
    /// high-churn pane roots, then emit those pane roots separately off the same
    /// retained layout.
    skipped_subtrees: Option<&'a FxHashSet<NodeId>>,
    /// Host source of chisel leaves' Path-A commands, keyed by leaf `key`.
    /// `None` on every path with no leaves (the overwhelming common case).
    leaves: Option<&'a dyn LeafPaintSource>,
}

impl<NodeId: Copy + Eq + Hash> Emitter<'_, NodeId> {
    /// The document (viewport) scroll offset, so the stacking painter can
    /// counter-translate `position: fixed` layers back to the viewport
    /// ([`crate::paint_stacking::paint_layer`]).
    pub(crate) fn viewport_scroll(&self) -> (f32, f32) {
        self.viewport_scroll
    }
}

/// Walk the DOM in pre-order, emitting paint commands for each
/// element + text leaf with a fragment. Coordinates are absolute
/// (parent-relative `taffy::Layout.location` accumulated through the
/// recursion). Element background colors come from
/// `ComputedValues::background_color` when the cascade has populated
/// `ElementData`; otherwise default to transparent.
///
/// Text glyph runs come from the `TextMeasureCtx`'s cached parley
/// `Layout`s (populated by `crate::layout::layout` via
/// `measure_text_leaf`); pass `None` for `text_ctx` to emit text
/// items without glyph data (probe-quality empty glyph runs — useful
/// when caller hasn't run layout yet, or wants to skip text shaping).
pub fn emit_paint_list<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let empty_images = ImagePlane::new();
    let empty_bg = BackgroundImagePlane::new();
    let no_scroll: FxHashMap<D::NodeId, (f32, f32)> = FxHashMap::default();
    // Cache-less: the laid-out box tree drives structure + position + style, but
    // `text_ctx` is `None`, so text leaves emit empty (un-shaped) glyph runs. No
    // document scroll on this path — the host wires the viewport offset through the
    // live pipeline.
    emit_inner(
        dom,
        styles,
        fragments,
        constructed,
        None,
        &empty_images,
        &empty_bg,
        &no_scroll,
        viewport,
        (0.0, 0.0),
        (0.0, 0.0),
        constructed.root_arena(),
        None,
    )
}

/// Variant of [`emit_paint_list`] that consumes the cached text
/// layouts + decoded images. `constructed` provides the DOM → Taffy
/// id mapping; `text_ctx` provides the cached parley `Layout`s;
/// `images` provides decoded `<img>` pixels. With these, `DrawText`
/// items carry shaped glyph runs + a font side-table, and `<img>`
/// elements emit `DrawImage` + an image side-table. `bg_images`
/// provides decoded CSS `background-image` pixels, emitted as a
/// `DrawRepeatingImage` (CSS default `background-repeat: repeat`).
pub fn emit_paint_list_with_layouts<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    images: &ImagePlane<D::NodeId>,
    bg_images: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_inner(
        dom,
        styles,
        fragments,
        constructed,
        Some(text_ctx),
        images,
        bg_images,
        scroll_offsets,
        viewport,
        // No document scroll on the direct entry point: callers that scroll the
        // document use `emit_paint_list_scrolled`. `(0,0)` reproduces today's
        // behavior exactly (no wrap, no fixed counter).
        (0.0, 0.0),
        (0.0, 0.0),
        constructed.root_arena(),
        None,
    )
}

/// Like [`emit_paint_list_with_layouts`] but with a document (viewport) scroll
/// offset: in-flow content paints translated by `-viewport_scroll` (CSS Overflow
/// §3.3, the document scroll), `position: fixed` stays pinned to the viewport, and
/// the canvas background does not move. [`emit_paint_list_with_layouts`] is the
/// `(0.0, 0.0)` (unscrolled) case. An [`IncrementalLayout`](crate::IncrementalLayout)
/// session supplies the offset from its viewport; a stateless host (pelt's static
/// viewer) supplies its own.
#[allow(clippy::too_many_arguments)]
pub fn emit_paint_list_scrolled<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    images: &ImagePlane<D::NodeId>,
    bg_images: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
    viewport: DeviceIntSize,
    viewport_scroll: (f32, f32),
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_inner(
        dom,
        styles,
        fragments,
        constructed,
        Some(text_ctx),
        images,
        bg_images,
        scroll_offsets,
        viewport,
        viewport_scroll,
        (0.0, 0.0),
        constructed.root_arena(),
        None,
    )
}

/// [`emit_paint_list_scrolled`] plus a chisel [`LeafPaintSource`]: the session
/// (retained-layout) counterpart of [`emit_paint_list_with_leaves`], carrying the
/// document scroll the session paths paint at.
#[allow(clippy::too_many_arguments)]
pub fn emit_paint_list_scrolled_with_leaves<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    images: &ImagePlane<D::NodeId>,
    bg_images: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
    viewport: DeviceIntSize,
    viewport_scroll: (f32, f32),
    leaves: &dyn LeafPaintSource,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_inner_with_leaves(
        dom,
        styles,
        fragments,
        constructed,
        Some(text_ctx),
        images,
        bg_images,
        scroll_offsets,
        viewport,
        viewport_scroll,
        (0.0, 0.0),
        constructed.root_arena(),
        None,
        Some(leaves),
    )
}

/// Like [`emit_paint_list_scrolled`] but skips any subtree whose root DOM node id
/// appears in `skipped_subtrees`. Intended for coarse retained shell partitioning:
/// emit the shell base without the churn-heavy pane roots, then emit those roots
/// separately from the same retained layout.
#[allow(clippy::too_many_arguments)]
pub fn emit_paint_list_scrolled_excluding_subtrees<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    images: &ImagePlane<D::NodeId>,
    bg_images: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
    skipped_subtrees: &FxHashSet<D::NodeId>,
    viewport: DeviceIntSize,
    viewport_scroll: (f32, f32),
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_inner(
        dom,
        styles,
        fragments,
        constructed,
        Some(text_ctx),
        images,
        bg_images,
        scroll_offsets,
        viewport,
        viewport_scroll,
        (0.0, 0.0),
        constructed.root_arena(),
        Some(skipped_subtrees),
    )
}

/// Emit one subtree rooted at `root` into a local coordinate space whose origin is
/// the subtree root's own border-box top-left. This is the companion to
/// [`emit_paint_list_scrolled_excluding_subtrees`] for shell partitioning: a host
/// emits the base shell without a pane root, then emits that pane root separately and
/// composites it at the root's laid-out rect.
///
/// Boundary: this is for roots that can be composited back as an axis-aligned
/// rectangle. Ancestor transforms above `root` are intentionally not re-applied here;
/// callers should use it for top-level pane roots such as Meerkat's shell children,
/// not arbitrary transformed descendants.
#[allow(clippy::too_many_arguments)]
pub fn emit_subtree_paint_list_scrolled<D>(
    dom: &D,
    root: D::NodeId,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    images: &ImagePlane<D::NodeId>,
    bg_images: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
    viewport: DeviceIntSize,
) -> Option<ServalPaintList>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let arena = constructed.arena_of(root)?;
    let root_layout = constructed.node(arena).final_layout;
    Some(emit_inner(
        dom,
        styles,
        fragments,
        constructed,
        Some(text_ctx),
        images,
        bg_images,
        scroll_offsets,
        viewport,
        (0.0, 0.0),
        // `walk` folds the context root's own `final_layout.location` into its first
        // push when `is_root`, so local subtree emit must cancel that offset back out.
        // Descendants then paint in the root-local coordinate space the host will
        // composite at the pane's laid-out rect.
        (-root_layout.location.x, -root_layout.location.y),
        arena,
        None,
    ))
}

/// Variant of [`emit_paint_list_with_layouts`] that also splices chisel Path-A
/// leaves' commands from `leaves` (see [`LeafPaintSource`]). The host owns the
/// leaf registry, runs each dirty leaf's paint into a buffer, and passes the
/// buffer source here; the leaf's box is a `<chisel-leaf key="…">` replaced
/// element. Zero document scroll, matching `emit_paint_list_with_layouts`.
#[allow(clippy::too_many_arguments)]
pub fn emit_paint_list_with_leaves<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    images: &ImagePlane<D::NodeId>,
    bg_images: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &ScrollOffsets<D::NodeId>,
    viewport: DeviceIntSize,
    leaves: &dyn LeafPaintSource,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_inner_with_leaves(
        dom,
        styles,
        fragments,
        constructed,
        Some(text_ctx),
        images,
        bg_images,
        scroll_offsets,
        viewport,
        (0.0, 0.0),
        (0.0, 0.0),
        constructed.root_arena(),
        None,
        Some(leaves),
    )
}

/// Forwarding wrapper: the historical `emit_inner` with no leaf source. Every
/// existing entry point calls this, so their call sites are unchanged; only
/// [`emit_paint_list_with_leaves`] supplies leaves.
#[allow(clippy::too_many_arguments)]
fn emit_inner<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: Option<&TextMeasureCtx>,
    images_plane: &ImagePlane<D::NodeId>,
    bg_images_plane: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &FxHashMap<D::NodeId, (f32, f32)>,
    viewport: DeviceIntSize,
    viewport_scroll: (f32, f32),
    root_origin: (f32, f32),
    root_arena: usize,
    skipped_subtrees: Option<&FxHashSet<D::NodeId>>,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    emit_inner_with_leaves(
        dom,
        styles,
        fragments,
        constructed,
        text_ctx,
        images_plane,
        bg_images_plane,
        scroll_offsets,
        viewport,
        viewport_scroll,
        root_origin,
        root_arena,
        skipped_subtrees,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_inner_with_leaves<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    constructed: &BoxTree<D::NodeId>,
    text_ctx: Option<&TextMeasureCtx>,
    images_plane: &ImagePlane<D::NodeId>,
    bg_images_plane: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &FxHashMap<D::NodeId, (f32, f32)>,
    viewport: DeviceIntSize,
    viewport_scroll: (f32, f32),
    root_origin: (f32, f32),
    root_arena: usize,
    skipped_subtrees: Option<&FxHashSet<D::NodeId>>,
    leaves: Option<&dyn LeafPaintSource>,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut commands = Vec::new();
    let mut emitter = Emitter {
        styles,
        images_plane,
        bg_images_plane,
        scroll_offsets,
        fonts: FontCollector::default(),
        images: ImageCollector::default(),
        canvas_bg_source: None,
        viewport_scroll,
        skipped_subtrees,
        leaves,
    };
    // CSS Backgrounds-3 §root-background: the root element's background (or, when
    // the root is transparent, the body's) is painted over the *entire* canvas,
    // not just the element's own box, positioned against the root's background
    // positioning area. Emitted first (behind all content); the source element is
    // then recorded so `walk` suppresses the duplicate paint on its own box. This
    // stays DOM-driven (it is about the real root / body elements). It paints
    // *before* the document-scroll wrap below, so the canvas background never
    // scrolls (CSS Overflow §3.3: the canvas is the viewport's, fixed in place).
    emitter.canvas_bg_source =
        emit_canvas_background(dom, styles, fragments, viewport, &mut commands);
    // Document (viewport) scroll: the whole document paints translated by
    // `-viewport_scroll` inside the canvas (scope doc rule 2). `position: fixed`
    // layers counter this translate back to the viewport in
    // `paint_stacking::paint_layer` (the Fixed≠Absolute distinction), so they stay
    // pinned while in-flow + absolute content scrolls under them. Skip the wrap at
    // the origin (the overwhelmingly common, unscrolled frame) so the command
    // stream is byte-identical to the pre-scroll engine there.
    let (sx, sy) = viewport_scroll;
    let document_scrolled = sx != 0.0 || sy != 0.0;
    if document_scrolled {
        commands.push(PaintCmd::PushTransform(TransformSpec {
            origin: LayoutPoint::new(-sx, -sy),
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
    }
    // Paint the document as the root stacking context, driven by the box-tree
    // arena. The recursive painter (crate::paint_stacking) walks each context's
    // own subtree for in-flow content, collects its positioned/z-index layers, and
    // orders them per CSS 2.1 Appendix E (negative-z behind, then in-flow, then
    // zero/positive on top), scoped to that context.
    crate::paint_stacking::paint_context(
        &mut emitter,
        constructed,
        text_ctx,
        root_arena,
        root_origin,
        &mut commands,
    );
    if document_scrolled {
        commands.push(PaintCmd::PopTransform);
    }
    ServalPaintList {
        viewport,
        commands,
        generation: 0,
        fonts: emitter.fonts.fonts,
        images: emitter.images.images,
    }
}

/// A positioned / z-index element lifted out of its context's in-flow walk into
/// a stacking layer (crate::paint_stacking orders the layers per Appendix E).
/// `origin` is the parent's accumulated absolute origin (where the layer's
/// parent-relative location is measured from); `z` is its paint-bucket z-index
/// (`auto` → 0); `seq` is document order (the z tiebreak).
pub(crate) struct Deferred {
    /// The lifted layer's box-tree arena index (the painter re-enters `walk` here).
    pub(crate) node: usize,
    pub(crate) origin: (f32, f32),
    pub(crate) z: i32,
    pub(crate) seq: usize,
    /// The cumulative CSS `transform` of this layer's transform-bearing ancestors
    /// *within its stacking context* (identity if none). A lifted layer is painted
    /// on a clean stack at its absolute layout `origin`, which captures ancestor
    /// *layout* offsets but not ancestor `transform` matrices — so without this an
    /// abs-pos child of a `transform`ed element (e.g. the orrery's camera
    /// container) would paint untransformed. `paint_context` re-establishes this
    /// matrix around the layer. Built by [`walk`] as a product of each transformed
    /// ancestor's matrix conjugated by its absolute origin
    /// (`T(O)·M·T(-O)`), which telescopes exactly through the layout-absolute
    /// origins, so it is correct for nested transforms too.
    pub(crate) ancestor_transform: LayoutTransform,
    /// Whether this layer attaches to the viewport (`position: fixed`): it is
    /// counter-translated by the document scroll in
    /// [`crate::paint_stacking::paint_layer`] so it stays pinned. `false` for
    /// `absolute` / z-index layers, which scroll with the document.
    pub(crate) attaches_to_viewport: bool,
}

/// Whether `id` is out of normal flow (`position: absolute`/`fixed`). Out-of-flow
/// elements are always lifted into a stacking layer; in-flow positioned elements
/// (`relative`/`sticky`) are lifted only when they carry an explicit `z-index`
/// (see [`crate::paint_stacking::defers_to_stacking`]).
pub(crate) fn is_out_of_flow(cv: &ComputedValues) -> bool {
    use style::values::computed::PositionProperty;
    matches!(
        cv.get_box().position,
        PositionProperty::Absolute | PositionProperty::Fixed
    )
}

/// Whether `id` is `position: fixed`. A fixed box attaches to the viewport: its
/// stacking layer counters the document scroll so it stays pinned while in-flow
/// and `absolute` content scrolls under it (CSS Position §6, scope doc rule 3 —
/// the Fixed≠Absolute distinction). Recorded on its [`Deferred`] for
/// [`crate::paint_stacking::paint_layer`].
pub(crate) fn is_fixed(cv: &ComputedValues) -> bool {
    use style::values::computed::PositionProperty;
    matches!(cv.get_box().position, PositionProperty::Fixed)
}

/// Whether the box computes `pointer-events: none` — it is not a hit-test target
/// (CSS-UI). The property inherits, so the per-box computed value already encodes the
/// cascade: a descendant that re-sets `pointer-events: auto` computes `Auto` and stays
/// hittable, which is exactly the non-blanket descendant rule (blanket suppression is
/// only for frame/iframe contents and `inert`, which serval does not model here). Read
/// by the hit walk to skip recording such boxes while still descending into them.
pub(crate) fn pointer_events_none(cv: &ComputedValues) -> bool {
    use style::values::computed::ui::PointerEvents;
    matches!(cv.clone_pointer_events(), PointerEvents::None)
}

/// Recursive paint-order walk emitting compositor-model commands:
///
/// For each node with a fragment:
///   1. `PushTransform` with the fragment's local origin (its
///      `taffy::Layout.location`, which is parent-relative).
///   2. The node's own paint primitive (`DrawRect` for elements, one
///      `DrawText` per parley glyph-run for text leaves), in local
///      `(0, 0, w, h)` coords.
///   3. Recurse into children — their `PushTransform` origins compose
///      with the active transform stack.
///   4. `PopTransform` matching the push.
///
/// Nodes without fragments (synthetic / skipped) don't push or pop,
/// but children still descend in the current coord space.
pub(crate) fn walk<Id>(
    em: &mut Emitter<'_, Id>,
    tree: &BoxTree<Id>,
    text_ctx: Option<&TextMeasureCtx>,
    arena: usize,
    origin: (f32, f32),
    commands: &mut Vec<PaintCmd>,
    deferred: &mut Vec<Deferred>,
    is_root: bool,
    // Cumulative CSS transform of transform-bearing ancestors within this stacking
    // context (identity at the context root). Read only when a descendant defers,
    // to record on its `Deferred` so the stacking painter re-establishes ancestor
    // transforms around it (see `Deferred::ancestor_transform`). In-flow content
    // does not use it — the per-node `PushTransform` chain already carries the
    // transform for content painted in place.
    ancestor_transform: LayoutTransform,
    // The accumulated scroll offset (device px) of this node's *scroll-container* ancestors
    // within the current stacking context. In-flow content scrolls via the `-offset` paint
    // transform pushed per container (below); a deferred positioned/absolute layer is painted
    // on a clean stack by the stacking painter instead — never riding that transform — so it
    // folds this into its recorded `origin` to scroll *with* its container rather than staying
    // pinned. The scroll twin of `ancestor_transform`. (Absolute-in-scroll fix.)
    accumulated_scroll: (f32, f32),
) where
    Id: Copy + Eq + Hash,
{
    let node = tree.node(arena);
    let cv: &ComputedValues = &node.style;
    let dom_id = node.source.dom_id();

    if em
        .skipped_subtrees
        .is_some_and(|skipped| skipped.contains(&dom_id))
        && !is_root
    {
        return;
    }

    // A positioned / z-index descendant is lifted out of this context's in-flow
    // walk into a stacking layer (recorded with its parent's absolute origin +
    // paint-bucket z, skipped here); the recursive stacking painter places it.
    // `is_root` is the one node we always emit — the context root the painter
    // entered on, which would otherwise re-defer itself into an infinite loop.
    if !is_root && crate::paint_stacking::defers_to_stacking(cv) {
        deferred.push(Deferred {
            node: arena,
            // Fold in the scroll of this layer's scroll-container ancestors, so it scrolls
            // with its container instead of staying at the un-scrolled origin (the stacking
            // painter paints it on a clean stack, off the `-offset` transform). (Abs-in-scroll.)
            origin: (
                origin.0 - accumulated_scroll.0,
                origin.1 - accumulated_scroll.1,
            ),
            z: crate::paint_stacking::bucket_z(cv),
            seq: deferred.len(),
            ancestor_transform,
            attaches_to_viewport: is_fixed(cv),
        });
        return;
    }

    // `display: none` paints nothing — skip the node and its whole subtree (the
    // box is built but laid out hidden; its marker would otherwise still hang).
    if cv.get_box().display.is_none() {
        return;
    }

    // Every box has a laid-out position + size (its `final_layout`); style + DOM
    // identity come off the box node, not a DOM lookup. `dom_id` keys the
    // remaining DOM-keyed concerns (scroll / images / canvas-bg).
    let l = node.final_layout;
    let is_anon = node.source.is_anonymous();
    let taffy_id = tree.arena_node_id(arena);

    // An overflow container clips its descendants to its padding box; captured
    // here and applied around the children below.
    let mut clip_rect: Option<LayoutRect> = None;
    // Children push their own parent-relative location, composing with this node's
    // transform. The context root has no enclosing transform on the stack (the
    // stacking painter emits each layer on a clean stack), so it folds its absolute
    // `origin` into its own push — its body is then absolute without an extra
    // wrapper transform.
    let push_origin = if is_root {
        LayoutPoint::new(origin.0 + l.location.x, origin.1 + l.location.y)
    } else {
        LayoutPoint::new(l.location.x, l.location.y)
    };
    // Fold the element's computed CSS transform into its push, so a
    // `transform: translate(x,y)` moves the painted node (the orrery's per-frame
    // motion). `origin` is the box-model position; `transform` is the CSS
    // transform layered on top.
    let node_transform = compute_transform_matrix(cv);
    // `opacity < 1` wraps the element + its in-flow subtree in an isolated
    // stacking layer the renderer composites at `alpha` (group opacity — see
    // `opacity_of`). The layer is the outermost wrapper, around the node's own
    // transform, so a transformed/positioned element fades as one unit. Anonymous
    // boxes carry no own style, so they never open a layer. (Positioned
    // descendants lifted into sibling stacking layers escape this group — a
    // documented edge, like the Appendix E deviations in `paint_stacking`.)
    let opacity = (!is_anon).then(|| opacity_of(cv)).flatten();
    let blend = (!is_anon).then(|| mix_blend_mode_of(cv)).flatten();
    let filters = if is_anon { Vec::new() } else { filters_of(cv) };
    // `opacity < 1`, a non-normal `mix-blend-mode`, or a non-empty `filter`
    // chain each force an isolated stacking layer: opacity composites the
    // subtree once at `alpha`, blend composites it into its backdrop with the
    // given mode, and filter applies to the layer's own rasterized output.
    let needs_layer = opacity.is_some() || blend.is_some() || !filters.is_empty();
    if needs_layer {
        commands.push(PaintCmd::PushLayer(LayerSpec {
            opacity: opacity.unwrap_or(1.0),
            mix_blend_mode: blend.unwrap_or(MixBlendMode::Normal),
            filters,
            ..Default::default()
        }));
    }
    commands.push(PaintCmd::PushTransform(TransformSpec {
        origin: push_origin,
        transform: node_transform,
        kind: TransformKind::Standard,
    }));
    // CSS `clip-path`: clip the whole element (box + content + descendants) to a
    // path, just inside the fragment transform (so the path is in local border-box
    // coords) and outermost of the element's own clips. Balanced before the
    // fragment `PopTransform` below.
    let clip_path = (!is_anon)
        .then(|| clip_path_of(cv, l.size.width, l.size.height))
        .flatten();
    if let Some(ref kind) = clip_path {
        commands.push(PaintCmd::PushClip(ClipSpec { kind: kind.clone() }));
    }
    // Absolute origin to pass to children (this node's origin + its location), so a
    // deferred descendant records where to place itself.
    let child_origin = (origin.0 + l.location.x, origin.1 + l.location.y);
    // Cumulative ancestor transform to pass to children: a CSS transform applies
    // around the element's absolute box top-left, so conjugate it there
    // (`T(O)·M·T(-O)`) and compose onto the inherited transform. Skip the identity
    // (the common no-transform case).
    let child_transform = if node_transform != LayoutTransform::identity() {
        conjugate_at(child_origin, node_transform).then(&ancestor_transform)
    } else {
        ancestor_transform
    };
    let local_bounds = LayoutRect::new(
        LayoutPoint::new(0.0, 0.0),
        LayoutPoint::new(l.size.width, l.size.height),
    );
    // The content-box top-left in local (border-box) coords. Inline content
    // (glyphs, underlines, inline boxes) lays out within the content box, so it is
    // offset by border + padding — matching `caret_rect`'s `content_x`.
    let content_offset = (l.border.left + l.padding.left, l.border.top + l.padding.top);

    // Outset box-shadows paint behind the border-box, so emit them before the
    // background. (Inset shadows paint over the background instead — emitted
    // after it, below.) An anonymous box paints none of its (borrowed-key's)
    // box decorations.
    for shadow in box_shadows_of(cv).into_iter().filter(|_| !is_anon) {
        if shadow.inset {
            continue;
        }
        commands.push(PaintCmd::DrawShadow(ShadowItem {
            placement: CommonPlacement::new(local_bounds),
            box_bounds: local_bounds,
            offset: LayoutVector2D::new(shadow.h, shadow.v),
            color: shadow.color,
            blur_radius: shadow.blur,
            spread_radius: shadow.spread,
            border_radius: BorderRadius::zero(),
            clip_mode: BoxShadowClipMode::Outset,
        }));
    }
    // border-radius: clip the background (color + image) to the rounded border-box.
    let bg_radius = if is_anon {
        None
    } else {
        border_radius_of(cv, local_bounds.width(), local_bounds.height())
    };
    if let Some(radius) = bg_radius {
        commands.push(PaintCmd::PushClip(ClipSpec {
            kind: ClipKind::RoundedRect {
                rect: local_bounds,
                radius,
                clip_out: false,
            },
        }));
    }
    // Background, then replaced content (image), then border — CSS paint order. The
    // element whose background was propagated to the canvas (root / body) skips its
    // own-box background — already painted over the canvas.
    let suppress_bg = is_anon || em.canvas_bg_source == Some(dom_id);
    if !suppress_bg {
        commands.push(PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(local_bounds),
            color: background_color_of(cv),
        }));
        // Background-image gradient layers paint over the color (back-to-front).
        let border = [l.border.left, l.border.top, l.border.right, l.border.bottom];
        let padding = [
            l.padding.left,
            l.padding.top,
            l.padding.right,
            l.padding.bottom,
        ];
        for cmd in background_gradient_layers(cv, local_bounds, border, padding, local_bounds) {
            commands.push(cmd);
        }
    }
    // CSS background-image (url()) paints over the color, under content + border.
    // A pseudo box's image is keyed by `(element, kind)` (no DOM id of its own);
    // every other box keys by its `dom_id`.
    let bg_image = match node.source {
        crate::box_tree::BoxSource::Pseudo(elem, kind) => em.bg_images_plane.get_pseudo(elem, kind),
        _ => em.bg_images_plane.get(dom_id),
    };
    if let Some(decoded) = bg_image.filter(|_| !suppress_bg) {
        let int_w = decoded.width as f32;
        let int_h = decoded.height as f32;
        let key = em.images.add(decoded);
        let bg_rendering = image_rendering_of(cv);
        let bg_style = bg_tile_style_of(cv);
        // The three reference boxes in this node's local (border-box) coords.
        let bw = local_bounds.width();
        let bh = local_bounds.height();
        let box_for = |which: BgBox| -> (f32, f32, f32, f32) {
            match which {
                BgBox::BorderBox => (0.0, 0.0, bw, bh),
                BgBox::PaddingBox => (
                    l.border.left,
                    l.border.top,
                    bw - l.border.left - l.border.right,
                    bh - l.border.top - l.border.bottom,
                ),
                BgBox::ContentBox => (
                    l.border.left + l.padding.left,
                    l.border.top + l.padding.top,
                    bw - l.border.left - l.border.right - l.padding.left - l.padding.right,
                    bh - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom,
                ),
            }
        };
        let origin_box = bg_style
            .as_ref()
            .map(|s| s.origin)
            .unwrap_or(BgBox::PaddingBox);
        let clip_box = bg_style
            .as_ref()
            .map(|s| s.clip)
            .unwrap_or(BgBox::BorderBox);
        let (orx, ory, aw, ah) = box_for(origin_box);
        let (tw, th, ox, oy) = match (&bg_style, int_w > 0.0 && int_h > 0.0) {
            (Some(s), true) => resolve_bg_tile(s, aw, ah, int_w, int_h),
            _ => (int_w, int_h, 0.0, 0.0),
        };
        let (rx, ry) = bg_style
            .as_ref()
            .map(|s| (s.repeat_x, s.repeat_y))
            .unwrap_or((BgRepeat::Repeat, BgRepeat::Repeat));
        let (x0, sw) = match rx {
            BgRepeat::NoRepeat => (ox, tw),
            _ => (0.0, aw),
        };
        let (y0, sh) = match ry {
            BgRepeat::NoRepeat => (oy, th),
            _ => (0.0, ah),
        };
        let (cx, cy, cw, ch) = box_for(clip_box);
        let clip_rect =
            LayoutRect::new(LayoutPoint::new(cx, cy), LayoutPoint::new(cx + cw, cy + ch));
        if tw > 0.0 && th > 0.0 {
            if rx == BgRepeat::NoRepeat && ry == BgRepeat::NoRepeat {
                let tile_rect = LayoutRect::new(
                    LayoutPoint::new(orx + ox, ory + oy),
                    LayoutPoint::new(orx + ox + tw, ory + oy + th),
                );
                if rects_intersect(tile_rect, clip_rect) {
                    if rect_contains(clip_rect, tile_rect) {
                        commands.push(PaintCmd::DrawRepeatingImage(RepeatingImageItem {
                            placement: CommonPlacement::new(tile_rect),
                            image_key: key,
                            stretch_size: LayoutSize::new(tw, th),
                            tile_spacing: LayoutSize::zero(),
                            image_rendering: bg_rendering,
                            alpha_type: AlphaType::PremultipliedAlpha,
                            color: ColorF::WHITE, // identity tint
                        }));
                    } else {
                        commands.push(PaintCmd::PushClip(ClipSpec {
                            kind: ClipKind::Rect(clip_rect),
                        }));
                        commands.push(PaintCmd::DrawImage(ImageItem {
                            placement: CommonPlacement::new(tile_rect),
                            image_key: key,
                            image_rendering: bg_rendering,
                            alpha_type: AlphaType::PremultipliedAlpha,
                            color: ColorF::WHITE, // identity tint
                        }));
                        commands.push(PaintCmd::PopClip);
                    }
                }
            } else {
                let px0 = (orx + x0).max(cx);
                let py0 = (ory + y0).max(cy);
                let px1 = (orx + x0 + sw).min(cx + cw);
                let py1 = (ory + y0 + sh).min(cy + ch);
                if px1 > px0 && py1 > py0 {
                    commands.push(PaintCmd::DrawRepeatingImage(RepeatingImageItem {
                        placement: CommonPlacement::new(LayoutRect::new(
                            LayoutPoint::new(px0, py0),
                            LayoutPoint::new(px1, py1),
                        )),
                        image_key: key,
                        stretch_size: LayoutSize::new(tw, th),
                        tile_spacing: LayoutSize::zero(),
                        image_rendering: bg_rendering,
                        alpha_type: AlphaType::PremultipliedAlpha,
                        color: ColorF::WHITE, // identity tint
                    }));
                }
            }
        }
    }
    // Close the border-radius clip around the background layers.
    if bg_radius.is_some() {
        commands.push(PaintCmd::PopClip);
    }
    // Inset box-shadows paint over the background, clipped to the padding box,
    // under the content + border (CSS Backgrounds-3 paint order). `box_bounds` is
    // the padding box (border box inset by the border widths); the renderer casts
    // the shadow inward from that edge and clips it there.
    if !is_anon {
        let pad = LayoutRect::new(
            LayoutPoint::new(l.border.left, l.border.top),
            LayoutPoint::new(
                l.size.width - l.border.right,
                l.size.height - l.border.bottom,
            ),
        );
        for shadow in box_shadows_of(cv).into_iter().filter(|s| s.inset) {
            commands.push(PaintCmd::DrawShadow(ShadowItem {
                placement: CommonPlacement::new(pad),
                box_bounds: pad,
                offset: LayoutVector2D::new(shadow.h, shadow.v),
                color: shadow.color,
                blur_radius: shadow.blur,
                spread_radius: shadow.spread,
                border_radius: BorderRadius::zero(),
                clip_mode: BoxShadowClipMode::Inset,
            }));
        }
    }
    if let Some(texture_key) = node.external_texture_key {
        if let Some((intrinsic_w, intrinsic_h)) = node.replaced_intrinsic_size {
            emit_object_external_texture(
                cv,
                content_box(&l),
                texture_key,
                intrinsic_w,
                intrinsic_h,
                commands,
            );
        }
    } else if let Some(decoded) = em.images_plane.get(dom_id) {
        emit_object_image(cv, content_box(&l), decoded, &mut em.images, commands);
    } else if let Some(leaf_key) = node.chisel_leaf_key {
        // A chisel Path-A leaf paints its own command stream in place of
        // serval-painted content, in the leaf's local coordinates with (0,0) at
        // the content-box origin — the same content box `<img>` and
        // `<external-texture>` paint into. Offset by `content_offset`
        // (border + padding) so a leaf with CSS border/padding lands correctly.
        // Chained onto the replaced-content `if`/`else if` so a box paints at
        // most one replaced payload.
        if let Some(cmds) = em.leaves.and_then(|src| src.leaf_commands(leaf_key)) {
            let (ox, oy) = content_offset;
            let shift = ox != 0.0 || oy != 0.0;
            if shift {
                commands.push(PaintCmd::PushTransform(TransformSpec {
                    origin: LayoutPoint::new(ox, oy),
                    transform: LayoutTransform::identity(),
                    kind: TransformKind::Standard,
                }));
            }
            commands.extend_from_slice(cmds);
            if shift {
                commands.push(PaintCmd::PopTransform);
            }
        }
    }
    // A loaded `border-image` replaces the normal border (CSS Backgrounds-3 §6):
    // paint the 9-slice and skip the regular border below. Anonymous boxes never
    // carry a border-image.
    let painted_border_image = !is_anon
        && em
            .bg_images_plane
            .get_border_image(dom_id)
            .is_some_and(|src| emit_border_image(cv, src, &l, &mut em.images, commands));
    if !painted_border_image {
        if let Some((widths, normal)) = (!is_anon)
            .then(|| border_of(cv, local_bounds.width(), local_bounds.height()))
            .flatten()
        {
            commands.push(PaintCmd::DrawBorder(BorderItem {
                placement: CommonPlacement::new(local_bounds),
                widths,
                details: BorderDetails::Normal(normal),
            }));
        }
    }
    // A measured leaf (inline formatting context) carries its text + replaced
    // boxes as `InlineContent` — emit its glyph runs and inline-box images. Block
    // boxes have no inline content, so this no-ops.
    // `text-overflow: ellipsis`: pass the content-box width so a single
    // overflowing line is truncated with a `…` (the leaf's ellipsis was shaped at
    // layout time). `None` = no truncation.
    let ellipsis = text_ellipsis(cv)
        .then(|| l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right);
    let emitted = emit_inline_content(
        taffy_id,
        text_ctx,
        node.inline_content.as_ref(),
        local_bounds,
        content_offset,
        ellipsis,
        em.styles,
        em.images_plane,
        &mut em.fonts,
        &mut em.images,
        commands,
    );
    if !emitted && node.inline_content.is_some() {
        // Cache-less path (no shaped layout): emit one empty text run so the
        // command structure still reflects the leaf's text.
        let [r, g, b, a] = *cv
            .get_inherited_text()
            .color
            .into_srgb_legacy()
            .raw_components();
        commands.push(PaintCmd::DrawText(TextRunItem {
            placement: CommonPlacement::new(local_bounds),
            font_instance: FontInstanceKey::default(),
            font_size: 16.0,
            color: ColorF::new(r, g, b, a),
            glyphs: Vec::new(),
            options: TextOptions::default(),
        }));
    }
    // A list item's marker (bullet / ordinal) hangs to the left of its content
    // box; no-op for non-list-items.
    emit_list_marker(taffy_id, text_ctx, content_offset, &mut em.fonts, commands);

    if clips_overflow(cv) {
        clip_rect = Some(LayoutRect::new(
            LayoutPoint::new(l.border.left, l.border.top),
            LayoutPoint::new(
                l.size.width - l.border.right,
                l.size.height - l.border.bottom,
            ),
        ));
    }
    // Clip the descendants of an overflow container to its padding box. The
    // container's own background/border (emitted above) are outside the clip.
    if let Some(rect) = clip_rect {
        commands.push(PaintCmd::PushClip(ClipSpec {
            kind: ClipKind::Rect(rect),
        }));
    }
    // Scroll: inside the clip, translate the content by `-offset` so it scrolls
    // under the fixed clip window. Only a clipping (overflow) container scrolls.
    let scroll = clip_rect.and_then(|_| em.scroll_offsets.get(&dom_id).copied());
    if let Some((ox, oy)) = scroll {
        commands.push(PaintCmd::PushTransform(TransformSpec {
            origin: LayoutPoint::new(-ox, -oy),
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
    }
    // This node's scroll adds to the accumulated scroll its deferred descendants must fold in
    // (in-flow descendants ride the `-offset` transform above instead). (Absolute-in-scroll fix.)
    let child_scroll = match scroll {
        Some((ox, oy)) => (accumulated_scroll.0 + ox, accumulated_scroll.1 + oy),
        None => accumulated_scroll,
    };

    for &child in &node.children {
        walk(
            em,
            tree,
            text_ctx,
            child,
            child_origin,
            commands,
            deferred,
            false,
            child_transform,
            child_scroll,
        );
    }

    // Unwind in reverse: scroll transform, then clip, then the origin transform.
    if scroll.is_some() {
        commands.push(PaintCmd::PopTransform);
    }
    if clip_rect.is_some() {
        commands.push(PaintCmd::PopClip);
    }
    if clip_path.is_some() {
        commands.push(PaintCmd::PopClip);
    }
    commands.push(PaintCmd::PopTransform);
    // Close the stacking layer opened above (opacity and/or mix-blend-mode); the
    // renderer composites the buffered subtree back into the parent accordingly.
    if needs_layer {
        commands.push(PaintCmd::PopLayer);
    }
}

/// Emit a list item's marker, hanging to the left of its content box. The
/// marker `Layout` was shaped after layout into `marker_layouts`; its right edge
/// sits a small gap left of the content-box left edge, top-aligned with the
/// content box (so a same-size first line shares the marker's baseline). No-op
/// for a node with no cached marker layout (every non-list-item).
fn emit_list_marker(
    taffy_id: taffy::NodeId,
    text_ctx: Option<&TextMeasureCtx>,
    content_offset: (f32, f32),
    fonts: &mut FontCollector,
    commands: &mut Vec<PaintCmd>,
) {
    let Some(text_ctx) = text_ctx else {
        return;
    };
    let Some(layout) = text_ctx.marker_layouts.get(&taffy_id) else {
        return;
    };
    let marker_width = layout.width();
    let gap = layout.height() * 0.25;
    let ox = content_offset.0 - gap - marker_width;
    let oy = content_offset.1;
    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(run) = item {
                let parley_run = run.run();
                let key = fonts.intern(parley_run.font());
                let font_size = parley_run.font_size();
                let [r, g, b, a] = run.style().brush.0;
                let glyphs: Vec<GlyphInstance> = run
                    .positioned_glyphs()
                    .map(|gl| GlyphInstance {
                        index: gl.id,
                        point: LayoutPoint::new(ox + gl.x, oy + gl.y),
                    })
                    .collect();
                if glyphs.is_empty() {
                    continue;
                }
                commands.push(PaintCmd::DrawText(TextRunItem {
                    placement: CommonPlacement::new(LayoutRect::new(
                        LayoutPoint::new(ox, oy),
                        LayoutPoint::new(ox + marker_width, oy + layout.height()),
                    )),
                    font_instance: key,
                    font_size,
                    color: ColorF::new(r, g, b, a),
                    glyphs,
                    options: TextOptions::default(),
                }));
            }
        }
    }
}

/// Emit the paint commands for a leaf's inline content from its cached
/// parley `Layout`: one `DrawText` per glyph-run and one `DrawImage`
/// per replaced inline box (`<img>` flowing among the text).
///
/// Each glyph-run is homogeneous in font + size, so it becomes one
/// `TextRunItem` carrying that run's `FontInstanceKey` (interned into
/// `fonts`), `font_size`, and positioned glyphs. Each inline box's
/// `id` indexes the leaf's `InlineContent::boxes`, recovering the
/// source `<img>` element for image lookup; the image draws at the
/// box's laid-out `(x, y, width, height)` in the leaf's local coords.
///
/// Returns whether any command was emitted (false → no cached layout,
/// or empty content; caller falls back to an empty run).
fn emit_inline_content<NodeId: Copy + Eq + Hash>(
    taffy_id: taffy::NodeId,
    text_ctx: Option<&TextMeasureCtx>,
    content: Option<&crate::text_measure::InlineContent<NodeId>>,
    bounds: LayoutRect,
    content_offset: (f32, f32),
    ellipsis: Option<f32>,
    styles: &StylePlane<NodeId>,
    images_plane: &ImagePlane<NodeId>,
    fonts: &mut FontCollector,
    images: &mut ImageCollector,
    commands: &mut Vec<PaintCmd>,
) -> bool {
    let Some(text_ctx) = text_ctx else {
        return false;
    };
    let Some(layout) = text_ctx.layouts.get(&taffy_id) else {
        return false;
    };
    // Byte ranges of the source runs (concatenation order), so a glyph run can be
    // mapped back to its `InlineRun` for `overline` (which parley does not carry
    // on the run style, unlike underline / strikethrough).
    let run_spans: Vec<(std::ops::Range<usize>, &crate::text_measure::InlineRun)> = content
        .map(|c| {
            let mut off = 0usize;
            c.runs
                .iter()
                .map(|r| {
                    let start = off;
                    off += r.text.len();
                    (start..off, r)
                })
                .collect()
        })
        .unwrap_or_default();
    // `text-overflow: ellipsis`: when the laid-out content is wider than its
    // content box, drop glyphs past `content_width - ellipsis_width` and draw a
    // trailing `…` at that cutoff. The ellipsis was shaped at layout time in the
    // leaf's font/size (cached under `taffy_id`), so its ascent — and therefore
    // baseline — matches the run it follows. `None` keep = no truncation.
    let truncate: Option<(f32, &parley::Layout<crate::text_measure::ColorBrush>)> = ellipsis
        .and_then(|cw| {
            let ell = text_ctx.ellipsis_layouts.get(&taffy_id)?;
            (layout.width() > cw).then_some((cw - ell.width(), ell))
        });
    let mut emitted = false;
    for line in layout.lines() {
        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(run) => {
                    let parley_run = run.run();
                    let key = fonts.intern(parley_run.font());
                    let font_size = parley_run.font_size();
                    // Per-run color rides the brush (set per span at
                    // measure time); a colored <span>/<a> in the flow
                    // keeps its color.
                    let [r, g, b, a] = run.style().brush.0;
                    let color = ColorF::new(r, g, b, a);
                    let glyphs: Vec<GlyphInstance> = run
                        .positioned_glyphs()
                        .filter(|g| truncate.is_none_or(|(cutoff, _)| g.x < cutoff))
                        .map(|g| GlyphInstance {
                            index: g.id,
                            point: LayoutPoint::new(content_offset.0 + g.x, content_offset.1 + g.y),
                        })
                        .collect();
                    if glyphs.is_empty() {
                        continue;
                    }
                    commands.push(PaintCmd::DrawText(TextRunItem {
                        placement: CommonPlacement::new(bounds),
                        font_instance: key,
                        font_size,
                        color,
                        glyphs,
                        options: TextOptions::default(),
                    }));
                    // `text-decoration: underline` — parley records it on the
                    // run's style but does not draw it, so emit a thin filled
                    // rect under the run. parley carries the font's offset (the
                    // distance from the baseline to the decoration top, measured
                    // upward / y-up), so the screen-space top is `baseline -
                    // offset`; thickness is `underline_size` (the run's Decoration
                    // overrides the font metrics when set). The color is the
                    // decoration brush (`text-decoration-color`, `currentColor` by
                    // default, so it matches the glyphs unless set otherwise).
                    if let Some(deco) = run.style().underline.as_ref() {
                        let m = parley_run.metrics();
                        let uo = deco.offset.unwrap_or(m.underline_offset);
                        let us = deco.size.unwrap_or(m.underline_size).max(1.0);
                        let y = bounds.min.y + content_offset.1 + run.baseline() - uo;
                        let x0 = bounds.min.x + content_offset.0 + run.offset();
                        let x1 = x0 + run.advance();
                        let [dr, dg, db, da] = deco.brush.0;
                        commands.push(PaintCmd::DrawRect(RectItem {
                            placement: CommonPlacement::new(LayoutRect::new(
                                LayoutPoint::new(x0, y),
                                LayoutPoint::new(x1, y + us),
                            )),
                            color: ColorF::new(dr, dg, db, da),
                        }));
                    }
                    // `text-decoration: line-through` — same arrangement as the
                    // underline (same y-up offset convention, so subtract; same
                    // decoration-brush color). The strikethrough offset sits well
                    // above the baseline, so the line crosses the text middle.
                    if let Some(deco) = run.style().strikethrough.as_ref() {
                        let m = parley_run.metrics();
                        let so = deco.offset.unwrap_or(m.strikethrough_offset);
                        let ss = deco.size.unwrap_or(m.strikethrough_size).max(1.0);
                        let y = bounds.min.y + content_offset.1 + run.baseline() - so;
                        let x0 = bounds.min.x + content_offset.0 + run.offset();
                        let x1 = x0 + run.advance();
                        let [dr, dg, db, da] = deco.brush.0;
                        commands.push(PaintCmd::DrawRect(RectItem {
                            placement: CommonPlacement::new(LayoutRect::new(
                                LayoutPoint::new(x0, y),
                                LayoutPoint::new(x1, y + ss),
                            )),
                            color: ColorF::new(dr, dg, db, da),
                        }));
                    }
                    // `text-decoration: overline` — parley carries no overline, so
                    // map this glyph run back to its source `InlineRun` and, when
                    // set, draw a line at the ascent (top of the text) in the run's
                    // decoration color.
                    if let Some((_, src)) = run_spans
                        .iter()
                        .find(|(span, _)| span.contains(&parley_run.text_range().start))
                    {
                        if src.overline {
                            let m = parley_run.metrics();
                            let thickness = m.underline_size.max(1.0);
                            let y = bounds.min.y + content_offset.1 + run.baseline() - m.ascent;
                            let x0 = bounds.min.x + content_offset.0 + run.offset();
                            let x1 = x0 + run.advance();
                            let [dr, dg, db, da] = src.decoration_color;
                            commands.push(PaintCmd::DrawRect(RectItem {
                                placement: CommonPlacement::new(LayoutRect::new(
                                    LayoutPoint::new(x0, y),
                                    LayoutPoint::new(x1, y + thickness),
                                )),
                                color: ColorF::new(dr, dg, db, da),
                            }));
                        }
                    }
                    emitted = true;
                },
                PositionedLayoutItem::InlineBox(pbox) => {
                    // Resolve the box id back to its source via the leaf's
                    // InlineContent. An inline-block (`block: Some`) paints its
                    // background + its own content glyphs; a replaced `<img>`
                    // paints the decoded image. Both at the laid-out box rect.
                    let Some(content) = content else { continue };
                    let Some(item) = content.boxes.get(pbox.id as usize) else {
                        continue;
                    };
                    // Box position is relative to the leaf origin (same space as
                    // glyph points); place in local coords.
                    let (ox, oy) = (
                        bounds.min.x + content_offset.0 + pbox.x,
                        bounds.min.y + content_offset.1 + pbox.y,
                    );
                    let rect = LayoutRect::new(
                        LayoutPoint::new(ox, oy),
                        LayoutPoint::new(ox + pbox.width, oy + pbox.height),
                    );
                    if let Some(block) = &item.block {
                        // Background box.
                        let [r, g, b, a] = block.background;
                        if a > 0.0 {
                            commands.push(PaintCmd::DrawRect(RectItem {
                                placement: CommonPlacement::new(rect),
                                color: ColorF::new(r, g, b, a),
                            }));
                        }
                        // The inline-block's own content glyphs, placed at the box
                        // origin (its Layout was cached under (this leaf, box id)).
                        if let Some(ib_layout) = text_ctx
                            .inline_block_layouts
                            .get(&(taffy_id, pbox.id as usize))
                        {
                            for ib_line in ib_layout.lines() {
                                for ib_item in ib_line.items() {
                                    let PositionedLayoutItem::GlyphRun(grun) = ib_item else {
                                        continue;
                                    };
                                    let prun = grun.run();
                                    let key = fonts.intern(prun.font());
                                    let [gr, gg, gb, ga] = grun.style().brush.0;
                                    let glyphs: Vec<GlyphInstance> = grun
                                        .positioned_glyphs()
                                        .map(|gl| GlyphInstance {
                                            index: gl.id,
                                            point: LayoutPoint::new(
                                                content_offset.0 + pbox.x + gl.x,
                                                content_offset.1 + pbox.y + gl.y,
                                            ),
                                        })
                                        .collect();
                                    if glyphs.is_empty() {
                                        continue;
                                    }
                                    commands.push(PaintCmd::DrawText(TextRunItem {
                                        placement: CommonPlacement::new(bounds),
                                        font_instance: key,
                                        font_size: prun.font_size(),
                                        color: ColorF::new(gr, gg, gb, ga),
                                        glyphs,
                                        options: TextOptions::default(),
                                    }));
                                }
                            }
                        }
                        emitted = true;
                    } else if let Some(decoded) = images_plane.get(item.source) {
                        if let Some(cv) = primary_cv(styles, item.source) {
                            emit_object_image(&cv, rect, decoded, images, commands);
                        } else {
                            emit_image_rect(rect, decoded, ImageRendering::Auto, images, commands);
                        }
                        emitted = true;
                    }
                },
            }
        }
    }
    // Draw the trailing `…` at the cutoff. Its glyph y-positions ride the
    // ellipsis line's baseline, which equals the truncated text's baseline
    // (same font/size, both layouts top-aligned at y=0), so no extra alignment
    // math is needed.
    if let Some((cutoff, ell)) = truncate {
        for el_line in ell.lines() {
            for el_item in el_line.items() {
                let PositionedLayoutItem::GlyphRun(grun) = el_item else {
                    continue;
                };
                let prun = grun.run();
                let key = fonts.intern(prun.font());
                let [r, g, b, a] = grun.style().brush.0;
                let glyphs: Vec<GlyphInstance> = grun
                    .positioned_glyphs()
                    .map(|gl| GlyphInstance {
                        index: gl.id,
                        point: LayoutPoint::new(
                            content_offset.0 + cutoff + gl.x,
                            content_offset.1 + gl.y,
                        ),
                    })
                    .collect();
                if glyphs.is_empty() {
                    continue;
                }
                commands.push(PaintCmd::DrawText(TextRunItem {
                    placement: CommonPlacement::new(bounds),
                    font_instance: key,
                    font_size: prun.font_size(),
                    color: ColorF::new(r, g, b, a),
                    glyphs,
                    options: TextOptions::default(),
                }));
                emitted = true;
            }
        }
    }
    emitted
}

/// CSS `background-repeat` keyword per axis, reduced to what the tiler
/// needs. `space` / `round` adjust spacing / scaling; v1 approximates
/// both as `repeat` (a small slice of the corpus).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BgRepeat {
    Repeat,
    NoRepeat,
    Space,
    Round,
}

/// Which box of an element a background layer references — for
/// `background-origin` (the positioning area the size/position resolve
/// against) and `background-clip` (the area the paint is clipped to).
/// `text` clipping (background painted through glyph shapes) is not
/// modeled; it falls back to `BorderBox`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BgBox {
    BorderBox,
    PaddingBox,
    ContentBox,
}

/// First-layer `background-size` / `-repeat` / `-position` / `-origin` /
/// `-clip` read from the cascade. Pixel geometry is resolved at emit
/// time against the positioning area (see [`resolve_bg_tile`]).
struct BgTileStyle {
    size: style::values::computed::background::BackgroundSize,
    repeat_x: BgRepeat,
    repeat_y: BgRepeat,
    pos_x: style::values::computed::LengthPercentage,
    pos_y: style::values::computed::LengthPercentage,
    /// The box the size + position resolve against (CSS default:
    /// padding-box).
    origin: BgBox,
    /// The box the painted background is clipped to (CSS default:
    /// border-box).
    clip: BgBox,
}

/// Read the first background layer's size / repeat / position from an
/// element's `ComputedValues`. `None` when the cascade has not run.
fn bg_tile_style_of(cv: &ComputedValues) -> Option<BgTileStyle> {
    use style::computed_values::background_clip::single_value::T as Clip;
    use style::computed_values::background_origin::single_value::T as Origin;
    use style::values::specified::background::BackgroundRepeatKeyword as K;
    let bg = cv.get_background();
    let size = bg.background_size.0.first()?.clone();
    let repeat = bg.background_repeat.0.first()?;
    let pos_x = bg.background_position_x.0.first()?.clone();
    let pos_y = bg.background_position_y.0.first()?.clone();
    let map = |k: K| match k {
        K::Repeat => BgRepeat::Repeat,
        K::NoRepeat => BgRepeat::NoRepeat,
        K::Space => BgRepeat::Space,
        K::Round => BgRepeat::Round,
    };
    let origin = match bg.background_origin.0.first() {
        Some(Origin::ContentBox) => BgBox::ContentBox,
        Some(Origin::BorderBox) => BgBox::BorderBox,
        _ => BgBox::PaddingBox, // CSS default
    };
    let clip = match bg.background_clip.0.first() {
        Some(Clip::ContentBox) => BgBox::ContentBox,
        Some(Clip::PaddingBox) => BgBox::PaddingBox,
        // BorderBox (default) and unmodeled `text` both clip to border box.
        _ => BgBox::BorderBox,
    };
    Some(BgTileStyle {
        size,
        repeat_x: map(repeat.0),
        repeat_y: map(repeat.1),
        pos_x,
        pos_y,
        origin,
        clip,
    })
}

/// Resolve a background layer's concrete tile geometry against the
/// positioning area (`area_w` x `area_h`) and the image's intrinsic
/// size (`int_w` x `int_h`, both > 0). Returns `(tile_w, tile_h,
/// offset_x, offset_y)` in px: the tile size from `background-size`
/// (cover / contain / explicit / auto, aspect-preserving when one axis
/// is `auto`) and the anchor offset from `background-position`
/// (percentages resolve against `area - tile`).
fn resolve_bg_tile(
    bg: &BgTileStyle,
    area_w: f32,
    area_h: f32,
    int_w: f32,
    int_h: f32,
) -> (f32, f32, f32, f32) {
    use style::values::computed::Length;
    use style::values::computed::length::NonNegativeLengthPercentageOrAuto as Lpa;
    use style::values::generics::background::BackgroundSize as Bs;
    use style::values::generics::length::GenericLengthPercentageOrAuto as Loa;

    let aspect = int_w / int_h;
    let (tw, th) = match bg.size {
        Bs::Cover | Bs::Contain => {
            let scale_w = area_w / int_w;
            let scale_h = area_h / int_h;
            let scale = if matches!(bg.size, Bs::Cover) {
                scale_w.max(scale_h)
            } else {
                scale_w.min(scale_h)
            };
            (int_w * scale, int_h * scale)
        },
        Bs::ExplicitSize {
            ref width,
            ref height,
        } => {
            let resolve = |v: &Lpa, basis: f32| -> Option<f32> {
                match v {
                    Loa::Auto => None,
                    Loa::LengthPercentage(npl) => {
                        Some(npl.0.resolve(Length::new(basis.max(0.0))).px().max(0.0))
                    },
                }
            };
            match (resolve(width, area_w), resolve(height, area_h)) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, w / aspect),
                (None, Some(h)) => (h * aspect, h),
                (None, None) => (int_w, int_h),
            }
        },
    };
    let pos = |lp: &style::values::computed::LengthPercentage, basis: f32| -> f32 {
        lp.resolve(Length::new(basis)).px()
    };
    let ox = pos(&bg.pos_x, area_w - tw);
    let oy = pos(&bg.pos_y, area_h - th);
    (tw, th, ox, oy)
}

/// A laid-out box's CSS content box in the node's local border-box coordinate
/// space. Replaced content is drawn into this box; background and border still
/// use their own CSS paint areas.
fn content_box(l: &taffy::Layout) -> LayoutRect {
    let x0 = l.border.left + l.padding.left;
    let y0 = l.border.top + l.padding.top;
    let x1 = (l.size.width - l.border.right - l.padding.right).max(x0);
    let y1 = (l.size.height - l.border.bottom - l.padding.bottom).max(y0);
    LayoutRect::new(LayoutPoint::new(x0, y0), LayoutPoint::new(x1, y1))
}

fn rects_intersect(a: LayoutRect, b: LayoutRect) -> bool {
    a.min.x < b.max.x && a.max.x > b.min.x && a.min.y < b.max.y && a.max.y > b.min.y
}

fn rect_contains(outer: LayoutRect, inner: LayoutRect) -> bool {
    inner.min.x >= outer.min.x
        && inner.min.y >= outer.min.y
        && inner.max.x <= outer.max.x
        && inner.max.y <= outer.max.y
}

/// Emit an `<img>` content object, applying `object-fit` and
/// `object-position` against the element's content box.
fn emit_object_image(
    cv: &ComputedValues,
    content_rect: LayoutRect,
    decoded: &DecodedImage,
    images: &mut ImageCollector,
    commands: &mut Vec<PaintCmd>,
) {
    let Some((object_rect, clips)) = object_fit_rect(
        cv,
        content_rect,
        decoded.width as f32,
        decoded.height as f32,
    ) else {
        return;
    };

    if clips {
        commands.push(PaintCmd::PushClip(ClipSpec {
            kind: ClipKind::Rect(content_rect),
        }));
    }
    emit_image_rect(object_rect, decoded, image_rendering_of(cv), images, commands);
    if clips {
        commands.push(PaintCmd::PopClip);
    }
}

fn emit_image_rect(
    rect: LayoutRect,
    decoded: &DecodedImage,
    rendering: ImageRendering,
    images: &mut ImageCollector,
    commands: &mut Vec<PaintCmd>,
) {
    let key = images.add(decoded);
    commands.push(PaintCmd::DrawImage(ImageItem {
        placement: CommonPlacement::new(rect),
        image_key: key,
        image_rendering: rendering,
        alpha_type: AlphaType::PremultipliedAlpha,
        color: ColorF::WHITE, // identity tint
    }));
}

/// The element's computed `image-rendering`, lowered to the paint-list
/// enum. Inherited-box property, so anonymous fragments ride their
/// parent's value through the primary style.
fn image_rendering_of(cv: &ComputedValues) -> ImageRendering {
    use style::computed_values::image_rendering::T as R;
    match cv.get_inherited_box().image_rendering {
        R::Auto => ImageRendering::Auto,
        R::CrispEdges => ImageRendering::CrispEdges,
        R::Pixelated => ImageRendering::Pixelated,
    }
}

/// Emit a host-composited replaced content object, applying the same concrete
/// object geometry as `<img>`.
fn emit_object_external_texture(
    cv: &ComputedValues,
    content_rect: LayoutRect,
    texture_key: u64,
    intrinsic_w: f32,
    intrinsic_h: f32,
    commands: &mut Vec<PaintCmd>,
) {
    let Some((object_rect, clips)) = object_fit_rect(cv, content_rect, intrinsic_w, intrinsic_h)
    else {
        return;
    };

    if clips {
        commands.push(PaintCmd::PushClip(ClipSpec {
            kind: ClipKind::Rect(content_rect),
        }));
    }
    commands.push(PaintCmd::DrawExternalTexture(ExternalTextureItem {
        placement: CommonPlacement::new(object_rect),
        texture_key,
        opacity: 1.0,
        content_generation: None,
    }));
    if clips {
        commands.push(PaintCmd::PopClip);
    }
}

/// Resolve CSS Images' concrete object size and position for a replaced content
/// object. Returns the object draw rect and whether it needs a content-box clip.
fn object_fit_rect(
    cv: &ComputedValues,
    content_rect: LayoutRect,
    intrinsic_w: f32,
    intrinsic_h: f32,
) -> Option<(LayoutRect, bool)> {
    use style::computed_values::object_fit::T as Fit;
    use style::values::computed::Length;

    let content_w = content_rect.width();
    let content_h = content_rect.height();
    if intrinsic_w <= 0.0 || intrinsic_h <= 0.0 || content_w <= 0.0 || content_h <= 0.0 {
        return None;
    }

    let contain_scale = (content_w / intrinsic_w).min(content_h / intrinsic_h);
    let pos = cv.get_position();
    let (object_w, object_h) = match pos.object_fit {
        Fit::Fill => (content_w, content_h),
        Fit::Contain => (intrinsic_w * contain_scale, intrinsic_h * contain_scale),
        Fit::Cover => {
            let scale = (content_w / intrinsic_w).max(content_h / intrinsic_h);
            (intrinsic_w * scale, intrinsic_h * scale)
        },
        Fit::None => (intrinsic_w, intrinsic_h),
        Fit::ScaleDown => {
            if contain_scale < 1.0 {
                (intrinsic_w * contain_scale, intrinsic_h * contain_scale)
            } else {
                (intrinsic_w, intrinsic_h)
            }
        },
    };

    let free_x = content_w - object_w;
    let free_y = content_h - object_h;
    let object_x = content_rect.min.x
        + pos
            .object_position
            .horizontal
            .resolve(Length::new(free_x))
            .px();
    let object_y = content_rect.min.y
        + pos
            .object_position
            .vertical
            .resolve(Length::new(free_y))
            .px();
    let object_rect = LayoutRect::new(
        LayoutPoint::new(object_x, object_y),
        LayoutPoint::new(object_x + object_w, object_y + object_h),
    );
    let clips = object_rect.min.x < content_rect.min.x
        || object_rect.min.y < content_rect.min.y
        || object_rect.max.x > content_rect.max.x
        || object_rect.max.y > content_rect.max.y;
    Some((object_rect, clips))
}

/// Read an element's background color from its `ComputedValues`.
/// Returns transparent when no cascade data is present (hand-rolled
/// styles bypass the cascade) — that matches CSS semantics for
/// "background-color: initial".
fn background_color_of(cv: &ComputedValues) -> ColorF {
    let bg = &cv.get_background().background_color;
    let current = cv.get_inherited_text().color;
    stylo_color_to_paint(bg, current)
}

/// CSS gradient-line angle in radians (0 = "to top", increasing clockwise) for a
/// `w`x`h` box. `to <side>` / `to <corner>` resolve to angles per CSS Images
/// (corners use the box aspect).
fn line_direction_angle(
    dir: &style::values::computed::image::LineDirection,
    w: f32,
    h: f32,
) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};

    use style::values::computed::image::LineDirection;
    use style::values::specified::position::{
        HorizontalPositionKeyword as HK, VerticalPositionKeyword as VK,
    };

    match dir {
        LineDirection::Angle(a) => a.radians(),
        LineDirection::Vertical(VK::Top) => 0.0,
        LineDirection::Vertical(VK::Bottom) => PI,
        LineDirection::Horizontal(HK::Left) => 3.0 * FRAC_PI_2,
        LineDirection::Horizontal(HK::Right) => FRAC_PI_2,
        LineDirection::Corner(hk, vk) => {
            // Angle from "to top" to the box's top-right corner direction.
            let base = w.atan2(h);
            match (hk, vk) {
                (HK::Right, VK::Top) => base,
                (HK::Right, VK::Bottom) => PI - base,
                (HK::Left, VK::Bottom) => PI + base,
                (HK::Left, VK::Top) => -base,
            }
        },
    }
}

/// For a `repeating-*` gradient whose `stops` are normalized 0..1 over the full
/// extent: return `(first_offset, last_offset, renormalized_stops)` so the
/// caller can shrink the geometry to that one period and tile it with
/// `ExtendMode::Repeat`. `None` for a degenerate period (≤ 0), so the caller
/// falls back to a single clamped fill.
fn repeating_period(stops: &[GradientStop]) -> Option<(f32, f32, Vec<GradientStop>)> {
    let first = stops.first()?.offset;
    let last = stops.last()?.offset;
    let period = last - first;
    if period <= 1e-4 {
        return None;
    }
    let renormalized = stops
        .iter()
        .map(|s| GradientStop {
            offset: ((s.offset - first) / period).clamp(0.0, 1.0),
            color: s.color,
        })
        .collect();
    Some((first, last, renormalized))
}

/// Find the canvas background source and paint it over the whole viewport.
///
/// CSS Backgrounds-3 §root-background: a background set on the root element is
/// painted over the *entire* canvas (the viewport), not merely the element's own
/// box, positioned against the root's background positioning area (its padding
/// box, which carries any root margin offset). HTML adds the body→canvas special
/// case: when the root element's own background is transparent, the background is
/// taken from `<body>` instead (and `<body>` then paints none on its own box).
///
/// Returns the source element (root or body) so [`walk`] can suppress the
/// duplicate paint on its box; `None` when neither carries a background.
///
/// Paint model: the gradient layers tile against the root's positioning area
/// (its box, carrying the margin offset and size) and paint across the whole
/// viewport. With `background-size: auto` the tile is the root box, repeated to
/// fill the canvas, matching how serval renders the §root-background reference
/// fixtures (a sized/positioned element over the canvas). `background-origin` /
/// `-clip` insets are not modeled (the positioning area is the border box).
fn emit_canvas_background<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    viewport: DeviceIntSize,
    commands: &mut Vec<PaintCmd>,
) -> Option<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::local_name;

    // Root element = the first element child of the document node.
    let root = dom
        .dom_children(dom.document())
        .find(|&c| dom.kind(c) == NodeKind::Element)?;

    // The root must generate a principal box at all: `display: none` on the root
    // hides the whole document, so there is no canvas background to propagate
    // (the *-propagation negative tests).
    let root_cv = primary_cv(styles, root)?;
    if !generates_box(&root_cv) {
        return None;
    }
    // Source: the root if it carries a background, else (root transparent) the
    // body — the HTML body→canvas propagation special case. A `display: none` /
    // `display: contents` body generates no principal box and so does not
    // propagate either.
    let (source, source_cv) = if has_canvas_background(&root_cv) {
        (root, root_cv)
    } else {
        let body = dom.dom_children(root).find(|&c| {
            dom.kind(c) == NodeKind::Element
                && dom
                    .element_name(c)
                    .is_some_and(|q| q.local == local_name!("body"))
        })?;
        let body_cv = primary_cv(styles, body)?;
        if generates_box(&body_cv) && has_canvas_background(&body_cv) {
            (body, body_cv)
        } else {
            return None;
        }
    };

    let cw = viewport.width as f32;
    let ch = viewport.height as f32;
    let canvas = LayoutRect::new(LayoutPoint::new(0.0, 0.0), LayoutPoint::new(cw, ch));

    // Background color first (behind the image layers), over the whole canvas.
    let color = background_color_of(&source_cv);
    if color.a > 0.0 {
        commands.push(PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(canvas),
            color,
        }));
    }
    // The positioning area is the *root's* border box (its margin offset and
    // size carry through to where the gradient tiles), even when the source is
    // the body. Tiles paint across the whole canvas. With `background-size: auto`
    // the tile is the root box, repeated to fill the viewport — matching the
    // §root-background reference fixtures (a sized/positioned element over the
    // canvas). taffy does not fold the *root's* margin into its `location` (the
    // root has no parent to offset against), so add it here to land the border
    // box at the margin offset. A root with no fragment falls back to the canvas.
    let (root_box, border, padding) = fragments
        .rect_of(root)
        .map(|l| {
            let x = l.location.x + l.margin.left;
            let y = l.location.y + l.margin.top;
            (
                LayoutRect::new(
                    LayoutPoint::new(x, y),
                    LayoutPoint::new(x + l.size.width, y + l.size.height),
                ),
                [l.border.left, l.border.top, l.border.right, l.border.bottom],
                [
                    l.padding.left,
                    l.padding.top,
                    l.padding.right,
                    l.padding.bottom,
                ],
            )
        })
        .unwrap_or((canvas, [0.0; 4], [0.0; 4]));
    for cmd in background_gradient_layers(&source_cv, root_box, border, padding, canvas) {
        commands.push(cmd);
    }

    Some(source)
}

/// Whether `id` carries a background that participates in canvas propagation: a
/// non-transparent background color, or any non-`none` background-image layer.
fn has_canvas_background(cv: &ComputedValues) -> bool {
    use style::values::generics::image::Image;
    let has_image = cv
        .get_background()
        .background_image
        .0
        .iter()
        .any(|img| !matches!(img, Image::None));
    has_image || background_color_of(cv).a > 0.0
}

/// Whether `id` generates a principal box — i.e. is not `display: none` or
/// `display: contents`. The root/body source of both canvas-background
/// propagation (CSS Backgrounds-3 §special-backgrounds) and viewport-overflow
/// propagation ([`crate::viewport`], CSS Overflow §3.3) must generate a box.
pub(crate) fn generates_box(cv: &ComputedValues) -> bool {
    let display = cv.get_box().display;
    !display.is_none() && !display.is_contents()
}

/// Cap on tiles emitted for one gradient layer (per axis the count is also
/// bounded by extent / tile). A layer that would exceed it falls back to a single
/// area-filling gradient, so a pathological `background-size: 1px` cannot flood
/// the command stream.
const MAX_GRADIENT_TILES: usize = 4096;

/// The `background-origin` box: `border_box` inset by `border` (padding-box) or
/// `border + padding` (content-box), or `border_box` itself (border-box). Insets
/// are `[left, top, right, bottom]`.
fn origin_box(
    border_box: LayoutRect,
    border: [f32; 4],
    padding: [f32; 4],
    which: BgBox,
) -> LayoutRect {
    let inset = |l: f32, t: f32, r: f32, b: f32| {
        LayoutRect::new(
            LayoutPoint::new(border_box.min.x + l, border_box.min.y + t),
            LayoutPoint::new(border_box.max.x - r, border_box.max.y - b),
        )
    };
    match which {
        BgBox::BorderBox => border_box,
        BgBox::PaddingBox => inset(border[0], border[1], border[2], border[3]),
        BgBox::ContentBox => inset(
            border[0] + padding[0],
            border[1] + padding[1],
            border[2] + padding[2],
            border[3] + padding[3],
        ),
    }
}

/// All `background-image` gradient layers of `id`, as paint commands in paint
/// order. Each layer's tile is sized/positioned against `pos_area` (the
/// background positioning area) and tiled across `paint_area` (the painting area,
/// where the tiles are clipped). The two coincide for an element (both the border
/// box); they differ for canvas propagation, where the positioning area is the
/// root box and the painting area is the whole viewport.
///
/// CSS lists the topmost layer first, so layers emit back-to-front. Each honors
/// `background-size` (the tile), `background-position` (the tile anchor within
/// the positioning area), and `background-repeat` (`repeat`, `no-repeat`,
/// `round` — which rescales the tile to a whole-number fit — and `space` — whole
/// tiles distributed with gaps). `auto` size fills the positioning area, so the
/// common default reduces to one area-filling gradient identical to the un-tiled
/// emit. Each layer's positioning area is its `background-origin` box, derived
/// from `border_box` inset by `border` / `padding` ([l, t, r, b] each); the
/// painting area `paint_area` is the clip. Non-gradient layers (`url()` images)
/// are emitted separately.
fn background_gradient_layers(
    cv: &ComputedValues,
    border_box: LayoutRect,
    border: [f32; 4],
    padding: [f32; 4],
    paint_area: LayoutRect,
) -> Vec<PaintCmd> {
    let bg = cv.get_background();
    let current = cv.get_inherited_text().color;
    let layers = &bg.background_image.0;
    // Per-axis tile placement. Returns the tile positions (absolute coords on the
    // axis) and the *effective* tile size (`round` rescales it). `pos0` / `ext`
    // are the positioning-area origin / extent on this axis (where `round` /
    // `space` fit their tiles); `[lo, hi]` is the painting extent (where `repeat`
    // tiles); `off` is the resolved `background-position` offset. `None` ⇒ caller
    // falls back to a single area-filling gradient.
    let axis = |pos0: f32,
                ext: f32,
                lo: f32,
                hi: f32,
                tile: f32,
                off: f32,
                repeat: BgRepeat,
                cap: usize|
     -> Option<(Vec<f32>, f32)> {
        if tile <= 0.0 || ext <= 0.0 {
            return None;
        }
        let tiled = |start: f32, step: f32| -> Option<Vec<f32>> {
            if step <= 0.0 {
                return None;
            }
            let k0 = ((lo - start) / step).floor() as i64;
            let k1 = ((hi - start) / step).ceil() as i64;
            let count = (k1 - k0).max(0) as usize;
            (count != 0 && count <= cap)
                .then(|| (k0..k1).map(|k| start + k as f32 * step).collect())
        };
        match repeat {
            BgRepeat::NoRepeat => Some((vec![pos0 + off], tile)),
            BgRepeat::Repeat => tiled(pos0 + off, tile).map(|v| (v, tile)),
            // Rescale the tile so a whole number fills the positioning area, then
            // tile that across the painting extent from the area origin.
            BgRepeat::Round => {
                let n = (ext / tile).round().max(1.0);
                let eff = ext / n;
                tiled(pos0, eff).map(|v| (v, eff))
            },
            // Whole tiles spaced with gaps so the first and last touch the
            // positioning-area edges (so `background-position` is ignored), then
            // continued at that period across the painting area — the tiles
            // repeat into a larger clip box (e.g. behind a transparent border).
            // A single fitting tile is positioned like `no-repeat`.
            BgRepeat::Space => {
                let n = (ext / tile).floor() as i64;
                if n <= 1 {
                    return Some((vec![pos0 + off], tile));
                }
                let period = tile + (ext - n as f32 * tile) / (n as f32 - 1.0);
                tiled(pos0, period).map(|v| (v, tile))
            },
        }
    };
    // Forward index drives the per-layer property lookup (the size / position /
    // repeat lists cycle to the layer count); emission is back-to-front, so
    // collect per layer then reverse.
    let mut per_layer: Vec<Vec<PaintCmd>> = Vec::new();
    for (i, image) in layers.iter().enumerate() {
        let style = gradient_layer_tile_style(bg, i);
        // The positioning area is this layer's background-origin box (default
        // padding-box), `border_box` inset by the border (and padding for
        // content-box). `background-attachment: fixed` instead positions against
        // the painting area, which for canvas propagation is the viewport (a
        // fixed canvas layer is viewport-anchored, not root-box-anchored).
        let origin = style
            .as_ref()
            .map(|s| s.origin)
            .unwrap_or(BgBox::PaddingBox);
        let pos_area = if style.as_ref().is_some_and(|s| s.fixed) {
            paint_area
        } else {
            origin_box(border_box, border, padding, origin)
        };
        let (aw, ah) = (pos_area.width(), pos_area.height());
        let (tw, th, ox, oy) = match &style {
            Some(s) => resolve_gradient_tile(s, aw, ah),
            None => (aw, ah, 0.0, 0.0),
        };
        let (rx, ry) = style
            .as_ref()
            .map(|s| (s.repeat_x, s.repeat_y))
            .unwrap_or((BgRepeat::Repeat, BgRepeat::Repeat));
        let cap = (MAX_GRADIENT_TILES as f64).sqrt() as usize;
        let xs = axis(
            pos_area.min.x,
            aw,
            paint_area.min.x,
            paint_area.max.x,
            tw,
            ox,
            rx,
            cap,
        );
        let ys = axis(
            pos_area.min.y,
            ah,
            paint_area.min.y,
            paint_area.max.y,
            th,
            oy,
            ry,
            cap,
        );
        let mut cmds = Vec::new();
        match (xs, ys) {
            // A tile grid within the cap: one gradient per cell at the per-axis
            // effective tile size, clipped to the painting area.
            (Some((xs, etw)), Some((ys, eth))) if xs.len() * ys.len() <= MAX_GRADIENT_TILES => {
                for &ty in &ys {
                    for &tx in &xs {
                        if let Some(cmd) =
                            gradient_tile_cmd(image, current, etw, eth, tx, ty, paint_area)
                        {
                            cmds.push(cmd);
                        }
                    }
                }
            },
            // Over-cap or degenerate: stretch one gradient over the positioning
            // area, clipped to the painting area (the un-tiled behavior).
            _ => {
                if let Some(cmd) = gradient_tile_cmd(
                    image,
                    current,
                    aw,
                    ah,
                    pos_area.min.x,
                    pos_area.min.y,
                    paint_area,
                ) {
                    cmds.push(cmd);
                }
            },
        }
        per_layer.push(cmds);
    }
    per_layer.into_iter().rev().flatten().collect()
}

/// Per-layer `background-size` / `-position` / `-repeat` for gradient layer `i`,
/// cycling each list to the layer count (CSS list repetition). `None` when the
/// cascade carries no entries (hand-rolled styles).
fn gradient_layer_tile_style(
    bg: &style::properties::style_structs::Background,
    i: usize,
) -> Option<GradientTileStyle> {
    use style::values::specified::background::BackgroundRepeatKeyword as K;
    let sizes = &bg.background_size.0;
    let xs = &bg.background_position_x.0;
    let ys = &bg.background_position_y.0;
    let reps = &bg.background_repeat.0;
    if sizes.is_empty() || xs.is_empty() || ys.is_empty() || reps.is_empty() {
        return None;
    }
    let map = |k: K| match k {
        K::Repeat => BgRepeat::Repeat,
        K::NoRepeat => BgRepeat::NoRepeat,
        K::Space => BgRepeat::Space,
        K::Round => BgRepeat::Round,
    };
    use style::computed_values::background_origin::single_value::T as Origin;
    let origin = match bg
        .background_origin
        .0
        .get(i % bg.background_origin.0.len().max(1))
    {
        Some(Origin::ContentBox) => BgBox::ContentBox,
        Some(Origin::BorderBox) => BgBox::BorderBox,
        _ => BgBox::PaddingBox, // CSS default
    };
    use style::computed_values::background_attachment::single_value::T as Attach;
    let fixed = matches!(
        bg.background_attachment
            .0
            .get(i % bg.background_attachment.0.len().max(1)),
        Some(Attach::Fixed)
    );
    let repeat = &reps[i % reps.len()];
    Some(GradientTileStyle {
        size: sizes[i % sizes.len()].clone(),
        pos_x: xs[i % xs.len()].clone(),
        pos_y: ys[i % ys.len()].clone(),
        repeat_x: map(repeat.0),
        repeat_y: map(repeat.1),
        origin,
        fixed,
    })
}

/// A gradient layer's `background-size` / `-position` / `-repeat` / `-origin`
/// (the positioning box) and whether `background-attachment: fixed`.
struct GradientTileStyle {
    size: style::values::computed::background::BackgroundSize,
    pos_x: style::values::computed::LengthPercentage,
    pos_y: style::values::computed::LengthPercentage,
    repeat_x: BgRepeat,
    repeat_y: BgRepeat,
    origin: BgBox,
    fixed: bool,
}

/// Resolve a gradient layer's tile against a `w`×`h` positioning area:
/// `(tile_w, tile_h, offset_x, offset_y)`. A gradient has no intrinsic size, so
/// each `auto` axis (and `cover` / `contain`, which have no ratio to preserve)
/// fills the area; explicit lengths/percentages resolve against the area.
/// `background-position` anchors the tile within `area - tile`.
fn resolve_gradient_tile(s: &GradientTileStyle, w: f32, h: f32) -> (f32, f32, f32, f32) {
    use style::values::computed::Length;
    use style::values::computed::length::NonNegativeLengthPercentageOrAuto as Lpa;
    use style::values::generics::background::BackgroundSize as Bs;
    use style::values::generics::length::GenericLengthPercentageOrAuto as Loa;

    let (tw, th) = match s.size {
        Bs::Cover | Bs::Contain => (w, h),
        Bs::ExplicitSize {
            ref width,
            ref height,
        } => {
            let axis = |v: &Lpa, area: f32| match v {
                Loa::Auto => area,
                Loa::LengthPercentage(npl) => {
                    npl.0.resolve(Length::new(area.max(0.0))).px().max(0.0)
                },
            };
            (axis(width, w), axis(height, h))
        },
    };
    let ox = s.pos_x.resolve(Length::new(w - tw)).px();
    let oy = s.pos_y.resolve(Length::new(h - th)).px();
    (tw, th, ox, oy)
}

/// One gradient tile of `image` at offset (`tx`, `ty`), size (`tw`, `th`),
/// clipped to the `clip` painting area. The gradient ramp is built over the
/// tile, then translated into place; `placement` is the tile rect intersected
/// with `clip`, so a partial edge tile shows the correct slice. `None` when the
/// layer is not a gradient or the clipped tile is empty.
fn gradient_tile_cmd(
    image: &style::values::computed::image::Image,
    current: style::color::AbsoluteColor,
    tw: f32,
    th: f32,
    tx: f32,
    ty: f32,
    clip: LayoutRect,
) -> Option<PaintCmd> {
    let px0 = tx.max(clip.min.x);
    let py0 = ty.max(clip.min.y);
    let px1 = (tx + tw).min(clip.max.x);
    let py1 = (ty + th).min(clip.max.y);
    if px1 <= px0 || py1 <= py0 {
        return None;
    }
    let place = CommonPlacement::new(LayoutRect::new(
        LayoutPoint::new(px0, py0),
        LayoutPoint::new(px1, py1),
    ));
    if let Some(mut g) = linear_gradient_layer(image, current, tw, th) {
        g.gradient.start_point.x += tx;
        g.gradient.start_point.y += ty;
        g.gradient.end_point.x += tx;
        g.gradient.end_point.y += ty;
        g.placement = place;
        Some(PaintCmd::DrawLinearGradient(g))
    } else if let Some(mut g) = radial_gradient_layer(image, current, tw, th) {
        g.gradient.center.x += tx;
        g.gradient.center.y += ty;
        g.placement = place;
        Some(PaintCmd::DrawRadialGradient(g))
    } else if let Some(mut g) = conic_gradient_layer(image, current, tw, th) {
        g.gradient.center.x += tx;
        g.gradient.center.y += ty;
        g.placement = place;
        Some(PaintCmd::DrawConicGradient(g))
    } else {
        None
    }
}

/// A single `background-image` layer `image`, if it is a **linear** gradient,
/// resolved to a paint-list [`LinearGradientItem`] over a `w`x`h` border box in
/// node-local coords (the paint walk's transform places it). `current` resolves
/// `currentColor` in the stops. `None` when the layer is not a linear gradient.
fn linear_gradient_layer(
    image: &style::values::computed::image::Image,
    current: style::color::AbsoluteColor,
    w: f32,
    h: f32,
) -> Option<LinearGradientItem> {
    use style::values::generics::image::{Gradient, GradientFlags, Image};

    let Image::Gradient(gradient) = image else {
        return None;
    };
    let Gradient::Linear {
        direction,
        items,
        flags,
        ..
    } = &**gradient
    else {
        return None;
    };
    let repeating = flags.contains(GradientFlags::REPEATING);

    let angle = line_direction_angle(direction, w, h);
    let (sin, cos) = (angle.sin(), angle.cos());
    // Gradient line: centered, length = projection of the box onto the line.
    let len = (w * sin).abs() + (h * cos).abs();
    if len <= 0.0 {
        return None;
    }
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (dx, dy) = (sin, -cos);
    let start = LayoutPoint::new(cx - dx * len / 2.0, cy - dy * len / 2.0);
    let end = LayoutPoint::new(cx + dx * len / 2.0, cy + dy * len / 2.0);

    // Color stops resolve to 0..1 offsets along the gradient line (length `len`).
    use style::values::computed::Length;
    let stops = resolve_gradient_stops(items, current, |p| p.resolve(Length::new(len)).px() / len)?;

    // `repeating-linear-gradient`: shrink the endpoints to one period (first to
    // last stop) and re-normalize the stops over it, so the renderer's Repeat
    // tiling reproduces the CSS pattern. A degenerate period falls back to a
    // single clamped fill.
    let (start_point, end_point, stops, extend_mode) =
        match repeating.then(|| repeating_period(&stops)).flatten() {
            Some((first, last, renorm)) => {
                let at = |f: f32| LayoutPoint::new(start.x + dx * f * len, start.y + dy * f * len);
                (at(first), at(last), renorm, ExtendMode::Repeat)
            },
            None => (start, end, stops, ExtendMode::Clamp),
        };

    Some(LinearGradientItem {
        placement: CommonPlacement::new(LayoutRect::new(
            LayoutPoint::new(0.0, 0.0),
            LayoutPoint::new(w, h),
        )),
        gradient: LinearGradientPayload {
            start_point,
            end_point,
            extend_mode,
            stops,
        },
        tile_size: LayoutSize::new(w, h),
        tile_spacing: LayoutSize::zero(),
    })
}

/// Resolve a gradient's color stops to ascending 0..1 offsets. `offset_of` maps
/// a positioned stop's position (a length along the gradient line, or an angle
/// around the sweep) to a raw 0..1 offset; unpositioned (`auto`) stops fill in by
/// even distribution between bracketing positioned ones; offsets clamp monotonic.
/// Interpolation hints (midpoint biasing) are skipped for now. `None` for fewer
/// than two stops. Shared by the linear, radial, and conic emitters (generic over
/// the position type `T`: `LengthPercentage` for linear/radial, `AngleOrPercentage`
/// for conic).
fn resolve_gradient_stops<T>(
    items: &[style::values::generics::image::GradientItem<style::values::computed::Color, T>],
    current: style::color::AbsoluteColor,
    mut offset_of: impl FnMut(&T) -> f32,
) -> Option<Vec<GradientStop>> {
    use style::values::generics::image::GradientItem;

    let mut raw: Vec<(Option<f32>, ColorF)> = Vec::new();
    for item in items.iter() {
        match item {
            GradientItem::SimpleColorStop(color) => {
                raw.push((None, stylo_color_to_paint(color, current)));
            },
            GradientItem::ComplexColorStop { color, position } => {
                let off = offset_of(position).clamp(0.0, 1.0);
                raw.push((Some(off), stylo_color_to_paint(color, current)));
            },
            GradientItem::InterpolationHint(_) => {},
        }
    }
    let n = raw.len();
    if n < 2 {
        return None;
    }
    if raw[0].0.is_none() {
        raw[0].0 = Some(0.0);
    }
    if raw[n - 1].0.is_none() {
        raw[n - 1].0 = Some(1.0);
    }
    // Monotonic clamp on positioned stops.
    let mut running = 0.0_f32;
    for (off, _) in raw.iter_mut() {
        if let Some(o) = off.as_mut() {
            *o = o.max(running);
            running = *o;
        }
    }
    // Fill `auto` runs evenly between the bracketing positioned stops.
    let mut i = 0;
    while i < n {
        if raw[i].0.is_some() {
            i += 1;
            continue;
        }
        let prev = raw[i - 1].0.unwrap();
        let mut j = i;
        while j < n && raw[j].0.is_none() {
            j += 1;
        }
        let next = raw[j].0.unwrap();
        let span = (j - i + 1) as f32;
        for (k, slot) in raw[i..j].iter_mut().enumerate() {
            slot.0 = Some(prev + (next - prev) * (k as f32 + 1.0) / span);
        }
        i = j;
    }

    Some(
        raw.into_iter()
            .map(|(off, color)| GradientStop {
                offset: off.unwrap(),
                color,
            })
            .collect(),
    )
}

/// A single `background-image` layer `image`, if it is a **radial** gradient,
/// resolved to a paint-list [`RadialGradientItem`] over a `w`x`h` border box in
/// node-local coords. Circle and ellipse shapes are supported, sized by explicit
/// radii or any extent keyword (`closest`/`farthest-side`/`-corner`, `contain`,
/// `cover`); `position` sets the center. `None` when the layer is not a radial
/// gradient.
fn radial_gradient_layer(
    image: &style::values::computed::image::Image,
    current: style::color::AbsoluteColor,
    w: f32,
    h: f32,
) -> Option<RadialGradientItem> {
    use style::values::computed::Length;
    use style::values::generics::image::{Circle, Ellipse, EndingShape, Gradient, Image};

    let Image::Gradient(gradient) = image else {
        return None;
    };
    let Gradient::Radial {
        shape,
        position,
        items,
        ..
    } = &**gradient
    else {
        return None;
    };

    // Center: resolve the position against the box (percentages vs each axis).
    let cx = position.horizontal.resolve(Length::new(w)).px();
    let cy = position.vertical.resolve(Length::new(h)).px();

    // Ending-shape radii (rx, ry) in box-local px.
    let (rx, ry) = match shape {
        EndingShape::Circle(Circle::Radius(r)) => {
            let r = r.0.px();
            (r, r)
        },
        EndingShape::Circle(Circle::Extent(ext)) => resolve_extent_radii(*ext, true, cx, cy, w, h),
        EndingShape::Ellipse(Ellipse::Radii(rx_lp, ry_lp)) => (
            rx_lp.0.resolve(Length::new(w)).px(),
            ry_lp.0.resolve(Length::new(h)).px(),
        ),
        EndingShape::Ellipse(Ellipse::Extent(ext)) => {
            resolve_extent_radii(*ext, false, cx, cy, w, h)
        },
    };
    if rx <= 0.0 || ry <= 0.0 {
        return None;
    }

    // Stops resolve along the horizontal radius: the renderer builds a unit-circle
    // radial and scales it by (rx, ry), so offset 1.0 lands on the ending shape on
    // every ray regardless of which radius the lengths were resolved against.
    let stops = resolve_gradient_stops(items, current, |p| p.resolve(Length::new(rx)).px() / rx)?;

    Some(RadialGradientItem {
        placement: CommonPlacement::new(LayoutRect::new(
            LayoutPoint::new(0.0, 0.0),
            LayoutPoint::new(w, h),
        )),
        gradient: RadialGradientPayload {
            center: LayoutPoint::new(cx, cy),
            radius: LayoutSize::new(rx, ry),
            extend_mode: ExtendMode::Clamp,
            stops,
        },
        tile_size: LayoutSize::new(w, h),
        tile_spacing: LayoutSize::zero(),
    })
}

/// Resolve a radial gradient's ending-shape radii from an extent keyword. For a
/// circle (`is_circle`) both radii are equal; for an ellipse they are computed
/// per axis. `(cx, cy)` is the center and `(w, h)` the box, both in box-local px.
/// `contain` aliases `closest-side`, `cover` aliases `farthest-corner`.
fn resolve_extent_radii(
    extent: style::values::generics::image::ShapeExtent,
    is_circle: bool,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
) -> (f32, f32) {
    use std::f32::consts::SQRT_2;

    use style::values::generics::image::ShapeExtent as E;

    // Distances from the center to each box edge.
    let left = cx.max(0.0);
    let right = (w - cx).max(0.0);
    let top = cy.max(0.0);
    let bottom = (h - cy).max(0.0);
    let extent = match extent {
        E::Contain => E::ClosestSide,
        E::Cover => E::FarthestCorner,
        other => other,
    };
    if is_circle {
        let r = match extent {
            E::ClosestSide => left.min(right).min(top).min(bottom),
            E::FarthestSide => left.max(right).max(top).max(bottom),
            E::ClosestCorner => {
                let (dx, dy) = (left.min(right), top.min(bottom));
                (dx * dx + dy * dy).sqrt()
            },
            E::FarthestCorner => {
                let (dx, dy) = (left.max(right), top.max(bottom));
                (dx * dx + dy * dy).sqrt()
            },
            E::Contain | E::Cover => unreachable!("aliased above"),
        };
        (r, r)
    } else {
        // Ellipse: a corner case keeps the aspect ratio of the matching side case,
        // so the ellipse through the corner at (side_x, side_y) is a uniform
        // sqrt(2) scale of those side radii.
        match extent {
            E::ClosestSide => (left.min(right), top.min(bottom)),
            E::FarthestSide => (left.max(right), top.max(bottom)),
            E::ClosestCorner => (left.min(right) * SQRT_2, top.min(bottom) * SQRT_2),
            E::FarthestCorner => (left.max(right) * SQRT_2, top.max(bottom) * SQRT_2),
            E::Contain | E::Cover => unreachable!("aliased above"),
        }
    }
}

/// A single `background-image` layer `image`, if it is a **conic** gradient,
/// resolved to a paint-list [`ConicGradientItem`] over a `w`x`h` border box in
/// node-local coords (the paint walk's transform places it). `from <angle>` sets
/// the seam and `at <position>` the center; angular color stops resolve to 0..1
/// around the clockwise sweep. `None` when the layer is not a conic gradient.
fn conic_gradient_layer(
    image: &style::values::computed::image::Image,
    current: style::color::AbsoluteColor,
    w: f32,
    h: f32,
) -> Option<ConicGradientItem> {
    use std::f32::consts::{FRAC_PI_2, TAU};

    use style::values::computed::AngleOrPercentage;
    use style::values::computed::Length;
    use style::values::generics::image::{Gradient, Image};

    let Image::Gradient(gradient) = image else {
        return None;
    };
    let Gradient::Conic {
        angle,
        position,
        items,
        ..
    } = &**gradient
    else {
        return None;
    };

    let cx = position.horizontal.resolve(Length::new(w)).px();
    let cy = position.vertical.resolve(Length::new(h)).px();
    // CSS conic 0deg points up (12 o'clock) and sweeps clockwise; the renderer's
    // sweep seam at 0 is the +x axis (3 o'clock). Rotate the `from` angle back a
    // quarter turn so the gradient start lands at the top.
    let start_angle = angle.radians() - FRAC_PI_2;

    // Angular stops -> 0..1 around the turn: an angle as a fraction of the full
    // turn, or the percentage directly.
    let stops = resolve_gradient_stops(items, current, |p| match p {
        AngleOrPercentage::Angle(a) => a.radians() / TAU,
        AngleOrPercentage::Percentage(pc) => pc.0,
    })?;

    Some(ConicGradientItem {
        placement: CommonPlacement::new(LayoutRect::new(
            LayoutPoint::new(0.0, 0.0),
            LayoutPoint::new(w, h),
        )),
        gradient: ConicGradientPayload {
            center: LayoutPoint::new(cx, cy),
            angle: start_angle,
            extend_mode: ExtendMode::Clamp,
            stops,
        },
        tile_size: LayoutSize::new(w, h),
        tile_spacing: LayoutSize::zero(),
    })
}

/// Whether `id` clips its overflow on either axis — i.e. `overflow-x` or
/// `overflow-y` is anything other than `visible` (`hidden`/`scroll`/`auto`/
/// `clip`). Such an element clips its descendants to its padding box (and, for
/// the scrollable values, is a scroll container). `false` when the cascade
/// hasn't run. `pub(crate)` so hit-testing ([`crate::serval_lane`]) clips the
/// same boxes the paint does.
pub(crate) fn clips_overflow(cv: &ComputedValues) -> bool {
    use style::values::computed::Overflow;
    let box_style = cv.get_box();
    !matches!(box_style.overflow_x, Overflow::Visible)
        || !matches!(box_style.overflow_y, Overflow::Visible)
}

/// Whether the box's inline (x) axis is wheel-scrollable: `overflow-x: scroll` or
/// `auto`. `hidden`/`clip` clip but only scroll programmatically, so they are not
/// wheel targets. The per-axis predicate the host's wheel routing walks for.
pub(crate) fn scrolls_overflow_x(cv: &ComputedValues) -> bool {
    use style::values::computed::Overflow;
    matches!(cv.get_box().overflow_x, Overflow::Scroll | Overflow::Auto)
}

/// Whether the box's block (y) axis is wheel-scrollable (`overflow-y: scroll`/`auto`).
pub(crate) fn scrolls_overflow_y(cv: &ComputedValues) -> bool {
    use style::values::computed::Overflow;
    matches!(cv.get_box().overflow_y, Overflow::Scroll | Overflow::Auto)
}

/// Whether `text-overflow: ellipsis` is in effect for line-end truncation: the
/// element asks for an ellipsis *and* clips its inline overflow (`overflow-x` not
/// `visible` — ellipsis only applies when content is clipped). The line-end side
/// is what a left-to-right label truncates; either side reading `ellipsis`
/// qualifies (the common single-value `text-overflow: ellipsis`).
pub(crate) fn text_ellipsis(cv: &ComputedValues) -> bool {
    use style::values::computed::Overflow;
    use style::values::specified::text::TextOverflowSide as Side;
    let to = &cv.get_text().text_overflow;
    let wants = matches!(to.first, Side::Ellipsis) || matches!(to.second, Side::Ellipsis);
    wants && !matches!(cv.get_box().overflow_x, Overflow::Visible)
}

/// Convert Stylo's `computed::Color` to a PaintList `ColorF`.
/// Resolves `currentColor` via the provided `current_color`, then
/// flattens to sRGB and reads the raw `[r, g, b, a]` components.
fn stylo_color_to_paint(
    color: &style::values::computed::Color,
    current_color: style::color::AbsoluteColor,
) -> ColorF {
    let absolute = color.resolve_to_absolute(&current_color);
    let srgb = absolute.into_srgb_legacy();
    let [r, g, b, a] = *srgb.raw_components();
    ColorF::new(r, g, b, a)
}

/// Read an element's `border-radius` from `ComputedValues`, resolved
/// against the border-box size (`w` x `h`) into a paint [`BorderRadius`]
/// (per-corner x/y in px). Percentages resolve against the box per axis
/// (CSS: horizontal radii vs width, vertical vs height). `None` when the
/// cascade has not run or every corner is zero (the common case — keeps
/// the paint stream free of no-op rounded clips). Independent of
/// [`border_of`]: a rounded box need not have a border.
fn border_radius_of(cv: &ComputedValues, w: f32, h: f32) -> Option<BorderRadius> {
    use style::values::computed::Length;
    let b = cv.get_border();
    // A `BorderCornerRadius` is a Size2D of NonNegativeLengthPercentage:
    // `.0.width` is the horizontal radius, `.0.height` the vertical.
    let corner = |c: &style::values::computed::BorderCornerRadius| -> LayoutSize {
        let rx = c.0.width.0.resolve(Length::new(w.max(0.0))).px().max(0.0);
        let ry = c.0.height.0.resolve(Length::new(h.max(0.0))).px().max(0.0);
        LayoutSize::new(rx, ry)
    };
    let radius = BorderRadius {
        top_left: corner(&b.border_top_left_radius),
        top_right: corner(&b.border_top_right_radius),
        bottom_right: corner(&b.border_bottom_right_radius),
        bottom_left: corner(&b.border_bottom_left_radius),
    };
    let zero = |s: LayoutSize| s.width <= 0.0 && s.height <= 0.0;
    if zero(radius.top_left)
        && zero(radius.top_right)
        && zero(radius.bottom_right)
        && zero(radius.bottom_left)
    {
        return None;
    }
    Some(radius)
}

/// Read an element's border (widths + per-side color/style) from
/// `ComputedValues`. Returns `None` if no side has a renderable
/// border (all widths zero or all sides are `none`/`hidden`) — keeps
/// the paint stream uncluttered for un-bordered elements.
fn border_of(cv: &ComputedValues, w: f32, h: f32) -> Option<(LayoutSideOffsets, NormalBorder)> {
    let border = cv.get_border();
    let current_color = cv.get_inherited_text().color;

    let top_w = border.border_top_width.0.to_f32_px();
    let right_w = border.border_right_width.0.to_f32_px();
    let bottom_w = border.border_bottom_width.0.to_f32_px();
    let left_w = border.border_left_width.0.to_f32_px();

    let top_style = stylo_border_style(border.border_top_style);
    let right_style = stylo_border_style(border.border_right_style);
    let bottom_style = stylo_border_style(border.border_bottom_style);
    let left_style = stylo_border_style(border.border_left_style);

    // No-op early-out: every side is zero-width or none/hidden style.
    let renderable =
        |w: f32, s: BorderStyle| w > 0.0 && !matches!(s, BorderStyle::None | BorderStyle::Hidden);
    if !renderable(top_w, top_style)
        && !renderable(right_w, right_style)
        && !renderable(bottom_w, bottom_style)
        && !renderable(left_w, left_style)
    {
        return None;
    }

    let widths = LayoutSideOffsets::new(top_w, right_w, bottom_w, left_w);
    let details = NormalBorder {
        top: BorderSide {
            color: stylo_color_to_paint(&border.border_top_color, current_color),
            style: top_style,
        },
        right: BorderSide {
            color: stylo_color_to_paint(&border.border_right_color, current_color),
            style: right_style,
        },
        bottom: BorderSide {
            color: stylo_color_to_paint(&border.border_bottom_color, current_color),
            style: bottom_style,
        },
        left: BorderSide {
            color: stylo_color_to_paint(&border.border_left_color, current_color),
            style: left_style,
        },
        radius: border_radius_of(cv, w, h).unwrap_or_else(BorderRadius::zero),
        do_aa: true,
    };
    Some((widths, details))
}

/// Paint a `border-image` over the element's border area as a 9-slice: the source
/// is carved into four corners, four edges, and an optional center (`fill`), each
/// drawn to its destination region in the node's local (border-box) coords.
/// Returns `true` when it emitted, so the caller suppresses the normal border (a
/// loaded border-image replaces it, CSS Backgrounds-3 §6).
///
/// v1 scope: `url()` source (decoded into `src`); `border-image-slice`
/// (number = source px, percentage of the source, `fill` for the center);
/// `border-image-width` (number × the side's border-width, length, or `auto` =
/// the intrinsic slice); `border-image-outset` (length, or number × border-width).
/// **Every region is *stretched* to its destination** — `border-image-repeat`
/// `repeat`/`round`/`space` are a follow-up (BI-3); until then they stretch.
fn emit_border_image(
    cv: &ComputedValues,
    src: &DecodedImage,
    l: &taffy::Layout,
    images: &mut ImageCollector,
    commands: &mut Vec<PaintCmd>,
) -> bool {
    use style::values::computed::{NonNegativeNumberOrPercentage as NoP, NumberOrPercentage};
    use style::values::generics::border::BorderImageSideWidth as SideW;
    use style::values::generics::length::GenericLengthOrNumber as LoN;
    use style::values::specified::border::BorderImageRepeatKeyword as RepKw;

    let b = cv.get_border();
    let (sw, sh) = (src.width as f32, src.height as f32);
    if sw <= 0.0 || sh <= 0.0 {
        return false;
    }
    // The element's own border widths (px) per side — the `<number>` basis for
    // border-image-width / -outset.
    let (bw_t, bw_r, bw_b, bw_l) = (l.border.top, l.border.right, l.border.bottom, l.border.left);

    // --- Source slices (px into the source image). number = px, % of the source
    // dimension (top/bottom against height, left/right against width). ---
    let slice_px = |v: &NoP, dim: f32| -> f32 {
        match v.0 {
            NumberOrPercentage::Number(n) => n,
            NumberOrPercentage::Percentage(p) => p.0 * dim,
        }
        .clamp(0.0, dim)
    };
    let off = &b.border_image_slice.offsets;
    let mut st = slice_px(&off.0, sh);
    let mut sr = slice_px(&off.1, sw);
    let mut sb = slice_px(&off.2, sh);
    let mut sl = slice_px(&off.3, sw);
    // Opposite slices may not overlap; if they do, both become 0 (spec: the
    // resulting edge/center has zero size and the slice is treated as 0).
    if sl + sr > sw {
        sl = 0.0;
        sr = 0.0;
    }
    if st + sb > sh {
        st = 0.0;
        sb = 0.0;
    }

    // --- Destination border widths (px) per side. ---
    let width_px = |v: &style::values::computed::BorderImageSideWidth,
                    border_w: f32,
                    intrinsic: f32,
                    area_dim: f32|
     -> f32 {
        match v {
            SideW::Number(n) => n.0 * border_w,
            SideW::LengthPercentage(lp) => {
                lp.0.resolve(style::values::computed::Length::new(area_dim))
                    .px()
            },
            SideW::Auto => intrinsic,
        }
        .max(0.0)
    };
    // --- Outset (px) per side: length, or number × border-width. ---
    let outset_px =
        |v: &style::values::computed::NonNegativeLengthOrNumber, border_w: f32| -> f32 {
            match v {
                LoN::Number(n) => n.0 * border_w,
                LoN::Length(len) => len.0.px(),
            }
            .max(0.0)
        };
    let ow = &b.border_image_outset;
    let (ot, or, ob, ol) = (
        outset_px(&ow.0, bw_t),
        outset_px(&ow.1, bw_r),
        outset_px(&ow.2, bw_b),
        outset_px(&ow.3, bw_l),
    );

    // The border-image area is the border box expanded outward by outset. `l.size`
    // is the border-box size; local coords have its top-left at (0, 0).
    let ax0 = -ol;
    let ay0 = -ot;
    let aw = l.size.width + ol + or;
    let ah = l.size.height + ot + ob;

    let bi = &b.border_image_width;
    let mut wt = width_px(&bi.0, bw_t, st, ah);
    let mut wr = width_px(&bi.1, bw_r, sr, aw);
    let mut wb = width_px(&bi.2, bw_b, sb, ah);
    let mut wl = width_px(&bi.3, bw_l, sl, aw);
    // Clamp so opposite widths fit the area (CSS: scale both down by the same
    // factor when they exceed the area).
    if wl + wr > aw && wl + wr > 0.0 {
        let f = aw / (wl + wr);
        wl *= f;
        wr *= f;
    }
    if wt + wb > ah && wt + wb > 0.0 {
        let f = ah / (wt + wb);
        wt *= f;
        wb *= f;
    }

    // Emit one nine-patch border command: the rasterizer slices the source and
    // draws the corners / edges / fill, UV-sampled from this single image — no
    // producer-side cropping or tiling. The border-image area (border box +
    // outset) is the placement; `widths` are the resolved dest border widths.
    let map_repeat = |kw: RepKw| match kw {
        RepKw::Stretch => RepeatMode::Stretch,
        RepKw::Repeat => RepeatMode::Repeat,
        RepKw::Round => RepeatMode::Round,
        RepKw::Space => RepeatMode::Space,
    };
    let key = images.add(src);
    commands.push(PaintCmd::DrawBorder(BorderItem {
        placement: CommonPlacement::new(LayoutRect::new(
            LayoutPoint::new(ax0, ay0),
            LayoutPoint::new(ax0 + aw, ay0 + ah),
        )),
        widths: LayoutSideOffsets::new(wt, wr, wb, wl),
        details: BorderDetails::NinePatch(NinePatchBorder {
            source: NinePatchSource::Image(key, ImageRendering::Auto),
            width: src.width as i32,
            height: src.height as i32,
            slice: DeviceIntSideOffsets::new(
                st.round() as i32,
                sr.round() as i32,
                sb.round() as i32,
                sl.round() as i32,
            ),
            fill: b.border_image_slice.fill,
            repeat_horizontal: map_repeat(b.border_image_repeat.0),
            repeat_vertical: map_repeat(b.border_image_repeat.1),
        }),
    }));
    true
}

/// Resolved box-shadow params for one shadow (cascade → paint units).
struct ShadowData {
    color: ColorF,
    h: f32,
    v: f32,
    blur: f32,
    spread: f32,
    inset: bool,
}

/// Read an element's cascaded `box-shadow` list. Returns the shadows
/// in declaration order (paint order is back-to-front = last-declared
/// paints first; the producer emits in list order and the renderer's
/// later-paints-on-top handles depth). Empty when no cascade data or
/// no shadows.
fn box_shadows_of(cv: &ComputedValues) -> Vec<ShadowData> {
    let current = cv.get_inherited_text().color;
    cv.get_effects()
        .box_shadow
        .0
        .iter()
        .map(|sh| ShadowData {
            color: stylo_color_to_paint(&sh.base.color, current),
            h: sh.base.horizontal.px(),
            v: sh.base.vertical.px(),
            blur: sh.base.blur.0.px(),
            spread: sh.spread.px(),
            inset: sh.inset,
        })
        .collect()
}

/// A circle/ellipse as a closed `PathData` of four cubic Beziers (the standard
/// kappa approximation), centered `(cx, cy)` with radii `(rx, ry)` in local
/// coords. Starts at 3 o'clock and sweeps clockwise.
fn ellipse_path(cx: f32, cy: f32, rx: f32, ry: f32) -> PathData {
    const K: f32 = 0.552_284_75; // 4/3 * (sqrt(2) - 1)
    let (ox, oy) = (rx * K, ry * K);
    let pt = LayoutPoint::new;
    PathData {
        commands: vec![
            PathCommand::MoveTo(pt(cx + rx, cy)),
            PathCommand::CurveTo {
                control1: pt(cx + rx, cy + oy),
                control2: pt(cx + ox, cy + ry),
                to: pt(cx, cy + ry),
            },
            PathCommand::CurveTo {
                control1: pt(cx - ox, cy + ry),
                control2: pt(cx - rx, cy + oy),
                to: pt(cx - rx, cy),
            },
            PathCommand::CurveTo {
                control1: pt(cx - rx, cy - oy),
                control2: pt(cx - ox, cy - ry),
                to: pt(cx, cy - ry),
            },
            PathCommand::CurveTo {
                control1: pt(cx + ox, cy - ry),
                control2: pt(cx + rx, cy - oy),
                to: pt(cx + rx, cy),
            },
            PathCommand::Close,
        ],
    }
}

/// CSS `clip-path` as a paint-list clip in the element's local (border-box)
/// coordinates, or `None` when there's no clip serval emits. Handles the basic
/// shapes `polygon()` / `circle()` / `ellipse()`; `inset()` / `rect()` / `path()`
/// / `url()` and a reference box other than the border box are follow-ups (the
/// box clips normally meanwhile). The border box is `(0,0)..(w,h)` here — the
/// space inside the fragment's transform.
fn clip_path_of(cv: &ComputedValues, w: f32, h: f32) -> Option<ClipKind> {
    use style::values::computed::Length;
    use style::values::generics::basic_shape::{BasicShape, ClipPath, ShapeRadius};
    use style::values::generics::position::PositionOrAuto;

    let shape = match cv.get_svg().clip_path {
        ClipPath::Shape(ref shape, _reference_box) => shape,
        _ => return None,
    };
    let (cw, ch) = (w.max(0.0), h.max(0.0));
    // `at <position>` center (default center). `Position` resolves keywords to
    // percentages at compute time, so the components are plain LengthPercentages.
    let center = |pos: &PositionOrAuto<style::values::computed::Position>| -> (f32, f32) {
        match pos {
            PositionOrAuto::Auto => (w / 2.0, h / 2.0),
            PositionOrAuto::Position(p) => (
                p.horizontal.resolve(Length::new(cw)).px(),
                p.vertical.resolve(Length::new(ch)).px(),
            ),
        }
    };
    match **shape {
        BasicShape::Polygon(ref poly) => {
            if poly.coordinates.is_empty() {
                return None;
            }
            let commands = poly
                .coordinates
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let pt = LayoutPoint::new(
                        c.0.resolve(Length::new(cw)).px(),
                        c.1.resolve(Length::new(ch)).px(),
                    );
                    if i == 0 {
                        PathCommand::MoveTo(pt)
                    } else {
                        PathCommand::LineTo(pt)
                    }
                })
                .chain(std::iter::once(PathCommand::Close))
                .collect();
            Some(ClipKind::Path(PathData { commands }))
        },
        BasicShape::Circle(ref c) => {
            let (cx, cy) = center(&c.position);
            // CSS: a `%` circle radius resolves against `sqrt(w² + h²) / sqrt(2)`.
            let r = match c.radius {
                ShapeRadius::Length(ref nn) => {
                    let basis = (cw * cw + ch * ch).sqrt() / std::f32::consts::SQRT_2;
                    nn.0.resolve(Length::new(basis)).px()
                },
                ShapeRadius::ClosestSide => cx.min(cy).min(w - cx).min(h - cy).max(0.0),
                ShapeRadius::FarthestSide => cx.max(cy).max(w - cx).max(h - cy).max(0.0),
            };
            Some(ClipKind::Path(ellipse_path(cx, cy, r, r)))
        },
        BasicShape::Ellipse(ref e) => {
            let (cx, cy) = center(&e.position);
            let axis = |r: &ShapeRadius<style::values::computed::LengthPercentage>,
                        c: f32,
                        near: f32,
                        basis: f32|
             -> f32 {
                match r {
                    ShapeRadius::Length(nn) => nn.0.resolve(Length::new(basis)).px(),
                    ShapeRadius::ClosestSide => c.min(near).max(0.0),
                    ShapeRadius::FarthestSide => c.max(near).max(0.0),
                }
            };
            let rx = axis(&e.semiaxis_x, cx, w - cx, cw);
            let ry = axis(&e.semiaxis_y, cy, h - cy, ch);
            Some(ClipKind::Path(ellipse_path(cx, cy, rx, ry)))
        },
        // `inset()` (computed `<basic-shape-rect>`): an inset rect. CSS order is
        // top / right / bottom / left. First cut: sharp corners (the `round`
        // radius is deferred); the box clips to the rect meanwhile.
        BasicShape::Rect(ref inset) => {
            let top = inset.rect.0.resolve(Length::new(ch)).px();
            let right = inset.rect.1.resolve(Length::new(cw)).px();
            let bottom = inset.rect.2.resolve(Length::new(ch)).px();
            let left = inset.rect.3.resolve(Length::new(cw)).px();
            let x1 = (w - right).max(left);
            let y1 = (h - bottom).max(top);
            Some(ClipKind::Rect(LayoutRect::new(
                LayoutPoint::new(left, top),
                LayoutPoint::new(x1, y1),
            )))
        },
        // `path()` / `shape()` are follow-ups.
        BasicShape::PathOrShape(_) => None,
    }
}

/// Read an element's cascaded `opacity` as `Some(alpha)` when it is less than
/// fully opaque — the signal to wrap the element + its in-flow subtree in an
/// isolated stacking layer the renderer composites at `alpha`. Group opacity
/// (one layer for the whole subtree) is the correct CSS semantics: overlapping
/// descendants composite once, so they do not double-darken the way a
/// per-primitive alpha multiply would. `None` for the opaque default (1.0, the
/// common case) — painted with no layer. Clamped to `[0, 1]`.
fn opacity_of(cv: &ComputedValues) -> Option<f32> {
    let a = f32::from(cv.get_effects().opacity);
    (a < 1.0).then_some(a.clamp(0.0, 1.0))
}

/// CSS `mix-blend-mode` as a paint-list blend mode, or `None` for `normal` (the
/// common case — no blend, so no stacking layer is forced on its account). A
/// non-normal mode wraps the element + subtree in an isolated layer that the
/// renderer composites back into its backdrop with that blend (the canonical CSS
/// set maps 1:1).
fn mix_blend_mode_of(cv: &ComputedValues) -> Option<MixBlendMode> {
    use style::computed_values::mix_blend_mode::T as M;
    Some(match cv.get_effects().mix_blend_mode {
        M::Normal => return None,
        M::Multiply => MixBlendMode::Multiply,
        M::Screen => MixBlendMode::Screen,
        M::Overlay => MixBlendMode::Overlay,
        M::Darken => MixBlendMode::Darken,
        M::Lighten => MixBlendMode::Lighten,
        M::ColorDodge => MixBlendMode::ColorDodge,
        M::ColorBurn => MixBlendMode::ColorBurn,
        M::HardLight => MixBlendMode::HardLight,
        M::SoftLight => MixBlendMode::SoftLight,
        M::Difference => MixBlendMode::Difference,
        M::Exclusion => MixBlendMode::Exclusion,
        M::Hue => MixBlendMode::Hue,
        M::Saturation => MixBlendMode::Saturation,
        M::Color => MixBlendMode::Color,
        M::Luminosity => MixBlendMode::Luminosity,
        M::PlusLighter => MixBlendMode::PlusLighter,
    })
}

/// One computed CSS `filter` function as a paint-list `FilterOp`, or `None` for
/// the functions serval doesn't emit yet. `blur()` carries a px radius;
/// `brightness/contrast/saturate` a `NonNegative<Number>` and
/// `grayscale/invert/sepia/opacity` a `ZeroToOne<Number>` (both unwrap to
/// `f32`); `hue-rotate` an angle in degrees. `drop-shadow()` is deferred
/// end-to-end (no `SceneFilter::DropShadow` yet), and `url()` (an SVG filter
/// reference) is uninhabited under the servo feature.
fn filter_op_of(f: &style::values::computed::effects::Filter) -> Option<FilterOp> {
    use style::values::generics::effects::GenericFilter as GF;
    Some(match f {
        GF::Blur(len) => FilterOp::Blur(len.0.px()),
        GF::Brightness(n) => FilterOp::Brightness(n.0),
        GF::Contrast(n) => FilterOp::Contrast(n.0),
        GF::Saturate(n) => FilterOp::Saturate(n.0),
        GF::Grayscale(n) => FilterOp::Grayscale(n.0),
        GF::Invert(n) => FilterOp::Invert(n.0),
        GF::Sepia(n) => FilterOp::Sepia(n.0),
        GF::HueRotate(a) => FilterOp::HueRotate(a.degrees()),
        GF::Opacity(n) => FilterOp::Opacity(n.0),
        GF::DropShadow(_) | GF::Url(_) => return None,
    })
}

/// CSS `filter` as a paint-list filter chain, or an empty vec for `filter: none`
/// (the common case — no chain, so no stacking layer is forced on its account).
/// A non-empty chain wraps the element + subtree in an isolated layer whose
/// rasterized output the renderer filters before compositing.
fn filters_of(cv: &ComputedValues) -> Vec<FilterOp> {
    cv.get_effects()
        .filter
        .0
        .iter()
        .filter_map(filter_op_of)
        .collect()
}

/// A transform `m` applied around the absolute point `origin`: `T(O)·M·T(-O)`.
/// CSS transforms apply in the element's box-local frame, so an ancestor's
/// transform that should affect a deferred descendant must be conjugated by the
/// ancestor's absolute box origin. These conjugated factors telescope through
/// layout-absolute origins, so composing them gives the exact cumulative ancestor
/// transform for a lifted layer (see [`Deferred::ancestor_transform`]).
pub(crate) fn conjugate_at(origin: (f32, f32), m: LayoutTransform) -> LayoutTransform {
    let (ox, oy) = origin;
    LayoutTransform::translation(-ox, -oy, 0.0)
        .then(&m)
        .then(&LayoutTransform::translation(ox, oy, 0.0))
}

/// Fold the element's computed CSS `transform` + `translate` into a
/// `LayoutTransform`. Identity when neither is set (the common case). The CSS
/// used transform applies the `translate` longhand first, then the `transform`
/// list. v1 resolves a percentage `translate` against a zero reference box —
/// absolute px (the orrery's `transform:translate(px,px)`) is exact; a
/// fragment-rect reference box for `%` is a follow-up. `rotate`/`scale` longhands
/// and 3D (preserve-3d/perspective) are out of scope.
pub(crate) fn compute_transform_matrix(cv: &ComputedValues) -> LayoutTransform {
    use app_units::Au;
    use style::values::generics::transform::GenericTranslate as Tr;

    let box_style = cv.get_box();

    // `translate` longhand (the separate `translate:` property).
    let translate = match &box_style.translate {
        Tr::None => LayoutTransform::identity(),
        Tr::Translate(x, y, z) => LayoutTransform::translation(
            x.to_used_value(Au(0)).to_f32_px(),
            y.to_used_value(Au(0)).to_f32_px(),
            z.px(),
        ),
    };

    // `transform` list — e.g. `transform: translate(x, y)`, the orrery's path.
    let transform = box_style
        .transform
        .to_transform_3d_matrix(None)
        .map(|(m, _is_3d)| {
            LayoutTransform::new(
                m.m11, m.m12, m.m13, m.m14, m.m21, m.m22, m.m23, m.m24, m.m31, m.m32, m.m33, m.m34,
                m.m41, m.m42, m.m43, m.m44,
            )
        })
        .unwrap_or_else(|_| LayoutTransform::identity());

    translate.then(&transform)
}

/// Map Stylo's specified BorderStyle to paint-types BorderStyle.
fn stylo_border_style(s: style::values::specified::border::BorderStyle) -> BorderStyle {
    use style::values::specified::border::BorderStyle as S;
    match s {
        S::None => BorderStyle::None,
        S::Solid => BorderStyle::Solid,
        S::Double => BorderStyle::Double,
        S::Dotted => BorderStyle::Dotted,
        S::Dashed => BorderStyle::Dashed,
        S::Hidden => BorderStyle::Hidden,
        S::Groove => BorderStyle::Groove,
        S::Ridge => BorderStyle::Ridge,
        S::Inset => BorderStyle::Inset,
        S::Outset => BorderStyle::Outset,
    }
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use paint_list_api::PaintList;
    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::cascade::run_cascade;
    use crate::image_decode::ImagePlane;
    use crate::layout::layout;

    /// `text-overflow: ellipsis` (+ `overflow: hidden`) is read from the cascade
    /// — i.e. the property is parse-enabled in serval's stylo, not gated off.
    #[test]
    fn text_overflow_ellipsis_is_read_from_cascade() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["p { text-overflow: ellipsis; overflow: hidden; white-space: nowrap; }"],
            None,
        );
        let p = {
            let mut q = vec![document.document()];
            let mut found = None;
            while let Some(id) = q.pop() {
                if document
                    .element_name(id)
                    .is_some_and(|n| n.local == local_name!("p"))
                {
                    found = Some(id);
                    break;
                }
                q.extend(document.dom_children(id));
            }
            found.expect("<p>")
        };
        let cv = primary_cv(&styles, p).expect("p cascade");
        assert!(
            text_ellipsis(&cv),
            "text-overflow: ellipsis + overflow:hidden must be active"
        );
    }

    /// `text-overflow: ellipsis` truncates an overflowing single line: glyphs past
    /// the content edge are dropped and a trailing `…` is drawn. A narrow box with
    /// ellipsis emits fewer glyphs than the same text laid out wide (untruncated),
    /// and every emitted glyph sits within the content box. (Chrome-UI labels.)
    #[test]
    fn text_overflow_ellipsis_truncates_overflowing_line() {
        // All glyph x-positions emitted across the paint list (text leaves only).
        let glyph_xs = |sheet: &str| -> Vec<f32> {
            let document = StaticDocument::parse(
                "<html><body><p>aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</p></body></html>",
            );
            let plist = emit_with_sheet(&document, sheet);
            plist
                .commands()
                .iter()
                .filter_map(|c| match c {
                    PaintCmd::DrawText(t) => Some(t.glyphs.iter().map(|g| g.point.x)),
                    _ => None,
                })
                .flatten()
                .collect()
        };

        let base = "html, body, p { display: block; margin: 0; font-size: 16px; } \
                    p { white-space: nowrap; overflow: hidden; ";
        // Wide box: the whole string fits, no truncation.
        let wide = glyph_xs(&format!("{base} width: 800px; }}"));
        // Narrow box with ellipsis: the line overflows and is truncated.
        let narrow = glyph_xs(&format!("{base} width: 60px; text-overflow: ellipsis; }}"));

        assert!(
            narrow.len() < wide.len(),
            "ellipsis drops glyphs: narrow {} should be fewer than wide {}",
            narrow.len(),
            wide.len()
        );
        assert!(
            !narrow.is_empty(),
            "some glyphs (incl. the ellipsis) still draw"
        );
        // Every emitted glyph — the kept run plus the trailing `…` — sits within
        // the 60px content box (a small epsilon for the ellipsis glyph advance).
        let max_x = narrow.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            max_x <= 60.0 + 2.0,
            "all glyphs stay within the content box, max_x = {max_x}"
        );
    }

    /// Threading guard. The two stages a future parallel pipeline would move
    /// across threads must stay `Send`: the shaped-text cache (one
    /// [`TextMeasureCtx`] moved per shaping worker) and the produced
    /// [`ServalPaintList`] (handed to the renderer / serialized across IPC, as
    /// `PaintEnvelope` already does). If a contributor introduces a non-`Send`
    /// type into either — an `Rc`, a bare `RefCell`, a `!Send` handle — this stops
    /// compiling, flagging the regression before it becomes a parallelization
    /// blocker. See the threading scoping discussion (2026-06-12).
    #[test]
    fn paint_and_shaping_stages_stay_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ServalPaintList>();
        assert_send::<TextMeasureCtx>();
    }

    /// `opacity < 1` wraps the element and its in-flow subtree in one isolated
    /// stacking layer (group opacity), composited at `alpha`; the opaque default
    /// opens no layer. The faded subtree's content paints inside the layer.
    #[test]
    fn opacity_wraps_subtree_in_a_stacking_layer() {
        let document = StaticDocument::parse("<html><body><p>hi</p></body></html>");

        // Opaque default: no layer at all.
        let opaque = emit_with_sheet(&document, "p { display: block; }");
        assert!(
            !opaque
                .commands()
                .iter()
                .any(|c| matches!(c, PaintCmd::PushLayer(_))),
            "the opaque default opens no stacking layer"
        );

        // opacity: 0.5 on <p>: exactly one balanced layer at alpha 0.5.
        let faded = emit_with_sheet(&document, "p { display: block; opacity: 0.5; }");
        let cmds = faded.commands();
        let alphas: Vec<f32> = cmds
            .iter()
            .filter_map(|c| match c {
                PaintCmd::PushLayer(s) => Some(s.opacity),
                _ => None,
            })
            .collect();
        assert_eq!(alphas.len(), 1, "exactly one opacity layer");
        assert!(
            (alphas[0] - 0.5).abs() < 1e-6,
            "layer alpha is 0.5, got {}",
            alphas[0]
        );
        assert_eq!(
            cmds.iter()
                .filter(|c| matches!(c, PaintCmd::PopLayer))
                .count(),
            1,
            "the layer is balanced by one PopLayer"
        );
        // The <p>'s text paints between the push and the pop — inside the group.
        let push_i = cmds
            .iter()
            .position(|c| matches!(c, PaintCmd::PushLayer(_)))
            .unwrap();
        let pop_i = cmds
            .iter()
            .rposition(|c| matches!(c, PaintCmd::PopLayer))
            .unwrap();
        assert!(
            cmds[push_i..pop_i]
                .iter()
                .any(|c| matches!(c, PaintCmd::DrawText(_))),
            "the faded subtree's text is composited inside the layer"
        );
    }

    /// Cascade-driven style plane sizing block elements 200×50 (no
    /// spacing). The box tree reads `ComputedValues`, so emit tests now
    /// drive layout through the cascade rather than hand-built styles.
    fn build_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            document,
            &mut plane,
            euclid::Size2D::new(800.0, 600.0),
            &["p, div { display: block; width: 200px; height: 50px; }"],
            None,
        );
        plane
    }

    #[test]
    fn emit_produces_drawrect_for_each_element_and_drawtext_for_text() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);

        // <p> is an inline-context leaf carrying "Hello"; its text is
        // emitted via the cached layout, so use the with-layouts path
        // (the cache-less emit_paint_list emits boxes only).
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &crate::image_decode::ImagePlane::new(),
            &crate::image_decode::BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        // Trait accessor sanity.
        assert_eq!(plist.engine_id(), EngineId::SERVAL);
        assert_eq!(plist.viewport(), DeviceIntSize::new(800, 600));

        let mut rect_count = 0;
        let mut text_count = 0;
        for cmd in plist.commands() {
            match cmd {
                PaintCmd::DrawRect(_) => rect_count += 1,
                PaintCmd::DrawText(_) => text_count += 1,
                _ => {},
            }
        }

        // html, body, p — at least three element rects.
        assert!(
            rect_count >= 3,
            "expected at least 3 DrawRects (html/body/p), got {rect_count}"
        );
        // "Hello" — at least one text run (from <p>'s inline content).
        assert!(
            text_count >= 1,
            "expected at least 1 DrawText, got {text_count}"
        );
    }

    /// Emit a paint list for `document` cascaded with `sheet`, at 800×600.
    fn emit_with_sheet(document: &StaticDocument, sheet: &str) -> ServalPaintList {
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(document, &styles, &ImagePlane::new(), viewport);
        emit_paint_list_with_layouts(
            document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        )
    }

    /// Emit `html` cascaded with `sheet` at 800×600 with an explicit document
    /// (viewport) scroll, driving the private `emit_inner` directly. The public
    /// entry points pass zero document scroll until the host threads the viewport
    /// offset through the live pipeline (Phase C / incremental), so the scrolled
    /// path is exercised here at the engine boundary.
    fn emit_scrolled(html: &str, sheet: &str, scroll: (f32, f32)) -> Vec<PaintCmd> {
        let document = StaticDocument::parse(html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        emit_inner(
            &document,
            &styles,
            &fragments,
            &built,
            Some(&text_ctx),
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
            scroll,
            (0.0, 0.0),
            built.root_arena(),
            None,
        )
        .commands()
        .to_vec()
    }

    struct StubLeaves(Vec<PaintCmd>);
    impl LeafPaintSource for StubLeaves {
        fn leaf_commands(&self, key: u64) -> Option<&[PaintCmd]> {
            (key == 7).then_some(self.0.as_slice())
        }
    }

    /// End-to-end: a `<chisel-leaf key="7">` element is recognized as a replaced
    /// leaf (construct), carries its key onto the box (box_tree), and paint pulls
    /// its Path-A commands from the `LeafPaintSource` and splices them.
    #[test]
    fn chisel_leaf_splices_its_path_a_commands() {
        let document = StaticDocument::parse(
            "<html><body><chisel-leaf key='7' style='width:20px;height:10px'></chisel-leaf></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);

        // The leaf's Path-A output: one distinctive green rect in local coords.
        let green = ColorF {
            r: 0.1,
            g: 0.9,
            b: 0.2,
            a: 1.0,
        };
        let leaves = StubLeaves(vec![PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(LayoutRect::new(
                LayoutPoint::new(0.0, 0.0),
                LayoutPoint::new(20.0, 10.0),
            )),
            color: green,
        })]);

        let plist = emit_paint_list_with_leaves(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
            &leaves,
        );
        assert!(
            has_rect_rgb(&plist, (0.1, 0.9, 0.2)),
            "the chisel leaf's Path-A command should appear in the paint list"
        );

        // A source with no matching key splices nothing.
        let empty = StubLeaves(Vec::new());
        let plist2 = emit_paint_list_with_leaves(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
            &empty,
        );
        assert!(!has_rect_rgb(&plist2, (0.1, 0.9, 0.2)));
    }

    /// A chisel leaf with CSS border + padding paints in its content box: the
    /// splice is wrapped in a `PushTransform` at the content-box origin (border +
    /// padding), not at the border-box origin.
    #[test]
    fn chisel_leaf_offsets_commands_into_its_content_box() {
        let document = StaticDocument::parse(
            "<html><body><chisel-leaf key='7' style='width:20px;height:10px;padding:10px;border:5px solid'></chisel-leaf></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);

        let green = ColorF {
            r: 0.1,
            g: 0.9,
            b: 0.2,
            a: 1.0,
        };
        let leaves = StubLeaves(vec![PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(LayoutRect::new(
                LayoutPoint::new(0.0, 0.0),
                LayoutPoint::new(20.0, 10.0),
            )),
            color: green,
        })]);
        let plist = emit_paint_list_with_leaves(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
            &leaves,
        );

        let cmds = plist.commands();
        let gi = cmds
            .iter()
            .position(|c| {
                matches!(c, PaintCmd::DrawRect(r)
                    if (r.color.r - 0.1).abs() < 0.05
                        && (r.color.g - 0.9).abs() < 0.05
                        && (r.color.b - 0.2).abs() < 0.05)
            })
            .expect("green leaf rect present");
        assert!(gi > 0, "the leaf rect is not the first command");
        // border(5) + padding(10) = 15px content offset on each axis.
        assert!(
            matches!(&cmds[gi - 1], PaintCmd::PushTransform(spec)
                if (spec.origin.x - 15.0).abs() < 0.01 && (spec.origin.y - 15.0).abs() < 0.01),
            "a content-offset PushTransform (15,15) should precede the leaf commands, got {:?}",
            cmds[gi - 1]
        );
        assert!(
            matches!(&cmds[gi + 1], PaintCmd::PopTransform),
            "a PopTransform should follow the leaf commands"
        );
    }

    fn has_rect_rgb(plist: &ServalPaintList, rgb: (f32, f32, f32)) -> bool {
        plist.commands().iter().any(|cmd| {
            matches!(cmd, PaintCmd::DrawRect(rect)
                if (rect.color.r - rgb.0).abs() < 0.05
                    && (rect.color.g - rgb.1).abs() < 0.05
                    && (rect.color.b - rgb.2).abs() < 0.05)
        })
    }

    fn first_push_origin(plist: &ServalPaintList) -> Option<(f32, f32)> {
        plist.commands().iter().find_map(|cmd| match cmd {
            PaintCmd::PushTransform(spec) => Some((spec.origin.x, spec.origin.y)),
            _ => None,
        })
    }

    #[test]
    fn emit_excluding_subtrees_skips_the_named_root() {
        let document = StaticDocument::parse(
            "<html><body><div class='keep'></div><div class='skip'></div></body></html>",
        );
        let sheet = concat!(
            "html, body { display:block; margin:0; padding:0; background:transparent; }",
            ".keep, .skip { display:block; width:120px; height:40px; }",
            ".keep { background: rgb(255, 0, 0); }",
            ".skip { background: rgb(0, 0, 255); }",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let skip = document
            .first_with_class(document.document(), "skip")
            .expect(".skip root");
        let mut skipped = FxHashSet::default();
        skipped.insert(skip);

        let plist = emit_paint_list_scrolled_excluding_subtrees(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            &skipped,
            DeviceIntSize::new(800, 600),
            (0.0, 0.0),
        );

        assert!(
            has_rect_rgb(&plist, (1.0, 0.0, 0.0)),
            "the kept subtree still paints its red rect"
        );
        assert!(
            !has_rect_rgb(&plist, (0.0, 0.0, 1.0)),
            "the skipped subtree's blue rect is absent from the base paint list"
        );
    }

    #[test]
    fn emit_subtree_localizes_the_root_origin() {
        let document = StaticDocument::parse(
            "<html><body><div class='pane'><div class='inner'></div></div></body></html>",
        );
        let sheet = concat!(
            "html, body { display:block; margin:0; padding:0; position:relative; }",
            ".pane { position:absolute; left:120px; top:80px; width:90px; height:50px; ",
            "background: rgb(255, 0, 0); }",
            ".inner { display:block; width:30px; height:20px; background: rgb(0, 0, 255); }",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let pane = document
            .first_with_class(document.document(), "pane")
            .expect(".pane root");
        let plist = emit_subtree_paint_list_scrolled(
            &document,
            pane,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(90, 50),
        )
        .expect("subtree paint list");

        assert_eq!(
            first_push_origin(&plist),
            Some((0.0, 0.0)),
            "the subtree root paints in its own local coordinate space"
        );
        assert!(
            has_rect_rgb(&plist, (1.0, 0.0, 0.0)) && has_rect_rgb(&plist, (0.0, 0.0, 1.0)),
            "the subtree emit still includes the root and its descendant content"
        );
    }

    fn data_uri_png(width: u32, height: u32) -> String {
        use base64::Engine as _;

        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 255, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode PNG");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        format!("data:image/png;base64,{b64}")
    }

    fn emit_img_commands(intrinsic: (u32, u32), img_rule: &str) -> Vec<PaintCmd> {
        let uri = data_uri_png(intrinsic.0, intrinsic.1);
        let document =
            StaticDocument::parse(&format!("<html><body><img src=\"{uri}\"></body></html>"));
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        let sheet =
            format!("html, body {{ display: block; margin: 0; padding: 0; }} img {{ {img_rule} }}");
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet.as_str()],
            None,
        );
        let images = ImagePlane::decode_from_dom(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &images, viewport);
        emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &images,
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        )
        .commands()
        .to_vec()
    }

    fn emit_external_texture_commands(element: &str, rule: &str) -> Vec<PaintCmd> {
        let document = StaticDocument::parse(&format!("<html><body>{element}</body></html>"));
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        let sheet = format!(
            "html, body {{ display: block; margin: 0; padding: 0; }} \
             canvas, video, external-texture {{ {rule} }}"
        );
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet.as_str()],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        )
        .commands()
        .to_vec()
    }

    fn image_bounds(cmds: &[PaintCmd]) -> LayoutRect {
        let images: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawImage(i) => Some(i.placement.bounds),
                _ => None,
            })
            .collect();
        assert_eq!(
            images.len(),
            1,
            "expected one DrawImage, got {}",
            images.len()
        );
        images[0]
    }

    fn external_texture_bounds(cmds: &[PaintCmd]) -> LayoutRect {
        let textures: Vec<_> = cmds
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawExternalTexture(e) => Some(e.placement.bounds),
                _ => None,
            })
            .collect();
        assert_eq!(
            textures.len(),
            1,
            "expected one DrawExternalTexture, got {}",
            textures.len()
        );
        textures[0]
    }

    fn assert_rect(rect: LayoutRect, min_x: f32, min_y: f32, max_x: f32, max_y: f32) {
        assert!(
            approx(rect.min.x, min_x),
            "min.x expected {min_x}, got {}",
            rect.min.x
        );
        assert!(
            approx(rect.min.y, min_y),
            "min.y expected {min_y}, got {}",
            rect.min.y
        );
        assert!(
            approx(rect.max.x, max_x),
            "max.x expected {max_x}, got {}",
            rect.max.x
        );
        assert!(
            approx(rect.max.y, max_y),
            "max.y expected {max_y}, got {}",
            rect.max.y
        );
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.5
    }

    #[test]
    fn object_fit_cover_centers_and_clips_block_img() {
        let cmds = emit_img_commands(
            (200, 100),
            "display:block; width:100px; height:100px; object-fit:cover; object-position:center;",
        );
        assert_rect(image_bounds(&cmds), -50.0, 0.0, 150.0, 100.0);

        let draw_i = cmds
            .iter()
            .position(|c| matches!(c, PaintCmd::DrawImage(_)))
            .unwrap();
        assert!(
            cmds[..draw_i].iter().any(|c| matches!(
                c,
                PaintCmd::PushClip(ClipSpec { kind: ClipKind::Rect(r) })
                    if approx(r.min.x, 0.0)
                        && approx(r.min.y, 0.0)
                        && approx(r.max.x, 100.0)
                        && approx(r.max.y, 100.0)
            )),
            "cover image must be clipped to the content box"
        );
        assert!(
            cmds[draw_i + 1..]
                .iter()
                .any(|c| matches!(c, PaintCmd::PopClip)),
            "cover clip must be balanced after DrawImage"
        );
    }

    #[test]
    fn object_fit_contain_preserves_aspect_inside_block_img() {
        let cmds = emit_img_commands(
            (200, 100),
            "display:block; width:100px; height:100px; object-fit:contain; object-position:center;",
        );
        assert_rect(image_bounds(&cmds), 0.0, 25.0, 100.0, 75.0);
    }

    #[test]
    fn object_fit_none_uses_intrinsic_size() {
        let cmds = emit_img_commands(
            (40, 20),
            "display:block; width:100px; height:100px; object-fit:none; object-position:center;",
        );
        assert_rect(image_bounds(&cmds), 30.0, 40.0, 70.0, 60.0);
    }

    #[test]
    fn object_fit_scale_down_does_not_upscale() {
        let cmds = emit_img_commands(
            (40, 20),
            "display:block; width:100px; height:100px; object-fit:scale-down; object-position:center;",
        );
        assert_rect(image_bounds(&cmds), 30.0, 40.0, 70.0, 60.0);
    }

    #[test]
    fn object_position_offsets_cover_image() {
        let cmds = emit_img_commands(
            (200, 100),
            "display:block; width:100px; height:100px; object-fit:cover; object-position:left top;",
        );
        assert_rect(image_bounds(&cmds), 0.0, 0.0, 200.0, 100.0);
    }

    #[test]
    fn object_fit_paints_inside_content_box_with_padding_and_border() {
        let cmds = emit_img_commands(
            (20, 20),
            "display:block; width:100px; height:100px; padding:10px; border:5px solid black; object-fit:fill;",
        );
        assert_rect(image_bounds(&cmds), 15.0, 15.0, 115.0, 115.0);
    }

    #[test]
    fn object_fit_contain_applies_to_canvas_external_texture() {
        let cmds = emit_external_texture_commands(
            "<canvas data-serval-external-texture-key=\"7\" width=\"300\" height=\"150\"></canvas>",
            "display:block; width:100px; height:100px; object-fit:contain; object-position:center;",
        );
        assert_rect(external_texture_bounds(&cmds), 0.0, 25.0, 100.0, 75.0);
    }

    #[test]
    fn object_fit_cover_clips_video_external_texture() {
        let cmds = emit_external_texture_commands(
            "<video data-serval-external-texture-key=\"9\" width=\"300\" height=\"150\"></video>",
            "display:block; width:100px; height:100px; object-fit:cover; object-position:left top;",
        );
        assert_rect(external_texture_bounds(&cmds), 0.0, 0.0, 200.0, 100.0);
        let draw_i = cmds
            .iter()
            .position(|c| matches!(c, PaintCmd::DrawExternalTexture(_)))
            .unwrap();
        assert!(
            cmds[..draw_i].iter().any(|c| matches!(
                c,
                PaintCmd::PushClip(ClipSpec { kind: ClipKind::Rect(r) })
                    if approx(r.min.x, 0.0)
                        && approx(r.min.y, 0.0)
                        && approx(r.max.x, 100.0)
                        && approx(r.max.y, 100.0)
            )),
            "cover texture must be clipped to the content box"
        );
        assert!(
            cmds[draw_i + 1..]
                .iter()
                .any(|c| matches!(c, PaintCmd::PopClip)),
            "cover texture clip must be balanced after DrawExternalTexture"
        );
    }

    /// Index of the first `PushTransform` whose translate `origin` is `(x, y)`
    /// (within 0.5px). The document-scroll wrap and the fixed counter are both
    /// pure translates, so they are findable by their offset.
    fn push_translate_index(cmds: &[PaintCmd], x: f32, y: f32) -> Option<usize> {
        cmds.iter().position(|c| {
            matches!(c, PaintCmd::PushTransform(t)
                if (t.origin.x - x).abs() < 0.5 && (t.origin.y - y).abs() < 0.5)
        })
    }

    /// A2 — document scroll: the whole document paints inside a `-scroll` translate
    /// (CSS Overflow §3.3 viewport scroll), while the canvas background — painted
    /// over the viewport *before* the wrap — does not move with it. An unscrolled
    /// frame emits no wrap at all (byte-identical to the pre-scroll engine).
    #[test]
    fn document_scroll_translates_in_flow_but_not_the_canvas_background() {
        // A body background propagates to the canvas; the tall div makes the
        // document taller than the 600px viewport.
        let html = "<html><body><div class=\"tall\"></div></body></html>";
        let sheet = "html, body, div { display: block; margin: 0; } \
                     body { background-color: rgb(0, 128, 0); } \
                     .tall { height: 2000px; }";

        // Unscrolled: no document-scroll wrap.
        let still = emit_scrolled(html, sheet, (0.0, 0.0));
        assert!(
            push_translate_index(&still, 0.0, -120.0).is_none(),
            "an unscrolled document emits no scroll wrap"
        );

        // Scrolled down 120px: the content wraps in a translate that shifts it up
        // by 120 (origin (0, -120)).
        let scrolled = emit_scrolled(html, sheet, (0.0, 120.0));
        let wrap = push_translate_index(&scrolled, 0.0, -120.0)
            .expect("the scrolled document wraps its content in a -120 translate");

        // The canvas background (a full-viewport green DrawRect) paints before the
        // wrap, so it stays put while the document scrolls under it.
        let canvas_bg = scrolled
            .iter()
            .position(|c| {
                matches!(c, PaintCmd::DrawRect(r)
                    if r.color.g > 0.4 && r.color.r < 0.1 && r.color.b < 0.1
                        && (r.placement.bounds.max.y - r.placement.bounds.min.y - 600.0).abs() < 1.0)
            })
            .expect("the body background propagates to a full-viewport canvas rect");
        assert!(
            canvas_bg < wrap,
            "the canvas background (idx {canvas_bg}) paints before the scroll wrap (idx {wrap}) — \
             it does not scroll with the document"
        );
    }

    /// A3 — `position: fixed` attaches to the viewport: under document scroll its
    /// stacking layer is counter-translated by `+scroll`, cancelling the document
    /// wrap so it stays pinned. An `absolute` box is not (Fixed≠Absolute): it
    /// scrolls with the document, so no counter is emitted.
    #[test]
    fn fixed_layer_counters_document_scroll_but_absolute_does_not() {
        let sheet = "html, body, div { display: block; margin: 0; } \
                     .fixed { position: fixed; top: 0; left: 0; width: 50px; height: 50px; } \
                     .abs { position: absolute; top: 0; left: 0; width: 50px; height: 50px; } \
                     .tall { height: 2000px; }";

        // A fixed element under a 90px document scroll: a `+90` counter-translate
        // cancels the document's `-90` wrap, so it stays pinned to the viewport.
        let fixed = emit_scrolled(
            "<html><body><div class=\"fixed\"></div><div class=\"tall\"></div></body></html>",
            sheet,
            (0.0, 90.0),
        );
        assert!(
            push_translate_index(&fixed, 0.0, -90.0).is_some(),
            "the document still wraps its in-flow content in a -90 translate"
        );
        assert!(
            push_translate_index(&fixed, 0.0, 90.0).is_some(),
            "the fixed layer counters the scroll with a +90 translate (stays pinned)"
        );

        // An absolute element in the same place scrolls with the document: no
        // counter-translate is emitted — it rides the -90 document wrap.
        let abs = emit_scrolled(
            "<html><body><div class=\"abs\"></div><div class=\"tall\"></div></body></html>",
            sheet,
            (0.0, 90.0),
        );
        assert!(
            push_translate_index(&abs, 0.0, 90.0).is_none(),
            "an absolute layer is not viewport-attached — it scrolls with the document"
        );
    }

    /// A block-`display` `::before` paints its own box: its background `DrawRect`
    /// appears in the stream, sized to its laid-out block box (full container
    /// width, its own height) — the box-tree-rooted walk renders the synthetic
    /// pseudo box with no DOM node behind it. (Pseudo follow-ups §5 slice 3.)
    #[test]
    fn block_before_pseudo_paints_its_box() {
        let document = StaticDocument::parse("<html><body><p>hi</p></body></html>");
        let plist = emit_with_sheet(
            &document,
            "html, body, p { display: block; margin: 0; width: 100px; } \
             p::before { content: \"X\"; display: block; height: 20px; \
             background-color: rgb(255, 0, 0); }",
        );
        // The red ::before background, sized to its 100×20 block box.
        let before = plist.commands().iter().find_map(|c| match c {
            PaintCmd::DrawRect(r)
                if (r.color.r - 1.0).abs() < 0.05 && r.color.g < 0.05 && r.color.b < 0.05 =>
            {
                Some(r.placement.bounds)
            },
            _ => None,
        });
        let b = before.expect("block ::before paints a red background box");
        assert!(
            (b.max.x - b.min.x - 100.0).abs() < 1.0,
            "::before spans the 100px width, got {b:?}"
        );
        assert!(
            (b.max.y - b.min.y - 20.0).abs() < 1.0,
            "::before is 20px tall, got {b:?}"
        );
    }

    /// A block `::before` with a `url()` `background-image` paints it. The image
    /// is keyed by `(element, ::before)` in the `BackgroundImagePlane` (the pseudo
    /// box has no DOM id), and the box-tree walk fetches it via the box's
    /// `BoxSource::Pseudo`. (Pseudo follow-ups F1.)
    #[test]
    fn block_pseudo_paints_its_url_background_image() {
        use base64::Engine as _;
        // An 8×8 PNG as a data-URI, so decode needs no loader.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 255, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode PNG");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let sheet = format!(
            "html, body, p {{ display: block; margin: 0; width: 100px; }} \
             p::before {{ content: \"\"; display: block; height: 20px; \
             background-image: url(\"data:image/png;base64,{b64}\"); }}"
        );

        let document = StaticDocument::parse("<html><body><p>hi</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet.as_str()],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(
            &document,
            &styles,
            &crate::image_decode::NoImageLoader,
        );
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &bg,
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        assert!(
            plist
                .commands()
                .iter()
                .any(|c| matches!(c, PaintCmd::DrawRepeatingImage(_))),
            "block ::before with a url() background-image paints a repeating image"
        );
    }

    /// A `border-image` paints its 9-slice — the four corners (at least) emit as
    /// `DrawImage` regions — and *replaces* the normal border (no `DrawBorder`).
    /// (Pseudo-unrelated, but the same box-tree paint path: border-image, BI-2.)
    #[test]
    fn border_image_paints_nine_slice_and_replaces_border() {
        use base64::Engine as _;
        // 4×4 source: a 1px frame so slice:1 carves a real border ring.
        let mut img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 255, 255]));
        for x in 0..4 {
            img.put_pixel(x, 0, image::Rgba([0, 255, 0, 255]));
            img.put_pixel(x, 3, image::Rgba([0, 255, 0, 255]));
        }
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let sheet = format!(
            "html, body, div {{ display: block; margin: 0; }} \
             div {{ width: 100px; height: 100px; border: 10px solid red; \
             border-image-source: url(\"data:image/png;base64,{b64}\"); \
             border-image-slice: 1; }}"
        );

        let document = StaticDocument::parse("<html><body><div></div></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet.as_str()],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(
            &document,
            &styles,
            &crate::image_decode::NoImageLoader,
        );
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &bg,
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        // border-image now emits a single nine-patch DrawBorder (the rasterizer
        // slices it), replacing the normal border — not pre-sliced DrawImages.
        let borders: Vec<_> = plist
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawBorder(b) => Some(b),
                _ => None,
            })
            .collect();
        assert_eq!(
            borders.len(),
            1,
            "one DrawBorder for the border-image, got {}",
            borders.len()
        );
        match &borders[0].details {
            BorderDetails::NinePatch(np) => {
                assert!(
                    matches!(np.source, NinePatchSource::Image(..)),
                    "nine-patch image source"
                );
                // slice:1 → a 1px slice on every side of the 4×4 source.
                assert_eq!(
                    (np.slice.top, np.slice.left),
                    (1, 1),
                    "1px slice, got {:?}",
                    np.slice
                );
            },
            BorderDetails::Normal(_) => {
                panic!("a loaded border-image must emit a nine-patch border")
            },
        }
        assert!(
            !plist
                .commands()
                .iter()
                .any(|c| matches!(c, PaintCmd::DrawImage(_))),
            "no pre-sliced DrawImages — the rasterizer owns the slicing"
        );
    }

    /// An `<external-texture key="…">` emits a compositor-pass `DrawExternalTexture`
    /// at its box (instead of serval-painted content), carrying the host's texture
    /// key — the element behind the actor-texture / scrying / pelt-tile external lanes.
    #[test]
    fn external_texture_element_emits_a_compositor_pass() {
        let document = StaticDocument::parse(
            "<html><body><external-texture key=\"42\" \
             style=\"display:block;width:200px;height:120px\"></external-texture></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(
            &document,
            &styles,
            &crate::image_decode::NoImageLoader,
        );
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &bg,
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        let ext: Vec<_> = plist
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawExternalTexture(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(
            ext.len(),
            1,
            "one DrawExternalTexture for the element, got {}",
            ext.len()
        );
        assert_eq!(
            ext[0].texture_key, 42,
            "carries the element's host texture key"
        );
        assert!(
            !plist
                .commands()
                .iter()
                .any(|c| matches!(c, PaintCmd::DrawImage(_))),
            "an external-texture paints no DrawImage — the compositor pass replaces it",
        );
    }

    /// A WebGL-backed `<canvas>` advertises the same compositor key that
    /// `<external-texture>` uses, so live canvas output rides the established
    /// `DrawExternalTexture` path instead of a separate paint command.
    #[test]
    fn webgl_canvas_emits_a_compositor_pass() {
        let document = StaticDocument::parse(
            "<html><body><canvas data-serval-external-texture-key=\"7\" \
             style=\"display:block;width:64px;height:64px\"></canvas></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(
            &document,
            &styles,
            &crate::image_decode::NoImageLoader,
        );
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &bg,
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        let ext: Vec<_> = plist
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawExternalTexture(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(ext.len(), 1, "one DrawExternalTexture for the canvas");
        assert_eq!(
            ext[0].texture_key, 7,
            "carries the WebGL canvas texture key"
        );
    }

    /// A full-viewport (800×600) `DrawRect`'s color, if one was emitted — the
    /// shape of a propagated canvas background.
    fn viewport_fill(plist: &ServalPaintList) -> Option<ColorF> {
        plist.commands().iter().find_map(|c| match c {
            PaintCmd::DrawRect(r) => {
                let b = r.placement.bounds;
                (b.min.x == 0.0 && b.min.y == 0.0 && b.max.x == 800.0 && b.max.y == 600.0)
                    .then_some(r.color)
            },
            _ => None,
        })
    }

    #[test]
    fn canvas_background_propagates_root_color_over_viewport() {
        // CSS Backgrounds-3 §root-background: the root element's background is
        // painted over the whole canvas, not just the (small, content-sized)
        // root box. The root's own-box paint is suppressed, so the only
        // viewport-sized fill is the propagated canvas background.
        let document = StaticDocument::parse("<html><body></body></html>");
        let plist = emit_with_sheet(&document, "html { background-color: rgb(0, 128, 0); }");
        let color = viewport_fill(&plist).expect("canvas background over the viewport");
        assert!(
            color.g > 0.4 && color.r < 0.1 && color.b < 0.1,
            "expected a green canvas background, got {color:?}"
        );
    }

    #[test]
    fn canvas_background_suppressed_for_display_none_root() {
        // `display: none` on the root hides the whole document — no canvas
        // background propagates (the *-propagation negative reftests).
        let document = StaticDocument::parse("<html><body></body></html>");
        let plist = emit_with_sheet(
            &document,
            "html { background-color: green; display: none; }",
        );
        assert!(
            viewport_fill(&plist).is_none(),
            "display:none root must not propagate a canvas background"
        );
    }

    #[test]
    fn canvas_background_propagates_from_body_when_root_transparent() {
        // HTML body→canvas special case: a transparent root takes its canvas
        // background from <body>.
        let document = StaticDocument::parse("<html><body></body></html>");
        let plist = emit_with_sheet(&document, "body { background-color: rgb(0, 0, 200); }");
        let color = viewport_fill(&plist).expect("body background propagated to the canvas");
        assert!(
            color.b > 0.6 && color.r < 0.1,
            "expected a blue canvas background from <body>, got {color:?}"
        );
    }

    /// Count emitted linear-gradient draw commands for `div` with `decl`.
    fn gradient_tile_count(decl: &str) -> usize {
        let document = StaticDocument::parse("<html><body><div></div></body></html>");
        let plist = emit_with_sheet(
            &document,
            &format!("div {{ display: block; width: 72px; height: 72px; {decl} }}"),
        );
        plist
            .commands()
            .iter()
            .filter(|c| matches!(c, PaintCmd::DrawLinearGradient(_)))
            .count()
    }

    #[test]
    fn gradient_auto_size_emits_single_area_fill() {
        // The default (`background-size: auto`) fills the positioning area with
        // one gradient — identical to the pre-tiling emit.
        assert_eq!(
            gradient_tile_count("background-image: linear-gradient(red, green);"),
            1
        );
    }

    #[test]
    fn gradient_no_repeat_emits_single_tile() {
        assert_eq!(
            gradient_tile_count(
                "background-image: linear-gradient(red, green); \
                 background-size: 20px 20px; background-repeat: no-repeat;"
            ),
            1
        );
    }

    #[test]
    fn gradient_repeat_tiles_to_fill_the_box() {
        // 20px tiles across a 72px box → ceil(72/20) = 4 per axis → 16 tiles.
        assert_eq!(
            gradient_tile_count(
                "background-image: linear-gradient(red, green); \
                 background-size: 20px 20px; background-repeat: repeat;"
            ),
            16
        );
    }

    #[test]
    fn inline_block_emits_its_background_box() {
        // An inline-block is an atomic inline box: it paints its own background
        // (and content) at its measured/CSS size, rather than being recursed as
        // transparent inline content (which dropped the box).
        let document = StaticDocument::parse("<html><body><span>x</span></body></html>");
        let plist = emit_with_sheet(
            &document,
            "span { display: inline-block; width: 50px; height: 20px; \
                    background: rgb(0, 128, 0); }",
        );
        let green_box = plist.commands().iter().any(|c| match c {
            PaintCmd::DrawRect(r) => {
                r.color.g > 0.4
                    && r.color.r < 0.1
                    && r.color.b < 0.1
                    && (r.placement.bounds.width() - 50.0).abs() < 2.0
            },
            _ => false,
        });
        assert!(
            green_box,
            "inline-block should emit a ~50px green background box"
        );
    }

    #[test]
    fn inline_block_among_blocks_paints_shrunk_background() {
        // An inline-block among block siblings is wrapped in an anonymous block
        // box and laid out as an atomic, shrink-to-fit inline box — so its
        // background paints at its content width, not stretched to the container.
        // (The anonymous wrapper itself paints no box decorations.)
        let document = StaticDocument::parse("<html><body><p>x</p><span>x</span></body></html>");
        let plist = emit_with_sheet(
            &document,
            "span { display: inline-block; background: rgb(200, 0, 0); }",
        );
        let red_widths: Vec<f32> = plist
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawRect(r) if r.color.r > 0.5 && r.color.g < 0.2 && r.color.b < 0.2 => {
                    Some(r.placement.bounds.width())
                },
                _ => None,
            })
            .collect();
        assert!(
            !red_widths.is_empty(),
            "inline-block red background should paint"
        );
        assert!(
            red_widths.iter().all(|&w| w < 100.0),
            "inline-block background shrink-to-fit, not full 800px width: {red_widths:?}"
        );
    }

    #[test]
    fn gradient_round_repeat_emits_rescaled_tile_grid() {
        // `round` rescales the 32px tile so a whole number fits 72px:
        // round(72/32) = 2 → a 2×2 grid of 36×36 tiles.
        let document = StaticDocument::parse("<html><body><div></div></body></html>");
        let plist = emit_with_sheet(
            &document,
            "div { display: block; width: 72px; height: 72px; \
                   background-image: linear-gradient(red, green); \
                   background-size: 32px 32px; background-repeat: round; }",
        );
        let grads: Vec<_> = plist
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawLinearGradient(g) => Some(g),
                _ => None,
            })
            .collect();
        assert_eq!(grads.len(), 4, "round → 2×2 rescaled tiles");
        // Each tile is rescaled from 32 to 72/2 = 36 (the first, unclipped, is
        // exactly 36 wide).
        let w = grads[0].placement.bounds.width();
        assert!(
            (w - 36.0).abs() < 0.5,
            "round rescales 32→36, tile width {w}"
        );
    }

    #[test]
    fn emit_round_trips_through_serde() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let (fragments, built, _) = layout(
            &document,
            &styles,
            &ImagePlane::new(),
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );

        let json = serde_json::to_string(&plist).expect("serialize ServalPaintList");
        let parsed: ServalPaintList =
            serde_json::from_str(&json).expect("deserialize ServalPaintList");
        assert_eq!(parsed.commands().len(), plist.commands().len());
        assert_eq!(parsed.viewport(), plist.viewport());
    }

    /// Probe glyph caching: pass the layout's TextMeasureCtx +
    /// BoxTree to emission and verify the resulting DrawText
    /// items carry positioned glyph runs (non-empty) rather than the
    /// empty Vec the cache-less path produces.
    #[test]
    fn emit_with_layouts_extracts_positioned_glyphs() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);

        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &crate::image_decode::ImagePlane::new(),
            &crate::image_decode::BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        let mut text_with_glyphs = 0;
        for cmd in plist.commands() {
            if let PaintCmd::DrawText(t) = cmd {
                if !t.glyphs.is_empty() {
                    text_with_glyphs += 1;
                }
            }
        }
        assert!(
            text_with_glyphs >= 1,
            "expected at least one DrawText with non-empty glyph run, got {text_with_glyphs}"
        );
    }

    /// Text emission populates the font side-table, and each
    /// `DrawText`'s `font_instance` resolves to a `FontResource` in
    /// the list's `fonts()`. This is the producer-side half of the
    /// FontRegistry contract: the bytes the renderer needs travel
    /// with the paint output, keyed to the run.
    #[test]
    fn emit_with_layouts_populates_font_table() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &crate::image_decode::ImagePlane::new(),
            &crate::image_decode::BackgroundImagePlane::new(),
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        // At least one font was collected, and it carries non-empty
        // bytes (a real system font blob).
        assert!(
            !plist.fonts().is_empty(),
            "expected font side-table populated"
        );
        assert!(
            plist.fonts().iter().all(|f| !f.data.is_empty()),
            "every FontResource should carry font bytes"
        );

        // Every text run with glyphs references a key present in fonts().
        let font_keys: std::collections::HashSet<_> = plist.fonts().iter().map(|f| f.key).collect();
        for cmd in plist.commands() {
            if let PaintCmd::DrawText(t) = cmd {
                if !t.glyphs.is_empty() {
                    assert!(
                        font_keys.contains(&t.font_instance),
                        "DrawText font_instance {:?} not in fonts() table",
                        t.font_instance
                    );
                    assert!(
                        t.font_size > 0.0,
                        "shaped run should have positive font_size"
                    );
                }
            }
        }
    }

    /// Sanity-check the cache-less emit path still produces empty
    /// glyph runs (probe-mode behavior — useful when caller hasn't
    /// run layout or doesn't want to pay for glyph extraction).
    #[test]
    fn emit_without_layouts_produces_empty_glyph_runs() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );

        for cmd in plist.commands() {
            if let PaintCmd::DrawText(t) = cmd {
                assert!(
                    t.glyphs.is_empty(),
                    "expected empty glyph run from cache-less emit"
                );
            }
        }
    }

    /// An `overflow: scroll` element clips its descendants: the emitted stream
    /// wraps the container's subtree in a balanced `PushClip`/`PopClip`, with
    /// the clip rect at the container's padding box.
    #[test]
    fn overflow_container_emits_clip() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse(
            "<html><body><div class=\"scroller\"><p>content</p></div></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "html, body, div, p { display: block; margin: 0; padding: 0; border: 0; }",
                ".scroller { overflow: scroll; width: 100px; height: 40px; }",
            ],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );
        let cmds = plist.commands();

        let pushes = cmds
            .iter()
            .filter(|c| matches!(c, PaintCmd::PushClip(_)))
            .count();
        let pops = cmds
            .iter()
            .filter(|c| matches!(c, PaintCmd::PopClip))
            .count();
        assert_eq!(
            pushes, pops,
            "PushClip / PopClip balanced: {pushes} vs {pops}"
        );

        // The scroller's clip is its 100×40 padding box (no border/padding).
        let clip = cmds
            .iter()
            .find_map(|c| match c {
                PaintCmd::PushClip(ClipSpec {
                    kind: ClipKind::Rect(r),
                }) => Some(*r),
                _ => None,
            })
            .expect("an overflow container emits a rect clip");
        assert!(
            (clip.width() - 100.0).abs() < 0.5,
            "clip width = box width, got {}",
            clip.width()
        );
        assert!(
            (clip.height() - 40.0).abs() < 0.5,
            "clip height = box height, got {}",
            clip.height()
        );
    }

    /// A scroll offset on an overflow container emits a `PushTransform` of
    /// `-offset` immediately inside its clip, so the clipped content scrolls
    /// under the fixed clip window.
    #[test]
    fn scroll_offset_translates_clipped_content() {
        use crate::cascade::run_cascade;
        use crate::image_decode::BackgroundImagePlane;

        let document = StaticDocument::parse(
            "<html><body><div class=\"scroller\"><p>content</p></div></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "html, body, div, p { display: block; margin: 0; padding: 0; border: 0; }",
                ".scroller { overflow: scroll; width: 100px; height: 40px; }",
            ],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);

        // The scroller div, scrolled down 25 px.
        let div = {
            let mut q = vec![document.document()];
            let mut found = None;
            while let Some(id) = q.pop() {
                if document
                    .element_name(id)
                    .is_some_and(|n| n.local == local_name!("div"))
                {
                    found = Some(id);
                    break;
                }
                q.extend(document.dom_children(id));
            }
            found.expect("scroller div")
        };
        let mut offsets: FxHashMap<StaticNodeId, (f32, f32)> = FxHashMap::default();
        offsets.insert(div, (0.0, 25.0));

        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &offsets,
            DeviceIntSize::new(800, 600),
        );
        let cmds = plist.commands();

        // The command right after the container's PushClip is the scroll
        // PushTransform with origin = -offset = (0, -25).
        let clip_idx = cmds
            .iter()
            .position(|c| matches!(c, PaintCmd::PushClip(_)))
            .expect("overflow container emits a clip");
        match &cmds[clip_idx + 1] {
            PaintCmd::PushTransform(t) => {
                assert!(
                    t.origin.x.abs() < 0.01,
                    "scroll x origin 0, got {}",
                    t.origin.x
                );
                assert!(
                    (t.origin.y + 25.0).abs() < 0.01,
                    "scroll y origin -25, got {}",
                    t.origin.y
                );
            },
            other => panic!("expected scroll PushTransform after PushClip, got {other:?}"),
        }
    }

    /// A `position: absolute` child of a scrolled `overflow` container scrolls *with* it:
    /// the deferred layer's recorded origin folds in the container's scroll, so it does not
    /// stay pinned while in-flow content scrolls under it. (Absolute-in-scroll fix — the bug
    /// where the orrery/facet swatch's sprite stayed put as the menu scrolled.)
    #[test]
    fn absolute_child_scrolls_with_its_overflow_container() {
        use crate::cascade::run_cascade;
        use crate::image_decode::BackgroundImagePlane;

        let document = StaticDocument::parse(
            "<html><body><div class=\"scroller\"><div class=\"abs\"></div></div></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "html, body, div { display: block; margin: 0; padding: 0; border: 0; }",
                ".scroller { position: relative; overflow: scroll; width: 100px; height: 100px; }",
                ".abs { position: absolute; top: 50px; left: 0; width: 20px; height: 20px; \
                    background-color: rgb(255, 0, 0); }",
            ],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);

        // The .scroller div.
        let scroller = {
            let mut q = vec![document.document()];
            let mut found = None;
            while let Some(id) = q.pop() {
                if document
                    .element_name(id)
                    .is_some_and(|n| n.local == local_name!("div"))
                {
                    found = Some(id);
                    break;
                }
                q.extend(document.dom_children(id));
            }
            found.expect("scroller div")
        };

        // The absolute y where the abs child's red rect lands, accumulating translate origins.
        let red_y = |offsets: &FxHashMap<StaticNodeId, (f32, f32)>| -> f32 {
            let plist = emit_paint_list_with_layouts(
                &document,
                &styles,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &BackgroundImagePlane::new(),
                offsets,
                DeviceIntSize::new(800, 600),
            );
            let mut acc_y = 0.0f32;
            let mut stack: Vec<f32> = Vec::new();
            for c in plist.commands() {
                match c {
                    PaintCmd::PushTransform(t) => {
                        acc_y += t.origin.y;
                        stack.push(t.origin.y);
                    },
                    PaintCmd::PopTransform => {
                        if let Some(y) = stack.pop() {
                            acc_y -= y;
                        }
                    },
                    PaintCmd::DrawRect(r)
                        if r.color.r > 0.9 && r.color.g < 0.1 && r.color.b < 0.1 =>
                    {
                        return acc_y + r.placement.bounds.min.y;
                    },
                    _ => {},
                }
            }
            panic!("the abs child's red rect was not emitted");
        };

        let unscrolled = red_y(&FxHashMap::default());
        let mut offsets: FxHashMap<StaticNodeId, (f32, f32)> = FxHashMap::default();
        offsets.insert(scroller, (0.0, 30.0));
        let scrolled = red_y(&offsets);
        assert!(
            (unscrolled - scrolled - 30.0).abs() < 0.5,
            "the absolute child scrolls 30px with its container: {unscrolled} -> {scrolled}",
        );
    }

    /// z-index Tier 1: an out-of-flow (`position: absolute`) box declared
    /// *before* a later in-flow sibling paints *after* it — the positioned pass
    /// puts it on top regardless of document order.
    #[test]
    fn out_of_flow_paints_after_in_flow() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse(
            "<html><body><div class=\"abs\"></div><div class=\"flow\"></div></body></html>",
        );
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "html, body, div { display: block; margin: 0; }",
                ".abs { position: absolute; top: 0; left: 0; width: 50px; height: 50px; \
                    background-color: rgb(255, 0, 0); }",
                ".flow { width: 50px; height: 50px; background-color: rgb(0, 0, 255); }",
            ],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );
        let cmds = plist.commands();

        let red = cmds.iter().position(|c| {
            matches!(c, PaintCmd::DrawRect(r) if r.color.r > 0.9 && r.color.g < 0.1 && r.color.b < 0.1)
        });
        let blue = cmds.iter().position(|c| {
            matches!(c, PaintCmd::DrawRect(r) if r.color.b > 0.9 && r.color.r < 0.1 && r.color.g < 0.1)
        });
        let red = red.expect(".abs red background rect");
        let blue = blue.expect(".flow blue background rect");
        assert!(
            red > blue,
            "out-of-flow .abs (idx {red}) paints after in-flow .flow (idx {blue})"
        );
    }

    /// `background-origin: content-box` + `background-clip: content-box`
    /// inset the no-repeat tile to the content box. A 100×100 border box
    /// with 10px border + 10px padding has a content box at local
    /// (20, 20) sized 60×60; a `no-repeat` 20×20 image at `background-size:
    /// 20px` must place its top-left at (20, 20), not (0, 0).
    #[test]
    fn background_origin_content_box_insets_tile() {
        use crate::image_decode::{BackgroundImagePlane, NoImageLoader};
        use base64::Engine as _;

        let img = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 255, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode test PNG");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let document = StaticDocument::parse("<html><body><div></div></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        let sheet = format!(
            "div {{ display: block; width: 60px; height: 60px; \
             border: 10px solid black; padding: 10px; margin: 0; \
             background-image: url(data:image/png;base64,{b64}); \
             background-size: 20px 20px; background-repeat: no-repeat; \
             background-origin: content-box; background-clip: content-box; }}"
        );
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet.as_str()],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(&document, &styles, &NoImageLoader);
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &bg,
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );
        let placement = plist.commands().iter().find_map(|c| match c {
            PaintCmd::DrawRepeatingImage(item) => Some(item.placement.bounds),
            _ => None,
        });
        let r = placement.expect("a DrawRepeatingImage for the content-box background");
        // Border box is 100×100 (60 content + 2×10 padding + 2×10 border);
        // content box origin is at (20, 20) in node-local coords.
        assert!(
            (r.min.x - 20.0).abs() < 0.5 && (r.min.y - 20.0).abs() < 0.5,
            "content-box tile must start at (20,20), got ({}, {})",
            r.min.x,
            r.min.y,
        );
    }

    /// `text-decoration: underline` draws a line: the underlined run emits one
    /// extra `DrawRect` (the underline) over the same content without it.
    #[test]
    fn underline_text_decoration_emits_a_line() {
        use crate::cascade::run_cascade;
        use crate::image_decode::BackgroundImagePlane;

        let draw_rects = |decoration: &str| -> usize {
            let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            let p_rule = format!("p {{ font-size: 40px; {decoration} }}");
            run_cascade(
                &document,
                &mut styles,
                euclid::Size2D::new(800.0, 600.0),
                &[
                    "html, body, p { display: block; margin: 0; }",
                    p_rule.as_str(),
                ],
                None,
            );
            let viewport = Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            };
            let (fragments, built, text_ctx) =
                layout(&document, &styles, &ImagePlane::new(), viewport);
            let plist = emit_paint_list_with_layouts(
                &document,
                &styles,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &BackgroundImagePlane::new(),
                &FxHashMap::default(),
                DeviceIntSize::new(800, 600),
            );
            plist
                .commands()
                .iter()
                .filter(|c| matches!(c, PaintCmd::DrawRect(_)))
                .count()
        };

        assert_eq!(
            draw_rects("text-decoration: underline;"),
            draw_rects("") + 1,
            "underlined text emits one extra DrawRect (the underline line)"
        );
    }

    /// Probe DrawBorder emission: a CSS-declared border produces a
    /// DrawBorder command alongside the element's DrawRect, with the
    /// expected widths + per-side color.
    #[test]
    fn emit_draws_borders_when_cascade_assigns_them() {
        use crate::cascade::run_cascade;
        use paint_list_api::items::BorderDetails;

        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["p { display: block; width: 100px; height: 50px; \
                    border: 4px solid rgb(0, 128, 255); }"],
            None,
        );

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );

        let mut found_p_border = false;
        for cmd in plist.commands() {
            if let PaintCmd::DrawBorder(item) = cmd {
                // The <p>'s border: all sides 4px, solid, color (0, 0.5, 1, 1).
                if (item.widths.top - 4.0).abs() < 0.001
                    && (item.widths.right - 4.0).abs() < 0.001
                    && (item.widths.bottom - 4.0).abs() < 0.001
                    && (item.widths.left - 4.0).abs() < 0.001
                {
                    if let BorderDetails::Normal(n) = &item.details {
                        if matches!(n.top.style, paint_list_api::BorderStyle::Solid)
                            && (n.top.color.b - 1.0).abs() < 0.05
                        {
                            found_p_border = true;
                        }
                    }
                }
            }
        }
        assert!(
            found_p_border,
            "expected a 4px solid blue DrawBorder for the <p> element"
        );
    }

    /// `border-radius` reaches the emit: a rounded element's border carries the
    /// resolved per-corner radius, and its background is wrapped in a
    /// `PushClip(RoundedRect)` so it clips to the curve. A 100px-wide box with
    /// `border-radius: 20px` must emit a 20px top-left radius on both.
    #[test]
    fn border_radius_rounds_border_and_clips_background() {
        use crate::cascade::run_cascade;
        use paint_list_api::items::BorderDetails;
        use paint_list_api::specs::ClipKind;

        let document = StaticDocument::parse("<html><body><div></div></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "div { display: block; width: 100px; height: 100px; margin: 0; \
               border: 4px solid black; border-radius: 20px; \
               background-color: rgb(0, 128, 0); }",
            ],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );

        let border_radius = plist.commands().iter().find_map(|c| match c {
            PaintCmd::DrawBorder(BorderItem {
                details: BorderDetails::Normal(n),
                ..
            }) => Some(n.radius.top_left.width),
            _ => None,
        });
        assert!(
            border_radius.is_some_and(|r| (r - 20.0).abs() < 0.5),
            "border carries the 20px corner radius, got {border_radius:?}"
        );

        let clip_radius = plist.commands().iter().find_map(|c| match c {
            PaintCmd::PushClip(ClipSpec {
                kind: ClipKind::RoundedRect { radius, .. },
            }) => Some(radius.top_left.width),
            _ => None,
        });
        assert!(
            clip_radius.is_some_and(|r| (r - 20.0).abs() < 0.5),
            "background wrapped in a 20px rounded clip, got {clip_radius:?}"
        );
    }

    /// No border in CSS = no DrawBorder command. The probe-stage
    /// optimization that suppresses zero-width/none-style borders.
    #[test]
    fn emit_omits_drawborder_when_no_border_declared() {
        use crate::cascade::run_cascade;

        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        // No border in this sheet — only background.
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["p { background-color: rgb(255, 0, 0); }"],
            None,
        );

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );

        for cmd in plist.commands() {
            assert!(
                !matches!(cmd, PaintCmd::DrawBorder(_)),
                "expected no DrawBorder commands, got {cmd:?}"
            );
        }
    }

    /// Probe the cascade → emit color path: run a real stylesheet
    /// through the cascade and verify the emitted DrawRect for the
    /// matched element carries the cascaded color.
    #[test]
    fn emit_color_comes_from_cascade_when_stylesheet_applies() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["body { background-color: rgb(255, 0, 0); }"],
            None,
        );

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );

        // Among DrawRects, at least one must be opaque red — that's body.
        let mut found_red = false;
        for cmd in plist.commands() {
            if let PaintCmd::DrawRect(rect) = cmd {
                if (rect.color.r - 1.0).abs() < 0.001
                    && rect.color.g < 0.001
                    && rect.color.b < 0.001
                    && (rect.color.a - 1.0).abs() < 0.001
                {
                    found_red = true;
                }
            }
        }
        assert!(
            found_red,
            "expected at least one DrawRect with cascade-applied red background"
        );
    }

    /// `background-size` reaches the emit: a 20×20 data-URI image on a
    /// 100×100 box with `background-size: 50%` must emit a
    /// `DrawRepeatingImage` whose `stretch_size` is the scaled 50×50,
    /// NOT the intrinsic 20×20. This is the runtime receipt that
    /// [`bg_tile_style_of`] + [`resolve_bg_tile`] run on the cascade path.
    #[test]
    fn background_size_percent_scales_emitted_tile() {
        use crate::image_decode::{BackgroundImagePlane, NoImageLoader};
        use base64::Engine as _;

        // 20×20 solid image, inline data-URI so decode_from_cascade
        // decodes it with the no-op loader (no filesystem).
        let img = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 128, 0, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode test PNG");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let html = format!("<html><body><div></div></body></html>");
        let document = StaticDocument::parse(&html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        let sheet = format!(
            "div {{ display: block; width: 100px; height: 100px; margin: 0; \
             background-image: url(data:image/png;base64,{b64}); \
             background-size: 50%; background-repeat: no-repeat; }}"
        );
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet.as_str()],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(&document, &styles, &NoImageLoader);
        assert_eq!(
            bg.len(),
            1,
            "the div's background-image must decode (else the emit branch never runs)"
        );

        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &bg,
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );
        let tile = plist.commands().iter().find_map(|c| match c {
            PaintCmd::DrawRepeatingImage(item) => Some(item.stretch_size),
            _ => None,
        });
        let tile = tile.expect("a DrawRepeatingImage must be emitted for the background-image");
        assert!(
            (tile.width - 50.0).abs() < 0.5 && (tile.height - 50.0).abs() < 0.5,
            "background-size: 50% of a 100px box must scale the 20px image to 50×50, got {}×{}",
            tile.width,
            tile.height,
        );
    }

    #[test]
    fn background_position_right_offsets_oversized_cover_tile() {
        use crate::image_decode::{BackgroundImagePlane, NoImageLoader};

        let uri = data_uri_png(16, 8);
        let document = StaticDocument::parse("<html><body><div></div></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        let sheet = format!(
            "html, body {{ margin: 0; padding: 0; }} \
             div {{ display: block; width: 32px; height: 48px; margin: 0; \
             background-image: url({uri}); background-size: cover; \
             background-repeat: no-repeat; background-position: right top; }}"
        );
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[sheet.as_str()],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(&document, &styles, &NoImageLoader);
        let plist = emit_paint_list_with_layouts(
            &document,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &bg,
            &FxHashMap::default(),
            DeviceIntSize::new(800, 600),
        );

        assert_rect(image_bounds(plist.commands()), -64.0, 0.0, 32.0, 48.0);
    }

    #[test]
    fn emit_paint_order_is_pre_order() {
        // Sanity-check that children paint after parents (so they
        // appear later in the command list), matching pre-order DOM
        // traversal.
        let document = StaticDocument::parse("<html><body><p>a</p><p>b</p></body></html>");
        let styles = build_style_plane(&document);
        let (fragments, built, _) = layout(
            &document,
            &styles,
            &ImagePlane::new(),
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
            &built,
            DeviceIntSize::new(800, 600),
        );

        // The first command must be PushTransform (compositor model:
        // each fragment opens a new coord space before painting itself).
        match plist.commands().first() {
            Some(PaintCmd::PushTransform(_)) => {},
            other => panic!("expected leading PushTransform, got {other:?}"),
        }

        // The command right after the leading PushTransform must be a
        // DrawRect — the html element painting itself in local coords.
        match plist.commands().get(1) {
            Some(PaintCmd::DrawRect(_)) => {},
            other => panic!("expected DrawRect after leading PushTransform, got {other:?}"),
        }

        // Push/Pop pairs must balance — the compositor-stack invariant.
        let mut depth = 0i32;
        for cmd in plist.commands() {
            match cmd {
                PaintCmd::PushTransform(_) => depth += 1,
                PaintCmd::PopTransform => depth -= 1,
                _ => {},
            }
            assert!(depth >= 0, "transform stack underflowed at command {cmd:?}");
        }
        assert_eq!(depth, 0, "transform stack didn't return to zero");

        // Find the p count — there should be at least two.
        let p_count = document
            .dom_children(document.document())
            .flat_map(|html| document.dom_children(html))
            .flat_map(|body| document.dom_children(body))
            .filter(|id| {
                document
                    .element_name(*id)
                    .is_some_and(|q| q.local == local_name!("p"))
            })
            .count();
        assert_eq!(p_count, 2, "fixture has two <p> siblings");
    }

    /// Prereq C (fixed): an element's computed CSS `transform` is folded into its
    /// `PushTransform`, so `transform: translate(x,y)` moves the painted node.
    /// Before the fix the push stayed identity and the painted position never moved.
    #[test]
    fn css_transform_folds_into_pushtransform() {
        fn push_transforms(sheet: &[&str]) -> Vec<LayoutTransform> {
            let document = StaticDocument::parse("<html><body><div></div></body></html>");
            let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &document,
                &mut plane,
                euclid::Size2D::new(800.0, 600.0),
                sheet,
                None,
            );
            let viewport = Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            };
            let (fragments, built, text_ctx) =
                layout(&document, &plane, &ImagePlane::new(), viewport);
            let plist = emit_paint_list_with_layouts(
                &document,
                &plane,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &crate::image_decode::BackgroundImagePlane::new(),
                &FxHashMap::default(),
                DeviceIntSize::new(800, 600),
            );
            plist
                .commands()
                .iter()
                .filter_map(|c| match c {
                    PaintCmd::PushTransform(spec) => Some(spec.transform),
                    _ => None,
                })
                .collect()
        }

        // With `transform: translate(40px,40px)`, the div's push carries it.
        let with = push_transforms(&[
            "html,body,div{display:block;width:80px;height:40px}",
            "div{transform:translate(40px,40px)}",
        ]);
        assert!(
            with.iter()
                .any(|t| (t.m41 - 40.0).abs() < 0.5 && (t.m42 - 40.0).abs() < 0.5),
            "transform:translate(40,40) must fold into a PushTransform (m41/m42 ≈ 40): {with:?}",
        );

        // Without a transform, no push translates (box position lives in `origin`,
        // not the CSS-transform matrix).
        let without = push_transforms(&["html,body,div{display:block;width:80px;height:40px}"]);
        assert!(
            without
                .iter()
                .all(|t| t.m41.abs() < 0.5 && t.m42.abs() < 0.5),
            "no CSS transform → no translating push: {without:?}",
        );
    }

    /// `conjugate_at(O, M)` applies `M` around the absolute point `O`
    /// (`T(O)·M·T(-O)`), the form used to carry an ancestor transform onto a
    /// deferred descendant (see `Deferred::ancestor_transform`). A 2× scale around
    /// x=10 maps x → 2x − 10, so m11 = 2 and m41 = −10. A pure translate conjugates
    /// to itself, so this scale case is what pins the conjugation order.
    #[test]
    fn conjugate_at_applies_transform_around_origin() {
        let scale2 = LayoutTransform::new(
            2.0, 0.0, 0.0, 0.0, //
            0.0, 2.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        );
        let c = conjugate_at((10.0, 0.0), scale2);
        assert!(
            (c.m11 - 2.0).abs() < 1e-4,
            "scale preserved: m11 {} != 2",
            c.m11
        );
        assert!(
            (c.m41 + 10.0).abs() < 1e-4,
            "scale around x=10 → m41 = −10, got {}",
            c.m41
        );
    }
}
