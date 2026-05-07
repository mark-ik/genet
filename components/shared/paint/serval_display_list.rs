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
    BorderRadius, BorderStyle, ColorF, ExtendMode, ExternalScrollId, FontInstanceKey, GradientStop,
    ImageKey, ImageRendering, MixBlendMode, PipelineId, RepeatMode, TransformStyle,
};
use serde::{Deserialize, Serialize};

// =============================================================================
// Opaque palette indices
// =============================================================================

/// Index into [`ServalDisplayList::spatial_nodes`]. The root scroll
/// frame is always at index 0; pipelines push additional spatial
/// nodes (scroll frames, sticky frames, reference frames) and refer
/// to them by id on subsequent items.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub struct SpatialId(pub u32);

impl SpatialId {
    pub const ROOT: SpatialId = SpatialId(0);

    pub fn is_root(&self) -> bool {
        self.0 == 0
    }
}

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

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct StickyOffsetBounds {
    pub min: f32,
    pub max: f32,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ReferenceFrameDef {
    pub parent: SpatialId,
    pub origin: LayoutPoint,
    pub transform: ReferenceFrameId,
    pub kind: ReferenceFrameKind,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum ReferenceFrameKind {
    #[default]
    Transform,
    Perspective,
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
    pub bounds: LayoutRect,
    pub clip: ClipChainId,
    pub spatial: SpatialId,
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
pub enum LineStyle {
    #[default]
    Solid,
    Dotted,
    Dashed,
    Wavy,
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
    Normal(NormalBorderDetails),
    NinePatch(NinePatchBorderDetails),
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct NormalBorderDetails {
    pub left: BorderSide,
    pub right: BorderSide,
    pub top: BorderSide,
    pub bottom: BorderSide,
    pub radius: BorderRadius,
    pub do_aa: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BorderSide {
    pub color: ColorF,
    pub style: BorderStyle,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct NinePatchBorderDetails {
    pub source: NinePatchBorderSource,
    pub width: i32,
    pub height: i32,
    pub slice: LayoutSideOffsets,
    pub fill: bool,
    pub repeat_horizontal: RepeatMode,
    pub repeat_vertical: RepeatMode,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum NinePatchBorderSource {
    Image(ImageKey),
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

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum BoxShadowClipMode {
    #[default]
    Outset,
    Inset,
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

    /// Register a spatial node; returns the [`SpatialId`].
    pub fn define_spatial_node(&mut self, def: SpatialNodeDef) -> SpatialId {
        let id = SpatialId(self.spatial_nodes.len() as u32);
        self.spatial_nodes.push(def);
        id
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
        let id1 = list.define_clip(ClipDef::Rect(ClipRectDef {
            spatial: SpatialId::ROOT,
            rect: LayoutRect::zero(),
        }));
        let id2 = list.define_clip(ClipDef::Rect(ClipRectDef {
            spatial: SpatialId::ROOT,
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
    fn spatial_id_root_round_trip() {
        assert!(SpatialId::ROOT.is_root());
        assert!(!SpatialId(1).is_root());
    }

    #[test]
    fn clip_chain_id_invalid_round_trip() {
        assert!(ClipChainId::INVALID.is_invalid());
        assert!(!ClipChainId(0).is_invalid());
    }
}
