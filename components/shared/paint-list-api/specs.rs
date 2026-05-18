/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Compositor-primitive payloads: clip / transform / layer / shadow
//! specs and the filter-op enum carried on `LayerSpec`.

use malloc_size_of_derive::MallocSizeOf;
use paint_types::units::{LayoutPoint, LayoutRect, LayoutTransform, LayoutVector2D};
use paint_types::{BorderRadius, ColorF, MixBlendMode};
use serde::{Deserialize, Serialize};

use crate::items::PathData;

// =============================================================================
// ClipSpec — what PushClip carries
// =============================================================================

/// Pushed onto the compositor's clip stack. NetRender intersects this
/// with the active clip when rasterizing subsequent primitives.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ClipSpec {
    pub kind: ClipKind,
}

#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum ClipKind {
    /// Sharp-edged rectangular clip.
    Rect(LayoutRect),
    /// Rounded-rect clip with per-corner radii. `clip_out` inverts to
    /// "clip out the rounded region" (CSS `clip-path: inset(...)`-style
    /// outer mask).
    RoundedRect {
        rect: LayoutRect,
        radius: BorderRadius,
        clip_out: bool,
    },
    /// Arbitrary path clip. NetRender lowers via vello's kurbo path
    /// machinery (R3-cleared).
    Path(PathData),
}

// =============================================================================
// TransformSpec — what PushTransform carries
// =============================================================================

/// PM-3 rename: was `PushReferenceFrame` in PM-2. Pushes a coordinate
/// space onto the transform stack; subsequent primitives are rendered
/// under the resulting composition.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct TransformSpec {
    /// Origin of the new coordinate space, in the parent frame.
    pub origin: LayoutPoint,
    /// Local-to-parent transform applied at `origin`.
    pub transform: LayoutTransform,
    /// 3D-context behavior on this frame.
    pub kind: TransformKind,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize,
)]
pub enum TransformKind {
    /// 2D transform. Most common case.
    #[default]
    Standard,
    /// CSS `transform-style: preserve-3d` — children participate in
    /// the parent's 3D rendering context.
    Preserve3D,
    /// Perspective transform (e.g. CSS `perspective`). Children are
    /// rendered with this perspective applied.
    Perspective,
}

// =============================================================================
// LayerSpec — what PushLayer carries
// =============================================================================

/// Stacking layer. Carries everything that needs the compositor to
/// allocate an intermediate buffer: opacity, blend mode, filter chain,
/// raster-space hint, mask spec.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct LayerSpec {
    /// Origin of the layer in the parent coordinate space.
    pub origin: LayoutPoint,
    /// Layer opacity. 1.0 = opaque (default).
    pub opacity: f32,
    /// Blend mode the layer composes back into its parent with.
    pub mix_blend_mode: MixBlendMode,
    /// Filter chain applied to the layer's rasterized output. Empty
    /// vec means no filters. PM-3: filters are common renderer
    /// capability (D1-cleared backdrop machinery in netrender).
    pub filters: Vec<FilterOp>,
    /// Raster-space hint. Screen-space rasterization is the default
    /// (most browser content); local-space is the SVG-style choice
    /// where the rasterization tracks the layer's transform.
    pub raster_space: RasterSpace,
    /// Layer-level flags (blend container, backdrop root, etc).
    pub flags: LayerFlags,
    /// Optional alpha-mask layer. When set, the layer is composited
    /// using DestIn against this mask (Roadmap C3).
    pub mask: Option<LayerMask>,
}

impl Default for LayerSpec {
    fn default() -> Self {
        Self {
            origin: LayoutPoint::new(0.0, 0.0),
            opacity: 1.0,
            mix_blend_mode: MixBlendMode::Normal,
            filters: Vec::new(),
            raster_space: RasterSpace::default(),
            flags: LayerFlags::empty(),
            mask: None,
        }
    }
}

/// Alpha-mask carried on a layer. `mask_clip_id` references a clip
/// previously pushed; the layer's raster is composited under DestIn
/// against the masked region.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct LayerMask {
    pub mask_rect: LayoutRect,
    /// Image atlas key for image-mask cases. None means alpha-only
    /// (use a child layer's content as the mask via the standard
    /// outer-then-inner pattern).
    pub image_mask: Option<paint_types::ImageKey>,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize,
)]
pub enum RasterSpace {
    #[default]
    Screen,
    Local,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct LayerFlags(pub u32);

impl LayerFlags {
    /// The layer establishes a blend container — children's blend
    /// modes apply against the layer's backdrop, not the parent's.
    pub const BLEND_CONTAINER: Self = Self(1 << 0);
    /// The layer is a backdrop root for child `backdrop-filter` ops.
    pub const BACKDROP_ROOT: Self = Self(1 << 1);
    /// The layer participates in a scroll-linked effect (sticky etc).
    pub const HAS_SCROLL_LINKED_EFFECT: Self = Self(1 << 2);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for LayerFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for LayerFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// =============================================================================
// FilterOp — SVG/CSS filter primitives in LayerSpec.filters
// =============================================================================

/// One primitive in a layer's filter chain. Applied in order during
/// compositing. PM-3: filters are common renderer capability, not a
/// per-engine extension — netrender's Roadmap D1 already ships
/// `SceneFilter::Blur` + per-layer backdrop machinery.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum FilterOp {
    /// Gaussian blur with the given radius in CSS pixels.
    Blur(f32),
    Brightness(f32),
    Contrast(f32),
    Grayscale(f32),
    HueRotate(f32),
    Invert(f32),
    /// Multiplicative alpha. Subsumes the older "alpha multiplier on
    /// every item" pattern.
    Opacity(f32),
    Saturate(f32),
    Sepia(f32),
    /// CSS `filter: drop-shadow(...)` — filter-chain shadow, distinct
    /// from `DrawShadow`'s box-shadow primitive.
    DropShadow {
        offset: LayoutVector2D,
        color: ColorF,
        blur_radius: f32,
    },
    /// Affine color-transform matrix (4×5 = 20 values, row-major).
    ColorMatrix([f32; 20]),
}

// =============================================================================
// ShadowSpec — what PushShadow carries
// =============================================================================

/// Text-shadow style pushed onto the shadow stack. Subsequent paint
/// items (text, rects, etc.) render with this shadow until a matching
/// `PaintCmd::PopAllShadows`.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct ShadowSpec {
    pub offset: LayoutVector2D,
    pub color: ColorF,
    pub blur_radius: f32,
}
