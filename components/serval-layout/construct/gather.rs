// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Inline-content gathering and pseudo-element / first-letter handling.

use super::*;

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
) -> (InlineContent<D::NodeId>, Vec<(Range<usize>, D::NodeId)>)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use style::selector_parser::PseudoElement;

    let mut runs = Vec::new();
    let mut boxes = Vec::new();
    // Byte-range → source-element index, parallel to the concatenated run text
    // `measure`/`paint` see, for inline hit-testing ([`crate::inline_hit`]). Each
    // text/`<br>` run records the element that styles it (the innermost inline
    // element, e.g. an `<a>`); built in run-append order so byte ranges line up.
    let mut sources = Vec::new();
    let mut offset = 0usize;
    // `::before` generated content, prepended; then the element's own content;
    // then `::after`, appended. Both are inline text runs here (the common case
    // — block-`display` generated content is a later slice), so they style and
    // paint through the existing run path with the pseudo's own cascade.
    push_pseudo_content(
        styles,
        elem.id(),
        PseudoElement::Before,
        &mut runs,
        &mut sources,
        &mut offset,
    );
    gather_runs(
        dom,
        styles,
        images,
        elem,
        &mut runs,
        &mut boxes,
        &mut sources,
        &mut offset,
    );
    push_pseudo_content(
        styles,
        elem.id(),
        PseudoElement::After,
        &mut runs,
        &mut sources,
        &mut offset,
    );
    // `::first-letter` restyles the first typographic letter of the block's
    // content. Applied after the runs are gathered (so it sees `::before`'s
    // generated text as the first letter when present), and only here at the
    // inline-formatting-context root — a child `span::first-letter` never reaches
    // this path, matching the spec's block-container restriction. It splits a run
    // at a byte boundary without moving any byte, so the `sources` ranges stay valid.
    apply_first_letter(styles, elem.id(), &mut runs);
    (
        InlineContent {
            runs,
            boxes,
            no_wrap: no_wrap_of(styles, elem.id()),
        },
        sources,
    )
}

/// Split the first content run at the `::first-letter` boundary, restyling the
/// first typographic letter with the element's eager `::first-letter` cascade. A
/// no-op when no `::first-letter` rule applies or the first content run has no
/// letter (whitespace / punctuation only). Text bytes are preserved exactly (the
/// run is split, never rewritten), so run-relative box indices and selection
/// offsets stay valid.
pub(crate) fn apply_first_letter<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    runs: &mut Vec<InlineRun>,
) {
    use style::selector_parser::PseudoElement;

    let Some(entry) = styles.get(id) else { return };
    let Some(data) = entry.borrow_data() else {
        return;
    };
    let Some(cv) = data.styles.pseudos.get(&PseudoElement::FirstLetter) else {
        return;
    };

    // The first run carrying non-whitespace content owns the first letter (a
    // leading whitespace-only run, e.g. from `::before { content: " " }`, stays).
    let Some(ri) = runs.iter().position(|r| !r.text.trim().is_empty()) else {
        return;
    };
    let Some((a, b)) = first_letter_boundary(&runs[ri].text) else {
        return;
    };

    let original = runs[ri].clone();
    let prefix = original.text[..a].to_string();
    let letter = original.text[a..b].to_string();
    let remainder = original.text[b..].to_string();

    let mut replacement = Vec::with_capacity(3);
    if !prefix.is_empty() {
        let mut p = original.clone();
        p.text = prefix;
        replacement.push(p);
    }
    replacement.push(run_from_computed(cv, letter));
    if !remainder.is_empty() {
        let mut r = original.clone();
        r.text = remainder;
        replacement.push(r);
    }
    runs.splice(ri..=ri, replacement);
}

/// The byte span `(a, b)` of the CSS first typographic letter unit in `text`:
/// `a` is the first non-whitespace byte, `b` the byte after the first letter (or
/// digit), with any punctuation between them included (`"(A` → `(A`). `None` when
/// the run has no letter — whitespace-only, or symbols/punctuation with no
/// following letter — so the run carries no `::first-letter`.
///
/// Tier-1 boundary: leading whitespace excluded, leading punctuation included,
/// the unit ends at the first letter. Combining marks *after* that letter are not
/// yet folded in (the documented §4 edge).
pub(crate) fn first_letter_boundary(text: &str) -> Option<(usize, usize)> {
    let a = text
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| i)?;
    for (rel, c) in text[a..].char_indices() {
        if c.is_whitespace() {
            return None; // whitespace before any letter: no unit on this run
        }
        if c.is_alphanumeric() {
            return Some((a, a + rel + c.len_utf8()));
        }
        // Punctuation / symbol preceding the first letter: include and continue.
    }
    None
}

/// Push a run for `elem`'s `pseudo` (`::before` / `::after`) generated content,
/// if the cascade resolved that pseudo with string `content`.
pub(crate) fn push_pseudo_content<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    pseudo: style::selector_parser::PseudoElement,
    runs: &mut Vec<InlineRun>,
    sources: &mut Vec<(Range<usize>, NodeId)>,
    offset: &mut usize,
) {
    let Some(entry) = styles.get(id) else { return };
    let Some(data) = entry.borrow_data() else {
        return;
    };
    let Some(cv) = data.styles.pseudos.get(&pseudo) else {
        return;
    };
    // A block-level pseudo is realized as its own block box (see
    // [`block_pseudo_content`]), not an inline run in the flow.
    use style::values::specified::box_::DisplayOutside;
    if matches!(cv.get_box().display.outside(), DisplayOutside::Block) {
        return;
    }
    if let Some(text) = pseudo_content_text(cv) {
        let start = *offset;
        *offset += text.len();
        runs.push(run_from_computed(cv, text));
        // Generated content's "source" for hit-testing is the element owning the
        // pseudo (there is no separate pseudo DOM node to address).
        sources.push((start..*offset, id));
    }
}

/// The style + generated inline content of a *block-level* `::before` / `::after`
/// pseudo, or `None` when the element has no such pseudo, its `content` is not a
/// string, or its computed `display` is inline-level (handled by the inline-run
/// path in [`push_pseudo_content`]). The box tree realizes the result as a
/// synthetic block box child of the element (§5 slice 3), so it participates in
/// block layout and paints its own decorations from the pseudo cascade.
pub(crate) fn block_pseudo_content<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
    kind: PseudoKind,
) -> Option<(ServoArc<ComputedValues>, InlineContent<NodeId>)> {
    use style::selector_parser::PseudoElement;
    use style::values::specified::box_::DisplayOutside;

    let pseudo = match kind {
        PseudoKind::Before => PseudoElement::Before,
        PseudoKind::After => PseudoElement::After,
    };
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let cv = data.styles.pseudos.get(&pseudo)?;
    if !matches!(cv.get_box().display.outside(), DisplayOutside::Block) {
        return None;
    }
    // A box is generated for any string `content` — including `content: ""` (a
    // decorative box, e.g. a background-image with no text) — but not for
    // `normal` / `none`. (Unlike the inline path, an empty block box is still
    // laid out and painted, so the empty-string case matters here.)
    use style::values::generics::counters::{Content, ContentItem};
    let Content::Items(items) = &cv.get_counters().content else {
        return None;
    };
    let mut text = String::new();
    for item in items.items.iter() {
        if let ContentItem::String(s) = item {
            text.push_str(s);
        }
    }
    let content = InlineContent {
        runs: vec![run_from_computed(cv, text)],
        boxes: Vec::new(),
        no_wrap: false,
    };
    Some((cv.clone(), content))
}

/// Recursive helper for [`gather_inline_content`]. `node`'s direct
/// text children are styled by `node` (the enclosing inline element);
/// element children recurse with themselves as the new styling element,
/// except replaced elements (`<img>`) which become inline boxes.
/// `offset` tracks the running byte position into the concatenated run
/// text so each box's `index` matches the parley `InlineBox` placement.
#[allow(clippy::too_many_arguments)]
pub(crate) fn gather_runs<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    node: NodeRef<'a, D>,
    runs: &mut Vec<InlineRun>,
    boxes: &mut Vec<InlineBoxItem<D::NodeId>>,
    sources: &mut Vec<(Range<usize>, D::NodeId)>,
    offset: &mut usize,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    for child in node.dom_children() {
        gather_child(
            dom,
            styles,
            images,
            node.id(),
            child,
            runs,
            boxes,
            sources,
            offset,
        );
    }
}

/// Gather one inline-level `child` into the running `runs` / `boxes`. `styling`
/// is the element whose cascade styles bare text (the enclosing inline element,
/// or — for an anonymous block box — the block container). Shared by
/// [`gather_runs`] (an element's own children) and [`gather_inline_group`] (a
/// run of a block container's inline-level children).
#[allow(clippy::too_many_arguments)]
pub(crate) fn gather_child<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    images: &ImagePlane<D::NodeId>,
    styling: D::NodeId,
    child: NodeRef<'a, D>,
    runs: &mut Vec<InlineRun>,
    boxes: &mut Vec<InlineBoxItem<D::NodeId>>,
    sources: &mut Vec<(Range<usize>, D::NodeId)>,
    offset: &mut usize,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    match dom.kind(child.id()) {
        NodeKind::Text => {
            // CSS white-space-collapse, applied per the text's computed value.
            // The `white-space: normal` / `nowrap` default collapses each run of
            // whitespace (source newlines + indentation) to one space, so
            // formatting whitespace does not force line breaks (parley breaks on
            // `\n`) or render as literal runs. `pre` / `pre-wrap` preserve
            // whitespace + newlines (each source `\n` becomes a parley break);
            // `pre-line` collapses spaces but keeps newlines. (Leading/trailing-
            // edge trimming is a follow-up.) A real break also comes from `<br>`.
            let text =
                apply_white_space_collapse(styles, styling, dom.text(child.id()).unwrap_or(""));
            if !text.is_empty() {
                let start = *offset;
                *offset += text.len();
                runs.push(run_for_element(styles, styling, text));
                // Bare text is owned (for hit-testing) by the inline element that
                // styles it — the innermost enclosing inline, so a click on a link's
                // text resolves to the `<a>`.
                sources.push((start..*offset, styling));
            }
        },
        NodeKind::Element => {
            if dom
                .element_name(child.id())
                .is_some_and(|q| q.local == html5ever::local_name!("br"))
            {
                // `<br>` is a forced line break: emit a newline run (parley
                // breaks lines at the mandatory `\n`).
                let start = *offset;
                *offset += 1;
                runs.push(run_for_element(styles, styling, "\n".to_string()));
                sources.push((start..*offset, styling));
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
                // unit. Its content is not flowed into this line. (Its own inline
                // sources are dropped: the inline-block is hit as a box via its
                // fragment; resolving links *inside* an inline-block is a follow-on.)
                let (content, _inner_sources) = gather_inline_content(dom, styles, images, child);
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
                // children join this line, styled by it — and recurse with the
                // child as the styling element, so its descendants' runs are
                // sourced to it (the innermost inline wins).
                gather_runs(dom, styles, images, child, runs, boxes, sources, offset);
            }
        },
        _ => {},
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
    if !matches!(dom.kind(id), NodeKind::Element)
        || is_replaced(dom, id)
        || is_floating(styles, id)
        || has_clearance(styles, id)
    {
        return false;
    }
    let Some(entry) = styles.get(id) else {
        return false;
    };
    let Some(data) = entry.borrow_data() else {
        return false;
    };
    let display = data.styles.primary().get_box().display;
    matches!(display.outside(), DisplayOutside::Inline)
        && matches!(
            display.inside(),
            DisplayInside::Flow | DisplayInside::FlowRoot
        )
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
) -> (InlineContent<D::NodeId>, Vec<(Range<usize>, D::NodeId)>)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut runs = Vec::new();
    let mut boxes = Vec::new();
    let mut sources = Vec::new();
    let mut offset = 0usize;
    for &child in group {
        gather_child(
            dom,
            styles,
            images,
            styling,
            child,
            &mut runs,
            &mut boxes,
            &mut sources,
            &mut offset,
        );
    }
    (
        InlineContent {
            runs,
            boxes,
            no_wrap: no_wrap_of(styles, styling),
        },
        sources,
    )
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

/// Build an [`InlineRun`] from an explicit [`ComputedValues`] (a `::before` /
/// `::after` pseudo-element style) rather than a DOM node's primary style.
///
/// Mirrors [`run_for_element`]'s field reads against `cv` directly. The two
/// don't share code today because `run_for_element`'s per-field helpers key off
/// `(styles, id)` and read `.primary()`; unify them (helpers taking
/// `&ComputedValues`) if a third caller appears.
pub(crate) fn run_from_computed(cv: &ComputedValues, text: String) -> InlineRun {
    use style::values::computed::font::{FontStyle, GenericFontFamily, SingleFontFamily};
    use style::values::computed::{Length, LineHeight, TextDecorationLine};

    let font = cv.get_font();
    let fs = font.font_size.computed_size().px();
    let itext = cv.get_inherited_text();

    let font_family = match font.font_family.families.iter().next() {
        Some(SingleFontFamily::FamilyName(name)) => FontFamilySpec::Named(name.name.to_string()),
        Some(SingleFontFamily::Generic(g)) => FontFamilySpec::Generic(match g {
            GenericFontFamily::Serif => GenericFamilyKind::Serif,
            GenericFontFamily::SansSerif => GenericFamilyKind::SansSerif,
            GenericFontFamily::Monospace => GenericFamilyKind::Monospace,
            GenericFontFamily::Cursive => GenericFamilyKind::Cursive,
            GenericFontFamily::Fantasy => GenericFamilyKind::Fantasy,
            _ => GenericFamilyKind::SansSerif,
        }),
        None => FontFamilySpec::default(),
    };

    let line_height = match &font.line_height {
        LineHeight::Normal => LineHeightSpec::Normal,
        LineHeight::Number(num) => LineHeightSpec::Factor(num.0),
        LineHeight::Length(l) => LineHeightSpec::Px(l.px()),
        #[allow(unreachable_patterns)]
        _ => LineHeightSpec::Normal,
    };

    let text_style = cv.get_text();
    let decoration = text_style.text_decoration_line;
    let color = *itext.color.into_srgb_legacy().raw_components();
    let decoration_color = *text_style
        .text_decoration_color
        .resolve_to_absolute(&itext.color)
        .into_srgb_legacy()
        .raw_components();

    InlineRun {
        text,
        font_size: fs,
        font_family,
        weight: font.font_weight.value(),
        italic: font.font_style != FontStyle::NORMAL,
        color,
        underline: decoration.contains(TextDecorationLine::UNDERLINE),
        strikethrough: decoration.contains(TextDecorationLine::LINE_THROUGH),
        overline: decoration.contains(TextDecorationLine::OVERLINE),
        decoration_color,
        letter_spacing: itext.letter_spacing.0.resolve(Length::new(fs)).px(),
        word_spacing: itext.word_spacing.resolve(Length::new(fs)).px(),
        line_height,
    }
}

/// The string a pseudo-element's `content` generates, or `None` for `normal` /
/// `none` / non-string content (counters, images, quotes — later slices).
pub(crate) fn pseudo_content_text(cv: &ComputedValues) -> Option<String> {
    use style::values::generics::counters::{Content, ContentItem};
    match &cv.get_counters().content {
        Content::Items(items) => {
            let mut out = String::new();
            for item in items.items.iter() {
                if let ContentItem::String(s) = item {
                    out.push_str(s);
                }
            }
            (!out.is_empty()).then_some(out)
        },
        _ => None,
    }
}
