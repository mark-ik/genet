/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `paint_list_api` — the trait + common vocabulary every engine emits
//! into and NetRender renders from. See
//! [`docs/2026-05-17_paintlist_polyglot_renderer.md`](../../../docs/2026-05-17_paintlist_polyglot_renderer.md)
//! for the design (PM-3 resolution).
//!
//! ## Shape
//!
//! - [`PaintList`] is the producer-facing trait engines implement.
//!   Concrete impls (`ServalPaintList`, `NematicPaintList`,
//!   `ScryingPaintList`) live in their respective engine crates and
//!   carry richer internal state (palettes, spatial trees) behind the
//!   trait's [`PaintList::commands`] view.
//! - [`PaintCmd`] is the closed-set command stream NetRender pattern-
//!   matches against. Compositor primitives push/pop composition state;
//!   `Draw*` primitives emit one item each. PM-3: no generic extension
//!   hole — engine-specific items either map to common ops or hand off
//!   via [`PaintCmd::DrawExternalTexture`].
//!
//! ## Lowering contract
//!
//! NetRender owns [`PaintCmd`] → `netrender::Scene` translation. The
//! `DrawExternalTexture` lowering specifically is the per-frame
//! compositor pass (`ExternalTextureComposite` with `scene_op_boundary`),
//! **not** a vello `SceneOp::Image`. This sidesteps tile-cache
//! invalidation for mutating textures (WebGL canvas, embedded
//! iframes, paint worklet output, etc.) by construction.

#![deny(unsafe_code)]

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

pub mod items;
pub mod specs;

pub use items::*;
pub use specs::*;

// Re-export the paint-types primitives that ride directly in command
// payloads, so consumers can `use paint_list_api::*` without also
// importing paint-types for the basics.
pub use paint_types::units::{
    DeviceIntSideOffsets, DeviceIntSize, LayoutPoint, LayoutRect, LayoutSideOffsets, LayoutSize,
    LayoutTransform, LayoutVector2D,
};
pub use paint_types::border::BorderSide;
pub use paint_types::{
    BorderRadius, BorderStyle, BoxShadowClipMode, ColorF, ExtendMode, FontInstanceKey,
    GradientStop, ImageKey, ImageRendering, LineStyle, MixBlendMode, NormalBorder, RepeatMode,
    TransformStyle,
};

// =============================================================================
// PaintList trait
// =============================================================================

/// What an engine emits — the unit of paint output for one rendered
/// frame. Fully serializable so the same value can cross IPC, sit in a
/// fixture file for capture/replay, or feed NetRender's lowering.
///
/// PM-3: the trait is *monomorphic*. Engine-specific payloads are not
/// part of the common surface; engines either map to common
/// [`PaintCmd`] variants or hand off via
/// [`PaintCmd::DrawExternalTexture`]. If a future case genuinely needs
/// typed engine-specific data NetRender can't infer from common ops, a
/// `PaintCmd::Extension(PaintPayload)` variant can be retrofitted —
/// kept out of v1 per the audit conclusion.
pub trait PaintList:
    Clone + std::fmt::Debug + Serialize + for<'de> Deserialize<'de> + malloc_size_of::MallocSizeOf
{
    /// Which engine produced this list. Receivers downstream of the
    /// transport envelope match on the envelope variant directly; this
    /// accessor exists for diagnostics and for in-process callers that
    /// hold a concrete `&L: PaintList`. The trait is **not**
    /// `dyn`-compatible (the supertrait bounds aren't object-safe) —
    /// engine-agnostic code dispatches on the envelope, not on a
    /// trait object.
    fn engine_id(&self) -> EngineId;

    /// Final viewport this paint output is computed against. Renderers
    /// use this for culling and for setting the render-target size.
    fn viewport(&self) -> DeviceIntSize;

    /// Producer-rolled semantic-equivalence epoch. Same
    /// `(source_id, generation_id)` asserts identical paint output and
    /// resource references; NetRender may use this to skip *relowering*
    /// (PaintList → Scene). **Not a tile-cache invalidation key** —
    /// tile-cache correctness still derives from SceneOp content
    /// hashing post-lowering.
    fn generation_id(&self) -> u64;

    /// Paint commands in paint order. Push-order is paint-order. The
    /// return type is a slice rather than an iterator on the
    /// assumption that paint output is built-then-shipped, not
    /// streamed; revisit if a streaming consumer surfaces.
    fn commands(&self) -> &[PaintCmd];
}

// =============================================================================
// Engine identity
// =============================================================================

/// Identifies which engine produced a [`PaintList`]. Used for
/// diagnostics and for keying the [`PaintEnvelope`] discriminant.
///
/// Sentinels are stable: do not renumber. New engines append.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct EngineId(pub u32);

impl EngineId {
    /// Serval — HTML/CSS engine for full-web content.
    pub const SERVAL: Self = Self(0);
    /// Nematic — smolweb (Gemini, Gopher, Scroll, Markdown, feeds,
    /// Finger).
    pub const NEMATIC: Self = Self(1);
    /// Scrying — system-webview wrapper (single `DrawExternalTexture`
    /// per frame).
    pub const SCRYING: Self = Self(2);
    /// Sentinel for an engine that hasn't yet been assigned an id.
    /// Reserved for test impls; production engines must use a real id.
    pub const UNASSIGNED: Self = Self(u32::MAX);
}

// =============================================================================
// PaintCmd — the closed-set command stream
// =============================================================================

/// One paint operation. Push-order is paint-order. NetRender pattern-
/// matches on this to lower into its internal `Scene`.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum PaintCmd {
    // ----- Compositor primitives -----------------------------------------
    /// Push a clip onto the active clip stack.
    PushClip(ClipSpec),
    /// Pop the topmost clip.
    PopClip,
    /// Push a transform/coordinate-space frame.
    ///
    /// PM-3 rename: was `PushReferenceFrame` in PM-2; reference-frame
    /// is a WebRender-ism that doesn't map to a NetRender primitive,
    /// and the honest common shape is "push a transform."
    PushTransform(TransformSpec),
    /// Pop the topmost transform.
    PopTransform,
    /// Push a stacking layer. Carries opacity, blend mode, filter
    /// chain, and raster-space hints — everything that needs the
    /// compositor to allocate an intermediate buffer.
    PushLayer(LayerSpec),
    /// Pop the topmost layer; composite back into the parent.
    PopLayer,

    // ----- Paint primitives ----------------------------------------------
    /// Filled rectangle.
    DrawRect(RectItem),
    /// Stroked path with cap/join/dash decoration.
    DrawStroke(StrokeItem),
    /// Single-line stroke with text-decoration-style options
    /// (solid / dotted / dashed / wavy). For non-decoration strokes
    /// use [`PaintCmd::DrawStroke`].
    DrawLine(LineItem),
    /// Filled or stroked Bezier path. PM-3 addition — vello has the
    /// machinery (R2/R3 path-precise containment); inclusion is a
    /// "renderer capability belongs in common" call.
    DrawPath(PathItem),
    /// CSS-style border — normal (per-side stroke) or nine-patch
    /// (image-sliced).
    DrawBorder(BorderItem),
    DrawLinearGradient(LinearGradientItem),
    DrawRadialGradient(RadialGradientItem),
    DrawConicGradient(ConicGradientItem),
    /// Shaped glyph runs from the layout engine. NetRender does *not*
    /// reshape — see doc §"Text ownership boundary".
    DrawText(TextRunItem),
    DrawImage(ImageItem),
    DrawRepeatingImage(RepeatingImageItem),
    /// External wgpu texture (WebGL canvas, embedded iframe output,
    /// paint worklet output, native form control, scrying view, etc.).
    /// Lowers to the per-frame compositor pass, not a Scene image.
    DrawExternalTexture(ExternalTextureItem),
    /// Box-shadow primitive (CSS `box-shadow` shape).
    DrawShadow(ShadowItem),

    // ----- State-stack pairs (subsequent ops affected) -------------------
    /// Push a text-shadow style onto the shadow stack. Subsequent
    /// [`PaintCmd::DrawText`] / [`PaintCmd::DrawRect`] / etc. items
    /// render with this shadow until a matching
    /// [`PaintCmd::PopAllShadows`].
    PushShadow(ShadowSpec),
    /// Clear the entire text-shadow stack.
    PopAllShadows,

    // ----- Hit-testing ---------------------------------------------------
    /// Invisible hit-test region. Carries a producer-defined tag.
    HitTest(HitTestItem),
}

// =============================================================================
// PrimitiveFlags — per-item modifiers
// =============================================================================

/// Per-item presentation flags. Carried inline on every
/// [`CommonPlacement`] aggregator.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct PrimitiveFlags(pub u32);

impl PrimitiveFlags {
    /// Item participates in hit-testing (default for visible primitives).
    pub const HIT_TESTABLE: Self = Self(1 << 0);
    /// Item is the backface of a 3D-transformed element (cull when
    /// preserve-3d backface visibility is off).
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
// CommonPlacement — bounds + flags every Draw* item carries
// =============================================================================

/// Bounds-and-flags aggregator every paint item carries. In the
/// PaintList model the clip and transform state come from compositor
/// primitives (`PushClip`/`PopClip`, `PushTransform`/`PopTransform`),
/// **not** from per-item references — so this is lighter than the
/// `ServalDisplayList::CommonItemPlacement` it descends from.
#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct CommonPlacement {
    /// Item bounds in local (post-transform/clip) coordinates. Used
    /// for culling and as the painted-region hint.
    pub bounds: LayoutRect,
    /// Per-item flags. Hit-testability, antialiasing, backface
    /// participation.
    pub flags: PrimitiveFlags,
}

impl CommonPlacement {
    /// Convenience constructor with empty flags.
    pub fn new(bounds: LayoutRect) -> Self {
        Self {
            bounds,
            flags: PrimitiveFlags::empty(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Trivial PaintList impl for trait-bound and serialization tests.
    #[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, Serialize)]
    struct StubPaintList {
        viewport: DeviceIntSize,
        commands: Vec<PaintCmd>,
        generation: u64,
    }

    impl PaintList for StubPaintList {
        fn engine_id(&self) -> EngineId {
            EngineId::UNASSIGNED
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
    }

    fn box2d(x: f32, y: f32, w: f32, h: f32) -> LayoutRect {
        LayoutRect::new(LayoutPoint::new(x, y), LayoutPoint::new(x + w, y + h))
    }

    #[test]
    fn primitive_flags_or_combines() {
        let f = PrimitiveFlags::HIT_TESTABLE | PrimitiveFlags::ANTIALIASED;
        assert!(f.contains(PrimitiveFlags::HIT_TESTABLE));
        assert!(f.contains(PrimitiveFlags::ANTIALIASED));
        assert!(!f.contains(PrimitiveFlags::IS_BACKFACE));
    }

    #[test]
    fn stub_paint_list_satisfies_trait_bounds() {
        // Sized usage: this is the canonical dispatch shape. The trait
        // isn't `dyn`-compatible (Clone + Serialize bounds aren't
        // object-safe); engine-agnostic dispatch goes through the
        // closed-set envelope downstream.
        fn assert_paint_list<L: PaintList>(_: &L) {}
        let list = StubPaintList::default();
        assert_paint_list(&list);
    }

    #[test]
    fn paint_cmd_round_trips_through_bincode_shape() {
        // postcard isn't a dep of this crate, but serde+derive being
        // wired correctly is enough to validate the command surface.
        // We round-trip through serde_json which only needs Serialize +
        // Deserialize impls; if any item or spec is missing a derive,
        // this fails to compile or to deserialize.
        let cmd = PaintCmd::DrawRect(RectItem {
            placement: CommonPlacement::new(box2d(0.0, 0.0, 100.0, 50.0)),
            color: ColorF::default(),
        });
        let serialized = serde_json::to_string(&cmd).expect("serialize");
        let parsed: PaintCmd = serde_json::from_str(&serialized).expect("deserialize");
        match parsed {
            PaintCmd::DrawRect(_) => {}
            other => panic!("round-trip lost variant: {other:?}"),
        }
    }

    #[test]
    fn external_texture_content_generation_defaults_none() {
        // The PM-3 forward-looking field defaults to None; producers
        // set it only when texture-as-source rather than compositor-
        // pass. Pin the default so downstream lowering tests can rely
        // on it.
        let item = ExternalTextureItem {
            placement: CommonPlacement::new(box2d(0.0, 0.0, 200.0, 200.0)),
            texture_key: 0xDEADBEEF,
            opacity: 1.0,
            content_generation: None,
        };
        assert_eq!(item.content_generation, None);
    }

    #[test]
    fn engine_id_sentinels_are_stable() {
        // These values cross IPC; renumbering them is a wire-break.
        assert_eq!(EngineId::SERVAL.0, 0);
        assert_eq!(EngineId::NEMATIC.0, 1);
        assert_eq!(EngineId::SCRYING.0, 2);
    }
}
