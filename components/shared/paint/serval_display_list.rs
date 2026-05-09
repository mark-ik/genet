/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ServalDisplayList` — netrender-shaped display-list intermediate.
//!
//! Layout emits a [`ServalDisplayList`] (a serializable IPC payload of
//! [`ServalDisplayItem`]s plus spatial-node / clip palettes); the
//! painter (in `components/paint/`) translates each item into one or
//! more `netrender::SceneOp`s and pushes onto a `netrender::Scene`.
//!
//! Cf. [`docs/2026-05-06_c3_layout_reshape_plan.md`](../../../docs/2026-05-06_c3_layout_reshape_plan.md)
//! for the design and the file-by-file plan that introduces this
//! module's consumers.
//!
//! ## Why this shape
//!
//! Servo's existing architecture sends display lists across IPC from
//! the script/layout thread to the paint thread; the post-C3 painter
//! is netrender-driven, not webrender-driven, so the wire format
//! changes from `webrender_api::BuiltDisplayList` to this module's
//! [`ServalDisplayList`]. The painter holds all netrender knowledge;
//! layout doesn't import `netrender::*`.
//!
//! ## Naming convention
//!
//! Where webrender used `*Item` for variant payloads (e.g.
//! `RectItem`), this module follows suit. Spatial / clip / reference-
//! frame indices are opaque newtypes ([`SpatialId`], [`ClipChainId`],
//! [`ReferenceFrameId`]) over `u32`; layout treats them as tokens.

use std::fmt;

use malloc_size_of_derive::MallocSizeOf;
use paint_types::units::{
    DeviceIntSize, LayoutPoint, LayoutRect, LayoutSideOffsets, LayoutSize, LayoutTransform,
    LayoutVector2D,
};
use paint_types::{
    BorderRadius, BorderStyle, BoxShadowClipMode, ColorF, ExtendMode, ExternalScrollId,
    FontInstanceKey, GradientStop, ImageKey, ImageRendering, LineStyle, MixBlendMode, PipelineId,
    ReferenceFrameKind, RepeatMode, SpatialId, SpatialTreeItemKey, StickyOffsetBounds,
    TransformStyle,
};
use serde::{Deserialize, Serialize};

// =============================================================================
// Opaque palette indices
// =============================================================================

/// Index into [`ServalDisplayList::clip_defs`]. Sentinel
/// [`ClipChainId::INVALID`] marks "no clip applied"; layout's
/// `ClipId::INVALID` (in the layout-internal clip-store sense) maps
/// to this on emission.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct ClipChainId(pub u32);

impl ClipChainId {
    pub const INVALID: ClipChainId = ClipChainId(u32::MAX);

    pub fn is_invalid(&self) -> bool {
        *self == Self::INVALID
    }
}

/// Index into [`ServalDisplayList::transforms`]. Index 0 is reserved
/// for identity.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct ReferenceFrameId(pub u32);

impl ReferenceFrameId {
    pub const IDENTITY: ReferenceFrameId = ReferenceFrameId(0);
}

// =============================================================================
// Common item metadata
// =============================================================================

/// Per-item presentation flags. Carried inline on every
/// [`ServalDisplayItem`] payload that needs them, replacing
/// webrender's `CommonItemProperties` aggregator.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct PrimitiveFlags(pub u32);

impl PrimitiveFlags {
    /// Item participates in hit-testing (default).
    pub const HIT_TESTABLE: Self = Self(1 << 0);
    /// Item is the backface of a 3D-transformed element.
    pub const IS_BACKFACE: Self = Self(1 << 1);
    /// Item should be clipped to the integer pixel grid.
    pub const ANTIALIASED: Self = Self(1 << 2);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for PrimitiveFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for PrimitiveFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// =============================================================================
// Spatial / clip palette definitions
// =============================================================================

/// One declared spatial node. The palette in
/// [`ServalDisplayList::spatial_nodes`] is z-order-independent (it's
/// a lookup table); items reference entries by [`SpatialId`].
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum SpatialNodeDef {
    /// Root frame for the pipeline. Always at [`SpatialId::ROOT`].
    Root,
    ScrollFrame(ScrollFrameDef),
    StickyFrame(StickyFrameDef),
    ReferenceFrame(ReferenceFrameDef),
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ScrollFrameDef {
    pub parent: SpatialId,
    pub external_id: ExternalScrollId,
    pub content_rect: LayoutRect,
    pub clip_rect: LayoutRect,
    pub external_scroll_offset: LayoutVector2D,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct StickyFrameDef {
    pub parent: SpatialId,
    pub frame_rect: LayoutRect,
    pub margins: StickyMargins,
    pub vertical_offset_bounds: StickyOffsetBounds,
    pub horizontal_offset_bounds: StickyOffsetBounds,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct StickyMargins {
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub left: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ReferenceFrameDef {
    pub parent: SpatialId,
    pub origin: LayoutPoint,
    pub transform: ReferenceFrameId,
    pub kind: ReferenceFrameKind,
}

/// One declared clip in [`ServalDisplayList::clip_defs`]. Items
/// reference entries by [`ClipChainId`]. A clip is one of:
/// rectangle, rounded-rectangle, or a chain of two (parent + this).
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum ClipDef {
    Rect(ClipRectDef),
    RoundedRect(ClipRoundedRectDef),
    Chain(ClipChainDef),
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ClipRectDef {
    pub spatial: SpatialId,
    pub rect: LayoutRect,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ClipRoundedRectDef {
    pub spatial: SpatialId,
    pub rect: LayoutRect,
    pub radius: BorderRadius,
    pub mode: ClipMode,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum ClipMode {
    /// Inside-the-shape pixels are kept (default).
    #[default]
    Clip,
    /// Inside-the-shape pixels are clipped *out*.
    ClipOut,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ClipChainDef {
    pub parent: ClipChainId,
    pub clip: ClipChainId,
}

// =============================================================================
// Item payloads
// =============================================================================

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct CommonItemPlacement {
    pub clip_rect: LayoutRect,
    pub clip_chain_id: ClipChainId,
    pub spatial_id: SpatialId,
    pub flags: PrimitiveFlags,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RectItem {
    pub placement: CommonItemPlacement,
    pub color: ColorF,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RectAnimItem {
    pub placement: CommonItemPlacement,
    pub color: ColorF,
    /// Animation hook — first cut emits `None`; downstream painter
    /// ignores. Real animation property binding lands in a follow-up.
    pub animation: Option<AnimationBindingKey>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct AnimationBindingKey(pub u64);

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct LineItem {
    pub placement: CommonItemPlacement,
    pub color: ColorF,
    pub style: LineStyle,
    pub orientation: LineOrientation,
    pub wavy_thickness: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum LineOrientation {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ImageItem {
    pub placement: CommonItemPlacement,
    pub image_key: ImageKey,
    pub image_rendering: ImageRendering,
    pub alpha_type: AlphaType,
    pub color: ColorF,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum AlphaType {
    #[default]
    Alpha,
    PremultipliedAlpha,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RepeatingImageItem {
    pub placement: CommonItemPlacement,
    pub image_key: ImageKey,
    pub stretch_size: LayoutSize,
    pub tile_spacing: LayoutSize,
    pub image_rendering: ImageRendering,
    pub alpha_type: AlphaType,
    pub color: ColorF,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct TextItem {
    pub placement: CommonItemPlacement,
    pub font_instance: FontInstanceKey,
    pub color: ColorF,
    pub glyphs: Vec<GlyphInstance>,
    pub glyph_options: Option<GlyphOptions>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct GlyphInstance {
    pub index: u32,
    pub point: LayoutPoint,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct GlyphOptions {
    pub render_mode: FontRenderMode,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum FontRenderMode {
    Mono,
    Alpha,
    #[default]
    Subpixel,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct BorderItem {
    pub placement: CommonItemPlacement,
    pub widths: LayoutSideOffsets,
    pub details: BorderDetails,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum BorderDetails {
    Normal(paint_types::NormalBorder),
    NinePatch(NinePatchBorderDetails),
}

pub use paint_types::border::BorderSide;

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct NinePatchBorderDetails {
    pub source: NinePatchBorderSource,
    pub width: i32,
    pub height: i32,
    pub slice: paint_types::units::DeviceIntSideOffsets,
    pub fill: bool,
    pub repeat_horizontal: RepeatMode,
    pub repeat_vertical: RepeatMode,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum NinePatchBorderSource {
    Image(ImageKey, ImageRendering),
    Gradient(GradientPayload),
    RadialGradient(RadialGradientPayload),
    ConicGradient(ConicGradientPayload),
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct BoxShadowItem {
    pub placement: CommonItemPlacement,
    pub box_bounds: LayoutRect,
    pub offset: LayoutVector2D,
    pub color: ColorF,
    pub blur_radius: f32,
    pub spread_radius: f32,
    pub border_radius: BorderRadius,
    pub clip_mode: BoxShadowClipMode,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ShadowItem {
    /// Shadow paints affect the next [`ServalDisplayItem`]s in the
    /// list until a [`ServalDisplayItem::PopAllShadows`] is emitted.
    pub offset: LayoutVector2D,
    pub color: ColorF,
    pub blur_radius: f32,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct GradientItem {
    pub placement: CommonItemPlacement,
    pub gradient: GradientPayload,
    pub tile_size: LayoutSize,
    pub tile_spacing: LayoutSize,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct GradientPayload {
    pub start_point: LayoutPoint,
    pub end_point: LayoutPoint,
    pub extend_mode: ExtendMode,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RadialGradientItem {
    pub placement: CommonItemPlacement,
    pub gradient: RadialGradientPayload,
    pub tile_size: LayoutSize,
    pub tile_spacing: LayoutSize,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RadialGradientPayload {
    pub center: LayoutPoint,
    pub radius: LayoutSize,
    pub extend_mode: ExtendMode,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ConicGradientItem {
    pub placement: CommonItemPlacement,
    pub gradient: ConicGradientPayload,
    pub tile_size: LayoutSize,
    pub tile_spacing: LayoutSize,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ConicGradientPayload {
    pub center: LayoutPoint,
    pub angle: f32,
    pub extend_mode: ExtendMode,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct IframeItem {
    pub placement: CommonItemPlacement,
    pub pipeline_id: PipelineId,
    pub ignore_missing_pipeline: bool,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct StackingContextItem {
    pub placement: CommonItemPlacement,
    pub origin: LayoutPoint,
    pub transform_style: TransformStyle,
    pub mix_blend_mode: MixBlendMode,
    pub filters: Vec<FilterOp>,
    pub flags: StackingContextFlags,
    pub raster_space: RasterSpace,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct StackingContextFlags(pub u32);

impl StackingContextFlags {
    pub const IS_BLEND_CONTAINER: Self = Self(1 << 0);
    pub const IS_BACKDROP_ROOT: Self = Self(1 << 1);
    pub const HAS_SCROLL_LINKED_EFFECT: Self = Self(1 << 2);

    pub const fn empty() -> Self {
        Self(0)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum RasterSpace {
    #[default]
    Screen,
    Local,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum FilterOp {
    Identity,
    Blur(f32),
    Brightness(f32),
    Contrast(f32),
    Grayscale(f32),
    HueRotate(f32),
    Invert(f32),
    Opacity(f32),
    Saturate(f32),
    Sepia(f32),
    DropShadow {
        offset: LayoutVector2D,
        color: ColorF,
        blur_radius: f32,
    },
    ColorMatrix([f32; 20]),
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ReferenceFramePushItem {
    pub origin: LayoutPoint,
    pub transform_id: ReferenceFrameId,
    pub kind: ReferenceFrameKind,
    pub spatial_id: SpatialId,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct HitTestItem {
    pub placement: CommonItemPlacement,
    pub tag: u64,
}

// =============================================================================
// Item enum
// =============================================================================

/// One paint operation in a [`ServalDisplayList`]. Painter-side
/// translation maps each variant to one or more `netrender::SceneOp`s.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum ServalDisplayItem {
    Rect(RectItem),
    RectWithAnimation(RectAnimItem),
    Line(LineItem),
    Image(ImageItem),
    RepeatingImage(RepeatingImageItem),
    Text(TextItem),
    Border(BorderItem),
    BoxShadow(BoxShadowItem),
    PushShadow(ShadowItem),
    PopAllShadows,
    Gradient(GradientItem),
    RadialGradient(RadialGradientItem),
    ConicGradient(ConicGradientItem),
    Iframe(IframeItem),
    PushStackingContext(StackingContextItem),
    PopStackingContext,
    PushReferenceFrame(ReferenceFramePushItem),
    PopReferenceFrame,
    HitTest(HitTestItem),
}

// =============================================================================
// ServalDisplayList — the wire/in-memory shape
// =============================================================================

/// A pipeline's emitted display list. Layout pushes items, declares
/// spatial nodes / clips / transforms, and ships this struct over IPC
/// to the painter (via the existing `PaintMessage::SendDisplayList`
/// envelope, post-C2 reshape).
///
/// The struct is the layout-side analog of webrender's
/// `BuiltDisplayList` + `SpaceAndClipInfo` aggregator. The painter
/// holds netrender-specific knowledge; this struct does not import
/// `netrender::*`.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ServalDisplayList {
    pub viewport: DeviceIntSize,
    pub pipeline_id: PipelineId,
    /// Painter-order operation stream. Push-order is paint-order.
    pub items: Vec<ServalDisplayItem>,
    /// Spatial-node palette. Index 0 is always
    /// [`SpatialNodeDef::Root`].
    pub spatial_nodes: Vec<SpatialNodeDef>,
    /// Clip palette. Items reference entries by [`ClipChainId`].
    pub clip_defs: Vec<ClipDef>,
    /// Transform palette. Index 0 is identity.
    pub transforms: Vec<LayoutTransform>,
}

impl ServalDisplayList {
    pub fn new(viewport: DeviceIntSize, pipeline_id: PipelineId) -> Self {
        Self {
            viewport,
            pipeline_id,
            items: Vec::new(),
            spatial_nodes: vec![SpatialNodeDef::Root],
            clip_defs: Vec::new(),
            transforms: vec![LayoutTransform::identity()],
        }
    }

    /// Append one paint op. Push-order is paint-order.
    pub fn push(&mut self, item: ServalDisplayItem) {
        self.items.push(item);
    }

    /// Register a clip; returns the [`ClipChainId`] that subsequent
    /// items can reference.
    pub fn define_clip(&mut self, def: ClipDef) -> ClipChainId {
        let id = ClipChainId(self.clip_defs.len() as u32);
        self.clip_defs.push(def);
        id
    }

    /// Register a spatial node; returns the [`SpatialId`]. The id's
    /// numeric component indexes into [`Self::spatial_nodes`]; the
    /// pipeline component echoes [`Self::pipeline_id`] so consumers can
    /// disambiguate cross-pipeline ids.
    pub fn define_spatial_node(&mut self, def: SpatialNodeDef) -> SpatialId {
        let id = SpatialId(self.spatial_nodes.len() as u64, self.pipeline_id);
        self.spatial_nodes.push(def);
        id
    }

    /// The root reference frame for this pipeline. Always index 0.
    pub fn root_spatial_id(&self) -> SpatialId {
        SpatialId(0, self.pipeline_id)
    }

    /// Register a transform; returns the [`ReferenceFrameId`]. Index 0
    /// is always identity (allocated in `new`).
    pub fn define_transform(&mut self, transform: LayoutTransform) -> ReferenceFrameId {
        let id = ReferenceFrameId(self.transforms.len() as u32);
        self.transforms.push(transform);
        id
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// =============================================================================
// Webrender-shaped compatibility layer
// =============================================================================
//
// During the C3 cut, layout-side code (`components/layout/display_list/`) is
// being retargeted from the webrender_api `DisplayListBuilder` API to direct
// `ServalDisplayItem` construction. Until that retargeting completes, the
// compat surface below lets the layout call sites continue to invoke a
// `wr`-shaped API (push_rect / push_image / define_clip_rect / etc) while
// the bodies push proper `ServalDisplayItem` variants.
//
// Most bodies here are first-cut stubs that emit reasonable defaults; the
// proper translation lands as part of Step 4 in
// `docs/2026-05-06_c3_layout_reshape_plan.md`.

/// Stub stand-in for webrender's `ComplexClipRegion`. Used by
/// `define_clip_rounded_rect`. Drops the optional rounded-rect mode in
/// favor of always-clip semantics for the first cut.
#[derive(Clone, Copy, Debug)]
pub struct ComplexClipRegion {
    pub rect: LayoutRect,
    pub radii: BorderRadius,
    pub mode: ClipMode,
}

/// Stub stand-in for webrender's `Shadow`. Mirrors `ShadowItem` in
/// content; kept as a separate type so the layout-side type-checks
/// against the historical `wr::Shadow` shape.
#[derive(Clone, Copy, Debug)]
pub struct Shadow {
    pub offset: LayoutVector2D,
    pub color: ColorF,
    pub blur_radius: f32,
}

/// Bundled (spatial_id, clip_chain_id) pair, as the webrender
/// `SpaceAndClipInfo` expressed it. Layout passes one of these to
/// `push_iframe` / `push_shadow`.
#[derive(Clone, Copy, Debug)]
pub struct SpaceAndClipInfo {
    pub spatial_id: SpatialId,
    pub clip_chain_id: ClipChainId,
}

/// Stub for webrender's `HasScrollLinkedEffect`. Layout uses it as an
/// opaque marker on `define_scroll_frame`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HasScrollLinkedEffect {
    Yes,
    #[default]
    No,
}

// Re-export paint_types' versions under the historical webrender name
// `PropertyBinding` for source-compat with layout call sites.
pub use paint_types::PropertyBindingKey;
pub use paint_types::property::PropertyValue as PropertyBinding;

/// Layout-side `ClipId`. Returned by `define_clip_rect` /
/// `define_clip_rounded_rect`; consumed by `define_clip_chain`. The
/// painter side does not yet act on these directly — they're recorded
/// in the `clip_defs` palette.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WrClipId(pub u32);

impl ServalDisplayList {
    // ----- structural ops -----------------------------------------------

    /// Webrender DisplayListBuilder mimicry: noop. ServalDisplayList is
    /// ready-to-fill from `new`; no explicit begin step is needed.
    pub fn begin(&mut self) {}

    /// Webrender DisplayListBuilder mimicry: returns `((), ())` matching
    /// the shape that layout's old `let (_, empty_display_list) = builder.end()`
    /// destructured. The "built" display list is `self`; layout retains
    /// the whole struct after this call.
    pub fn end(&mut self) -> ((), ()) {
        ((), ())
    }

    /// No-op. Original webrender call enabled serialized display-list
    /// dump on the next `finalize`. Netrender doesn't use that channel.
    pub fn dump_serialized_display_list(&mut self) {}

    // ----- primitive pushes ---------------------------------------------

    pub fn push_rect(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        color: ColorF,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::Rect(RectItem { placement, color }));
    }

    pub fn push_rect_with_animation(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        color: PropertyBinding<ColorF>,
    ) {
        let resolved_color = match color {
            PropertyBinding::Value(c) | PropertyBinding::Binding(_, c) => c,
        };
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::RectWithAnimation(RectAnimItem {
            placement,
            color: resolved_color,
            animation: None,
        }));
    }

    pub fn push_image(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        image_rendering: ImageRendering,
        alpha_type: AlphaType,
        image_key: ImageKey,
        color: ColorF,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::Image(ImageItem {
            placement,
            image_key,
            image_rendering,
            alpha_type,
            color,
        }));
    }

    pub fn push_repeating_image(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        stretch_size: LayoutSize,
        tile_spacing: LayoutSize,
        image_rendering: ImageRendering,
        alpha_type: AlphaType,
        image_key: ImageKey,
        color: ColorF,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::RepeatingImage(RepeatingImageItem {
            placement,
            image_key,
            stretch_size,
            tile_spacing,
            image_rendering,
            alpha_type,
            color,
        }));
    }

    pub fn push_text(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        glyphs: &[GlyphInstance],
        font_instance: FontInstanceKey,
        color: ColorF,
        glyph_options: Option<GlyphOptions>,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::Text(TextItem {
            placement,
            font_instance,
            color,
            glyphs: glyphs.to_vec(),
            glyph_options,
        }));
    }

    pub fn push_line(
        &mut self,
        common: &CommonItemPlacement,
        area: &LayoutRect,
        wavy_thickness: f32,
        orientation: LineOrientation,
        color: &ColorF,
        style: LineStyle,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: *area,
            ..*common
        };
        self.push(ServalDisplayItem::Line(LineItem {
            placement,
            color: *color,
            style,
            orientation,
            wavy_thickness,
        }));
    }

    pub fn push_border(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        widths: LayoutSideOffsets,
        details: BorderDetails,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::Border(BorderItem {
            placement,
            widths,
            details,
        }));
    }

    pub fn push_box_shadow(
        &mut self,
        common: &CommonItemPlacement,
        box_bounds: LayoutRect,
        offset: LayoutVector2D,
        color: ColorF,
        blur_radius: f32,
        spread_radius: f32,
        border_radius: BorderRadius,
        clip_mode: BoxShadowClipMode,
    ) {
        self.push(ServalDisplayItem::BoxShadow(BoxShadowItem {
            placement: *common,
            box_bounds,
            offset,
            color,
            blur_radius,
            spread_radius,
            border_radius,
            clip_mode,
        }));
    }

    pub fn push_shadow(
        &mut self,
        _info: &SpaceAndClipInfo,
        shadow: Shadow,
        _should_inflate: bool,
    ) {
        self.push(ServalDisplayItem::PushShadow(ShadowItem {
            offset: shadow.offset,
            color: shadow.color,
            blur_radius: shadow.blur_radius,
        }));
    }

    pub fn pop_all_shadows(&mut self) {
        self.push(ServalDisplayItem::PopAllShadows);
    }

    pub fn push_gradient(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        gradient: GradientPayload,
        tile_size: LayoutSize,
        tile_spacing: LayoutSize,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::Gradient(GradientItem {
            placement,
            gradient,
            tile_size,
            tile_spacing,
        }));
    }

    pub fn push_radial_gradient(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        gradient: RadialGradientPayload,
        tile_size: LayoutSize,
        tile_spacing: LayoutSize,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::RadialGradient(RadialGradientItem {
            placement,
            gradient,
            tile_size,
            tile_spacing,
        }));
    }

    pub fn push_conic_gradient(
        &mut self,
        common: &CommonItemPlacement,
        bounds: LayoutRect,
        gradient: ConicGradientPayload,
        tile_size: LayoutSize,
        tile_spacing: LayoutSize,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            ..*common
        };
        self.push(ServalDisplayItem::ConicGradient(ConicGradientItem {
            placement,
            gradient,
            tile_size,
            tile_spacing,
        }));
    }

    pub fn push_iframe(
        &mut self,
        bounds: LayoutRect,
        _clip_rect: LayoutRect,
        info: &SpaceAndClipInfo,
        pipeline_id: PipelineId,
        ignore_missing_pipeline: bool,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            clip_chain_id: info.clip_chain_id,
            spatial_id: info.spatial_id,
            flags: PrimitiveFlags::empty(),
        };
        self.push(ServalDisplayItem::Iframe(IframeItem {
            placement,
            pipeline_id,
            ignore_missing_pipeline,
        }));
    }

    pub fn push_hit_test(
        &mut self,
        bounds: LayoutRect,
        clip: ClipChainId,
        spatial: SpatialId,
        flags: PrimitiveFlags,
        tag: (u64, u16),
    ) {
        let placement = CommonItemPlacement {
            clip_rect: bounds,
            clip_chain_id: clip,
            spatial_id: spatial,
            flags,
        };
        let combined_tag = ((tag.0 as u64) << 16) | (tag.1 as u64);
        self.push(ServalDisplayItem::HitTest(HitTestItem {
            placement,
            tag: combined_tag,
        }));
    }

    // ----- stacking-context / reference-frame ---------------------------

    #[expect(clippy::too_many_arguments)]
    pub fn push_stacking_context(
        &mut self,
        origin: LayoutPoint,
        spatial: SpatialId,
        flags: PrimitiveFlags,
        clip: Option<ClipChainId>,
        transform_style: TransformStyle,
        mix_blend_mode: MixBlendMode,
        filters: &[FilterOp],
        _filter_datas: &[()],
        _filter_primitives: &[()],
        raster_space: RasterSpace,
        sc_flags: StackingContextFlags,
        _snapshot: Option<()>,
    ) {
        let placement = CommonItemPlacement {
            clip_rect: LayoutRect::zero(),
            clip_chain_id: clip.unwrap_or(ClipChainId::INVALID),
            spatial_id: spatial,
            flags,
        };
        self.push(ServalDisplayItem::PushStackingContext(StackingContextItem {
            placement,
            origin,
            transform_style,
            mix_blend_mode,
            filters: filters.to_vec(),
            flags: sc_flags,
            raster_space,
        }));
    }

    pub fn pop_stacking_context(&mut self) {
        self.push(ServalDisplayItem::PopStackingContext);
    }

    pub fn push_reference_frame(
        &mut self,
        origin: LayoutPoint,
        parent_spatial_id: SpatialId,
        _transform_style: TransformStyle,
        transform: PropertyBinding<LayoutTransform>,
        kind: ReferenceFrameKind,
        _key: SpatialTreeItemKey,
    ) -> SpatialId {
        let transform_value = match transform {
            PropertyBinding::Value(t) | PropertyBinding::Binding(_, t) => t,
        };
        let transform_id = self.define_transform(transform_value);
        let new_spatial = self.define_spatial_node(SpatialNodeDef::ReferenceFrame(
            ReferenceFrameDef {
                parent: parent_spatial_id,
                origin,
                transform: transform_id,
                kind,
            },
        ));
        self.push(ServalDisplayItem::PushReferenceFrame(ReferenceFramePushItem {
            origin,
            transform_id,
            kind,
            spatial_id: new_spatial,
        }));
        new_spatial
    }

    pub fn pop_reference_frame(&mut self) {
        self.push(ServalDisplayItem::PopReferenceFrame);
    }

    // ----- clip / scroll palette ----------------------------------------

    pub fn define_clip_rect(&mut self, spatial: SpatialId, rect: LayoutRect) -> WrClipId {
        let id = self.clip_defs.len() as u32;
        self.clip_defs
            .push(ClipDef::Rect(ClipRectDef { spatial, rect }));
        WrClipId(id)
    }

    pub fn define_clip_rounded_rect(
        &mut self,
        spatial: SpatialId,
        region: ComplexClipRegion,
    ) -> WrClipId {
        let id = self.clip_defs.len() as u32;
        self.clip_defs.push(ClipDef::RoundedRect(ClipRoundedRectDef {
            spatial,
            rect: region.rect,
            radius: region.radii,
            mode: region.mode,
        }));
        WrClipId(id)
    }

    pub fn define_clip_chain<I: IntoIterator<Item = WrClipId>>(
        &mut self,
        parent: Option<ClipChainId>,
        clips: I,
    ) -> ClipChainId {
        // First-cut: chain semantics collapse to the last clip in the
        // iterator, parented to whatever was passed in. Recorded in the
        // clip palette so the painter can read it back; no per-clip-chain
        // hierarchy materialized yet.
        let parent = parent.unwrap_or(ClipChainId::INVALID);
        let mut last = parent;
        for WrClipId(id) in clips {
            let chain_id = self.clip_defs.len() as u32;
            self.clip_defs.push(ClipDef::Chain(ClipChainDef {
                parent: last,
                clip: ClipChainId(id),
            }));
            last = ClipChainId(chain_id);
        }
        last
    }

    #[expect(clippy::too_many_arguments)]
    pub fn define_scroll_frame(
        &mut self,
        parent: SpatialId,
        external_id: ExternalScrollId,
        content_rect: LayoutRect,
        clip_rect: LayoutRect,
        external_scroll_offset: LayoutVector2D,
        _scroll_offset_generation: u64,
        _has_scroll_linked_effect: HasScrollLinkedEffect,
        _key: SpatialTreeItemKey,
    ) -> SpatialId {
        self.define_spatial_node(SpatialNodeDef::ScrollFrame(ScrollFrameDef {
            parent,
            external_id,
            content_rect,
            clip_rect,
            external_scroll_offset,
        }))
    }

    #[expect(clippy::too_many_arguments)]
    pub fn define_sticky_frame(
        &mut self,
        parent: SpatialId,
        frame_rect: LayoutRect,
        margins: euclid::SideOffsets2D<Option<f32>, paint_types::units::LayoutPixel>,
        vertical_offset_bounds: paint_types::StickyOffsetBounds,
        horizontal_offset_bounds: paint_types::StickyOffsetBounds,
        _previously_applied_offset: LayoutVector2D,
        _key: SpatialTreeItemKey,
        _transform: Option<PropertyBinding<LayoutTransform>>,
    ) -> SpatialId {
        self.define_spatial_node(SpatialNodeDef::StickyFrame(StickyFrameDef {
            parent,
            frame_rect,
            margins: StickyMargins {
                top: margins.top,
                right: margins.right,
                bottom: margins.bottom,
                left: margins.left,
            },
            vertical_offset_bounds,
            horizontal_offset_bounds,
        }))
    }

    /// No-op stub. Webrender accepted gradient stops as a separate
    /// stream; in the netrender shape stops live inline on the
    /// `GradientPayload` so this helper has no work to do.
    pub fn push_stops(&mut self, _stops: &[GradientStop]) {}
}

impl fmt::Display for ServalDisplayList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ServalDisplayList(pipeline={:?} items={} clips={} spatial_nodes={} transforms={})",
            self.pipeline_id,
            self.items.len(),
            self.clip_defs.len(),
            self.spatial_nodes.len(),
            self.transforms.len(),
        )
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_list_has_root_spatial_node_and_identity_transform() {
        let list = ServalDisplayList::new(
            DeviceIntSize::new(800, 600),
            PipelineId::default(),
        );
        assert!(list.is_empty());
        assert_eq!(list.spatial_nodes.len(), 1);
        assert_eq!(list.transforms.len(), 1);
        assert!(matches!(list.spatial_nodes[0], SpatialNodeDef::Root));
    }

    #[test]
    fn define_clip_returns_sequential_ids() {
        let mut list = ServalDisplayList::new(
            DeviceIntSize::new(800, 600),
            PipelineId::default(),
        );
        let root = list.root_spatial_id();
        let id1 = list.define_clip(ClipDef::Rect(ClipRectDef {
            spatial: root,
            rect: LayoutRect::zero(),
        }));
        let id2 = list.define_clip(ClipDef::Rect(ClipRectDef {
            spatial: root,
            rect: LayoutRect::zero(),
        }));
        assert_eq!(id1, ClipChainId(0));
        assert_eq!(id2, ClipChainId(1));
    }

    #[test]
    fn primitive_flags_or_combines() {
        let f = PrimitiveFlags::HIT_TESTABLE | PrimitiveFlags::ANTIALIASED;
        assert!(f.contains(PrimitiveFlags::HIT_TESTABLE));
        assert!(f.contains(PrimitiveFlags::ANTIALIASED));
        assert!(!f.contains(PrimitiveFlags::IS_BACKFACE));
    }

    #[test]
    fn clip_chain_id_invalid_round_trip() {
        assert!(ClipChainId::INVALID.is_invalid());
        assert!(!ClipChainId(0).is_invalid());
    }
}
