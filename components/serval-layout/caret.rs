/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Caret geometry: the screen rect of a character offset within a laid-out text
//! node — the shared primitive caret painting, IME candidate placement, and text
//! selection all need (the Lane C "Text" axis / Lane H form-control depth).
//!
//! It reads the `parley::Layout` serval already caches per inline-formatting leaf
//! ([`TextMeasureCtx::layouts`], keyed by `taffy::NodeId` via
//! [`BoxTree::node_map`]) — the same path paint emission uses for glyph runs — and
//! asks parley's [`Cursor`] for the caret rect at a byte offset, then offsets it
//! by the node's absolute content-box origin so the result is in scene
//! coordinates (what a paint overlay or `set_ime_cursor_area` consumes).
//!
//! This is the *production* primitive; wiring it into a painted caret (a thin
//! `DrawRect` in the scene) and to IME placement are the consumers.

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use parley::Layout;
use parley::layout::{Affinity, Cluster, Cursor, Selection};

use crate::box_tree::BoxTree;
use crate::fragment::FragmentPlane;
use crate::style::StylePlane;
use crate::text_measure::{ColorBrush, TextMeasureCtx};

/// The `::selection` highlight colors for `node` as `(background, foreground)`
/// straight RGBA, from the nearest ancestor (including `node`) whose cascade
/// resolved a `::selection` pseudo with a non-transparent background. `None`
/// when no ancestor carries a `::selection` background, so the host falls back
/// to its theme default highlight. `::selection` is eager, so it is in the
/// pseudo style map. The foreground (selected-glyph recolor) is returned for
/// callers that can repaint the range; background-only painting ignores it.
pub fn selection_style<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    node: D::NodeId,
) -> Option<([f32; 4], [f32; 4])>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    use style::selector_parser::PseudoElement;
    let mut cur = Some(node);
    while let Some(id) = cur {
        if let Some(data) = styles.get(id).and_then(|e| e.borrow_data()) {
            if let Some(cv) = data.styles.pseudos.get(&PseudoElement::Selection) {
                let current = cv.get_inherited_text().color;
                let bg = *cv
                    .get_background()
                    .background_color
                    .resolve_to_absolute(&current)
                    .into_srgb_legacy()
                    .raw_components();
                if bg[3] > 0.0 {
                    let fg = *current.into_srgb_legacy().raw_components();
                    return Some((bg, fg));
                }
            }
        }
        cur = dom.parent(id);
    }
    None
}

/// The caret colour for `node` as straight RGBA — the cascaded text `color`,
/// which is what `caret-color: auto` (the default) resolves to. Walks to the
/// nearest ancestor (including `node`) carrying style data, so a text leaf with
/// no own rule inherits its container's colour. `None` only when no ancestor has
/// style data, so the host keeps its theme default.
///
/// Reading the text colour makes the caret track the theme automatically (the
/// sheet already colours the text per theme); an explicit `caret-color` override
/// is a later refinement.
pub fn caret_color<D>(dom: &D, styles: &StylePlane<D::NodeId>, node: D::NodeId) -> Option<[f32; 4]>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut cur = Some(node);
    while let Some(id) = cur {
        if let Some(data) = styles.get(id).and_then(|e| e.borrow_data()) {
            let color = data.styles.primary().get_inherited_text().color;
            return Some(*color.into_srgb_legacy().raw_components());
        }
        cur = dom.parent(id);
    }
    None
}

/// A caret rectangle in absolute layout (scene) coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The caret rectangle for `byte_offset` within `node`'s laid-out text, or `None`
/// if `node` has no cached text layout (not a text-bearing leaf, or not laid
/// out) or no fragment.
///
/// `width` is the caret's thickness (e.g. `1.0`–`2.0` device px). The returned
/// rect is absolute: the node's content-box origin (its accumulated
/// parent-relative fragment positions, inset by border + padding) plus parley's
/// caret geometry within the text layout.
pub fn caret_rect<D>(
    dom: &D,
    node: D::NodeId,
    byte_offset: usize,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    fragments: &FragmentPlane<D::NodeId>,
    width: f32,
) -> Option<CaretRect>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // DOM node -> taffy id -> the cached parley layout (the glyph-run path).
    let taffy_id = built.node_map.get(&node)?;
    let layout = text_ctx.layouts.get(taffy_id)?;

    // parley's caret geometry within the text layout's local space.
    let cursor = Cursor::from_byte_index(layout, byte_offset, Affinity::default());
    let bb = cursor.geometry(layout, width);

    // Absolute content-box origin: accumulated border-box origin, inset by this
    // node's border + padding (parley lays text out within the content box).
    let (ox, oy) = absolute_origin(dom, fragments, node)?;
    let frame = fragments.rect_of(node)?;
    let content_x = ox + frame.border.left + frame.padding.left;
    let content_y = oy + frame.border.top + frame.padding.top;

    // Take the x extent from parley's caret geometry but the vertical extent from
    // the snug glyph band — `bb`'s height is the full line box (leading included,
    // and the font's tall ascent above), which paints a caret bar towering over
    // low-x-height words.
    let (top, height) = caret_band(layout, byte_offset);
    Some(CaretRect {
        x: content_x + bb.x0 as f32,
        y: content_y + top,
        width: (bb.x1 - bb.x0) as f32,
        height,
    })
}

/// The highlight rectangles for the selected byte range `[start, end)` within
/// `node`'s laid-out text, in absolute (scene) coordinates — one rect per line
/// the selection covers. Empty when `node` has no cached text layout / fragment,
/// or the range is collapsed.
///
/// The selection-highlight companion to [`caret_rect`], sharing the same
/// layout-lookup + absolute-origin path. parley's [`Selection`] (built from two
/// cursors) supplies the per-line geometry.
pub fn selection_rects<D>(
    dom: &D,
    node: D::NodeId,
    start: usize,
    end: usize,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    fragments: &FragmentPlane<D::NodeId>,
) -> Vec<CaretRect>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if start == end {
        return Vec::new();
    }
    let Some(taffy_id) = built.node_map.get(&node) else {
        return Vec::new();
    };
    let Some(layout) = text_ctx.layouts.get(taffy_id) else {
        return Vec::new();
    };
    let Some((ox, oy)) = absolute_origin(dom, fragments, node) else {
        return Vec::new();
    };
    let Some(frame) = fragments.rect_of(node) else {
        return Vec::new();
    };
    let content_x = ox + frame.border.left + frame.padding.left;
    let content_y = oy + frame.border.top + frame.padding.top;

    let anchor = Cursor::from_byte_index(layout, start, Affinity::default());
    let focus = Cursor::from_byte_index(layout, end, Affinity::default());
    // Selection stays browser-faithful: parley's per-line geometry is the full
    // line box (`block_min..block_max`), which is what a text selection highlights
    // — unlike the caret, we do not tighten to the glyph band.
    Selection::new(anchor, focus)
        .geometry(layout)
        .into_iter()
        .map(|(bb, _line)| CaretRect {
            x: content_x + bb.x0 as f32,
            y: content_y + bb.y0 as f32,
            width: (bb.x1 - bb.x0) as f32,
            height: (bb.y1 - bb.y0) as f32,
        })
        .collect()
}

/// A DOM text range: anchor and focus as `(inline-formatting-leaf, byte offset)`
/// pairs, in the caller's selection order — the anchor may sit after the focus in
/// document order (a backwards drag). Each node is an inline-formatting leaf (an
/// element with a cached `parley::Layout`, the unit [`selection_rects`] takes),
/// and the offset is a byte index into *that leaf's* concatenated inline text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRange<N> {
    pub anchor_node: N,
    pub anchor_offset: usize,
    pub focus_node: N,
    pub focus_offset: usize,
}

/// A selected DOM text span: the ordered range, its highlight rects in absolute
/// scene coordinates, and a plain-text export suitable for copy.
#[derive(Clone, Debug, PartialEq)]
pub struct TextSelection<N> {
    pub range: TextRange<N>,
    pub rects: Vec<CaretRect>,
    pub text: String,
}

/// The highlight rectangles for a selection `range` that may span several inline
/// leaves (across block boundaries), in absolute (scene) coordinates — the
/// multi-node generalisation of [`selection_rects`]. Empty when the range is
/// collapsed or neither endpoint resolves to a laid-out leaf.
///
/// The walk: enumerate the laid-out inline leaves in document order, order the two
/// endpoints by that order (same leaf → by offset), then for each leaf in the
/// span collect its per-line rects via [`selection_rects`] — the first leaf from
/// its start offset to its end, interior leaves whole, the last leaf to its end
/// offset. `usize::MAX` rides parley's index clamp to mean "to the end of this
/// leaf", so no per-leaf text length is needed. (Pseudo follow-ups §3.)
pub fn range_rects<D>(
    dom: &D,
    range: TextRange<D::NodeId>,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    fragments: &FragmentPlane<D::NodeId>,
) -> Vec<CaretRect>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // Document-order list of the leaves that actually have a cached text layout.
    let mut leaves: Vec<D::NodeId> = Vec::new();
    collect_text_leaves(dom, built, text_ctx, dom.document(), &mut leaves);

    let pos = |n: D::NodeId| leaves.iter().position(|&l| l == n);
    let (Some(ai), Some(fi)) = (pos(range.anchor_node), pos(range.focus_node)) else {
        return Vec::new();
    };

    // Order the endpoints in document order: by leaf position, then (same leaf) by
    // offset, so a backwards drag highlights the same span as a forwards one.
    let ((si, soff), (ei, eoff)) =
        if ai < fi || (ai == fi && range.anchor_offset <= range.focus_offset) {
            ((ai, range.anchor_offset), (fi, range.focus_offset))
        } else {
            ((fi, range.focus_offset), (ai, range.anchor_offset))
        };

    let mut rects = Vec::new();
    for i in si..=ei {
        let node = leaves[i];
        let (start, end) = match (i == si, i == ei) {
            (true, true) => (soff, eoff),        // single leaf
            (true, false) => (soff, usize::MAX), // first leaf: to its end
            (false, true) => (0, eoff),          // last leaf: from its start
            (false, false) => (0, usize::MAX),   // interior leaf: whole
        };
        rects.extend(selection_rects(
            dom, node, start, end, built, text_ctx, fragments,
        ));
    }
    rects
}

/// The highlight rects plus plain text for a selection `range` that may span
/// several inline leaves. Returns `None` when the range is collapsed or neither
/// endpoint resolves to a laid-out text leaf.
pub fn text_selection<D>(
    dom: &D,
    range: TextRange<D::NodeId>,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    fragments: &FragmentPlane<D::NodeId>,
) -> Option<TextSelection<D::NodeId>>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut leaves: Vec<D::NodeId> = Vec::new();
    collect_text_leaves(dom, built, text_ctx, dom.document(), &mut leaves);

    let pos = |n: D::NodeId| leaves.iter().position(|&l| l == n);
    let (Some(ai), Some(fi)) = (pos(range.anchor_node), pos(range.focus_node)) else {
        return None;
    };
    let ((si, soff), (ei, eoff)) =
        if ai < fi || (ai == fi && range.anchor_offset <= range.focus_offset) {
            ((ai, range.anchor_offset), (fi, range.focus_offset))
        } else {
            ((fi, range.focus_offset), (ai, range.anchor_offset))
        };
    if si == ei && soff == eoff {
        return None;
    }

    let ordered = TextRange {
        anchor_node: leaves[si],
        anchor_offset: soff,
        focus_node: leaves[ei],
        focus_offset: eoff,
    };
    let rects = range_rects(dom, ordered, built, text_ctx, fragments);
    if rects.is_empty() {
        return None;
    }

    let mut text = String::new();
    let mut prev_leaf = None;
    for i in si..=ei {
        let leaf = leaves[i];
        let Some(full) = leaf_text(leaf, built) else {
            continue;
        };
        let start = if i == si { soff.min(full.len()) } else { 0 };
        let end = if i == ei {
            eoff.min(full.len())
        } else {
            full.len()
        };
        if start >= end {
            continue;
        }
        if let Some(prev) = prev_leaf {
            if block_ancestor(dom, prev) != block_ancestor(dom, leaf) {
                text.push('\n');
            }
        }
        if let Some(slice) = full.get(start..end) {
            text.push_str(slice);
        }
        prev_leaf = Some(leaf);
    }

    Some(TextSelection {
        range: ordered,
        rects,
        text,
    })
}

/// Every occurrence of `needle` in the document's laid-out text, as highlight rects
/// in absolute (scene) coordinates — one inner `Vec` per match (a match wrapped across
/// lines yields several rects). Document order: leaves in pre-order, matches in byte
/// order within each leaf. Case-insensitive (ASCII fold). The find-in-page primitive.
///
/// It walks the inline-formatting leaves ([`collect_text_leaves`]), reconstructs each
/// leaf's concatenated run text (the same byte space [`selection_rects`] /
/// [`caret_rect`] index — both derive from the run concatenation order, so the offsets
/// line up by construction), finds substring matches, and maps each match's byte range
/// to per-line rects via [`selection_rects`]. An empty needle, or one that matches no
/// laid-out text, yields no rects. Matches do not overlap (search resumes past each
/// hit). The HTML/serval lane has no host-queryable packet, so the content actor runs
/// this where the layout lives (see [`crate::find_text_rects_from_layout_dom`]).
pub fn find_text_rects<D>(
    dom: &D,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    fragments: &FragmentPlane<D::NodeId>,
    needle: &str,
) -> Vec<Vec<CaretRect>>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let needle_lc = needle.to_ascii_lowercase();
    if needle_lc.is_empty() {
        return Vec::new();
    }
    let mut leaves = Vec::new();
    collect_text_leaves(dom, built, text_ctx, dom.document(), &mut leaves);
    let mut out = Vec::new();
    for leaf in leaves {
        let Some(&taffy_id) = built.node_map.get(&leaf) else {
            continue;
        };
        let Some(content) = built.get_node_context(taffy_id) else {
            continue;
        };
        // The leaf's concatenated run text — the byte space `selection_rects` indexes.
        let text: String = content.runs.iter().map(|r| r.text.as_str()).collect();
        let hay = text.to_ascii_lowercase();
        let mut from = 0;
        while let Some(rel) = hay[from..].find(&needle_lc) {
            let start = from + rel;
            let end = start + needle_lc.len();
            let rects = selection_rects(dom, leaf, start, end, built, text_ctx, fragments);
            if !rects.is_empty() {
                out.push(rects);
            }
            from = end;
        }
    }
    out
}

/// Pre-order (document-order) walk collecting nodes that own a cached
/// `parley::Layout` — the inline-formatting leaves selection geometry addresses
/// (and, via [`crate::link_harvest`], the leaves an `<a href>`'s text flows in).
pub(crate) fn collect_text_leaves<D>(
    dom: &D,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    node: D::NodeId,
    out: &mut Vec<D::NodeId>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    if let Some(taffy_id) = built.node_map.get(&node) {
        if text_ctx.layouts.contains_key(taffy_id) {
            out.push(node);
        }
    }
    for child in dom.dom_children(node) {
        collect_text_leaves(dom, built, text_ctx, child, out);
    }
}

fn leaf_text<N>(leaf: N, built: &BoxTree<N>) -> Option<String>
where
    N: Copy + Eq + Hash,
{
    let taffy_id = built.node_map.get(&leaf)?;
    let content = built.get_node_context(*taffy_id)?;
    Some(content.runs.iter().map(|r| r.text.as_str()).collect())
}

fn block_ancestor<D>(dom: &D, node: D::NodeId) -> D::NodeId
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut cur = Some(node);
    while let Some(id) = cur {
        if let Some(name) = dom.element_name(id) {
            if !is_inline_name(name.local.as_ref()) {
                return id;
            }
        }
        cur = dom.parent(id);
    }
    node
}

fn is_inline_name(name: &str) -> bool {
    matches!(
        name,
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "cite"
            | "code"
            | "data"
            | "del"
            | "dfn"
            | "em"
            | "i"
            | "ins"
            | "kbd"
            | "label"
            | "mark"
            | "q"
            | "rp"
            | "rt"
            | "ruby"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "u"
            | "var"
            | "wbr"
    )
}

/// The caret byte after moving `delta` visual lines (−1 = up, +1 = down) from
/// `byte_offset` within `node`'s laid-out text, keeping a **sticky goal column** —
/// soft-wrap-aware ArrowUp / ArrowDown that does not drift toward short visual rows.
/// `None` if `node` has no cached layout.
///
/// Unlike the buffer's `\n`-counting navigation (which jumps whole hard lines), this
/// honours parley's *visual* line breaks: a long unwrapped paragraph the layout
/// wrapped across several rows moves one wrapped row at a time. At the first/last row
/// it lands at the buffer start/end.
///
/// `goal_x` is the horizontal target in the layout's local coordinate space: pass
/// `None` to seed it from the caret's current x, or the value returned by the previous
/// call to keep the column across a run of up/down presses (Tier 2). Returns
/// `(new_byte, goal_x)` — feed `goal_x` back on the next vertical move; reset it to
/// `None` on any horizontal move or edit. (parley's own `Selection::move_lines` keeps
/// this `h_pos` internally across a retained selection; we rebuild from a byte each
/// call, so we thread the goal through the caller instead.)
pub fn caret_byte_vertical<D>(
    node: D::NodeId,
    byte_offset: usize,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    delta: isize,
    goal_x: Option<f32>,
) -> Option<(usize, f32)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let taffy_id = built.node_map.get(&node)?;
    let layout = text_ctx.layouts.get(taffy_id)?;
    let cursor = Cursor::from_byte_index(layout, byte_offset, Affinity::default());
    // The caret's geometry gives both the row it sits on (its y) and the x to seed the
    // goal from when the caller has none.
    let geo = cursor.geometry(layout, 0.0);
    let h_pos = goal_x.unwrap_or(geo.x0 as f32);

    let line_count = layout.len();
    if line_count == 0 {
        return Some((byte_offset, h_pos));
    }
    // The visual row the caret sits on: the first whose block band reaches past the
    // caret's y (rows run top-to-bottom). Keys off the caret's actual y, so a soft-wrap
    // boundary resolves to the row the caret visually occupies, not just its byte.
    let y = geo.y0 as f32;
    let current = (0..line_count)
        .find(|&i| {
            layout
                .get(i)
                .is_some_and(|l| y < l.metrics().block_max_coord)
        })
        .unwrap_or(line_count - 1);

    let target = current as isize + delta;
    if target < 0 {
        return Some((0, h_pos)); // above the first row -> buffer start
    }
    if target as usize >= line_count {
        // below the last row -> the last visual row's end (buffer end)
        let end = layout
            .get(line_count - 1)
            .map_or(byte_offset, |l| l.text_range().end);
        return Some((end, h_pos));
    }
    // Place the caret at the goal x on the target row's text band; parley snaps the x to
    // the nearest cluster, clamping a too-wide goal to that row's end (the sticky goal is
    // preserved for the next move, not lost to the clamp).
    let line = layout.get(target as usize)?;
    let m = line.metrics();
    let ty = m.block_max_coord - m.ascent * 0.5;
    let moved = Cursor::from_point(layout, h_pos, ty);
    Some((moved.index(), h_pos))
}

/// The caret byte nearest the scene point `(x, y)` within `node`'s laid-out text,
/// or `None` if `node` has no cached text layout / fragment — the inverse of
/// [`caret_rect`]. Maps the point into the text layout's local space (subtracting
/// the node's absolute content-box origin) and asks parley which cluster boundary
/// it lands on. The `point → caret` primitive behind click-to-place-caret and the
/// start/extend of a mouse text-selection.
pub fn caret_byte_at_point<D>(
    dom: &D,
    node: D::NodeId,
    x: f32,
    y: f32,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    fragments: &FragmentPlane<D::NodeId>,
) -> Option<usize>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let taffy_id = built.node_map.get(&node)?;
    let layout = text_ctx.layouts.get(taffy_id)?;
    let (ox, oy) = absolute_origin(dom, fragments, node)?;
    let frame = fragments.rect_of(node)?;
    let local_x = x - (ox + frame.border.left + frame.padding.left);
    let local_y = y - (oy + frame.border.top + frame.padding.top);
    Some(Cursor::from_point(layout, local_x, local_y).index())
}

/// The caret bar's vertical extent `(top, height)` in layout space: from the
/// line's `ascent` down to its `baseline`. The top reaches the tops of ascenders
/// and capitals (the visible top of the text), so the caret does not sit below
/// ascender-heavy words like "shifted" and read as shifted-down — which a
/// cap-height top did, since lowercase ascenders rise above the cap height. The
/// bottom stops at the baseline rather than the descender, so it does not dangle
/// below descender-less words like "next?".
///
/// Mid-text the line comes from the cluster at `byte`; at end-of-text (no cluster
/// contains the final index) it falls back to the last line. `(0, 0)` for an
/// empty layout.
fn caret_band(layout: &Layout<ColorBrush>, byte: usize) -> (f32, f32) {
    let line = match Cluster::from_byte_index(layout, byte) {
        Some(c) => c.line(),
        None => match layout.get(layout.len().saturating_sub(1)) {
            Some(line) => line,
            None => return (0.0, 0.0),
        },
    };
    let m = line.metrics();
    (m.baseline - m.ascent, m.ascent)
}

/// Absolute border-box origin of `target` as an `(x, y)` tuple — a thin tuple-typed
/// wrapper over the canonical [`serval_lane::absolute_origin`](crate::serval_lane::absolute_origin)
/// (this module's own copy of the parent-chain walk is retired, upstreaming P2). `None` if
/// `target` is unreachable / unlaid-out. Shared with [`crate::link_harvest`] for placing an
/// inline replaced box (an image link) in absolute coordinates.
pub(crate) fn absolute_origin<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    target: D::NodeId,
) -> Option<(f32, f32)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    crate::serval_lane::absolute_origin(dom, fragments, target).map(|p| (p.x, p.y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::run_cascade;
    use crate::image_decode::ImagePlane;
    use crate::layout::layout;
    use crate::style::StylePlane;
    use layout_dom_api::LocalName;
    use serval_static_dom::{StaticDocument, StaticNodeId};

    fn find_p(doc: &StaticDocument) -> StaticNodeId {
        let mut q = vec![doc.document()];
        while let Some(id) = q.pop() {
            if doc
                .element_name(id)
                .is_some_and(|n| n.local == LocalName::from("p"))
            {
                return id;
            }
            q.extend(doc.dom_children(id));
        }
        panic!("no <p>")
    }

    /// `::selection { background-color }` is read back from the eager pseudo by
    /// `selection_style`; absent a rule it returns `None` so the host keeps its
    /// theme default. (Pseudo follow-ups §1.)
    #[test]
    fn selection_style_reads_selection_background() {
        let doc = StaticDocument::parse("<html><body><p>abc</p></body></html>");
        let p = find_p(&doc);

        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["p::selection { background-color: rgb(0, 255, 0); }"],
            None,
        );
        let (bg, _fg) = selection_style(&doc, &styles, p).expect("::selection background");
        assert!(
            bg[1] > 0.99 && bg[0] < 0.01,
            "::selection bg green, got {bg:?}"
        );

        let mut bare: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut bare,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );
        assert!(
            selection_style(&doc, &bare, p).is_none(),
            "no ::selection rule → None"
        );
    }

    /// The caret colour tracks the cascaded text `color` (`caret-color: auto`), so
    /// it stays legible on whatever theme the sheet paints the text in.
    #[test]
    fn caret_color_tracks_the_text_color() {
        let doc = StaticDocument::parse("<html><body><p>abc</p></body></html>");
        let p = find_p(&doc);

        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &["p { color: rgb(0, 0, 255); }"],
            None,
        );
        let c = caret_color(&doc, &styles, p).expect("a cascaded text colour");
        assert!(
            c[2] > 0.99 && c[0] < 0.01,
            "caret tracks the text colour (blue), got {c:?}"
        );
    }

    /// A selection range spanning two block paragraphs highlights text in both
    /// inline leaves: rects land in the first paragraph's band and in the second's
    /// (lower) band. Reversing anchor/focus (a backwards drag) yields the same
    /// rects, and a collapsed range yields none. (Pseudo follow-ups §3.)
    #[test]
    fn range_rects_span_two_paragraphs() {
        let doc =
            StaticDocument::parse("<html><body><p>first para</p><p>second para</p></body></html>");
        let sheet = &["html, body, p { display: block; margin: 0; font-size: 20px; }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);

        let mut ps = Vec::new();
        let mut q = vec![doc.document()];
        while let Some(id) = q.pop() {
            if doc
                .element_name(id)
                .is_some_and(|n| n.local == LocalName::from("p"))
            {
                ps.push(id);
            }
            q.extend(doc.dom_children(id));
        }
        assert_eq!(ps.len(), 2, "two paragraphs");
        let y0 = fragments.rect_of(ps[0]).unwrap().location.y;
        let y1 = fragments.rect_of(ps[1]).unwrap().location.y;
        let (top_p, bot_p) = if y0 <= y1 {
            (ps[0], ps[1])
        } else {
            (ps[1], ps[0])
        };
        let mid = (fragments.rect_of(top_p).unwrap().location.y
            + fragments.rect_of(bot_p).unwrap().location.y)
            / 2.0;

        let range = TextRange {
            anchor_node: top_p,
            anchor_offset: 2,
            focus_node: bot_p,
            focus_offset: 3,
        };
        let rects = range_rects(&doc, range, &built, &text_ctx, &fragments);
        assert!(!rects.is_empty(), "spanning range highlights something");
        assert!(
            rects.iter().any(|r| r.y < mid),
            "highlight in the first paragraph"
        );
        assert!(
            rects.iter().any(|r| r.y >= mid),
            "highlight in the second paragraph"
        );

        // A backwards drag (anchor after focus in document order) is normalised
        // to the same span.
        let reversed = TextRange {
            anchor_node: bot_p,
            anchor_offset: 3,
            focus_node: top_p,
            focus_offset: 2,
        };
        let rev_rects = range_rects(&doc, reversed, &built, &text_ctx, &fragments);
        assert_eq!(rev_rects, rects, "backwards drag highlights the same rects");

        // A collapsed range (same leaf, same offset) highlights nothing.
        let collapsed = TextRange {
            anchor_node: top_p,
            anchor_offset: 2,
            focus_node: top_p,
            focus_offset: 2,
        };
        assert!(
            range_rects(&doc, collapsed, &built, &text_ctx, &fragments).is_empty(),
            "collapsed range → no rects"
        );
    }

    /// `find_text_rects` locates every (case-insensitive) occurrence of a needle in
    /// the laid-out text, in document order, each as a positive-area highlight rect;
    /// an absent or empty needle yields nothing.
    #[test]
    fn find_text_rects_locates_case_insensitive_matches() {
        let doc = StaticDocument::parse(
            "<html><body><p>The quick brown fox.</p><p>Another Fox here.</p></body></html>",
        );
        let sheet = &["html, body, p { display: block; margin: 0; font-size: 20px; }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);

        // "fox" / "Fox" — one per paragraph, matched case-insensitively.
        let matches = find_text_rects(&doc, &built, &text_ctx, &fragments, "FOX");
        assert_eq!(
            matches.len(),
            2,
            "two case-insensitive 'fox' matches, got {matches:?}"
        );
        for m in &matches {
            let r = m.first().expect("each match has a rect");
            assert!(
                r.width > 0.0 && r.height > 0.0,
                "positive-area highlight, got {r:?}"
            );
        }
        // Document order: the second match (second paragraph) sits below the first.
        assert!(
            matches[1][0].y > matches[0][0].y,
            "second match is below the first"
        );

        assert!(
            find_text_rects(&doc, &built, &text_ctx, &fragments, "zebra").is_empty(),
            "an absent needle finds nothing"
        );
        assert!(
            find_text_rects(&doc, &built, &text_ctx, &fragments, "").is_empty(),
            "an empty needle finds nothing"
        );
    }

    /// Padded inline text and the caret share the content-box origin: the
    /// emitted glyphs are inset by `border + padding` (so the text is actually
    /// padded), and the byte-0 caret coincides with the first glyph. Guards the
    /// fix for a padded field drawing its caret a padding-width right of the text
    /// (emit had painted glyphs from the border box while `caret_rect` used the
    /// content box).
    #[test]
    fn padded_text_and_caret_share_origin() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><p>abc</p></body></html>");
        let sheet = &["html, body, p { display: block; margin: 0; border: 0; \
            font-size: 40px; } p { padding-left: 30px; }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let p = find_p(&doc);

        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        // The first painted glyph's x (the `<p>` sits at absolute (0,0) with no
        // margin, so its emit-local x is its absolute x).
        let glyph_x = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawText(t) if !t.glyphs.is_empty() => Some(t.glyphs[0].point.x),
                _ => None,
            })
            .expect("a glyph run");
        let caret0 = caret_rect(&doc, p, 0, &built, &text_ctx, &fragments, 2.0)
            .unwrap()
            .x;

        assert!(
            glyph_x >= 25.0,
            "glyphs inset by ~padding (30 px), got {glyph_x}"
        );
        assert!(
            (glyph_x - caret0).abs() < 1.0,
            "first glyph x ({glyph_x}) coincides with the byte-0 caret x ({caret0})"
        );
    }

    /// A `background-image: linear-gradient(...)` emits a `DrawLinearGradient`
    /// with the stops resolved to 0..1 offsets and the gradient line oriented per
    /// the direction (`to bottom` = top-to-bottom, vertical).
    #[test]
    fn linear_gradient_background_emits_a_gradient_fill() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><div></div></body></html>");
        let sheet = &["html, body, div { display: block; margin: 0; border: 0; } \
            div { width: 100px; height: 50px; \
            background-image: linear-gradient(to bottom, rgb(255, 0, 0), rgb(0, 0, 255)); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let grad = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawLinearGradient(g) => Some(g),
                _ => None,
            })
            .expect("a linear-gradient background emits a DrawLinearGradient");
        assert_eq!(grad.gradient.stops.len(), 2, "two color stops");
        assert!(
            (grad.gradient.stops[0].offset - 0.0).abs() < 1e-3,
            "first stop at 0"
        );
        assert!(
            (grad.gradient.stops[1].offset - 1.0).abs() < 1e-3,
            "last stop at 1"
        );
        // `to bottom`: the line runs top (start) to bottom (end), vertical.
        assert!(
            grad.gradient.start_point.y < grad.gradient.end_point.y,
            "to bottom: start above end"
        );
        assert!(
            (grad.gradient.start_point.x - grad.gradient.end_point.x).abs() < 1e-3,
            "vertical line (x constant)"
        );
    }

    /// A default `radial-gradient(...)` is a centered farthest-corner ellipse:
    /// the center sits at the box midpoint and the radii are the farthest-side
    /// distances scaled by sqrt(2) (unequal on a non-square box -> an ellipse).
    #[test]
    fn radial_gradient_default_is_centered_ellipse() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><div></div></body></html>");
        let sheet = &["html, body, div { display: block; margin: 0; border: 0; } \
            div { width: 100px; height: 50px; \
            background-image: radial-gradient(rgb(255, 0, 0), rgb(0, 0, 255)); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let grad = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawRadialGradient(g) => Some(g),
                _ => None,
            })
            .expect("a radial-gradient background emits a DrawRadialGradient");
        assert!(
            (grad.gradient.center.x - 50.0).abs() < 1e-3,
            "center x at box midpoint"
        );
        assert!(
            (grad.gradient.center.y - 25.0).abs() < 1e-3,
            "center y at box midpoint"
        );
        let sqrt2 = std::f32::consts::SQRT_2;
        assert!(
            (grad.gradient.radius.width - 50.0 * sqrt2).abs() < 1e-2,
            "rx = farthest-side x * sqrt(2)"
        );
        assert!(
            (grad.gradient.radius.height - 25.0 * sqrt2).abs() < 1e-2,
            "ry = farthest-side y * sqrt(2)"
        );
        assert_eq!(grad.gradient.stops.len(), 2, "two color stops");
        assert!(
            (grad.gradient.stops[0].offset - 0.0).abs() < 1e-3,
            "first stop at 0"
        );
        assert!(
            (grad.gradient.stops[1].offset - 1.0).abs() < 1e-3,
            "last stop at 1"
        );
    }

    /// `radial-gradient(circle <r> at <x> <y>, ...)` emits equal radii (a circle)
    /// centered at the explicit position.
    #[test]
    fn radial_gradient_circle_uses_explicit_radius_and_position() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><div></div></body></html>");
        let sheet = &["html, body, div { display: block; margin: 0; border: 0; } \
            div { width: 100px; height: 50px; \
            background-image: radial-gradient(circle 40px at 30px 10px, \
            rgb(255, 0, 0), rgb(0, 0, 255)); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let grad = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawRadialGradient(g) => Some(g),
                _ => None,
            })
            .expect("a radial circle gradient emits a DrawRadialGradient");
        assert!((grad.gradient.center.x - 30.0).abs() < 1e-3, "center x");
        assert!((grad.gradient.center.y - 10.0).abs() < 1e-3, "center y");
        assert!(
            (grad.gradient.radius.width - 40.0).abs() < 1e-3,
            "circle rx"
        );
        assert!(
            (grad.gradient.radius.height - grad.gradient.radius.width).abs() < 1e-3,
            "circle ry = rx"
        );
    }

    /// A default `conic-gradient(...)` centers on the box and starts its seam at
    /// the top: the renderer's sweep is 0 at the +x axis, so the emitted start
    /// angle is rotated back a quarter turn (-pi/2).
    #[test]
    fn conic_gradient_default_starts_at_top() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><div></div></body></html>");
        let sheet = &["html, body, div { display: block; margin: 0; border: 0; } \
            div { width: 100px; height: 50px; \
            background-image: conic-gradient(rgb(255, 0, 0), rgb(0, 0, 255)); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let grad = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawConicGradient(g) => Some(g),
                _ => None,
            })
            .expect("a conic-gradient background emits a DrawConicGradient");
        assert!(
            (grad.gradient.center.x - 50.0).abs() < 1e-3,
            "center x at box midpoint"
        );
        assert!(
            (grad.gradient.center.y - 25.0).abs() < 1e-3,
            "center y at box midpoint"
        );
        assert!(
            (grad.gradient.angle - (-std::f32::consts::FRAC_PI_2)).abs() < 1e-3,
            "default seam (from 0deg) rotated to the top: start angle -pi/2"
        );
        assert_eq!(grad.gradient.stops.len(), 2, "two color stops");
    }

    /// `conic-gradient(from <a> at <x> <y>, c1 0deg, c2 90deg, c3 360deg)` puts
    /// the angular stops at 0, 0.25, and 1.0 of the turn, centered at the position,
    /// with `from 90deg` cancelling the top-rotation to a 0 start angle.
    #[test]
    fn conic_gradient_angular_stops_and_from_angle() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><div></div></body></html>");
        let sheet = &["html, body, div { display: block; margin: 0; border: 0; } \
            div { width: 100px; height: 50px; \
            background-image: conic-gradient(from 90deg at 10px 20px, \
            rgb(255, 0, 0) 0deg, rgb(0, 255, 0) 90deg, rgb(0, 0, 255) 360deg); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let grad = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawConicGradient(g) => Some(g),
                _ => None,
            })
            .expect("a conic gradient emits a DrawConicGradient");
        assert!((grad.gradient.center.x - 10.0).abs() < 1e-3, "center x");
        assert!((grad.gradient.center.y - 20.0).abs() < 1e-3, "center y");
        assert!(
            grad.gradient.angle.abs() < 1e-3,
            "from 90deg - 90deg top-rotation = 0 start angle"
        );
        assert_eq!(grad.gradient.stops.len(), 3, "three color stops");
        assert!(
            (grad.gradient.stops[0].offset - 0.0).abs() < 1e-3,
            "0deg -> 0.0"
        );
        assert!(
            (grad.gradient.stops[1].offset - 0.25).abs() < 1e-3,
            "90deg -> 0.25"
        );
        assert!(
            (grad.gradient.stops[2].offset - 1.0).abs() < 1e-3,
            "360deg -> 1.0"
        );
    }

    /// `text-decoration: line-through` emits a thin decoration rect through the
    /// text middle, above where the same text's underline would sit; a plain run
    /// emits none.
    #[test]
    fn line_through_sits_above_the_underline() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        // Top y of the single thin decoration rect a `<p>` with the given
        // `text-decoration` emits (the other DrawRects are full-height background
        // boxes), or None when there is no decoration.
        let decoration_y = |decoration: &str| -> Option<f32> {
            let doc = StaticDocument::parse("<html><body><p>strike</p></body></html>");
            let css = format!(
                "html, body, p {{ display: block; margin: 0; }} \
                 p {{ font-size: 40px; {decoration} }}"
            );
            let sheet: &[&str] = &[css.as_str()];
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &doc,
                &mut styles,
                euclid::Size2D::new(800.0, 600.0),
                sheet,
                None,
            );
            let viewport = taffy::Size {
                width: taffy::AvailableSpace::Definite(800.0),
                height: taffy::AvailableSpace::Definite(600.0),
            };
            let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
            let scroll = FxHashMap::default();
            let plist = emit_paint_list_with_layouts(
                &doc,
                &styles,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &BackgroundImagePlane::new(),
                &scroll,
                DeviceIntSize::new(800, 600),
            );
            plist.commands().iter().find_map(|c| match c {
                PaintCmd::DrawRect(r) => {
                    let b = &r.placement.bounds;
                    (b.height() > 0.0 && b.height() < 10.0).then_some(b.min.y)
                },
                _ => None,
            })
        };

        let strike_y =
            decoration_y("text-decoration: line-through;").expect("line-through emits a rect");
        let underline_y =
            decoration_y("text-decoration: underline;").expect("underline emits a rect");
        assert!(
            strike_y < underline_y,
            "line-through ({strike_y}) sits above the underline ({underline_y})"
        );
        let overline_y = decoration_y("text-decoration: overline;").expect("overline emits a rect");
        assert!(
            overline_y < strike_y,
            "overline ({overline_y}) sits above line-through ({strike_y})"
        );
        assert!(decoration_y("").is_none(), "no decoration -> no thin rect");
    }

    /// `text-decoration-color` colors the decoration independently of the text:
    /// blue text with a red underline emits a red decoration rect over blue glyphs.
    #[test]
    fn text_decoration_color_is_independent_of_text_color() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><p>link</p></body></html>");
        let sheet = &["html, body, p { display: block; margin: 0; } \
            p { font-size: 40px; color: rgb(0, 0, 255); \
            text-decoration: underline; text-decoration-color: rgb(255, 0, 0); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        // The thin DrawRect (height << font size) is the underline decoration.
        let deco = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawRect(r) => {
                    let b = &r.placement.bounds;
                    (b.height() > 0.0 && b.height() < 10.0).then_some(r.color)
                },
                _ => None,
            })
            .expect("underline emits a decoration rect");
        assert!(
            deco.r > 0.9 && deco.b < 0.1,
            "decoration is red, got {deco:?}"
        );
        // The glyph text stays blue.
        let text = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawText(t) if !t.glyphs.is_empty() => Some(t.color),
                _ => None,
            })
            .expect("text emits glyphs");
        assert!(text.b > 0.9 && text.r < 0.1, "text is blue, got {text:?}");
    }

    /// Multiple background-image layers all paint, back-to-front: CSS lists the
    /// topmost layer first, so `linear-gradient(...), radial-gradient(...)` emits
    /// the radial (bottom) then the linear (top).
    #[test]
    fn multiple_background_gradient_layers_paint_back_to_front() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><div></div></body></html>");
        let sheet = &["html, body, div { display: block; margin: 0; border: 0; } \
            div { width: 100px; height: 50px; background-image: \
            linear-gradient(rgb(255, 0, 0), rgb(0, 0, 255)), \
            radial-gradient(rgb(0, 255, 0), rgb(0, 0, 0)); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let grads: Vec<&str> = plist
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawLinearGradient(_) => Some("linear"),
                PaintCmd::DrawRadialGradient(_) => Some("radial"),
                PaintCmd::DrawConicGradient(_) => Some("conic"),
                _ => None,
            })
            .collect();
        assert_eq!(
            grads,
            vec!["radial", "linear"],
            "back-to-front: radial (bottom layer) then linear (top layer)"
        );
    }

    /// `repeating-linear-gradient` emits a Repeat gradient whose endpoints span
    /// one period (the first→last stop distance) rather than the whole box, with
    /// the stops re-normalized to 0..1.
    #[test]
    fn repeating_linear_gradient_spans_one_period() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, ExtendMode, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><div></div></body></html>");
        let sheet = &["html, body, div { display: block; margin: 0; border: 0; } \
            div { width: 100px; height: 50px; background-image: \
            repeating-linear-gradient(to right, rgb(255, 0, 0), rgb(0, 0, 255) 20px); }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let grad = plist
            .commands()
            .iter()
            .find_map(|c| match c {
                PaintCmd::DrawLinearGradient(g) => Some(g),
                _ => None,
            })
            .expect("a repeating-linear-gradient emits a DrawLinearGradient");
        assert!(
            matches!(grad.gradient.extend_mode, ExtendMode::Repeat),
            "repeating gradient uses ExtendMode::Repeat"
        );
        let span = (grad.gradient.end_point.x - grad.gradient.start_point.x).abs();
        assert!(
            (span - 20.0).abs() < 1.0,
            "endpoints span one 20px period, got {span}"
        );
        assert!(
            (grad.gradient.stops.first().unwrap().offset - 0.0).abs() < 1e-3,
            "first stop renormalized to 0"
        );
        assert!(
            (grad.gradient.stops.last().unwrap().offset - 1.0).abs() < 1e-3,
            "last stop renormalized to 1"
        );
    }

    /// An unordered-list item gets a bullet marker hanging to the left of its
    /// content box (negative local x), emitted as its own DrawText.
    #[test]
    fn unordered_list_item_marker_hangs_left() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><ul><li>Item</li></ul></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let hanging = plist.commands().iter().any(|c| {
            matches!(c, PaintCmd::DrawText(t)
                if !t.glyphs.is_empty() && t.placement.bounds.min.x < 0.0)
        });
        assert!(
            hanging,
            "unordered list item emits a marker hanging left (x < 0)"
        );
    }

    /// Ordered-list items get distinct ordinal markers (`1.`, `2.`), each hanging
    /// left of its content box — so the two markers' glyph sequences differ.
    #[test]
    fn ordered_list_markers_differ_per_item() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><ol><li>A</li><li>B</li></ol></body></html>");
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            &[],
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let markers: Vec<Vec<u32>> = plist
            .commands()
            .iter()
            .filter_map(|c| match c {
                PaintCmd::DrawText(t) if t.placement.bounds.min.x < 0.0 && !t.glyphs.is_empty() => {
                    Some(t.glyphs.iter().map(|g| g.index).collect())
                },
                _ => None,
            })
            .collect();
        assert_eq!(markers.len(), 2, "two list items -> two hanging markers");
        assert_ne!(
            markers[0], markers[1],
            "ordinals `1.` and `2.` differ in glyphs"
        );
    }

    /// `list-style-type` selects the marker: `decimal`, `lower-alpha`, and
    /// `square` produce visibly different first markers, and `none` suppresses
    /// the marker entirely.
    #[test]
    fn list_style_type_selects_the_marker() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        // Glyphs of the first hanging marker (x < 0) for an `<ol>` whose
        // `list-style-type` is set by the given rule; empty if no marker.
        let first_marker = |decl: &str| -> Vec<u32> {
            let doc = StaticDocument::parse("<html><body><ol><li>x</li></ol></body></html>");
            let sheet: &[&str] = &[decl];
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &doc,
                &mut styles,
                euclid::Size2D::new(800.0, 600.0),
                sheet,
                None,
            );
            let viewport = taffy::Size {
                width: taffy::AvailableSpace::Definite(800.0),
                height: taffy::AvailableSpace::Definite(600.0),
            };
            let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
            let scroll = FxHashMap::default();
            let plist = emit_paint_list_with_layouts(
                &doc,
                &styles,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &BackgroundImagePlane::new(),
                &scroll,
                DeviceIntSize::new(800, 600),
            );
            plist
                .commands()
                .iter()
                .find_map(|c| match c {
                    PaintCmd::DrawText(t)
                        if t.placement.bounds.min.x < 0.0 && !t.glyphs.is_empty() =>
                    {
                        Some(t.glyphs.iter().map(|g| g.index).collect())
                    },
                    _ => None,
                })
                .unwrap_or_default()
        };

        let decimal = first_marker("ol { list-style-type: decimal; }");
        let alpha = first_marker("ol { list-style-type: lower-alpha; }");
        let square = first_marker("ol { list-style-type: square; }");
        let none = first_marker("ol { list-style-type: none; }");

        assert!(!decimal.is_empty(), "decimal emits a marker");
        assert_ne!(decimal, alpha, "decimal `1.` differs from lower-alpha `a.`");
        assert_ne!(decimal, square, "decimal differs from the square bullet");
        assert!(none.is_empty(), "list-style-type: none emits no marker");
    }

    /// A `display: none` list item paints nothing, including no hanging marker
    /// (a marker sits outside the box, so a zero-size hidden box would otherwise
    /// leak it).
    #[test]
    fn display_none_list_item_emits_no_marker() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let doc = StaticDocument::parse("<html><body><ul><li>x</li></ul></body></html>");
        let sheet = &["li { display: none; }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
        let scroll = FxHashMap::default();
        let plist = emit_paint_list_with_layouts(
            &doc,
            &styles,
            &fragments,
            &built,
            &text_ctx,
            &ImagePlane::new(),
            &BackgroundImagePlane::new(),
            &scroll,
            DeviceIntSize::new(800, 600),
        );
        let has_marker = plist.commands().iter().any(|c| {
            matches!(c, PaintCmd::DrawText(t)
                if !t.glyphs.is_empty() && t.placement.bounds.min.x < 0.0)
        });
        assert!(
            !has_marker,
            "display:none list item must not paint a marker"
        );
    }

    /// `<ol start>` and `<li value>` offset the ordinals (HTML's counting): a
    /// `start="5"` list begins at `5.`, and a `<li value="10">` resets the
    /// counter so the items read `1.`, `10.`, `11.`. Verified structurally by
    /// matching marker glyphs against the corresponding positions of a plain list.
    #[test]
    fn ol_start_and_li_value_offset_the_ordinals() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let all_markers = |html: &str| -> Vec<Vec<u32>> {
            let doc = StaticDocument::parse(html);
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &doc,
                &mut styles,
                euclid::Size2D::new(800.0, 600.0),
                &[],
                None,
            );
            let viewport = taffy::Size {
                width: taffy::AvailableSpace::Definite(800.0),
                height: taffy::AvailableSpace::Definite(600.0),
            };
            let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
            let scroll = FxHashMap::default();
            let plist = emit_paint_list_with_layouts(
                &doc,
                &styles,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &BackgroundImagePlane::new(),
                &scroll,
                DeviceIntSize::new(800, 600),
            );
            plist
                .commands()
                .iter()
                .filter_map(|c| match c {
                    PaintCmd::DrawText(t)
                        if t.placement.bounds.min.x < 0.0 && !t.glyphs.is_empty() =>
                    {
                        Some(t.glyphs.iter().map(|g| g.index).collect())
                    },
                    _ => None,
                })
                .collect()
        };

        // A plain list: marker[n] renders "(n+1).".
        let plain = all_markers(
            "<html><body><ol><li>a</li><li>b</li><li>c</li><li>d</li><li>e</li>\
             <li>f</li><li>g</li><li>h</li><li>i</li><li>j</li><li>k</li></ol></body></html>",
        );
        assert_eq!(plain.len(), 11, "plain list emits 11 markers");

        // `start="5"`: the single item renders "5." == plain's 5th marker.
        let started = all_markers("<html><body><ol start=\"5\"><li>x</li></ol></body></html>");
        assert_eq!(started.len(), 1);
        assert_eq!(started[0], plain[4], "start=5 first marker == plain `5.`");

        // `<li value="10">` resets the counter: items read 1., 10., 11.
        let valued = all_markers(
            "<html><body><ol><li>a</li><li value=\"10\">b</li><li>c</li></ol></body></html>",
        );
        assert_eq!(valued.len(), 3);
        assert_eq!(valued[0], plain[0], "first item `1.`");
        assert_eq!(valued[1], plain[9], "value=10 item `10.`");
        assert_eq!(valued[2], plain[10], "item after value=10 `11.`");
    }

    /// `<ol reversed>` counts the items down: a three-item list reads `3.`, `2.`,
    /// `1.` (the reverse of a plain list's first three markers).
    #[test]
    fn ol_reversed_counts_down() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        let all_markers = |html: &str| -> Vec<Vec<u32>> {
            let doc = StaticDocument::parse(html);
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &doc,
                &mut styles,
                euclid::Size2D::new(800.0, 600.0),
                &[],
                None,
            );
            let viewport = taffy::Size {
                width: taffy::AvailableSpace::Definite(800.0),
                height: taffy::AvailableSpace::Definite(600.0),
            };
            let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
            let scroll = FxHashMap::default();
            let plist = emit_paint_list_with_layouts(
                &doc,
                &styles,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &BackgroundImagePlane::new(),
                &scroll,
                DeviceIntSize::new(800, 600),
            );
            plist
                .commands()
                .iter()
                .filter_map(|c| match c {
                    PaintCmd::DrawText(t)
                        if t.placement.bounds.min.x < 0.0 && !t.glyphs.is_empty() =>
                    {
                        Some(t.glyphs.iter().map(|g| g.index).collect())
                    },
                    _ => None,
                })
                .collect()
        };

        let plain =
            all_markers("<html><body><ol><li>a</li><li>b</li><li>c</li></ol></body></html>");
        let rev = all_markers(
            "<html><body><ol reversed><li>a</li><li>b</li><li>c</li></ol></body></html>",
        );
        assert_eq!(rev.len(), 3, "three reversed markers");
        assert_eq!(rev[0], plain[2], "first item counts down from 3 (`3.`)");
        assert_eq!(rev[1], plain[1], "`2.`");
        assert_eq!(rev[2], plain[0], "`1.`");
    }

    /// `list-style-position: inside` flows the marker into the item's inline
    /// content (no hanging marker, and more inline glyphs since the bullet is
    /// prepended), where the default `outside` hangs it to the left.
    #[test]
    fn list_style_position_inside_flows_marker_inline() {
        use crate::image_decode::BackgroundImagePlane;
        use crate::paint_emit::emit_paint_list_with_layouts;
        use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
        use rustc_hash::FxHashMap;

        // (count of hanging markers at x < 0, total glyphs of inline text at x >= 0)
        let analyze = |sheet: &[&str]| -> (usize, usize) {
            let doc = StaticDocument::parse("<html><body><ul><li>Item</li></ul></body></html>");
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &doc,
                &mut styles,
                euclid::Size2D::new(800.0, 600.0),
                sheet,
                None,
            );
            let viewport = taffy::Size {
                width: taffy::AvailableSpace::Definite(800.0),
                height: taffy::AvailableSpace::Definite(600.0),
            };
            let (fragments, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
            let scroll = FxHashMap::default();
            let plist = emit_paint_list_with_layouts(
                &doc,
                &styles,
                &fragments,
                &built,
                &text_ctx,
                &ImagePlane::new(),
                &BackgroundImagePlane::new(),
                &scroll,
                DeviceIntSize::new(800, 600),
            );
            let hanging = plist
                .commands()
                .iter()
                .filter(|c| {
                    matches!(c, PaintCmd::DrawText(t)
                        if !t.glyphs.is_empty() && t.placement.bounds.min.x < 0.0)
                })
                .count();
            let inline_glyphs: usize = plist
                .commands()
                .iter()
                .filter_map(|c| match c {
                    PaintCmd::DrawText(t) if t.placement.bounds.min.x >= 0.0 => {
                        Some(t.glyphs.len())
                    },
                    _ => None,
                })
                .sum();
            (hanging, inline_glyphs)
        };

        let outside = analyze(&[]);
        let inside = analyze(&["li { list-style-position: inside; }"]);
        assert_eq!(outside.0, 1, "outside (default): one hanging marker");
        assert_eq!(inside.0, 0, "inside: no hanging marker");
        assert!(
            inside.1 > outside.1,
            "inside prepends the marker inline -> more inline glyphs ({} vs {})",
            inside.1,
            outside.1
        );
    }

    /// A cascaded `line-height` controls the line-box height: `line-height: 2`
    /// on 40px text gives a ~80px line box (2 × font-size), vs the ~46px
    /// font-metric default. Verifies the cascade → parley line-height plumbing
    /// (`construct::line_height_of` → `StyleProperty::LineHeight`).
    #[test]
    fn css_line_height_controls_line_box() {
        let line_box_height = |sheet: &[&str]| {
            let doc = StaticDocument::parse("<html><body><p>yep</p></body></html>");
            let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
            run_cascade(
                &doc,
                &mut styles,
                euclid::Size2D::new(800.0, 600.0),
                sheet,
                None,
            );
            let viewport = taffy::Size {
                width: taffy::AvailableSpace::Definite(800.0),
                height: taffy::AvailableSpace::Definite(600.0),
            };
            let (_f, built, text_ctx) = layout(&doc, &styles, &ImagePlane::new(), viewport);
            let p = find_p(&doc);
            let taffy_id = built.node_map.get(&p).expect("taffy id");
            text_ctx.layouts.get(taffy_id).expect("layout").height()
        };

        let normal =
            line_box_height(&["html, body, p { display: block; margin: 0; font-size: 40px; }"]);
        let factor = line_box_height(&[
            "html, body, p { display: block; margin: 0; font-size: 40px; line-height: 2; }",
        ]);
        let absolute = line_box_height(&[
            "html, body, p { display: block; margin: 0; font-size: 40px; line-height: 70px; }",
        ]);

        assert!(
            (factor - 80.0).abs() < 1.0,
            "line-height:2 → ~80px line box, got {factor}"
        );
        assert!(
            (absolute - 70.0).abs() < 1.0,
            "line-height:70px → ~70px line box, got {absolute}"
        );
        assert!(
            factor > normal + 20.0,
            "line-height:2 ({factor}) is taller than normal ({normal})"
        );
    }

    /// The caret advances along the text: at offset 0 it sits at the content
    /// left; at the end of "abc" it sits further right (≈ text width). No padding
    /// keeps the content origin at the box origin so the assertion is on the
    /// glyph-advance geometry, not insets.
    #[test]
    fn caret_advances_with_offset() {
        let doc = StaticDocument::parse("<html><body><p>abc</p></body></html>");
        let sheet = &["html, body, p { display: block; margin: 0; padding: 0; border: 0; }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let images = ImagePlane::new();
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &images, viewport);
        let p = find_p(&doc);

        let at0 =
            caret_rect(&doc, p, 0, &built, &text_ctx, &fragments, 2.0).expect("caret at offset 0");
        let at3 = caret_rect(&doc, p, 3, &built, &text_ctx, &fragments, 2.0)
            .expect("caret at end of 'abc'");

        // Positive height (a line-tall bar) and the requested thickness.
        assert!(at0.height > 0.0, "caret has height: {at0:?}");
        assert!(
            (at0.width - 2.0).abs() < 0.01,
            "caret width is the thickness"
        );
        // The caret moves right as the offset grows past the glyphs.
        assert!(
            at3.x > at0.x,
            "caret at end ({}) is right of caret at start ({})",
            at3.x,
            at0.x
        );
        // Same line: y unchanged.
        assert!((at3.y - at0.y).abs() < 0.01, "single line: y constant");

        // An offset on a node with no cached text layout is None.
        let body = doc.document(); // document root: no text layout
        assert!(caret_rect(&doc, body, 0, &built, &text_ctx, &fragments, 2.0).is_none());
    }

    /// A non-collapsed selection over the text yields highlight rects with
    /// positive width; a collapsed range yields none.
    #[test]
    fn selection_covers_range() {
        let doc = StaticDocument::parse("<html><body><p>abc</p></body></html>");
        let sheet = &["html, body, p { display: block; margin: 0; padding: 0; border: 0; }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let images = ImagePlane::new();
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &images, viewport);
        let p = find_p(&doc);

        // Select all of "abc" (bytes 0..3, all ASCII).
        let rects = selection_rects(&doc, p, 0, 3, &built, &text_ctx, &fragments);
        assert!(!rects.is_empty(), "a non-empty selection produces rects");
        let total_width: f32 = rects.iter().map(|r| r.width).sum();
        assert!(total_width > 0.0, "selection has positive width: {rects:?}");

        // A collapsed range selects nothing.
        assert!(selection_rects(&doc, p, 1, 1, &built, &text_ctx, &fragments).is_empty());
    }

    /// Soft-wrap navigation and point hit-testing operate on *visual* lines: a
    /// narrow `<p>` wraps a space-separated run (no `\n`) across rows; moving down
    /// then up returns to the first row, and a click on a wrapped row resolves to
    /// a caret on that row.
    #[test]
    fn caret_navigates_visual_lines_and_points() {
        // Each 4-char word is far wider than the 20px width at the default font,
        // so parley puts one word per visual line — four rows, no `\n`.
        let doc = StaticDocument::parse("<html><body><p>aaaa bbbb cccc dddd</p></body></html>");
        let sheet = &[
            "html, body, p { display: block; margin: 0; padding: 0; border: 0; }",
            "p { width: 20px; }",
        ];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let images = ImagePlane::new();
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &images, viewport);
        let p = find_p(&doc);
        let rect_at = |byte| caret_rect(&doc, p, byte, &built, &text_ctx, &fragments, 2.0).unwrap();

        // Sanity: the run wrapped (the caret at the end sits below the start).
        let start = rect_at(0);
        assert!(
            rect_at(19).y > start.y,
            "text wrapped to multiple visual lines"
        );

        // Down one visual line lands on a later row (greater y) at a byte past
        // the first wrapped word.
        let (down, goal) =
            caret_byte_vertical::<StaticDocument>(p, 0, &built, &text_ctx, 1, None).unwrap();
        assert!(down > 0, "down moved off byte 0: {down}");
        assert!(
            rect_at(down).y > start.y,
            "down moved to a lower visual line"
        );

        // Up from there, feeding the goal back, returns to the first row.
        let (up, _) =
            caret_byte_vertical::<StaticDocument>(p, down, &built, &text_ctx, -1, Some(goal))
                .unwrap();
        assert!(
            (rect_at(up).y - start.y).abs() < 0.5,
            "up returned to the first row"
        );

        // A click on the wrapped row resolves to a caret on that same row.
        let down_rect = rect_at(down);
        let hit = caret_byte_at_point(
            &doc,
            p,
            down_rect.x + 1.0,
            down_rect.y + down_rect.height * 0.5,
            &built,
            &text_ctx,
            &fragments,
        )
        .unwrap();
        assert!(
            (rect_at(hit).y - down_rect.y).abs() < 0.5,
            "click maps to the clicked row"
        );

        // A node with no cached text layout yields None for both.
        let root = doc.document();
        assert!(
            caret_byte_vertical::<StaticDocument>(root, 0, &built, &text_ctx, 1, None).is_none()
        );
        assert!(caret_byte_at_point(&doc, root, 1.0, 1.0, &built, &text_ctx, &fragments).is_none());
    }

    /// The sticky goal column (Tier 2): moving the caret down from a long visual row
    /// through a SHORT row and on to another long row returns it to ~its original
    /// column — the goal x, fed back each call, survives the short row's clamp instead
    /// of drifting left.
    #[test]
    fn caret_vertical_keeps_a_sticky_goal_column() {
        // Each word far exceeds 20px, so parley puts one per row: "aaaaaaaa" / "bb" /
        // "cccccccc" — long, short, long.
        let doc = StaticDocument::parse("<html><body><p>aaaaaaaa bb cccccccc</p></body></html>");
        let sheet = &[
            "html, body, p { display: block; margin: 0; padding: 0; border: 0; }",
            "p { width: 20px; }",
        ];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(
            &doc,
            &mut styles,
            euclid::Size2D::new(800.0, 600.0),
            sheet,
            None,
        );
        let images = ImagePlane::new();
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &images, viewport);
        let p = find_p(&doc);
        let rect_at = |byte| caret_rect(&doc, p, byte, &built, &text_ctx, &fragments, 2.0).unwrap();

        // Start at the end of the long first row (after "aaaaaaaa", byte 8): a rightward x.
        let start_x = rect_at(8).x;
        // Down into the short row: the goal seeds from the rightward start (returned), and
        // the caret clamps onto the short row at a smaller x.
        let (on_short, goal) =
            caret_byte_vertical::<StaticDocument>(p, 8, &built, &text_ctx, 1, None).unwrap();
        assert!(
            rect_at(on_short).x < start_x - 5.0,
            "the short row clamps the caret leftward"
        );
        // Down again into the long row, feeding the goal back: the caret returns to ~its
        // original column, well right of where the short row clamped it.
        let (on_long, goal2) =
            caret_byte_vertical::<StaticDocument>(p, on_short, &built, &text_ctx, 1, Some(goal))
                .unwrap();
        assert!(
            (goal2 - goal).abs() < 0.5,
            "the goal x is stable across the run: {goal} vs {goal2}"
        );
        assert!(
            rect_at(on_long).x > rect_at(on_short).x + 5.0,
            "the sticky goal returns the caret rightward on the long row, not stuck at the short row's column",
        );
    }
}
