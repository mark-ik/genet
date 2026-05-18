/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Parley-backed text measurement for Taffy's measure_function hook.
//!
//! ## Scope (v1, 2026-05-18)
//!
//! Provides [`TextMeasureCtx`] (parley's `FontContext` + `LayoutContext`
//! bundled — created once per layout pass, threaded through every
//! measure call) and [`TextLeaf`] (per-text-node Taffy node context
//! carrying the text content + font properties needed to lay it out).
//!
//! [`measure_text_leaf`] builds a parley `Layout`, runs
//! `break_all_lines` against the available width, and returns the
//! measured `(width, height)` for Taffy to use as the leaf's natural
//! size.
//!
//! Cascade integration is deliberately minimal here: the only style
//! input is `font_size` (defaulted to 16 px) and the default font
//! family resolved by fontique. Real `ComputedValues`-driven text
//! styling (font-family, font-weight, line-height, letter-spacing,
//! etc.) arrives once the cascade applies real CSS rules to text
//! nodes — `TextLeaf` is the seam where that data lands.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md` —
//! parley wiring is step (2) in the roadmap.

use parley::{Alignment, AlignmentOptions, FontContext, Layout, LayoutContext, StyleProperty};
use rustc_hash::FxHashMap;
use taffy::geometry::Size;
use taffy::style::AvailableSpace;

/// Per-text-node context carried on Taffy leaves. Created in
/// [`crate::construct`] when walking text DOM nodes; consumed by the
/// measure function during [`taffy::TaffyTree::compute_layout_with_measure`].
#[derive(Clone, Debug)]
pub struct TextLeaf {
    /// The text content. Owned because the Taffy tree outlives any
    /// borrow into the DOM (Taffy moves the context in via
    /// `new_leaf_with_context`).
    pub text: String,
    /// Cascaded font size in CSS pixels. Defaults to 16.0 in the probe;
    /// real cascade integration replaces with `font.size` from
    /// `ComputedValues`.
    pub font_size: f32,
}

impl TextLeaf {
    /// Build a `TextLeaf` with default font size (16 px). Used when
    /// no cascade has applied a `font-size` to the text's parent.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font_size: 16.0,
        }
    }

    /// Build with an explicit font size.
    pub fn with_font_size(text: impl Into<String>, font_size: f32) -> Self {
        Self {
            text: text.into(),
            font_size,
        }
    }
}

/// Bundled parley contexts used by every measure call during one layout
/// pass. Holds the font database + scratch space + cached `Layout`s
/// keyed by `taffy::NodeId`. Build once per layout, thread through the
/// measure closure, then hand to paint emission so it can extract
/// positioned glyphs without re-shaping.
///
/// `FontContext::new()` discovers system fonts (parley's `system`
/// feature, enabled by default). Per the user's testing-hardware
/// memory: Windows / macOS / Linux all surface a `sans-serif` family
/// via fontique's default registry.
pub struct TextMeasureCtx {
    pub font_ctx: FontContext,
    pub layout_ctx: LayoutContext<()>,
    /// Cached `parley::Layout` per Taffy text leaf — populated by
    /// [`measure_text_leaf`] after each measure call. Paint emission
    /// reads from here via `ConstructedTree::node_map`
    /// (DOM `NodeId` → `taffy::NodeId` → cached `Layout`) to extract
    /// positioned glyphs without re-shaping.
    pub layouts: FxHashMap<taffy::NodeId, Layout<()>>,
}

impl Default for TextMeasureCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl TextMeasureCtx {
    pub fn new() -> Self {
        Self {
            font_ctx: FontContext::new(),
            layout_ctx: LayoutContext::new(),
            layouts: FxHashMap::default(),
        }
    }
}

impl std::fmt::Debug for TextMeasureCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextMeasureCtx").finish_non_exhaustive()
    }
}

/// Measure a text leaf against Taffy's known + available constraints
/// and cache the built `Layout` keyed by `taffy_id` so paint emission
/// can extract positioned glyphs without re-shaping.
///
/// Returns the natural `(width, height)` of the laid-out text:
/// - `known_dimensions` overrides any axis explicitly set by the
///   caller (e.g., flex item with `flex-basis`).
/// - Otherwise, the width comes from parley's break-all-lines using
///   the available space as `max_advance` (or no max for
///   `MinContent`/`MaxContent`).
/// - Height is parley's `layout.height()` after break.
///
/// The semantic intent matches Blitz's text-leaf measure
/// (`blitz-dom/src/inline_content.rs`): for `MinContent` we measure
/// at the smallest reasonable max-advance (longest unbreakable run),
/// and for `MaxContent` we measure with no wrap.
pub fn measure_text_leaf(
    ctx: &mut TextMeasureCtx,
    leaf: &TextLeaf,
    taffy_id: taffy::NodeId,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> Size<f32> {
    // Short-circuit when both axes are explicitly known. (Don't cache
    // — there's no shaped Layout to give back; emit will see no entry
    // and emit an empty glyph run.)
    if let (Some(w), Some(h)) = (known_dimensions.width, known_dimensions.height) {
        return Size { width: w, height: h };
    }

    // Empty text measures as zero-by-(font-size * 1.2) — a sensible
    // baseline that matches what a one-line empty `<span>` would do
    // (line-height defaults to roughly font-size * 1.2 in browsers).
    if leaf.text.is_empty() {
        return Size {
            width: known_dimensions.width.unwrap_or(0.0),
            height: known_dimensions.height.unwrap_or(leaf.font_size * 1.2),
        };
    }

    // Translate Taffy's available_space into parley's max_advance.
    // - Definite(w) → wrap at w
    // - MinContent → wrap as tight as possible (longest word)
    // - MaxContent → no wrap (single long line)
    let max_advance: Option<f32> = match available_space.width {
        AvailableSpace::Definite(w) => Some(w),
        AvailableSpace::MinContent => Some(0.0),
        AvailableSpace::MaxContent => None,
    };

    let mut builder = ctx
        .layout_ctx
        .ranged_builder(&mut ctx.font_ctx, leaf.text.as_str(), 1.0, true);
    builder.push_default(StyleProperty::FontSize(leaf.font_size));

    let mut layout: Layout<()> = builder.build(leaf.text.as_str());
    layout.break_all_lines(max_advance);
    layout.align(Alignment::Start, AlignmentOptions::default());

    let size = Size {
        width: known_dimensions.width.unwrap_or_else(|| layout.width()),
        height: known_dimensions.height.unwrap_or_else(|| layout.height()),
    };

    // Cache the shaped Layout for paint emission.
    ctx.layouts.insert(taffy_id, layout);
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_taffy_id() -> taffy::NodeId {
        // taffy::NodeId is From<u64> in recent versions; use a fixed id
        // — tests don't actually run a Taffy layout, just exercise the
        // measure function directly.
        taffy::NodeId::from(0u64)
    }

    #[test]
    fn empty_text_measures_as_one_line_baseline() {
        let mut ctx = TextMeasureCtx::new();
        let leaf = TextLeaf::new("");
        let size = measure_text_leaf(
            &mut ctx,
            &leaf,
            fake_taffy_id(),
            Size { width: None, height: None },
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        assert_eq!(size.width, 0.0);
        // 16 * 1.2 = 19.2
        assert!((size.height - 19.2).abs() < 0.01);
        // Empty text doesn't shape a Layout — nothing in the cache.
        assert!(ctx.layouts.is_empty());
    }

    #[test]
    fn nonempty_text_measures_positive_width_and_caches_layout() {
        let mut ctx = TextMeasureCtx::new();
        let leaf = TextLeaf::new("Hello, world!");
        let taffy_id = fake_taffy_id();
        let size = measure_text_leaf(
            &mut ctx,
            &leaf,
            taffy_id,
            Size { width: None, height: None },
            Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        );
        assert!(
            size.width > 0.0,
            "expected positive width for non-empty text, got {}",
            size.width
        );
        assert!(
            size.height > 0.0,
            "expected positive height, got {}",
            size.height
        );
        // Cache should hold the shaped Layout.
        let cached = ctx.layouts.get(&taffy_id).expect("layout cached");
        assert!(cached.width() > 0.0);
    }

    #[test]
    fn known_dimensions_override_measurement() {
        let mut ctx = TextMeasureCtx::new();
        let leaf = TextLeaf::new("ignored");
        let size = measure_text_leaf(
            &mut ctx,
            &leaf,
            fake_taffy_id(),
            Size { width: Some(100.0), height: Some(50.0) },
            Size {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            },
        );
        assert_eq!(size.width, 100.0);
        assert_eq!(size.height, 50.0);
    }
}
