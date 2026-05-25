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
//! ## Probe v1 scope (2026-05-18)
//!
//! - `DrawRect` per element with non-default background. The probe
//!   currently emits an opaque white rect per element since the
//!   cascade runs against an empty stylist; once real stylesheets
//!   apply, [`background_color_of`] becomes the place that reads
//!   `ComputedValues::background.background_color`.
//! - `DrawText` per text leaf with **empty glyph runs**. Real glyph
//!   shaping requires either (a) re-shaping in the emit phase or (b)
//!   caching the parley `Layout` from measure. Both are reasonable —
//!   deferred to a follow-up that picks one based on profile-data;
//!   for the trait-surface probe, empty glyphs is enough to validate
//!   that emission produces the right command structure.
//! - Coordinates are absolute (pre-order accumulated offsets), no
//!   `PushTransform`/`PopTransform` yet. The compositor model fits
//!   nicely with `taffy::Layout.location` being parent-relative, but
//!   emitting it requires `<element>` ↔ `<transform>` bookkeeping
//!   that's deferred until a renderer pulls on it.
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
use paint_list_api::specs::TransformKind;
use parley::PositionedLayoutItem;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::box_tree::BoxTree;
use crate::fragment::FragmentPlane;
use crate::image_decode::{BackgroundImagePlane, DecodedImage, ImagePlane};
use crate::style::StylePlane;
use crate::text_measure::TextMeasureCtx;

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
#[derive(Default)]
struct ImageCollector {
    images: Vec<ImageResource>,
    next_idx: u32,
}

impl ImageCollector {
    /// Add a decoded image, returning the key the matching
    /// `ImageItem::image_key` should carry.
    fn add(&mut self, decoded: &DecodedImage) -> ImageKey {
        let key = ImageKey::new(SERVAL_IMAGE_NAMESPACE, self.next_idx);
        self.next_idx += 1;
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
struct Emitter<'a, NodeId: Copy + Eq + Hash> {
    /// Shaped-glyph source (None on the cache-less probe path).
    glyphs: Option<GlyphSource<'a, NodeId>>,
    /// Decoded `<img>` images keyed by NodeId.
    images_plane: &'a ImagePlane<NodeId>,
    /// Decoded CSS `background-image`s keyed by NodeId.
    bg_images_plane: &'a BackgroundImagePlane<NodeId>,
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
    emit_inner(dom, styles, fragments, None, &empty_images, &empty_bg, viewport)
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
        fonts: FontCollector::default(),
        images: ImageCollector::default(),
    };
    walk(dom, styles, fragments, &mut emitter, dom.document(), &mut commands);
    ServalPaintList {
        viewport,
        commands,
        generation: 0,
        fonts: emitter.fonts.fonts,
        images: emitter.images.images,
    }
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
fn walk<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    em: &mut Emitter<'_, D::NodeId>,
    id: D::NodeId,
    commands: &mut Vec<PaintCmd>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let pushed = if let Some(l) = fragments.rect_of(id) {
        commands.push(PaintCmd::PushTransform(TransformSpec {
            origin: LayoutPoint::new(l.location.x, l.location.y),
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
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
                // Background, then replaced content (image), then
                // border — CSS paint order.
                commands.push(PaintCmd::DrawRect(RectItem {
                    placement: CommonPlacement::new(local_bounds),
                    color: background_color_of(styles, id),
                }));
                // CSS background-image paints over the background color,
                // under content + border. Default background-repeat is
                // `repeat`, so tile at the image's intrinsic size across
                // the element box (DrawRepeatingImage).
                if let Some(decoded) = em.bg_images_plane.get(id) {
                    let key = em.images.add(decoded);
                    commands.push(PaintCmd::DrawRepeatingImage(RepeatingImageItem {
                        placement: CommonPlacement::new(local_bounds),
                        image_key: key,
                        stretch_size: LayoutSize::new(
                            decoded.width as f32,
                            decoded.height as f32,
                        ),
                        tile_spacing: LayoutSize::zero(),
                        image_rendering: ImageRendering::Auto,
                        alpha_type: AlphaType::PremultipliedAlpha,
                        color: ColorF::WHITE, // identity tint
                    }));
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
                if let Some((widths, normal)) = border_of(styles, id) {
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
        true
    } else {
        false
    };

    for child in dom.dom_children(id) {
        walk(dom, styles, fragments, em, child, commands);
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

/// Read an element's border (widths + per-side color/style) from
/// `ComputedValues`. Returns `None` if no side has a renderable
/// border (all widths zero or all sides are `none`/`hidden`) — keeps
/// the paint stream uncluttered for un-bordered elements.
fn border_of<NodeId: Copy + Eq + std::hash::Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
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
        radius: BorderRadius::zero(),
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
