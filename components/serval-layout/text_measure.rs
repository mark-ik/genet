/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Parley-backed text measurement for Taffy's measure_function hook.
//!
//! Provides [`TextMeasureCtx`] (parley's `FontContext` + `LayoutContext`,
//! created once per layout pass and threaded through every measure call) and
//! [`InlineContent`] (a Taffy leaf's inline content: styled [`InlineRun`]s plus
//! replaced inline boxes such as `<img>`).
//!
//! [`measure_inline_content`] builds a parley `Layout` from the runs + boxes,
//! breaks lines against the available width, and returns the measured
//! `(width, height)` for Taffy to use as the leaf's natural size. The same
//! `Layout` is cached for paint emission (positioned glyph runs + per-run brush
//! color), so measurement and paint agree.
//!
//! Each [`InlineRun`] carries the cascaded text style (`font-size`,
//! `font-family`, `font-weight`, italic, `color`, `text-decoration`
//! underline / line-through, `line-height`); `construct::gather_inline_content`
//! reads it per styling element. Overline and decoration color / style are not
//! yet plumbed.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::borrow::Cow;

use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, GenericFamily,
    InlineBox, InlineBoxKind, Layout, LayoutContext, LineHeight, StyleProperty,
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

/// Glyph fill color carried as the parley layout brush, so each run's
/// cascaded `color` survives shaping into the `Layout` and is read
/// back per-`GlyphRun` at paint time. Straight (non-premultiplied)
/// RGBA in `[0, 1]`, matching `paint_list_api::ColorF`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorBrush(pub [f32; 4]);

impl Default for ColorBrush {
    fn default() -> Self {
        // Opaque black — the CSS initial `color`.
        Self([0.0, 0.0, 0.0, 1.0])
    }
}

/// A run's cascaded `line-height`, mapped to a parley `LineHeight` at shaping
/// time. `Normal` keeps parley's default (the font's natural metrics); the others
/// come from a CSS `<number>` or `<length>`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum LineHeightSpec {
    /// `line-height: normal` — font metrics (parley `MetricsRelative(1.0)`).
    #[default]
    Normal,
    /// `line-height: <number>` — a multiple of the font size.
    Factor(f32),
    /// `line-height: <length>` — an absolute height in CSS px.
    Px(f32),
}

/// One styled run of text within an inline formatting context — a
/// maximal span sharing one cascaded font + color (the text of a
/// single inline element / text node). `construct` produces these by
/// walking an inline subtree in document order.
#[derive(Clone, Debug)]
pub struct InlineRun {
    pub text: String,
    pub font_size: f32,
    pub font_family: FontFamilySpec,
    /// CSS numeric font-weight (400 = normal, 700 = bold).
    pub weight: f32,
    /// Italic / oblique.
    pub italic: bool,
    /// Cascaded `color`, straight RGBA in `[0, 1]`.
    pub color: [f32; 4],
    /// `text-decoration-line: underline` on the run's element. Pushed to parley
    /// as `StyleProperty::Underline`; the paint emit draws the line (parley
    /// supplies the geometry but does not draw it).
    pub underline: bool,
    /// `text-decoration-line: line-through` on the run's element. Pushed to
    /// parley as `StyleProperty::Strikethrough`; the paint emit draws the line
    /// from parley's strikethrough geometry (parley supplies it but does not
    /// draw it).
    pub strikethrough: bool,
    /// `text-decoration-line: overline` on the run's element. parley has no
    /// overline decoration, so paint maps each glyph run back to its source run
    /// and draws the line at the ascent from this flag.
    pub overline: bool,
    /// Cascaded `text-decoration-color` (straight RGBA), resolved from
    /// `currentColor` against the run's `color`. Pushed to parley as the
    /// underline / strikethrough brush, so the decoration can differ in color
    /// from the glyphs.
    pub decoration_color: [f32; 4],
    /// Cascaded `letter-spacing` in px (0 = `normal`). Pushed to parley as
    /// `StyleProperty::LetterSpacing`, so it widens the run's measured advance.
    pub letter_spacing: f32,
    /// Cascaded `word-spacing` in px (0 = `normal`). Pushed to parley as
    /// `StyleProperty::WordSpacing`.
    pub word_spacing: f32,
    /// Cascaded `line-height`. Pushed to parley as `StyleProperty::LineHeight`
    /// (skipped when `Normal`, which is parley's default).
    pub line_height: LineHeightSpec,
}

impl InlineRun {
    /// A run with default typography (16 px sans-serif, normal weight,
    /// opaque black).
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font_size: 16.0,
            font_family: FontFamilySpec::default(),
            weight: 400.0,
            italic: false,
            color: [0.0, 0.0, 0.0, 1.0],
            underline: false,
            strikethrough: false,
            overline: false,
            decoration_color: [0.0, 0.0, 0.0, 1.0],
            letter_spacing: 0.0,
            word_spacing: 0.0,
            line_height: LineHeightSpec::Normal,
        }
    }
}

/// An atomic inline box flowing among text runs: a replaced `<img>` or an
/// `inline-block`. parley reserves `width`×`height` at byte `index` in the
/// concatenated run text and reports its laid-out position; paint emission
/// draws the image (replaced) or the box + content (inline-block) there.
#[derive(Clone, Debug)]
pub struct InlineBoxItem<NodeId> {
    /// Byte offset into the concatenated run text where the box sits.
    pub index: usize,
    /// Reserved size. For `<img>` it is the intrinsic/CSS size set at
    /// construction; for an inline-block it is filled by the measure pass.
    pub width: f32,
    pub height: f32,
    /// DOM node of the source element (the `<img>` or the inline-block).
    pub source: NodeId,
    /// `Some` for an `inline-block` (its content + box style); `None` for a
    /// replaced `<img>` (sized intrinsically, painted as the image).
    pub block: Option<Box<InlineBlockBox<NodeId>>>,
}

/// An `inline-block`'s own content and box style. Its size is measured from
/// `content` (shrink-to-fit, clamped by any definite CSS `width`/`height`) and
/// its content Layout cached for paint.
#[derive(Clone, Debug)]
pub struct InlineBlockBox<NodeId> {
    /// The inline-block's own inline content (text runs / nested boxes).
    pub content: InlineContent<NodeId>,
    /// Definite CSS `width` / `height` in px, if set (else content size).
    pub css_width: Option<f32>,
    pub css_height: Option<f32>,
    /// Box background color (straight RGBA), painted behind the content.
    pub background: [f32; 4],
}

/// The inline content of a Taffy leaf — styled text runs plus replaced
/// inline boxes (`<img>`), which parley lays out together (text +
/// inline elements + images flow on shared lines, wrapping at the
/// container width). A bare text node is a one-run, no-box
/// `InlineContent`; a block element establishing an inline formatting
/// context may have many runs and boxes.
///
/// Generic over the DOM `NodeId` so inline boxes can carry their
/// source element for image lookup at paint time.
///
/// Created in [`crate::construct`]; consumed by the measure function
/// during `compute_layout_with_measure`.
#[derive(Clone, Debug)]
pub struct InlineContent<NodeId> {
    pub runs: Vec<InlineRun>,
    pub boxes: Vec<InlineBoxItem<NodeId>>,
}

impl<NodeId> InlineContent<NodeId> {
    /// Single-run content from one text string with default typography.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            runs: vec![InlineRun::new(text)],
            boxes: Vec::new(),
        }
    }

    /// Single-run content with explicit size + family (the common
    /// bare-text-node case). Default weight / normal / opaque black.
    pub fn single(text: impl Into<String>, font_size: f32, font_family: FontFamilySpec) -> Self {
        Self {
            runs: vec![InlineRun {
                text: text.into(),
                font_size,
                font_family,
                weight: 400.0,
                italic: false,
                color: [0.0, 0.0, 0.0, 1.0],
                underline: false,
                strikethrough: false,
                overline: false,
                decoration_color: [0.0, 0.0, 0.0, 1.0],
                letter_spacing: 0.0,
                word_spacing: 0.0,
                line_height: LineHeightSpec::Normal,
            }],
            boxes: Vec::new(),
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
fn family_property(spec: &FontFamilySpec) -> StyleProperty<'_, ColorBrush> {
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
    pub layout_ctx: LayoutContext<ColorBrush>,
    /// Cached `parley::Layout` per Taffy text leaf — populated by
    /// [`measure_inline_content`] after each measure call. Paint
    /// emission reads from here via `BoxTree::node_map`
    /// (DOM `NodeId` → `taffy::NodeId` → cached `Layout`) to extract
    /// positioned glyphs (and per-run color via the brush) without
    /// re-shaping.
    pub layouts: FxHashMap<taffy::NodeId, Layout<ColorBrush>>,
    /// Cached marker `Layout` per list item (keyed by the item's Taffy id),
    /// shaped after layout by [`TextMeasureCtx::shape_marker`]. Separate from
    /// `layouts` so an item's own inline text and its marker don't collide on
    /// the same key. Paint reads it to hang the marker left of the content box.
    pub marker_layouts: FxHashMap<taffy::NodeId, Layout<ColorBrush>>,
    /// Cached content `Layout` for each inline-block, keyed by `(the enclosing
    /// leaf's Taffy id, the box's index in that leaf's `InlineContent.boxes`)`.
    /// Built by [`measure_inline_content`]; paint reads it to draw the
    /// inline-block's glyphs at the box's parley-placed position.
    pub inline_block_layouts: FxHashMap<(taffy::NodeId, usize), Layout<ColorBrush>>,
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
            marker_layouts: FxHashMap::default(),
            inline_block_layouts: FxHashMap::default(),
        }
    }

    /// Shape a list item's marker (a single run) into a one-line `Layout` and
    /// cache it under `taffy_id`, so paint can extract its glyphs and hang it to
    /// the left of the item's content box. No wrap (markers are one line).
    pub fn shape_marker(&mut self, run: &InlineRun, taffy_id: taffy::NodeId) {
        let mut builder = self
            .layout_ctx
            .ranged_builder(&mut self.font_ctx, &run.text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(run.font_size));
        builder.push_default(family_property(&run.font_family));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(run.weight)));
        builder.push_default(StyleProperty::Brush(ColorBrush(run.color)));
        let mut layout: Layout<ColorBrush> = builder.build(&run.text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        self.marker_layouts.insert(taffy_id, layout);
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
pub fn measure_inline_content<NodeId>(
    ctx: &mut TextMeasureCtx,
    content: &InlineContent<NodeId>,
    taffy_id: taffy::NodeId,
    known_dimensions: Size<Option<f32>>,
    available_space: Size<AvailableSpace>,
) -> Size<f32> {
    // Short-circuit when both axes are explicitly known. (Don't cache
    // — there's no shaped Layout to give back; emit sees no entry.)
    if let (Some(w), Some(h)) = (known_dimensions.width, known_dimensions.height) {
        return Size { width: w, height: h };
    }

    // Empty content (no text and no inline boxes) measures as
    // zero-by-(font-size * 1.2) — a one-line baseline.
    if content.runs.iter().all(|r| r.text.is_empty()) && content.boxes.is_empty() {
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

    // Reserve each inline box's space. `<img>` uses its intrinsic/CSS size; an
    // inline-block is measured from its own content (and its content Layout
    // cached under (this leaf, box index) for paint).
    let mut box_sizes: Vec<(f32, f32)> = Vec::with_capacity(content.boxes.len());
    for (i, b) in content.boxes.iter().enumerate() {
        let (w, h, layout) = measure_inline_box(ctx, b);
        if let Some(l) = layout {
            ctx.inline_block_layouts.insert((taffy_id, i), l);
        }
        box_sizes.push((w, h));
    }

    let layout = build_inline_layout(ctx, content, &box_sizes, max_advance);
    let size = Size {
        width: known_dimensions.width.unwrap_or_else(|| layout.width()),
        height: known_dimensions.height.unwrap_or_else(|| layout.height()),
    };
    ctx.layouts.insert(taffy_id, layout);
    size
}

/// Reserved size of one inline box, and (for an inline-block) its shaped
/// content `Layout`. `<img>` reports its intrinsic/CSS size; an inline-block is
/// shrink-to-fit-measured from its content, clamped by any definite CSS
/// `width`/`height`.
fn measure_inline_box<NodeId>(
    ctx: &mut TextMeasureCtx,
    b: &InlineBoxItem<NodeId>,
) -> (f32, f32, Option<Layout<ColorBrush>>) {
    let Some(ib) = &b.block else {
        return (b.width, b.height, None); // replaced <img>
    };
    // Shrink-to-fit width: a definite CSS width caps the line, else max-content
    // (no max_advance). Nested inline boxes get their own reserved sizes.
    let inner_sizes: Vec<(f32, f32)> = ib
        .content
        .boxes
        .iter()
        .map(|bb| {
            let (w, h, _) = measure_inline_box(ctx, bb);
            (w, h)
        })
        .collect();
    let layout = build_inline_layout(ctx, &ib.content, &inner_sizes, ib.css_width);
    let w = ib.css_width.unwrap_or_else(|| layout.width());
    let h = ib.css_height.unwrap_or_else(|| layout.height());
    (w, h, Some(layout))
}

/// Shape `content` into a parley `Layout`, reserving each inline box at its
/// matching `box_sizes` entry and wrapping at `max_advance`. Shared by the leaf
/// measure and each inline-block's own measure.
fn build_inline_layout<NodeId>(
    ctx: &mut TextMeasureCtx,
    content: &InlineContent<NodeId>,
    box_sizes: &[(f32, f32)],
    max_advance: Option<f32>,
) -> Layout<ColorBrush> {
    // Concatenate run texts, tracking each run's byte range for the
    // per-run style spans.
    let mut text = String::with_capacity(content.total_len());
    let mut ranges: Vec<(std::ops::Range<usize>, &InlineRun)> = Vec::new();
    for run in &content.runs {
        let start = text.len();
        text.push_str(&run.text);
        ranges.push((start..text.len(), run));
    }

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
        // Per-run color rides the brush so it survives into the
        // Layout and is read back per GlyphRun at paint time.
        builder.push(StyleProperty::Brush(ColorBrush(run.color)), range.clone());
        // `text-decoration: underline` — parley records it on the run's style
        // (`GlyphRun::style().underline`); paint emission reads it back and draws
        // the line, since parley supplies the geometry but does not draw it.
        if run.underline {
            builder.push(StyleProperty::Underline(true), range.clone());
            builder.push(
                StyleProperty::UnderlineBrush(Some(ColorBrush(run.decoration_color))),
                range.clone(),
            );
        }
        // `text-decoration: line-through` — same arrangement as underline.
        if run.strikethrough {
            builder.push(StyleProperty::Strikethrough(true), range.clone());
            builder.push(
                StyleProperty::StrikethroughBrush(Some(ColorBrush(run.decoration_color))),
                range.clone(),
            );
        }
        // `letter-spacing` / `word-spacing` widen the run's advance at shape time
        // (0 = `normal` = parley's default). Pushed only when set, to keep the
        // common no-spacing path free of redundant spans.
        if run.letter_spacing != 0.0 {
            builder.push(StyleProperty::LetterSpacing(run.letter_spacing), range.clone());
        }
        if run.word_spacing != 0.0 {
            builder.push(StyleProperty::WordSpacing(run.word_spacing), range.clone());
        }
        // Cascaded `line-height`. `Normal` is parley's default (font metrics), so
        // only a CSS `<number>` / `<length>` overrides it.
        match run.line_height {
            LineHeightSpec::Normal => {},
            LineHeightSpec::Factor(f) => {
                builder.push(StyleProperty::LineHeight(LineHeight::FontSizeRelative(f)), range.clone());
            },
            LineHeightSpec::Px(px) => {
                builder.push(StyleProperty::LineHeight(LineHeight::Absolute(px)), range.clone());
            },
        }
    }

    // Atomic inline boxes (`<img>` / inline-block) — parley reserves the
    // `box_sizes[i]` space and reports the laid-out position. `id` is the index
    // into `content.boxes` so paint emission can recover the source.
    for (i, b) in content.boxes.iter().enumerate() {
        let (width, height) = box_sizes.get(i).copied().unwrap_or((b.width, b.height));
        builder.push_inline_box(InlineBox {
            id: i as u64,
            kind: InlineBoxKind::InFlow,
            index: b.index,
            width,
            height,
        });
    }

    let mut layout: Layout<ColorBrush> = builder.build(text.as_str());
    layout.break_all_lines(max_advance);
    layout.align(Alignment::Start, AlignmentOptions::default());
    layout
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
        let content = InlineContent::<u64>::new("");
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
        let content = InlineContent::<u64>::new("Hello, world!");
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
        let content = InlineContent::<u64>::new("ignored");
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
        let combined = InlineContent::<u64> {
            runs: vec![InlineRun::new("Hello "), InlineRun::new("world")],
            boxes: Vec::new(),
        };
        let just_hello = InlineContent::<u64>::new("Hello ");
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
