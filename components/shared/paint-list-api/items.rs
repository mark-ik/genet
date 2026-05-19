/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Paint-primitive payloads: the `*Item` types each `PaintCmd::Draw*`
//! variant carries, plus supporting types (`PathData`, `BorderDetails`,
//! gradient payloads, `GlyphInstance`, etc.).

use malloc_size_of_derive::MallocSizeOf;
use paint_types::units::{
    DeviceIntSideOffsets, LayoutPoint, LayoutRect, LayoutSideOffsets, LayoutSize, LayoutVector2D,
};
use paint_types::{
    BorderRadius, BoxShadowClipMode, ColorF, ExtendMode, FontInstanceKey, GradientStop, ImageKey,
    ImageRendering, LineStyle, NormalBorder, RepeatMode,
};
use serde::{Deserialize, Serialize};

use crate::CommonPlacement;

// =============================================================================
// Fills — DrawRect / DrawStroke
// =============================================================================

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RectItem {
    pub placement: CommonPlacement,
    pub color: ColorF,
}

/// Stroked path with cap / join / dash decoration. Use for paths that
/// need full stroke styling; for simple text-decoration-style single
/// lines use [`LineItem`].
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct StrokeItem {
    pub placement: CommonPlacement,
    pub path: PathData,
    pub color: ColorF,
    pub width: f32,
    pub cap: StrokeCap,
    pub join: StrokeJoin,
    pub dash: Option<DashPattern>,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize,
)]
pub enum StrokeCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize,
)]
pub enum StrokeJoin {
    Bevel,
    #[default]
    Miter,
    Round,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct DashPattern {
    /// Dash + gap lengths alternating, in CSS pixels. e.g.
    /// `[5.0, 3.0]` = "5px dash, 3px gap, repeat."
    pub intervals: Vec<f32>,
    /// Phase offset into `intervals`.
    pub offset: f32,
}

// =============================================================================
// Line — text-decoration-shaped single-line stroke
// =============================================================================

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct LineItem {
    pub placement: CommonPlacement,
    pub color: ColorF,
    pub style: LineStyle,
    pub orientation: LineOrientation,
    /// Wavy stroke thickness for `LineStyle::Wavy`; ignored otherwise.
    pub wavy_thickness: f32,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize,
)]
pub enum LineOrientation {
    #[default]
    Horizontal,
    Vertical,
}

// =============================================================================
// Path — PM-3 addition
// =============================================================================

/// Filled / stroked Bezier path. PM-3: common because netrender has the
/// machinery already (`SceneOp::Shape`, R2/R3 path-precise containment).
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct PathItem {
    pub placement: CommonPlacement,
    pub path: PathData,
    pub fill: Option<ColorF>,
    pub stroke: Option<StrokeStyle>,
}

/// Stroke style for [`PathItem`]; subset of [`StrokeItem`]'s fields.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct StrokeStyle {
    pub color: ColorF,
    pub width: f32,
    pub cap: StrokeCap,
    pub join: StrokeJoin,
    pub dash: Option<DashPattern>,
}

/// Serializable Bezier path data. NetRender's lowering reconstructs a
/// `kurbo::BezPath` from this command sequence.
#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, Serialize)]
pub struct PathData {
    pub commands: Vec<PathCommand>,
}

#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub enum PathCommand {
    MoveTo(LayoutPoint),
    LineTo(LayoutPoint),
    QuadTo {
        control: LayoutPoint,
        to: LayoutPoint,
    },
    CurveTo {
        control1: LayoutPoint,
        control2: LayoutPoint,
        to: LayoutPoint,
    },
    Close,
}

// =============================================================================
// Border — normal + nine-patch
// =============================================================================

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct BorderItem {
    pub placement: CommonPlacement,
    pub widths: LayoutSideOffsets,
    pub details: BorderDetails,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum BorderDetails {
    /// Per-side color + style + corner radii. Reuses paint-types
    /// `NormalBorder`.
    Normal(NormalBorder),
    /// Image-sliced nine-patch border.
    NinePatch(NinePatchBorder),
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct NinePatchBorder {
    pub source: NinePatchSource,
    pub width: i32,
    pub height: i32,
    pub slice: DeviceIntSideOffsets,
    pub fill: bool,
    pub repeat_horizontal: RepeatMode,
    pub repeat_vertical: RepeatMode,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum NinePatchSource {
    Image(ImageKey, ImageRendering),
    LinearGradient(LinearGradientPayload),
    RadialGradient(RadialGradientPayload),
    ConicGradient(ConicGradientPayload),
}

// =============================================================================
// Gradients
// =============================================================================

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct LinearGradientItem {
    pub placement: CommonPlacement,
    pub gradient: LinearGradientPayload,
    /// Tile size for tiled (repeating) gradients. Equal to the
    /// placement bounds size for a single-fill gradient.
    pub tile_size: LayoutSize,
    pub tile_spacing: LayoutSize,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct LinearGradientPayload {
    pub start_point: LayoutPoint,
    pub end_point: LayoutPoint,
    pub extend_mode: ExtendMode,
    pub stops: Vec<GradientStop>,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RadialGradientItem {
    pub placement: CommonPlacement,
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
    pub placement: CommonPlacement,
    pub gradient: ConicGradientPayload,
    pub tile_size: LayoutSize,
    pub tile_spacing: LayoutSize,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ConicGradientPayload {
    pub center: LayoutPoint,
    /// Starting angle in radians. Sweep is always clockwise.
    pub angle: f32,
    pub extend_mode: ExtendMode,
    pub stops: Vec<GradientStop>,
}

// =============================================================================
// Text — shaped glyph runs from layout
// =============================================================================

/// Shaped glyph runs from the layout engine. NetRender does *not*
/// reshape — see doc §"Text ownership boundary".
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct TextRunItem {
    pub placement: CommonPlacement,
    /// Font instance the run was shaped against. The renderer
    /// resolves this (via the `PaintList::fonts` side-table) to a
    /// concrete font in its palette.
    pub font_instance: FontInstanceKey,
    /// Em size the run was shaped at, in CSS pixels. The renderer
    /// needs this explicitly — `font_instance` identifies the face,
    /// not the size.
    pub font_size: f32,
    pub color: ColorF,
    /// Shaped + positioned glyphs from parley.
    pub glyphs: Vec<GlyphInstance>,
    pub options: TextOptions,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct GlyphInstance {
    /// Glyph index into the shaped font.
    pub index: u32,
    /// Baseline-aligned position in the run's local space.
    pub point: LayoutPoint,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct TextOptions {
    pub render_mode: FontRenderMode,
    pub hint_metrics: bool,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize,
)]
pub enum FontRenderMode {
    Mono,
    Alpha,
    #[default]
    Subpixel,
}

// =============================================================================
// Images
// =============================================================================

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ImageItem {
    pub placement: CommonPlacement,
    pub image_key: ImageKey,
    pub image_rendering: ImageRendering,
    pub alpha_type: AlphaType,
    /// Tint color multiplied through the sample. Default is opaque
    /// white (identity).
    pub color: ColorF,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct RepeatingImageItem {
    pub placement: CommonPlacement,
    pub image_key: ImageKey,
    pub stretch_size: LayoutSize,
    pub tile_spacing: LayoutSize,
    pub image_rendering: ImageRendering,
    pub alpha_type: AlphaType,
    pub color: ColorF,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize,
)]
pub enum AlphaType {
    #[default]
    Alpha,
    PremultipliedAlpha,
}

// =============================================================================
// External texture — PM-3 lowering contract lives in the docstring
// =============================================================================

/// Same-device producer texture composited into the frame.
///
/// **PM-3 lowering contract.** NetRender lowers this to the per-frame
/// compositor pass (`ExternalTextureComposite` with `scene_op_boundary`,
/// landed in netrender 2026-05-16), **not** a vello `SceneOp::Image`.
/// This sidesteps tile-cache invalidation for mutating textures (WebGL
/// canvas, embedded iframes, paint worklet output, etc.) by
/// construction: the compositor pass reads the texture view at frame
/// composite time, so producer redraws are picked up without Scene
/// mutation.
///
/// The actual `wgpu::Texture` is registered with NetRender's external-
/// texture registry out-of-band — GPU handles are not IPC payloads.
/// The display list carries only the stable producer-side `texture_key`
/// and placement metadata.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ExternalTextureItem {
    pub placement: CommonPlacement,
    /// Stable producer-side key for the texture. Resolves via
    /// NetRender's external-texture registry.
    pub texture_key: u64,
    pub opacity: f32,
    /// **PM-3 forward-looking field.** `None` for the compositor-pass
    /// lowering (the default); producers set when an external texture
    /// is used as a sampling source for *other* `PaintCmd`s (e.g., a
    /// future repeating-pattern op) where the lowering emits SceneOps
    /// that tile-cache on the texture key. Tile cache then keys on
    /// `(texture_key, content_generation)`; rolling the generation
    /// invalidates the cached output.
    pub content_generation: Option<u64>,
}

// =============================================================================
// Shadow — box-shadow primitive
// =============================================================================

/// Box-shadow primitive (CSS `box-shadow` shape). For state-stack
/// text-shadow use `PaintCmd::PushShadow(ShadowSpec)` instead.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ShadowItem {
    pub placement: CommonPlacement,
    /// Box being shadowed (typically the originating element's
    /// padding-box rect).
    pub box_bounds: LayoutRect,
    pub offset: LayoutVector2D,
    pub color: ColorF,
    pub blur_radius: f32,
    pub spread_radius: f32,
    pub border_radius: BorderRadius,
    pub clip_mode: BoxShadowClipMode,
}

// =============================================================================
// Hit-test region
// =============================================================================

/// Invisible region carrying a producer-defined hit-test tag.
/// Composited into the renderer's hit-test pass; never rasterized.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct HitTestItem {
    pub placement: CommonPlacement,
    /// Producer-defined tag. Returned from hit-tests against this
    /// region. Interpretation is producer-side (e.g., serval encodes
    /// a `(spatial_id, hit_tag)` pair into the high/low halves).
    pub tag: u64,
}
