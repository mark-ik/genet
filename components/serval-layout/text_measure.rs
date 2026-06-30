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
use taffy::InlineFloatBand;
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
    /// `white-space: nowrap` (CSS `text-wrap-mode: nowrap`): the content is laid
    /// out on a single line — parley does not soft-wrap it to the available
    /// width; only a `<br>` / `\n` breaks. Set from the element's cascade in
    /// `construct`; read by the measure pass to drop `max_advance`.
    pub no_wrap: bool,
}

impl<NodeId> InlineContent<NodeId> {
    /// Single-run content from one text string with default typography.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            runs: vec![InlineRun::new(text)],
            boxes: Vec::new(),
            no_wrap: false,
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
            no_wrap: false,
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
    /// Cached `…` (ellipsis) `Layout` per `text-overflow: ellipsis` leaf, keyed by
    /// the leaf's Taffy id and shaped in the leaf's own font by
    /// [`TextMeasureCtx::shape_ellipsis`]. Paint reads it to truncate an
    /// overflowing line and draw the ellipsis at the cut.
    pub ellipsis_layouts: FxHashMap<taffy::NodeId, Layout<ColorBrush>>,
    /// Cached content `Layout` for each inline-block, keyed by `(the enclosing
    /// leaf's Taffy id, the box's index in that leaf's `InlineContent.boxes`)`.
    /// Built by [`measure_inline_content`]; paint reads it to draw the
    /// inline-block's glyphs at the box's parley-placed position.
    pub inline_block_layouts: FxHashMap<(taffy::NodeId, usize), Layout<ColorBrush>>,
    /// Float exclusion bands per inline-context leaf (keyed by the leaf's Taffy
    /// id), in the leaf's content-box-local space. Populated by the box-tree's
    /// block-child layout when a text leaf sits in a block formatting context
    /// with active floats (`BlockContext::inline_exclusion_bands`), and read by
    /// [`measure_inline_content`] to narrow each line box around the floats so
    /// text wraps to their side and reclaims the column below them. Absent ⇒ no
    /// active floats ⇒ the scalar single-width break path runs unchanged.
    /// Cleared per pass by [`TextMeasureCtx::reset`].
    pub float_bands: FxHashMap<taffy::NodeId, Vec<InlineFloatBand>>,
}

impl Default for TextMeasureCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl TextMeasureCtx {
    pub fn new() -> Self {
        let mut font_ctx = FontContext::new();
        // Register the Ahem test font (`font-family: Ahem`). Ahem renders every
        // glyph as a solid square of the em size, so the CSS test suite uses it
        // pervasively to assert exact box geometry; without it those tests fall
        // back to a proportional font and mis-measure. The face self-names
        // "Ahem", so `font-family: Ahem` resolves once registered.
        const AHEM: &[u8] = include_bytes!("Ahem.ttf");
        font_ctx
            .collection
            .register_fonts(parley::fontique::Blob::from(AHEM.to_vec()), None);
        Self {
            font_ctx,
            layout_ctx: LayoutContext::new(),
            layouts: FxHashMap::default(),
            marker_layouts: FxHashMap::default(),
            ellipsis_layouts: FxHashMap::default(),
            inline_block_layouts: FxHashMap::default(),
            float_bands: FxHashMap::default(),
        }
    }

    /// Clear the per-pass `parley::Layout` caches (keyed by Taffy ids, which are
    /// stale across layouts) while keeping the persistent `font_ctx` /
    /// `layout_ctx`. This lets one context be reused across layout passes
    /// without re-running font discovery — the host or session holds the
    /// context for its life and the layout entry points `reset` it per pass.
    pub fn reset(&mut self) {
        self.layouts.clear();
        self.marker_layouts.clear();
        self.ellipsis_layouts.clear();
        self.inline_block_layouts.clear();
        self.float_bands.clear();
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

    /// Shape an ellipsis (`…`) in `style`'s font / size / color into a one-line
    /// `Layout` cached under `taffy_id`, for a `text-overflow: ellipsis` leaf.
    /// Paint reads it to draw the ellipsis where it truncates the overflowing
    /// text. `style` is the leaf's representative run (its first), so the ellipsis
    /// matches the text's typography and baseline.
    pub fn shape_ellipsis(&mut self, style: &InlineRun, taffy_id: taffy::NodeId) {
        const ELLIPSIS: &str = "\u{2026}";
        let mut builder = self
            .layout_ctx
            .ranged_builder(&mut self.font_ctx, ELLIPSIS, 1.0, true);
        builder.push_default(StyleProperty::FontSize(style.font_size));
        builder.push_default(family_property(&style.font_family));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight)));
        builder.push_default(StyleProperty::Brush(ColorBrush(style.color)));
        let mut layout: Layout<ColorBrush> = builder.build(ELLIPSIS);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        self.ellipsis_layouts.insert(taffy_id, layout);
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
        return Size {
            width: w,
            height: h,
        };
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

    // Translate Taffy's available_space into parley's max_advance. `white-space:
    // nowrap` forces a single line (no soft wrap) regardless of available width —
    // `None` lets parley break only on mandatory `\n` / `<br>`.
    let max_advance: Option<f32> = if content.no_wrap {
        None
    } else {
        match available_space.width {
            AvailableSpace::Definite(w) => Some(w),
            AvailableSpace::MinContent => Some(0.0),
            AvailableSpace::MaxContent => None,
        }
    };

    // Float wrap-around: if this leaf has float exclusion bands snapshotted for
    // this pass (a paragraph in a block formatting context with active floats)
    // and a definite wrap width, each line breaks against the band at its own y
    // so text wraps to the float's side and reclaims the column below it. Cloned
    // up front (a tiny Vec) so it does not borrow `ctx` across the layout-cache
    // mutation below; absent/empty ⇒ the scalar `break_and_align` path runs
    // unchanged (so every non-float inline test is byte-identical). The bands
    // are only ever populated on the definite-width final block-layout pass, so
    // the intrinsic min/max-content probes never see them.
    let float_bands: Option<Vec<InlineFloatBand>> =
        match (ctx.float_bands.get(&taffy_id), max_advance) {
            (Some(b), Some(_)) if !content.no_wrap && !b.is_empty() => Some(b.clone()),
            _ => None,
        };

    // Re-measure fast path. Taffy probes each leaf at min-content, then
    // max-content, then its resolved width, so this runs 2-3× per leaf. The
    // glyph shaping is width-independent — only the line breaks change — so once
    // a leaf is shaped this pass, re-break the cached `Layout` at the new width
    // instead of re-shaping from scratch. The cache is cleared each pass by
    // `TextMeasureCtx::reset`, so a hit means "already shaped this pass"; the last
    // break wins, leaving the cached layout broken at the final width for paint.
    if let Some(layout) = ctx.layouts.get_mut(&taffy_id) {
        match (&float_bands, max_advance) {
            (Some(bands), Some(content_width)) => {
                break_and_align_floats(layout, bands, content_width)
            },
            _ => break_and_align(layout, max_advance),
        }
        return Size {
            width: known_dimensions.width.unwrap_or_else(|| layout.width()),
            height: known_dimensions.height.unwrap_or_else(|| layout.height()),
        };
    }

    // First measure of this leaf this pass (when the shaping pre-pass did not
    // already populate the cache): shape it now — reserving each inline box's
    // space and caching every inline-block sublayout under `(this leaf, box
    // index)` for paint — then break and cache the shaped `Layout` for the
    // re-measure path + paint.
    let (mut layout, sublayouts) = shape_leaf(&mut ctx.font_ctx, &mut ctx.layout_ctx, content);
    for (i, l) in sublayouts {
        ctx.inline_block_layouts.insert((taffy_id, i), l);
    }
    match (&float_bands, max_advance) {
        (Some(bands), Some(content_width)) => {
            break_and_align_floats(&mut layout, bands, content_width)
        },
        _ => break_and_align(&mut layout, max_advance),
    }
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
    font_ctx: &mut FontContext,
    layout_ctx: &mut LayoutContext<ColorBrush>,
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
            let (w, h, _) = measure_inline_box(font_ctx, layout_ctx, bb);
            (w, h)
        })
        .collect();
    let mut layout = shape_inline_layout(font_ctx, layout_ctx, &ib.content, &inner_sizes);
    break_and_align(&mut layout, ib.css_width);
    let w = ib.css_width.unwrap_or_else(|| layout.width());
    let h = ib.css_height.unwrap_or_else(|| layout.height());
    (w, h, Some(layout))
}

/// Shape a leaf's inline content into its (unbroken) `Layout` plus the shaped
/// `Layout` of each top-level inline-block (paired with its box index). This is
/// the whole width-independent half of inline measurement — reserve each inline
/// box, then shape the runs — with no line breaking, so it can run ahead of
/// layout (serial or fanned out across threads; see the shaping pre-pass in
/// `layout_via_box_tree`). It takes the parley contexts directly, not a
/// `TextMeasureCtx`, so a worker thread can drive it with its own cloned
/// `FontContext` + a fresh `LayoutContext`. The caller breaks the leaf layout at
/// each probed width and caches both it and the inline-block sublayouts.
pub(crate) fn shape_leaf<NodeId>(
    font_ctx: &mut FontContext,
    layout_ctx: &mut LayoutContext<ColorBrush>,
    content: &InlineContent<NodeId>,
) -> (Layout<ColorBrush>, Vec<(usize, Layout<ColorBrush>)>) {
    let mut box_sizes: Vec<(f32, f32)> = Vec::with_capacity(content.boxes.len());
    let mut sublayouts: Vec<(usize, Layout<ColorBrush>)> = Vec::new();
    for (i, b) in content.boxes.iter().enumerate() {
        let (w, h, layout) = measure_inline_box(font_ctx, layout_ctx, b);
        if let Some(l) = layout {
            sublayouts.push((i, l));
        }
        box_sizes.push((w, h));
    }
    let layout = shape_inline_layout(font_ctx, layout_ctx, content, &box_sizes);
    (layout, sublayouts)
}

/// Break a shaped `Layout` into lines at `max_advance` and start-align it.
/// Split from [`shape_inline_layout`] so a leaf shaped once can be re-broken at
/// each candidate width Taffy probes (min-content, max-content, then the final
/// width) without re-shaping — the glyphs are width-independent; only the line
/// breaks change. This is the cheap half of inline measurement.
fn break_and_align(layout: &mut Layout<ColorBrush>, max_advance: Option<f32>) {
    layout.break_all_lines(max_advance);
    layout.align(Alignment::Start, AlignmentOptions::default());
}

/// Break a shaped `Layout` into lines that wrap around float exclusion `bands`,
/// then start-align each line within its own (narrowed) box. The float-aware
/// twin of [`break_and_align`]: where that breaks every line at one width, this
/// gives each line the width left by the floats at its own y.
///
/// `bands` are content-box-local (`y` from the content-box top, `left`/`right`
/// insets inward from the edges); `content_width` is the float-free content-box
/// width. For each line, the band covering its top y narrows it to
/// `[left, content_width - right]`; a line with no covering band — below all
/// floats, or in a zero-inset gap between them — keeps the full width, which is
/// how text reclaims the column under a float.
///
/// parley breaks lines incrementally: before each line we read its top y
/// (`committed_y`, set from the prior lines' heights), look up the active band,
/// and set the line's x-offset + max-advance. Those flow into the line metrics
/// (`inline_min_coord` / `inline_max_coord`) and so into glyph positions at
/// align time. `set_layout_max_advance(content_width)` first satisfies parley's
/// invariant that each line's max-advance is `<= the layout's`.
fn break_and_align_floats(
    layout: &mut Layout<ColorBrush>,
    bands: &[InlineFloatBand],
    content_width: f32,
) {
    {
        let mut breaker = layout.break_lines();
        breaker.state_mut().set_layout_max_advance(content_width);
        loop {
            let line_top = breaker.committed_y() as f32;
            let (line_x, line_max_advance) = band_for_line(bands, line_top, content_width);
            let state = breaker.state_mut();
            state.set_line_x(line_x);
            state.set_line_max_advance(line_max_advance);
            if breaker.break_next().is_none() {
                break;
            }
        }
        // `breaker` drops here: its lines swap back into `layout` and the
        // layout's width/height are recomputed from the per-line metrics.
    }
    layout.align(Alignment::Start, AlignmentOptions::default());
}

/// The inline `(x_offset, max_advance)` for a line whose top sits at `y`
/// (content-box-local), given the float exclusion `bands` and the float-free
/// `content_width`. The first band covering `y` narrows the line by its insets;
/// no covering band (below the floats, or a zero-inset gap) yields the full
/// width at x = 0. Insets are clamped so a float wider than the column cannot
/// produce a negative advance.
fn band_for_line(bands: &[InlineFloatBand], y: f32, content_width: f32) -> (f32, f32) {
    for band in bands {
        if y >= band.y_start && y < band.y_end {
            let line_x = band.left.clamp(0.0, content_width);
            let line_max_advance =
                (content_width - band.left - band.right).clamp(0.0, content_width);
            return (line_x, line_max_advance);
        }
    }
    (0.0, content_width)
}

/// Shape `content` into a parley `Layout`, reserving each inline box at its
/// matching `box_sizes` entry. Returns the shaped-but-unbroken layout — the
/// caller runs [`break_and_align`] at the wrap width. Shaping is the expensive,
/// width-independent half (glyph runs, font resolution); separating it lets a
/// leaf shape once per pass and re-break per probed width. Shared by the leaf
/// measure and each inline-block's own measure.
fn shape_inline_layout<NodeId>(
    font_ctx: &mut FontContext,
    layout_ctx: &mut LayoutContext<ColorBrush>,
    content: &InlineContent<NodeId>,
    box_sizes: &[(f32, f32)],
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

    let mut builder = layout_ctx.ranged_builder(font_ctx, text.as_str(), 1.0, true);
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
            builder.push(
                StyleProperty::LetterSpacing(run.letter_spacing),
                range.clone(),
            );
        }
        if run.word_spacing != 0.0 {
            builder.push(StyleProperty::WordSpacing(run.word_spacing), range.clone());
        }
        // Cascaded `line-height`. `Normal` is parley's default (font metrics), so
        // only a CSS `<number>` / `<length>` overrides it.
        match run.line_height {
            LineHeightSpec::Normal => {},
            LineHeightSpec::Factor(f) => {
                builder.push(
                    StyleProperty::LineHeight(LineHeight::FontSizeRelative(f)),
                    range.clone(),
                );
            },
            LineHeightSpec::Px(px) => {
                builder.push(
                    StyleProperty::LineHeight(LineHeight::Absolute(px)),
                    range.clone(),
                );
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

    builder.build(text.as_str())
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
            Size {
                width: None,
                height: None,
            },
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
            Size {
                width: None,
                height: None,
            },
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
            Size {
                width: Some(100.0),
                height: Some(50.0),
            },
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
            no_wrap: false,
        };
        let just_hello = InlineContent::<u64>::new("Hello ");
        let avail = Size {
            width: AvailableSpace::MaxContent,
            height: AvailableSpace::MaxContent,
        };
        let none = Size {
            width: None,
            height: None,
        };
        let combined_w =
            measure_inline_content(&mut ctx, &combined, taffy::NodeId::from(1u64), none, avail)
                .width;
        let hello_w = measure_inline_content(
            &mut ctx,
            &just_hello,
            taffy::NodeId::from(2u64),
            none,
            avail,
        )
        .width;
        assert!(
            combined_w > hello_w,
            "combined run width {combined_w} should exceed 'Hello ' alone {hello_w}"
        );
    }
}
