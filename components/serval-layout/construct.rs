/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DOM inline-content gathering + cascade property readers.
//!
//! Shared helpers consumed by [`crate::box_tree`] when it builds the
//! layout arena: inline-formatting-context detection, gathering an
//! inline subtree into [`InlineContent`] (styled runs + replaced inline
//! boxes), replaced-element sizing, and the per-element cascade reads
//! (font + color) that style each text run.
//!
//! (Previously this module also built the `TaffyTree` directly; that
//! owned-`Style` path was retired when the box tree — Taffy's trait-impl
//! tree over `TaffyStyloStyle` — became the engine. See
//! `docs/2026-05-25_box_tree_trait_impl_plan.md`.)

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};

use crate::adapter::NodeRef;
use crate::image_decode::ImagePlane;
use crate::style::StylePlane;
use crate::text_measure::{
    FontFamilySpec, GenericFamilyKind, InlineBlockBox, InlineBoxItem, InlineContent, InlineRun,
    LineHeightSpec,
};

/// Default font size used for runs whose element has no cascaded
/// `font-size` (hand-rolled style fixtures). 16 px matches the
/// CSS/UA-stylesheet convention and parley's own default.
const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Whether `elem` establishes an inline formatting context: every
/// element child is either `display:inline` or a replaced inline box
/// (`<img>`), text children flow inline by nature, and there is at
/// least one piece of *inline text* (a text node or a non-replaced
/// inline element) to flow.
///
/// The inline-text requirement keeps a lone `<img>` on the block path:
/// `<body><img></body>` stays a block with the image as its own child
/// box (the established, working behavior). Only when an `<img>` is
/// mixed with text — `<p>before <img> after</p>` — does the element
/// become an inline context where the image flows as a parley
/// `InlineBox` among the runs.
///
/// Comments / PIs are ignored. With no cascade data (`is_inline_element`
/// → `None`), non-replaced elements are treated as block — preserving
/// the pre-inline behavior for hand-rolled style fixtures.
pub(crate) fn establishes_inline_context<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    elem: NodeRef<'a, D>,
) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut has_inline_text = false;
    let mut replaced_count = 0;
    for child in elem.dom_children() {
        match dom.kind(child.id()) {
            // Whitespace-only text is collapsible formatting, not real inline
            // content — it must not by itself turn a block container into an
            // inline context.
            NodeKind::Text => {
                if dom.text(child.id()).is_some_and(|t| !t.trim().is_empty()) {
                    has_inline_text = true;
                }
            },
            NodeKind::Element => {
                if is_replaced(dom, child.id()) {
                    // A replaced element flows as an inline box. A *lone* img
                    // with no other inline content stays on the block path
                    // (intrinsic sizing); two or more flow inline, side by side.
                    replaced_count += 1;
                    continue;
                }
                // `inline-block` is an atomic inline-level box: it participates
                // in the line like inline content (so it keeps this an inline
                // context), but `gather_runs` reserves it as an atomic
                // `InlineBox` rather than recursing into its content.
                if is_inline_block(styles, child.id()) {
                    has_inline_text = true;
                } else if is_inline_element(styles, child.id()).unwrap_or(false) {
                    has_inline_text = true;
                } else {
                    // A block-level child forces block layout.
                    return false;
                }
            }
            _ => {}
        }
    }
    // Inline context when there is real inline text / an inline element /
    // inline-block, or when two or more replaced boxes flow side by side. A
    // single lone img with nothing else stays on the block path.
    has_inline_text || replaced_count >= 2
}

/// Whether an element is replaced content we render as its own box rather than
/// as flowed inline text: `<img>`, plus `<iframe>` / `<canvas>`, which size to
/// the 300×150 default object size when they have no intrinsic / CSS size.
///
/// `<video>` / `<object>` / `<embed>` are deliberately excluded: their content
/// is image-like, and without decoding it a 300×150 placeholder mis-renders the
/// `object-fit` / `object-position` corpus more than a 300×150 default helps.
pub(crate) fn is_replaced<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use html5ever::local_name;
    dom.element_name(id).is_some_and(|q| {
        q.local == local_name!("img")
            || q.local == local_name!("iframe")
            || q.local == local_name!("canvas")
    })
}

/// Whether `id` is a replaced element that, lacking intrinsic content, falls
/// back to the CSS default object size (300×150) — every replaced element except
/// `<img>` (which sizes to its decoded pixels, or 0 when undecoded).
fn uses_default_object_size<D>(dom: &D, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    dom.element_name(id)
        .is_some_and(|q| q.local != html5ever::local_name!("img"))
}

/// Read an element's cascaded outer display: `Some(true)` for
/// `display:inline`, `Some(false)` for block-level, `None` when the
/// cascade hasn't run for this element.
fn is_inline_element<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::specified::box_::DisplayOutside;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let display = data.styles.primary().get_box().display;
    Some(matches!(display.outside(), DisplayOutside::Inline))
}

/// Collapse each maximal run of ASCII/Unicode whitespace in `s` to a single
/// space (CSS `white-space: normal` collapsing). Source newlines + indentation
/// become a single space rather than a literal run or a forced line break.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    out
}

/// Whether `id` is `display: inline-block` — inline-level outside, but an
/// independent (flow-root) formatting context inside.
pub(crate) fn is_inline_block<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> bool {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    let Some(entry) = styles.get(id) else { return false };
    let Some(data) = entry.borrow_data() else { return false };
    let display = data.styles.primary().get_box().display;
    matches!(display.outside(), DisplayOutside::Inline)
        && matches!(display.inside(), DisplayInside::FlowRoot)
}

/// Pixel size for a replaced element: the decoded intrinsic size from `images`
/// (for `<img>`), or the CSS default object size 300×150 for the embedded-content
/// elements (`<iframe>` etc.) that have no intrinsic content, with each axis then
/// overridden by a definite CSS `width`/`height`. Shared by the block-level
/// replaced leaf ([`crate::box_tree`]) and the inline replaced box. Non-length
/// dimensions (`auto`, percentages) leave the base size in place; an undecoded
/// `<img>` with no CSS size reserves 0×0.
pub(crate) fn replaced_px_size<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    id: D::NodeId,
) -> (f32, f32)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let (mut w, mut h) = images
        .get(id)
        .map(|d| (d.width as f32, d.height as f32))
        .unwrap_or_else(|| {
            // No decoded pixels: embedded content defaults to 300×150; an
            // undecoded <img> reserves 0×0.
            if uses_default_object_size(dom, id) {
                (300.0, 150.0)
            } else {
                (0.0, 0.0)
            }
        });

    if let Some(entry) = styles.get(id) {
        if let Some(data) = entry.borrow_data() {
            let pos = data.styles.primary().get_position();
            if let Some(cw) = definite_px(&pos.width) {
                w = cw;
            }
            if let Some(ch) = definite_px(&pos.height) {
                h = ch;
            }
        }
    }
    (w, h)
}

/// A CSS `Size` as definite pixels, or `None` for `auto` / percentage /
/// intrinsic keywords.
fn definite_px(size: &style::values::computed::Size) -> Option<f32> {
    use style::values::computed::Size as CssSize;
    match size {
        CssSize::LengthPercentage(lp) => lp.0.to_length().map(|l| l.px()),
        _ => None,
    }
}

/// Gather an inline-context element's subtree into [`InlineContent`].
/// Walks in document order; each text node becomes a run styled by the
/// nearest enclosing inline element (which carries the cascade), and
/// each replaced element (`<img>`) becomes an [`InlineBoxItem`] anchored
/// at the current byte offset into the concatenated run text, sized from
/// `images` + any definite CSS size.
pub(crate) fn gather_inline_content<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    elem: NodeRef<'a, D>,
) -> InlineContent<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut runs = Vec::new();
    let mut boxes = Vec::new();
    let mut offset = 0usize;
    gather_runs(dom, styles, images, elem, &mut runs, &mut boxes, &mut offset);
    InlineContent { runs, boxes }
}

/// Recursive helper for [`gather_inline_content`]. `node`'s direct
/// text children are styled by `node` (the enclosing inline element);
/// element children recurse with themselves as the new styling element,
/// except replaced elements (`<img>`) which become inline boxes.
/// `offset` tracks the running byte position into the concatenated run
/// text so each box's `index` matches the parley `InlineBox` placement.
fn gather_runs<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    node: NodeRef<'a, D>,
    runs: &mut Vec<InlineRun>,
    boxes: &mut Vec<InlineBoxItem<D::NodeId>>,
    offset: &mut usize,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    for child in node.dom_children() {
        gather_child(dom, styles, images, node.id(), child, runs, boxes, offset);
    }
}

/// Gather one inline-level `child` into the running `runs` / `boxes`. `styling`
/// is the element whose cascade styles bare text (the enclosing inline element,
/// or — for an anonymous block box — the block container). Shared by
/// [`gather_runs`] (an element's own children) and [`gather_inline_group`] (a
/// run of a block container's inline-level children).
#[allow(clippy::too_many_arguments)]
fn gather_child<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    styling: D::NodeId,
    child: NodeRef<'a, D>,
    runs: &mut Vec<InlineRun>,
    boxes: &mut Vec<InlineBoxItem<D::NodeId>>,
    offset: &mut usize,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    match dom.kind(child.id()) {
        NodeKind::Text => {
            // CSS `white-space: normal` — collapse each run of whitespace
            // (including source newlines + indentation) to a single space, so
            // formatting whitespace does not force line breaks (parley breaks
            // on `\n`) or render as literal runs. A real line break comes only
            // from `<br>`. (Leading/trailing-edge trimming and `pre` are
            // follow-ups.)
            let text = collapse_whitespace(dom.text(child.id()).unwrap_or(""));
            if !text.is_empty() {
                *offset += text.len();
                runs.push(run_for_element(styles, styling, text));
            }
        }
        NodeKind::Element => {
            if dom
                .element_name(child.id())
                .is_some_and(|q| q.local == html5ever::local_name!("br"))
            {
                // `<br>` is a forced line break: emit a newline run (parley
                // breaks lines at the mandatory `\n`).
                *offset += 1;
                runs.push(run_for_element(styles, styling, "\n".to_string()));
            } else if is_replaced(dom, child.id()) {
                let (width, height) = replaced_px_size(dom, styles, images, child.id());
                boxes.push(InlineBoxItem {
                    index: *offset,
                    width,
                    height,
                    source: child.id(),
                    block: None,
                });
            } else if is_inline_block(styles, child.id()) {
                // Atomic inline-block: gather its own inline content + box
                // style; the measure pass sizes it and parley places it as a
                // unit. Its content is not flowed into this line.
                let content = gather_inline_content(dom, styles, images, child);
                let (css_width, css_height) = inline_block_css_size(styles, child.id());
                boxes.push(InlineBoxItem {
                    index: *offset,
                    width: 0.0,
                    height: 0.0,
                    source: child.id(),
                    block: Some(Box::new(InlineBlockBox {
                        content,
                        css_width,
                        css_height,
                        background: inline_block_bg_of(styles, child.id()),
                    })),
                });
            } else {
                // A non-replaced inline element flows transparently: its own
                // children join this line, styled by it.
                gather_runs(dom, styles, images, child, runs, boxes, offset);
            }
        }
        _ => {}
    }
}

/// Whether `id` is a non-replaced inline-level *element* whose content flows as
/// inline runs and can join an anonymous block box on a line: a `display:inline`
/// element or an `inline-block`. Excludes replaced boxes (`<img>` etc.) and the
/// atomic inline-level layout boxes (`inline-table` / `-flex` / `-grid`), which
/// keep their own block-path box rather than having their content flattened into
/// runs. Element-only (text is grouped separately, after whitespace handling).
pub(crate) fn flows_inline<D>(dom: &D, styles: &StylePlane<D::NodeId>, id: D::NodeId) -> bool
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    if !matches!(dom.kind(id), NodeKind::Element) || is_replaced(dom, id) {
        return false;
    }
    let Some(entry) = styles.get(id) else { return false };
    let Some(data) = entry.borrow_data() else { return false };
    let display = data.styles.primary().get_box().display;
    matches!(display.outside(), DisplayOutside::Inline)
        && matches!(display.inside(), DisplayInside::Flow | DisplayInside::FlowRoot)
}

/// Gather a run of a block container's inline-level children (`group`) into one
/// [`InlineContent`] for an anonymous block box. Bare text is styled by
/// `styling` (the container).
pub(crate) fn gather_inline_group<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    styling: D::NodeId,
    group: &[NodeRef<'a, D>],
) -> InlineContent<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut runs = Vec::new();
    let mut boxes = Vec::new();
    let mut offset = 0usize;
    for &child in group {
        gather_child(dom, styles, images, styling, child, &mut runs, &mut boxes, &mut offset);
    }
    InlineContent { runs, boxes }
}

/// Build an [`InlineRun`] for `text` styled by element `id`'s cascade
/// (size / family / weight / italic), defaulting where the cascade
/// hasn't run.
pub(crate) fn run_for_element<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    text: String,
) -> InlineRun {
    InlineRun {
        text,
        font_size: font_size_of(styles, id).unwrap_or(DEFAULT_FONT_SIZE),
        font_family: font_family_of(styles, id).unwrap_or_default(),
        weight: font_weight_of(styles, id).unwrap_or(400.0),
        italic: font_italic_of(styles, id).unwrap_or(false),
        // Per-run color from the styling element's cascaded `color`.
        color: text_color_of(styles, id).unwrap_or([0.0, 0.0, 0.0, 1.0]),
        underline: text_underline_of(styles, id).unwrap_or(false),
        strikethrough: text_strikethrough_of(styles, id).unwrap_or(false),
        overline: text_overline_of(styles, id).unwrap_or(false),
        decoration_color: text_decoration_color_of(styles, id).unwrap_or([0.0, 0.0, 0.0, 1.0]),
        letter_spacing: letter_spacing_of(styles, id).unwrap_or(0.0),
        word_spacing: word_spacing_of(styles, id).unwrap_or(0.0),
        line_height: line_height_of(styles, id).unwrap_or_default(),
    }
}

/// An element's cascaded `line-height` mapped to a [`LineHeightSpec`]. `normal`
/// (and the un-cascaded case) keeps parley's font-metric default; a `<number>`
/// becomes a font-size multiple, a `<length>` an absolute px height.
fn line_height_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<LineHeightSpec> {
    use style::values::computed::LineHeight;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(match &data.styles.primary().get_font().line_height {
        LineHeight::Normal => LineHeightSpec::Normal,
        LineHeight::Number(num) => LineHeightSpec::Factor(num.0),
        LineHeight::Length(l) => LineHeightSpec::Px(l.px()),
        // `-moz-block-height` is gecko-only (cfg'd out here); treat as normal.
        #[allow(unreachable_patterns)]
        _ => LineHeightSpec::Normal,
    })
}

/// Whether an element's cascaded `text-decoration-line` includes `underline`.
/// `None` when the cascade hasn't run.
fn text_underline_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::computed::TextDecorationLine;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(
        data.styles
            .primary()
            .get_text()
            .text_decoration_line
            .contains(TextDecorationLine::UNDERLINE),
    )
}

/// Whether an element's cascaded `text-decoration-line` includes `line-through`.
/// `None` when the cascade hasn't run.
fn text_strikethrough_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::computed::TextDecorationLine;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(
        data.styles
            .primary()
            .get_text()
            .text_decoration_line
            .contains(TextDecorationLine::LINE_THROUGH),
    )
}

/// Whether an element's cascaded `text-decoration-line` includes `overline`.
/// `None` when the cascade hasn't run.
fn text_overline_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::computed::TextDecorationLine;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(
        data.styles
            .primary()
            .get_text()
            .text_decoration_line
            .contains(TextDecorationLine::OVERLINE),
    )
}

/// Read an element's cascaded text `color` as straight RGBA in
/// `[0, 1]`. `None` when the cascade hasn't run.
fn text_color_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<[f32; 4]> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let absolute = data.styles.primary().get_inherited_text().color;
    let srgb = absolute.into_srgb_legacy();
    Some(*srgb.raw_components())
}

/// Definite CSS `width` / `height` (px) of an inline-block, or `None` per axis
/// for `auto` / percentage / intrinsic (→ shrink-to-fit / content height).
fn inline_block_css_size<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> (Option<f32>, Option<f32>) {
    let Some(entry) = styles.get(id) else { return (None, None) };
    let Some(data) = entry.borrow_data() else { return (None, None) };
    let pos = data.styles.primary().get_position();
    (definite_px(&pos.width), definite_px(&pos.height))
}

/// An inline-block's cascaded `background-color` as straight RGBA, resolving
/// `currentColor` against its own `color`. Transparent when no cascade data.
fn inline_block_bg_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> [f32; 4] {
    let Some(entry) = styles.get(id) else { return [0.0; 4] };
    let Some(data) = entry.borrow_data() else { return [0.0; 4] };
    let primary = data.styles.primary();
    let current = primary.get_inherited_text().color;
    let absolute = primary
        .get_background()
        .background_color
        .resolve_to_absolute(&current);
    *absolute.into_srgb_legacy().raw_components()
}

/// Read an element's cascaded `text-decoration-color` as straight RGBA in
/// `[0, 1]`, resolving the default `currentColor` against the element's own
/// `color`. `None` when the cascade hasn't run.
fn text_decoration_color_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<[f32; 4]> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let current = primary.get_inherited_text().color;
    let absolute = primary.get_text().text_decoration_color.resolve_to_absolute(&current);
    let srgb = absolute.into_srgb_legacy();
    Some(*srgb.raw_components())
}

/// How a list item's marker is rendered, from its cascaded `list-style-type`.
/// A bullet carries its glyph; the counter kinds format the item's ordinal.
enum MarkerKind {
    Bullet(&'static str),
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

/// Map an element's cascaded `list-style-type` to a [`MarkerKind`]. `None` for
/// `list-style-type: none` or when the cascade hasn't run. Custom counter styles
/// (`symbols()`, string) and unrecognized names fall back to a disc bullet.
fn marker_kind<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<MarkerKind> {
    use style::counter_style::CounterStyle;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let cs = &data.styles.primary().get_list().list_style_type.0;
    match cs {
        CounterStyle::None => None,
        CounterStyle::Name(name) => Some(match name.0.to_string().as_str() {
            "disc" => MarkerKind::Bullet("\u{2022}"),
            "circle" => MarkerKind::Bullet("\u{25E6}"),
            "square" => MarkerKind::Bullet("\u{25AA}"),
            "decimal" => MarkerKind::Decimal,
            "lower-alpha" | "lower-latin" => MarkerKind::LowerAlpha,
            "upper-alpha" | "upper-latin" => MarkerKind::UpperAlpha,
            "lower-roman" => MarkerKind::LowerRoman,
            "upper-roman" => MarkerKind::UpperRoman,
            _ => MarkerKind::Bullet("\u{2022}"),
        }),
        _ => Some(MarkerKind::Bullet("\u{2022}")),
    }
}

/// Bijective base-26 alphabetic counter (`1→a`, `26→z`, `27→aa`, …), uppercased
/// when `upper`.
fn alpha_marker(mut n: usize, upper: bool) -> String {
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        s.insert(0, (b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    if upper {
        s = s.to_uppercase();
    }
    s
}

/// Roman-numeral counter, uppercased when `upper`. Out-of-range values
/// (`0`, `>= 4000`) fall back to decimal.
fn roman_marker(mut n: usize, upper: bool) -> String {
    if n == 0 || n >= 4000 {
        return n.to_string();
    }
    const VALUES: [(usize, &str); 13] = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"),
        (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut s = String::new();
    for (value, sym) in VALUES {
        while n >= value {
            s.push_str(sym);
            n -= value;
        }
    }
    if upper {
        s = s.to_uppercase();
    }
    s
}

/// The list marker string for a `<li>` element, or `None` for non-list-items
/// (and for `list-style-type: none`). The cascaded `list-style-type` chooses a
/// bullet (`disc` / `circle` / `square`) or a counter (`decimal`,
/// `lower`/`upper-alpha`, `lower`/`upper-roman`); counters use the item's 1-based
/// ordinal, counted from preceding `<li>` siblings under the same parent. The
/// `start` attr / `<li> value` and `list-style-image` are deferred.
fn list_marker_text<NodeId: Copy + Eq + Hash, D>(
    dom: &D,
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<String>
where
    D: LayoutDom<NodeId = NodeId>,
{
    if dom.element_name(id)?.local != html5ever::local_name!("li") {
        return None;
    }
    let kind = marker_kind(styles, id)?;
    if let MarkerKind::Bullet(glyph) = kind {
        return Some(glyph.to_string());
    }
    // Counter kinds: the item's ordinal, honoring `start`, `<ol reversed>`, and
    // any `<li value>` (HTML's ordinal algorithm). The counter steps by +1, or
    // -1 when reversed; it begins at `start`, defaulting to 1 (or, when reversed
    // without an explicit `start`, the item count). Each `<li value>` resets it
    // for that item and the ones after.
    let parent = dom.parent(id)?;
    let no_ns: html5ever::Namespace = html5ever::ns!();
    let is_li = |n| dom.element_name(n).is_some_and(|q| q.local == html5ever::local_name!("li"));
    let reversed = dom
        .attribute(parent, &no_ns, &html5ever::LocalName::from("reversed"))
        .is_some();
    let step: i64 = if reversed { -1 } else { 1 };
    let start: i64 = dom
        .attribute(parent, &no_ns, &html5ever::LocalName::from("start"))
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| {
            if reversed {
                dom.dom_children(parent).filter(|n| is_li(*n)).count() as i64
            } else {
                1
            }
        });
    let mut counter = start;
    let mut ordinal = start;
    for sib in dom.dom_children(parent) {
        if !is_li(sib) {
            continue;
        }
        if let Some(v) = dom
            .attribute(sib, &no_ns, &html5ever::LocalName::from("value"))
            .and_then(|s| s.trim().parse::<i64>().ok())
        {
            counter = v;
        }
        if sib == id {
            ordinal = counter;
            break;
        }
        counter += step;
    }
    // Alphabetic / roman counters are defined for positive ordinals; outside that
    // range (a 0 / negative `start` or `value`) they fall back to decimal.
    let positive = usize::try_from(ordinal).ok().filter(|n| *n >= 1);
    let body = match (kind, positive) {
        (MarkerKind::Decimal, _) | (_, None) => ordinal.to_string(),
        (MarkerKind::LowerAlpha, Some(n)) => alpha_marker(n, false),
        (MarkerKind::UpperAlpha, Some(n)) => alpha_marker(n, true),
        (MarkerKind::LowerRoman, Some(n)) => roman_marker(n, false),
        (MarkerKind::UpperRoman, Some(n)) => roman_marker(n, true),
        (MarkerKind::Bullet(_), _) => unreachable!("bullets returned above"),
    };
    Some(format!("{body}."))
}

/// The marker for a list item as single-run [`InlineContent`], styled by the
/// `<li>`'s own font + color, ready to shape and hang to the left of the item.
/// `None` for non-list-items. Decoration is cleared (a marker is never
/// underlined / struck through by the item's own `text-decoration`).
pub(crate) fn list_marker_content<NodeId, D>(
    dom: &D,
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<InlineContent<NodeId>>
where
    NodeId: Copy + Eq + Hash,
    D: LayoutDom<NodeId = NodeId>,
{
    let text = list_marker_text(dom, styles, id)?;
    let mut run = run_for_element(styles, id, text);
    run.underline = false;
    run.strikethrough = false;
    Some(InlineContent { runs: vec![run], boxes: Vec::new() })
}

/// Whether an element's cascaded `list-style-position` is `inside` (the marker
/// flows as the item's first inline content rather than hanging outside).
pub(crate) fn list_marker_is_inside<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> bool {
    use style::computed_values::list_style_position::T as ListPosition;
    styles
        .get(id)
        .and_then(|e| e.borrow_data())
        .is_some_and(|d| d.styles.primary().get_list().list_style_position == ListPosition::Inside)
}

/// The marker as an inline run (with a trailing space) for `list-style-position:
/// inside`, styled by the item's font + color. `None` for non-list-items and
/// `list-style-type: none`.
pub(crate) fn list_marker_inline_run<NodeId, D>(
    dom: &D,
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<InlineRun>
where
    NodeId: Copy + Eq + Hash,
    D: LayoutDom<NodeId = NodeId>,
{
    let text = list_marker_text(dom, styles, id)?;
    let mut run = run_for_element(styles, id, format!("{text} "));
    run.underline = false;
    run.strikethrough = false;
    Some(run)
}

/// Read an element's cascaded `font-size` in CSS px. Returns `None`
/// when the cascade hasn't been applied to that element (hand-rolled
/// style fixtures); the caller defaults to `DEFAULT_FONT_SIZE`.
fn font_size_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<f32> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(data.styles.primary().get_font().font_size.computed_size().px())
}

/// An element's cascaded `letter-spacing` in CSS px (`normal` resolves to 0).
/// Percentages resolve against the font size. `None` when the cascade hasn't run.
fn letter_spacing_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<f32> {
    use style::values::computed::Length;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let fs = primary.get_font().font_size.computed_size().px();
    Some(primary.get_inherited_text().letter_spacing.0.resolve(Length::new(fs)).px())
}

/// An element's cascaded `word-spacing` in CSS px (`normal` resolves to 0).
/// Percentages resolve against the font size. `None` when the cascade hasn't run.
fn word_spacing_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<f32> {
    use style::values::computed::Length;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let fs = primary.get_font().font_size.computed_size().px();
    Some(primary.get_inherited_text().word_spacing.resolve(Length::new(fs)).px())
}

/// Read an element's cascaded `font-family` and collapse the family
/// list to its first entry (probe scope — no fallback-chain walking).
/// Returns `None` when the cascade hasn't run for this element.
fn font_family_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<FontFamilySpec> {
    use style::values::computed::font::{GenericFontFamily, SingleFontFamily};

    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let primary = data.styles.primary();
    let first = primary.get_font().font_family.families.iter().next()?;
    let spec = match first {
        SingleFontFamily::FamilyName(name) => FontFamilySpec::Named(name.name.to_string()),
        SingleFontFamily::Generic(g) => {
            let kind = match g {
                GenericFontFamily::Serif => GenericFamilyKind::Serif,
                GenericFontFamily::SansSerif => GenericFamilyKind::SansSerif,
                GenericFontFamily::Monospace => GenericFamilyKind::Monospace,
                GenericFontFamily::Cursive => GenericFamilyKind::Cursive,
                GenericFontFamily::Fantasy => GenericFamilyKind::Fantasy,
                // None / SystemUi / other internal generics → sans-serif.
                _ => GenericFamilyKind::SansSerif,
            };
            FontFamilySpec::Generic(kind)
        },
    };
    Some(spec)
}

/// Read an element's cascaded numeric `font-weight` (400 normal, 700
/// bold). `None` when the cascade hasn't run.
fn font_weight_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<f32> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(data.styles.primary().get_font().font_weight.value())
}

/// Whether an element's cascaded `font-style` is non-normal
/// (italic / oblique). `None` when the cascade hasn't run.
fn font_italic_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<bool> {
    use style::values::computed::font::FontStyle;
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(data.styles.primary().get_font().font_style != FontStyle::NORMAL)
}
