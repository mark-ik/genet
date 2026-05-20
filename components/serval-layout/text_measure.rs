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

use std::borrow::Cow;

use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, GenericFamily,
    Layout, LayoutContext, StyleProperty,
};
use rustc_hash::FxHashMap;
use taffy::geometry::Size;
use taffy::style::AvailableSpace;

/// CSS generic font family. Serval-local mirror of the subset of
/// Stylo's `GenericFontFamily` we map to parley — keeps the leaf
/// context decoupled from both Stylo and parley enums (the conversion
/// to parley's `GenericFamily` happens at measure time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenericFamilyKind {
    Serif,
    SansSerif,
    Monospace,
    Cursive,
    Fantasy,
}

/// Resolved font-family choice. The cascade in `construct` collapses
/// CSS's family *list* to the first entry for the probe (no
/// fallback-chain walking yet).
#[derive(Clone, Debug)]
pub enum FontFamilySpec {
    /// A CSS generic family (`serif`, `sans-serif`, …).
    Generic(GenericFamilyKind),
    /// A named family (`"Arial"`, `Times New Roman`, …).
    Named(String),
}

impl Default for FontFamilySpec {
    fn default() -> Self {
        Self::Generic(GenericFamilyKind::SansSerif)
    }
}

/// One styled run of text within an inline formatting context — a
/// maximal span sharing one cascaded font (the text of a single inline
/// element / text node). `construct` produces these by walking an
/// inline subtree in document order.
#[derive(Clone, Debug)]
pub struct InlineRun {
    pub text: String,
    pub font_size: f32,
    pub font_family: FontFamilySpec,
    /// CSS numeric font-weight (400 = normal, 700 = bold).
    pub weight: f32,
    /// Italic / oblique.
    pub italic: bool,
}

impl InlineRun {
    /// A run with default typography (16 px sans-serif, normal weight).
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font_size: 16.0,
            font_family: FontFamilySpec::default(),
            weight: 400.0,
            italic: false,
        }
    }
}

/// The inline content of a Taffy leaf — one or more styled runs that
/// parley lays out together (text + inline elements flow on shared
/// lines, wrapping at the container width). A bare text node is a
/// one-run `InlineContent`; a block element establishing an inline
/// formatting context is a multi-run one.
///
/// Created in [`crate::construct`]; consumed by the measure function
/// during `compute_layout_with_measure`.
#[derive(Clone, Debug)]
pub struct InlineContent {
    pub runs: Vec<InlineRun>,
}

impl InlineContent {
    /// Single-run content from one text string with default typography.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            runs: vec![InlineRun::new(text)],
        }
    }

    /// Single-run content with explicit size + family (the common
    /// bare-text-node case).
    pub fn single(text: impl Into<String>, font_size: f32, font_family: FontFamilySpec) -> Self {
        Self {
            runs: vec![InlineRun {
                text: text.into(),
                font_size,
                font_family,
                weight: 400.0,
                italic: false,
            }],
        }
    }

    /// Total text length in bytes across all runs.
    fn total_len(&self) -> usize {
        self.runs.iter().map(|r| r.text.len()).sum()
    }

    /// The font-size of the first run, for baseline sizing of empty
    /// content. 16 px when there are no runs.
    fn first_font_size(&self) -> f32 {
        self.runs.first().map(|r| r.font_size).unwrap_or(16.0)
    }
}

/// Map a serval [`GenericFamilyKind`] to parley's `GenericFamily`.
fn to_parley_generic(kind: GenericFamilyKind) -> GenericFamily {
    match kind {
        GenericFamilyKind::Serif => GenericFamily::Serif,
        GenericFamilyKind::SansSerif => GenericFamily::SansSerif,
        GenericFamilyKind::Monospace => GenericFamily::Monospace,
        GenericFamilyKind::Cursive => GenericFamily::Cursive,
        GenericFamilyKind::Fantasy => GenericFamily::Fantasy,
    }
}

/// The parley font-family `StyleProperty` for a run's family spec.
fn family_property(spec: &FontFamilySpec) -> StyleProperty<'_, ()> {
    match spec {
        FontFamilySpec::Generic(kind) => to_parley_generic(*kind).into(),
        FontFamilySpec::Named(name) => {
            StyleProperty::FontFamily(FontFamily::Source(Cow::Borrowed(name.as_str())))
        },
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

/// Measure a leaf's inline content against Taffy's known + available
/// constraints and cache the built `Layout` keyed by `taffy_id` so
/// paint emission can extract positioned glyphs without re-shaping.
///
/// All runs lay out together in one parley `Layout`: their texts are
/// concatenated and each run's cascaded font (size / family / weight /
/// style) is pushed as a `StyleProperty` span over its byte range.
/// So `<p>Hello <b>world</b></p>` flows on one line with `world` bold,
/// wrapping at the container width.
///
/// Returns the natural `(width, height)`:
/// - `known_dimensions` overrides any axis explicitly set by the
///   caller (e.g., a flex item with `flex-basis`).
/// - Otherwise width comes from parley's break-all-lines using the
///   available space as `max_advance` (no max for
///   `MinContent`/`MaxContent`); height is `layout.height()`.
pub fn measure_inline_content(
    ctx: &mut TextMeasureCtx,
    content: &InlineContent,
    taffy_id: taffy::NodeId,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> Size<f32> {
    // Short-circuit when both axes are explicitly known. (Don't cache
    // — there's no shaped Layout to give back; emit sees no entry.)
    if let (Some(w), Some(h)) = (known_dimensions.width, known_dimensions.height) {
        return Size { width: w, height: h };
    }

    // Concatenate run texts, tracking each run's byte range for the
    // per-run style spans.
    let mut text = String::with_capacity(content.total_len());
    let mut ranges: Vec<(std::ops::Range<usize>, &InlineRun)> = Vec::new();
    for run in &content.runs {
        let start = text.len();
        text.push_str(&run.text);
        ranges.push((start..text.len(), run));
    }

    // Empty content measures as zero-by-(font-size * 1.2) — a one-line
    // baseline (line-height ≈ font-size * 1.2 in browsers).
    if text.is_empty() {
        return Size {
            width: known_dimensions.width.unwrap_or(0.0),
            height: known_dimensions
                .height
                .unwrap_or(content.first_font_size() * 1.2),
        };
    }

    // Translate Taffy's available_space into parley's max_advance.
    let max_advance: Option<f32> = match available_space.width {
        AvailableSpace::Definite(w) => Some(w),
        AvailableSpace::MinContent => Some(0.0),
        AvailableSpace::MaxContent => None,
    };

    let mut builder = ctx
        .layout_ctx
        .ranged_builder(&mut ctx.font_ctx, text.as_str(), 1.0, true);
    // Defaults from the first run; per-run spans override below.
    if let Some(first) = content.runs.first() {
        builder.push_default(StyleProperty::FontSize(first.font_size));
        builder.push_default(family_property(&first.font_family));
    }
    for (range, run) in &ranges {
        builder.push(StyleProperty::FontSize(run.font_size), range.clone());
        builder.push(family_property(&run.font_family), range.clone());
        builder.push(
            StyleProperty::FontWeight(FontWeight::new(run.weight)),
            range.clone(),
        );
        let style = if run.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        builder.push(StyleProperty::FontStyle(style), range.clone());
    }

    let mut layout: Layout<()> = builder.build(text.as_str());
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
        let content = InlineContent::new("");
        let size = measure_inline_content(
            &mut ctx,
            &content,
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
        let content = InlineContent::new("Hello, world!");
        let taffy_id = fake_taffy_id();
        let size = measure_inline_content(
            &mut ctx,
            &content,
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
        let content = InlineContent::new("ignored");
        let size = measure_inline_content(
            &mut ctx,
            &content,
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

    #[test]
    fn multi_run_content_is_wider_than_either_run_alone() {
        // Two runs concatenate into one line; combined width exceeds
        // each run's own width (sanity that runs lay out together).
        let mut ctx = TextMeasureCtx::new();
        let combined = InlineContent {
            runs: vec![InlineRun::new("Hello "), InlineRun::new("world")],
        };
        let just_hello = InlineContent::new("Hello ");
        let avail = Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::MaxContent,
        };
        let none = Size { width: None, height: None };
        let combined_w = measure_inline_content(
            &mut ctx,
            &combined,
            taffy::NodeId::from(1u64),
            none,
            avail,
        )
        .width;
        let hello_w =
            measure_inline_content(&mut ctx, &just_hello, taffy::NodeId::from(2u64), none, avail)
                .width;
        assert!(
            combined_w > hello_w,
            "combined run width {combined_w} should exceed 'Hello ' alone {hello_w}"
        );
    }
}
