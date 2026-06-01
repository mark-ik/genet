/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Producer-side: emit [`ServalPaintList`] from `FragmentPlane` +
//! `StylePlane` + DOM.
//!
//! Walks the DOM in paint order (pre-order traversal — normal-flow
//! paint order matches DOM order; positioned descendants would
//! reorder via z-index, but the probe doesn't exercise positioning).
//! Reads per-node layout from `FragmentPlane`, reads per-node style
//! from `StylePlane`, and produces a closed-set [`PaintCmd`] stream.
//!
//! ## Scope
//!
//! - `DrawRect` per element from the cascade's
//!   `ComputedValues::background.background_color` (via
//!   [`background_color_of`]); transparent when no cascade data.
//! - `DrawText` per inline-context leaf carrying shaped glyph runs.
//!   Path (b) is the live one: [`emit_paint_list_with_layouts`] reads
//!   cached parley `Layout`s from the [`TextMeasureCtx`] populated by
//!   `crate::layout::layout`. The cache-less [`emit_paint_list`] still
//!   exists for probes / callers that haven't run layout; it emits
//!   empty glyph runs so the command structure is still present.
//! - `PushTransform`/`PopTransform` per fragment around the node's
//!   primitives, composing the parent-relative `taffy::Layout.location`
//!   onto the transform stack; absolute scene coordinates fall out
//!   of the composition.
//!
//! Cf. `docs/2026-05-17_paintlist_polyglot_renderer.md` (PM-3).

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use paint_list_api::{
    AlphaType, BorderRadius, BorderSide, BorderStyle, BoxShadowClipMode, ColorF, CommonPlacement,
    DeviceIntSize, EngineId, FontInstanceKey, FontResource, GlyphInstance, IdNamespace, ImageItem,
    ImageKey, ImageRendering, ImageResource, LayoutPoint, LayoutRect, LayoutSideOffsets,
    LayoutSize, LayoutTransform, LayoutVector2D, NormalBorder, PaintCmd, PaintList, RectItem,
    RepeatingImageItem, TextOptions, TextRunItem, TransformSpec,
};
use paint_list_api::items::{BorderDetails, BorderItem, ShadowItem};
use paint_list_api::specs::{ClipKind, ClipSpec, TransformKind};
use parley::PositionedLayoutItem;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::box_tree::BoxTree;
use crate::fragment::FragmentPlane;
use crate::image_decode::{BackgroundImagePlane, DecodedImage, ImagePlane};
use crate::style::StylePlane;
use crate::text_measure::TextMeasureCtx;

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
    next_idx: u32,
}

impl FontCollector {
    /// Intern a parley `FontData`, returning the key the matching
    /// `TextRunItem::font_instance` should carry. Adds a
    /// [`FontResource`] (font bytes + index) on first sight of a blob.
    fn intern(&mut self, font: &parley::FontData) -> FontInstanceKey {
        let blob_id = font.data.id();
        if let Some(k) = self.by_blob.get(&blob_id) {
            return *k;
        }
        let key = FontInstanceKey::new(SERVAL_FONT_NAMESPACE, self.next_idx);
        self.next_idx += 1;
        self.by_blob.insert(blob_id, key);
        self.fonts.push(FontResource {
            key,
            data: font.data.data().to_vec(),
            index: font.index,
        });
        key
    }
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
    /// Shaped-glyph source (None on the cache-less probe path).
    glyphs: Option<GlyphSource<'a, NodeId>>,
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
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let empty_images = ImagePlane::new();
    let empty_bg = BackgroundImagePlane::new();
    let no_scroll: FxHashMap<D::NodeId, (f32, f32)> = FxHashMap::default();
    emit_inner(dom, styles, fragments, None, &empty_images, &empty_bg, &no_scroll, viewport)
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
        Some(GlyphSource { constructed, text_ctx }),
        images,
        bg_images,
        scroll_offsets,
        viewport,
    )
}

/// Source for shaped-glyph lookup during emission. Borrowed view over
/// the box tree's node_map + the text measure cache.
struct GlyphSource<'a, NodeId: Copy + Eq + Hash> {
    constructed: &'a BoxTree<NodeId>,
    text_ctx: &'a TextMeasureCtx,
}

fn emit_inner<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    glyphs: Option<GlyphSource<'_, D::NodeId>>,
    images_plane: &ImagePlane<D::NodeId>,
    bg_images_plane: &BackgroundImagePlane<D::NodeId>,
    scroll_offsets: &FxHashMap<D::NodeId, (f32, f32)>,
    viewport: DeviceIntSize,
) -> ServalPaintList
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut commands = Vec::new();
    let mut emitter = Emitter {
        glyphs,
        images_plane,
        bg_images_plane,
        scroll_offsets,
        fonts: FontCollector::default(),
        images: ImageCollector::default(),
    };
    // Paint the document as the root stacking context. The recursive painter
    // (crate::paint_stacking) walks each context's own tree for in-flow content,
    // collects its positioned/z-index layers, and orders them per CSS 2.1
    // Appendix E (negative-z behind, then in-flow, then zero/positive on top),
    // scoped to that context — so a nested context sorts its own layers rather
    // than all layers sharing one global z-order (the Tier 1 limitation).
    crate::paint_stacking::paint_context(
        dom,
        styles,
        fragments,
        &mut emitter,
        dom.document(),
        (0.0, 0.0),
        &mut commands,
    );
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
pub(crate) struct Deferred<NodeId> {
    pub(crate) node: NodeId,
    pub(crate) origin: (f32, f32),
    pub(crate) z: i32,
    pub(crate) seq: usize,
}

/// Whether `id` is out of normal flow (`position: absolute`/`fixed`). Out-of-flow
/// elements are always lifted into a stacking layer; in-flow positioned elements
/// (`relative`/`sticky`) are lifted only when they carry an explicit `z-index`
/// (see [`crate::paint_stacking::defers_to_stacking`]).
pub(crate) fn is_out_of_flow<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> bool {
    use style::values::computed::PositionProperty;
    let Some(entry) = styles.get(id) else {
        return false;
    };
    let Some(data) = entry.borrow_data() else {
        return false;
    };
    matches!(
        data.styles.primary().get_box().position,
        PositionProperty::Absolute | PositionProperty::Fixed
    )
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
pub(crate) fn walk<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    em: &mut Emitter<'_, D::NodeId>,
    id: D::NodeId,
    origin: (f32, f32),
    commands: &mut Vec<PaintCmd>,
    deferred: &mut Vec<Deferred<D::NodeId>>,
    is_root: bool,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // A positioned / z-index descendant is lifted out of this context's in-flow
    // walk into a stacking layer (recorded with its parent's absolute origin +
    // paint-bucket z, skipped here); the recursive stacking painter places it.
    // `is_root` is the one node we always emit — the context root the painter
    // entered on, which would otherwise re-defer itself into an infinite loop.
    if !is_root && crate::paint_stacking::defers_to_stacking(styles, id) {
        deferred.push(Deferred {
            node: id,
            origin,
            z: crate::paint_stacking::bucket_z(styles, id),
            seq: deferred.len(),
        });
        return;
    }

    // An overflow container clips its descendants to its padding box; captured
    // here (while the layout is in scope) and applied around the children below.
    let mut clip_rect: Option<LayoutRect> = None;
    // Absolute origin to pass to children (this node's origin + its location),
    // so a deferred descendant records where to place itself.
    let mut child_origin = origin;
    let pushed = if let Some(l) = fragments.rect_of(id) {
        // Children push their own parent-relative location, composing with this
        // node's transform. The context root has no enclosing transform on the
        // stack (the stacking painter emits each layer on a clean stack), so it
        // folds its absolute `origin` into its own push — its body is then
        // absolute without an extra wrapper transform.
        let push_origin = if is_root {
            LayoutPoint::new(origin.0 + l.location.x, origin.1 + l.location.y)
        } else {
            LayoutPoint::new(l.location.x, l.location.y)
        };
        commands.push(PaintCmd::PushTransform(TransformSpec {
            origin: push_origin,
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
        child_origin = (origin.0 + l.location.x, origin.1 + l.location.y);
        let local_bounds = LayoutRect::new(
            LayoutPoint::new(0.0, 0.0),
            LayoutPoint::new(l.size.width, l.size.height),
        );
        match dom.kind(id) {
            NodeKind::Element => {
                // Outset box-shadows paint behind the border-box, so
                // emit them before the background. (Inset shadows,
                // which paint over the background, are deferred —
                // skipped here, warn-skipped in the translator.)
                for shadow in box_shadows_of(styles, id) {
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
                // border-radius: clip the background (color + image) to the
                // rounded border-box. The border itself rounds via its own
                // `radius` (border_of); replaced <img> content is not clipped
                // here yet (a follow-up). Pushed only when a corner is non-zero,
                // so square boxes keep a flat command stream.
                let bg_radius =
                    border_radius_of(styles, id, local_bounds.width(), local_bounds.height());
                if let Some(radius) = bg_radius {
                    commands.push(PaintCmd::PushClip(ClipSpec {
                        kind: ClipKind::RoundedRect {
                            rect: local_bounds,
                            radius,
                            clip_out: false,
                        },
                    }));
                }
                // Background, then replaced content (image), then
                // border — CSS paint order.
                commands.push(PaintCmd::DrawRect(RectItem {
                    placement: CommonPlacement::new(local_bounds),
                    color: background_color_of(styles, id),
                }));
                // CSS background-image paints over the background color,
                // under content + border. Resolve background-size /
                // -position / -repeat for the first layer against the
                // background-origin box, then clip to the background-clip
                // box. Defaults (auto / 0 0 / repeat / origin=padding-box
                // / clip=border-box) match CSS; with zero borders +
                // padding the origin/clip boxes collapse to the border box
                // and this reduces to the prior behavior.
                if let Some(decoded) = em.bg_images_plane.get(id) {
                    let int_w = decoded.width as f32;
                    let int_h = decoded.height as f32;
                    let key = em.images.add(decoded);
                    let bg_style = bg_tile_style_of(styles, id);
                    // The three reference boxes in this node's local
                    // (border-box) coords. `l.border` / `l.padding` are the
                    // resolved insets; (x, y, w, h) per box.
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
                                bw - l.border.left - l.border.right
                                    - l.padding.left - l.padding.right,
                                bh - l.border.top - l.border.bottom
                                    - l.padding.top - l.padding.bottom,
                            ),
                        }
                    };
                    let origin_box = bg_style.as_ref().map(|s| s.origin).unwrap_or(BgBox::PaddingBox);
                    let clip_box = bg_style.as_ref().map(|s| s.clip).unwrap_or(BgBox::BorderBox);
                    let (orx, ory, aw, ah) = box_for(origin_box);
                    // Tile geometry resolves against the positioning area (origin box).
                    let (tw, th, ox, oy) = match (&bg_style, int_w > 0.0 && int_h > 0.0) {
                        (Some(s), true) => resolve_bg_tile(s, aw, ah, int_w, int_h),
                        _ => (int_w, int_h, 0.0, 0.0),
                    };
                    let (rx, ry) = bg_style
                        .as_ref()
                        .map(|s| (s.repeat_x, s.repeat_y))
                        .unwrap_or((BgRepeat::Repeat, BgRepeat::Repeat));
                    // Per axis, in origin-box-local coords: `repeat` tiles across
                    // the full positioning area (phase 0, exact at position 0,
                    // the common case); `no-repeat` paints one tile at the
                    // resolved offset.
                    let (x0, sw) = match rx {
                        BgRepeat::NoRepeat => (ox, tw),
                        _ => (0.0, aw),
                    };
                    let (y0, sh) = match ry {
                        BgRepeat::NoRepeat => (oy, th),
                        _ => (0.0, ah),
                    };
                    // Translate to node-local (border-box) coords (add the origin
                    // box offset), then intersect with the clip box — the region
                    // the paint is allowed in.
                    let (cx, cy, cw, ch) = box_for(clip_box);
                    let px0 = (orx + x0).max(cx);
                    let py0 = (ory + y0).max(cy);
                    let px1 = (orx + x0 + sw).min(cx + cw);
                    let py1 = (ory + y0 + sh).min(cy + ch);
                    if tw > 0.0 && th > 0.0 && px1 > px0 && py1 > py0 {
                        commands.push(PaintCmd::DrawRepeatingImage(RepeatingImageItem {
                            placement: CommonPlacement::new(LayoutRect::new(
                                LayoutPoint::new(px0, py0),
                                LayoutPoint::new(px1, py1),
                            )),
                            image_key: key,
                            stretch_size: LayoutSize::new(tw, th),
                            tile_spacing: LayoutSize::zero(),
                            image_rendering: ImageRendering::Auto,
                            alpha_type: AlphaType::PremultipliedAlpha,
                            color: ColorF::WHITE, // identity tint
                        }));
                    }
                }
                // Close the border-radius clip around the background layers.
                if bg_radius.is_some() {
                    commands.push(PaintCmd::PopClip);
                }
                if let Some(decoded) = em.images_plane.get(id) {
                    let key = em.images.add(decoded);
                    commands.push(PaintCmd::DrawImage(ImageItem {
                        placement: CommonPlacement::new(local_bounds),
                        image_key: key,
                        image_rendering: ImageRendering::Auto,
                        alpha_type: AlphaType::PremultipliedAlpha,
                        color: ColorF::WHITE, // identity tint
                    }));
                }
                if let Some((widths, normal)) =
                    border_of(styles, id, local_bounds.width(), local_bounds.height())
                {
                    commands.push(PaintCmd::DrawBorder(BorderItem {
                        placement: CommonPlacement::new(local_bounds),
                        widths,
                        details: BorderDetails::Normal(normal),
                    }));
                }
                // An element establishing an inline formatting context
                // carries its text + replaced boxes as the leaf's
                // InlineContent — emit its glyph runs and inline-box
                // images (paints over background/image, under the
                // border conceptually; border is rare on inline
                // contexts). Non-inline elements have no cached layout,
                // so emit_inline_content no-ops.
                if let Some(g) = em.glyphs.as_ref() {
                    emit_inline_content(
                        g,
                        id,
                        local_bounds,
                        em.images_plane,
                        &mut em.fonts,
                        &mut em.images,
                        commands,
                    );
                }
            }
            NodeKind::Text => {
                let emitted = match em.glyphs.as_ref() {
                    Some(g) => emit_inline_content(
                        g,
                        id,
                        local_bounds,
                        em.images_plane,
                        &mut em.fonts,
                        &mut em.images,
                        commands,
                    ),
                    None => false,
                };
                if !emitted {
                    // Cache-less path (no layout cache, or no glyphs):
                    // emit one empty text run so the command structure
                    // still reflects the text node.
                    commands.push(PaintCmd::DrawText(TextRunItem {
                        placement: CommonPlacement::new(local_bounds),
                        font_instance: FontInstanceKey::default(),
                        // No shaped run to read a size from; 16 px is
                        // the CSS/UA default.
                        font_size: 16.0,
                        color: text_color_of(dom, styles, id),
                        glyphs: Vec::new(),
                        options: TextOptions::default(),
                    }));
                }
            }
            _ => {}
        }
        if dom.kind(id) == NodeKind::Element && clips_overflow(styles, id) {
            clip_rect = Some(LayoutRect::new(
                LayoutPoint::new(l.border.left, l.border.top),
                LayoutPoint::new(l.size.width - l.border.right, l.size.height - l.border.bottom),
            ));
        }
        true
    } else {
        false
    };

    // Clip the descendants of an overflow container to its padding box. The
    // container's own background/border (emitted above) are outside the clip.
    if let Some(rect) = clip_rect {
        commands.push(PaintCmd::PushClip(ClipSpec { kind: ClipKind::Rect(rect) }));
    }

    // Scroll: inside the clip, translate the content by `-offset` so it scrolls
    // under the fixed clip window. Only a clipping (overflow) container scrolls.
    let scroll = clip_rect.and_then(|_| em.scroll_offsets.get(&id).copied());
    if let Some((ox, oy)) = scroll {
        commands.push(PaintCmd::PushTransform(TransformSpec {
            origin: LayoutPoint::new(-ox, -oy),
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
    }

    for child in dom.dom_children(id) {
        walk(dom, styles, fragments, em, child, child_origin, commands, deferred, false);
    }

    // Unwind in reverse: scroll transform, then clip, then the origin transform.
    if scroll.is_some() {
        commands.push(PaintCmd::PopTransform);
    }
    if clip_rect.is_some() {
        commands.push(PaintCmd::PopClip);
    }
    if pushed {
        commands.push(PaintCmd::PopTransform);
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
    source: &GlyphSource<'_, NodeId>,
    dom_id: NodeId,
    bounds: LayoutRect,
    images_plane: &ImagePlane<NodeId>,
    fonts: &mut FontCollector,
    images: &mut ImageCollector,
    commands: &mut Vec<PaintCmd>,
) -> bool {
    let Some(taffy_id) = source.constructed.node_map.get(&dom_id) else {
        return false;
    };
    let Some(layout) = source.text_ctx.layouts.get(taffy_id) else {
        return false;
    };
    // The leaf's inline content, for mapping inline-box ids → source
    // `<img>` nodes. Absent for a fixed-size leaf (no shaped content);
    // glyph runs still emit, boxes just won't resolve.
    let content = source.constructed.get_node_context(*taffy_id);
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
                        .map(|g| GlyphInstance {
                            index: g.id,
                            point: LayoutPoint::new(g.x, g.y),
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
                    // rect under the run. The underline's top is `baseline +
                    // underline_offset`, thickness `underline_size` (the run's
                    // Decoration overrides the font metrics when set). Same
                    // text color as the glyphs.
                    if let Some(deco) = run.style().underline.as_ref() {
                        let m = parley_run.metrics();
                        let uo = deco.offset.unwrap_or(m.underline_offset);
                        let us = deco.size.unwrap_or(m.underline_size).max(1.0);
                        let y = bounds.min.y + run.baseline() + uo;
                        let x0 = bounds.min.x + run.offset();
                        let x1 = x0 + run.advance();
                        commands.push(PaintCmd::DrawRect(RectItem {
                            placement: CommonPlacement::new(LayoutRect::new(
                                LayoutPoint::new(x0, y),
                                LayoutPoint::new(x1, y + us),
                            )),
                            color,
                        }));
                    }
                    emitted = true;
                },
                PositionedLayoutItem::InlineBox(pbox) => {
                    // Resolve the box id back to its source <img> via the
                    // leaf's InlineContent, then look up its decoded
                    // pixels and draw at the laid-out box rect.
                    let Some(content) = content else { continue };
                    let Some(item) = content.boxes.get(pbox.id as usize) else {
                        continue;
                    };
                    let Some(decoded) = images_plane.get(item.source) else {
                        continue;
                    };
                    let key = images.add(decoded);
                    // Box position is relative to the leaf origin (same
                    // space as glyph points); place in local coords.
                    let rect = LayoutRect::new(
                        LayoutPoint::new(bounds.min.x + pbox.x, bounds.min.y + pbox.y),
                        LayoutPoint::new(
                            bounds.min.x + pbox.x + pbox.width,
                            bounds.min.y + pbox.y + pbox.height,
                        ),
                    );
                    commands.push(PaintCmd::DrawImage(ImageItem {
                        placement: CommonPlacement::new(rect),
                        image_key: key,
                        image_rendering: ImageRendering::Auto,
                        alpha_type: AlphaType::PremultipliedAlpha,
                        color: ColorF::WHITE, // identity tint
                    }));
                    emitted = true;
                },
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
fn bg_tile_style_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<BgTileStyle> {
    use style::computed_values::background_clip::single_value::T as Clip;
    use style::computed_values::background_origin::single_value::T as Origin;
    use style::values::specified::background::BackgroundRepeatKeyword as K;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let bg = data.styles.primary().get_background();
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
    use style::values::computed::length::NonNegativeLengthPercentageOrAuto as Lpa;
    use style::values::computed::Length;
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
        Bs::ExplicitSize { ref width, ref height } => {
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
        lp.resolve(Length::new(basis.max(0.0))).px()
    };
    let ox = pos(&bg.pos_x, area_w - tw);
    let oy = pos(&bg.pos_y, area_h - th);
    (tw, th, ox, oy)
}

/// Read an element's background color from its `ComputedValues`.
/// Returns transparent when no cascade data is present (hand-rolled
/// styles bypass the cascade) — that matches CSS semantics for
/// "background-color: initial".
fn background_color_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> ColorF {
    let Some(entry) = styles.get(id) else { return ColorF::TRANSPARENT; };
    let Some(data) = entry.borrow_data() else { return ColorF::TRANSPARENT; };
    let primary = data.styles.primary();
    let bg = &primary.get_background().background_color;
    let current = primary.get_inherited_text().color;
    stylo_color_to_paint(bg, current)
}

/// Whether `id` clips its overflow on either axis — i.e. `overflow-x` or
/// `overflow-y` is anything other than `visible` (`hidden`/`scroll`/`auto`/
/// `clip`). Such an element clips its descendants to its padding box (and, for
/// the scrollable values, is a scroll container). `false` when the cascade
/// hasn't run. `pub(crate)` so hit-testing ([`crate::serval_lane`]) clips the
/// same boxes the paint does.
pub(crate) fn clips_overflow<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> bool {
    use style::values::computed::Overflow;
    let Some(entry) = styles.get(id) else { return false; };
    let Some(data) = entry.borrow_data() else { return false; };
    let box_style = data.styles.primary().get_box();
    !matches!(box_style.overflow_x, Overflow::Visible)
        || !matches!(box_style.overflow_y, Overflow::Visible)
}

/// Resolve a text node's effective color: walk to its parent
/// element, read that element's `color` from `ComputedValues`
/// (a `color` value resolves to an `AbsoluteColor` directly —
/// `inherited_text.color` is already `AbsoluteColor`, not the
/// `Color` complex enum). Falls back to opaque black.
fn text_color_of<D>(dom: &D, styles: &StylePlane<D::NodeId>, text_id: D::NodeId) -> ColorF
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + std::hash::Hash,
{
    match dom.parent(text_id) {
        Some(parent_id) => element_text_color(styles, parent_id),
        None => ColorF::BLACK,
    }
}

/// An element's own cascaded text `color` as a `ColorF`. Used for the
/// uniform color of an inline-context element's text. (Per-span color
/// — colored `<span>` / `<a>` inside the flow — is a follow-up; v1
/// colors the whole inline content with the context element's color.)
/// Falls back to opaque black when the cascade hasn't run.
fn element_text_color<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> ColorF {
    let Some(entry) = styles.get(id) else { return ColorF::BLACK; };
    let Some(data) = entry.borrow_data() else { return ColorF::BLACK; };
    let absolute = data.styles.primary().get_inherited_text().color;
    let srgb = absolute.into_srgb_legacy();
    let [r, g, b, a] = *srgb.raw_components();
    ColorF::new(r, g, b, a)
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
fn border_radius_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    w: f32,
    h: f32,
) -> Option<BorderRadius> {
    use style::values::computed::Length;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let b = data.styles.primary().get_border();
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
    if zero(radius.top_left) && zero(radius.top_right)
        && zero(radius.bottom_right) && zero(radius.bottom_left)
    {
        return None;
    }
    Some(radius)
}

/// Read an element's border (widths + per-side color/style) from
/// `ComputedValues`. Returns `None` if no side has a renderable
/// border (all widths zero or all sides are `none`/`hidden`) — keeps
/// the paint stream uncluttered for un-bordered elements.
fn border_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    w: f32,
    h: f32,
) -> Option<(LayoutSideOffsets, NormalBorder)> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let border = primary.get_border();
    let current_color = primary.get_inherited_text().color;

    let top_w = border.border_top_width.0.to_f32_px();
    let right_w = border.border_right_width.0.to_f32_px();
    let bottom_w = border.border_bottom_width.0.to_f32_px();
    let left_w = border.border_left_width.0.to_f32_px();

    let top_style = stylo_border_style(border.border_top_style);
    let right_style = stylo_border_style(border.border_right_style);
    let bottom_style = stylo_border_style(border.border_bottom_style);
    let left_style = stylo_border_style(border.border_left_style);

    // No-op early-out: every side is zero-width or none/hidden style.
    let renderable = |w: f32, s: BorderStyle| {
        w > 0.0 && !matches!(s, BorderStyle::None | BorderStyle::Hidden)
    };
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
        radius: border_radius_of(styles, id, w, h).unwrap_or_else(BorderRadius::zero),
        do_aa: true,
    };
    Some((widths, details))
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
fn box_shadows_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Vec<ShadowData> {
    let Some(entry) = styles.get(id) else {
        return Vec::new();
    };
    let Some(data) = entry.borrow_data() else {
        return Vec::new();
    };
    let primary = data.styles.primary();
    let current = primary.get_inherited_text().color;
    primary
        .get_effects()
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
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
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
                _ => {}
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

    #[test]
    fn emit_round_trips_through_serde() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let (fragments, _, _) = layout(
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
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
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
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
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
        assert!(!plist.fonts().is_empty(), "expected font side-table populated");
        assert!(
            plist.fonts().iter().all(|f| !f.data.is_empty()),
            "every FontResource should carry font bytes"
        );

        // Every text run with glyphs references a key present in fonts().
        let font_keys: std::collections::HashSet<_> =
            plist.fonts().iter().map(|f| f.key).collect();
        for cmd in plist.commands() {
            if let PaintCmd::DrawText(t) = cmd {
                if !t.glyphs.is_empty() {
                    assert!(
                        font_keys.contains(&t.font_instance),
                        "DrawText font_instance {:?} not in fonts() table",
                        t.font_instance
                    );
                    assert!(t.font_size > 0.0, "shaped run should have positive font_size");
                }
            }
        }
    }

    /// Sanity-check the cache-less emit path still produces empty
    /// glyph runs (probe-mode behavior — useful when caller hasn't
    /// run layout or doesn't want to pay for glyph extraction).
    #[test]
    fn emit_without_layouts_produces_empty_glyph_runs() {
        let document =
            StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
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
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(&document, &styles, &fragments, DeviceIntSize::new(800, 600));
        let cmds = plist.commands();

        let pushes = cmds.iter().filter(|c| matches!(c, PaintCmd::PushClip(_))).count();
        let pops = cmds.iter().filter(|c| matches!(c, PaintCmd::PopClip)).count();
        assert_eq!(pushes, pops, "PushClip / PopClip balanced: {pushes} vs {pops}");

        // The scroller's clip is its 100×40 padding box (no border/padding).
        let clip = cmds
            .iter()
            .find_map(|c| match c {
                PaintCmd::PushClip(ClipSpec { kind: ClipKind::Rect(r) }) => Some(*r),
                _ => None,
            })
            .expect("an overflow container emits a rect clip");
        assert!((clip.width() - 100.0).abs() < 0.5, "clip width = box width, got {}", clip.width());
        assert!((clip.height() - 40.0).abs() < 0.5, "clip height = box height, got {}", clip.height());
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
                if document.element_name(id).is_some_and(|n| n.local == local_name!("div")) {
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
                assert!(t.origin.x.abs() < 0.01, "scroll x origin 0, got {}", t.origin.x);
                assert!((t.origin.y + 25.0).abs() < 0.01, "scroll y origin -25, got {}", t.origin.y);
            },
            other => panic!("expected scroll PushTransform after PushClip, got {other:?}"),
        }
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
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(&document, &styles, &fragments, DeviceIntSize::new(800, 600));
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
        use base64::Engine as _;
        use crate::image_decode::{BackgroundImagePlane, NoImageLoader};

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
        run_cascade(&document, &mut styles, euclid::Size2D::new(800.0, 600.0), &[sheet.as_str()], None);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(&document, &styles, &NoImageLoader);
        let plist = emit_paint_list_with_layouts(
            &document, &styles, &fragments, &built, &text_ctx,
            &ImagePlane::new(), &bg, &FxHashMap::default(),
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
            r.min.x, r.min.y,
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
                &["html, body, p { display: block; margin: 0; }", p_rule.as_str()],
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
            plist.commands().iter().filter(|c| matches!(c, PaintCmd::DrawRect(_))).count()
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

        let document =
            StaticDocument::parse("<html><body><p>x</p></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &document,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[
                "p { display: block; width: 100px; height: 50px; \
                    border: 4px solid rgb(0, 128, 255); }",
            ],
            None,
        );

        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
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
            &["div { display: block; width: 100px; height: 100px; margin: 0; \
               border: 4px solid black; border-radius: 20px; \
               background-color: rgb(0, 128, 0); }"],
            None,
        );
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(&document, &styles, &fragments, DeviceIntSize::new(800, 600));

        let border_radius = plist.commands().iter().find_map(|c| match c {
            PaintCmd::DrawBorder(BorderItem { details: BorderDetails::Normal(n), .. }) => {
                Some(n.radius.top_left.width)
            },
            _ => None,
        });
        assert!(
            border_radius.is_some_and(|r| (r - 20.0).abs() < 0.5),
            "border carries the 20px corner radius, got {border_radius:?}"
        );

        let clip_radius = plist.commands().iter().find_map(|c| match c {
            PaintCmd::PushClip(ClipSpec { kind: ClipKind::RoundedRect { radius, .. } }) => {
                Some(radius.top_left.width)
            },
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

        let document =
            StaticDocument::parse("<html><body><p>x</p></body></html>");
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
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
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
        let document =
            StaticDocument::parse("<html><body><p>x</p></body></html>");
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
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let plist = emit_paint_list(
            &document,
            &styles,
            &fragments,
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
        use base64::Engine as _;
        use crate::image_decode::{BackgroundImagePlane, NoImageLoader};

        // 20×20 solid image, inline data-URI so decode_from_cascade
        // decodes it with the no-op loader (no filesystem).
        let img = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 128, 0, 255]));
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode test PNG");
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let html = format!(
            "<html><body><div></div></body></html>"
        );
        let document = StaticDocument::parse(&html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        let sheet = format!(
            "div {{ display: block; width: 100px; height: 100px; margin: 0; \
             background-image: url(data:image/png;base64,{b64}); \
             background-size: 50%; background-repeat: no-repeat; }}"
        );
        run_cascade(&document, &mut styles, euclid::Size2D::new(800.0, 600.0), &[sheet.as_str()], None);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&document, &styles, &ImagePlane::new(), viewport);
        let bg = BackgroundImagePlane::decode_from_cascade(&document, &styles, &NoImageLoader);
        assert_eq!(bg.len(), 1, "the div's background-image must decode (else the emit branch never runs)");

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
            tile.width, tile.height,
        );
    }

    #[test]
    fn emit_paint_order_is_pre_order() {
        // Sanity-check that children paint after parents (so they
        // appear later in the command list), matching pre-order DOM
        // traversal.
        let document = StaticDocument::parse(
            "<html><body><p>a</p><p>b</p></body></html>",
        );
        let styles = build_style_plane(&document);
        let (fragments, _, _) = layout(
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
            DeviceIntSize::new(800, 600),
        );

        // The first command must be PushTransform (compositor model:
        // each fragment opens a new coord space before painting itself).
        match plist.commands().first() {
            Some(PaintCmd::PushTransform(_)) => {}
            other => panic!("expected leading PushTransform, got {other:?}"),
        }

        // The command right after the leading PushTransform must be a
        // DrawRect — the html element painting itself in local coords.
        match plist.commands().get(1) {
            Some(PaintCmd::DrawRect(_)) => {}
            other => panic!("expected DrawRect after leading PushTransform, got {other:?}"),
        }

        // Push/Pop pairs must balance — the compositor-stack invariant.
        let mut depth = 0i32;
        for cmd in plist.commands() {
            match cmd {
                PaintCmd::PushTransform(_) => depth += 1,
                PaintCmd::PopTransform => depth -= 1,
                _ => {}
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
}
