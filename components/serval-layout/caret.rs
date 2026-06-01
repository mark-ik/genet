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
use crate::text_measure::{ColorBrush, TextMeasureCtx};

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

/// The caret byte after moving `delta` visual lines (−1 = up, +1 = down) from
/// `byte_offset` within `node`'s laid-out text, keeping the horizontal position
/// — soft-wrap-aware ArrowUp / ArrowDown. `None` if `node` has no cached layout.
///
/// Unlike the buffer's `\n`-counting navigation (which jumps whole hard lines),
/// this honours parley's *visual* line breaks: a long unwrapped paragraph that
/// the layout wrapped across several rows moves one wrapped row at a time. At the
/// first/last line it lands at the line start/end. parley clamps a too-wide
/// horizontal position to the target line's end.
///
/// Tier 1: a fresh [`Selection`] is built each call, so the goal column is the
/// caret's current x — there is no sticky goal column preserved across a run of
/// up/down presses (matching the hard-line navigation in `TextInput`).
pub fn caret_byte_vertical<D>(
    node: D::NodeId,
    byte_offset: usize,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    delta: isize,
) -> Option<usize>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let taffy_id = built.node_map.get(&node)?;
    let layout = text_ctx.layouts.get(taffy_id)?;
    let moved = Selection::from_byte_index(layout, byte_offset, Affinity::default())
        .move_lines(layout, delta, false);
    Some(moved.focus().index())
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

/// Absolute border-box origin of `target`: walk from the document root,
/// accumulating each ancestor's parent-relative `taffy::Layout.location`. `None`
/// if `target` is unreachable / unlaid-out.
fn absolute_origin<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    target: D::NodeId,
) -> Option<(f32, f32)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    fn walk<D>(
        dom: &D,
        fragments: &FragmentPlane<D::NodeId>,
        id: D::NodeId,
        target: D::NodeId,
        acc: (f32, f32),
    ) -> Option<(f32, f32)>
    where
        D: LayoutDom,
        D::NodeId: Copy + Eq + Hash,
    {
        let origin = match fragments.rect_of(id) {
            Some(l) => (acc.0 + l.location.x, acc.1 + l.location.y),
            None => acc,
        };
        if id == target {
            return Some(origin);
        }
        for child in dom.dom_children(id) {
            if let Some(found) = walk(dom, fragments, child, target, origin) {
                return Some(found);
            }
        }
        None
    }
    walk(dom, fragments, dom.document(), target, (0.0, 0.0))
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
            if doc.element_name(id).is_some_and(|n| n.local == LocalName::from("p")) {
                return id;
            }
            q.extend(doc.dom_children(id));
        }
        panic!("no <p>")
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
            run_cascade(&doc, &mut styles, euclid::Size2D::new(800.0, 600.0), sheet, None);
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

        assert!((factor - 80.0).abs() < 1.0, "line-height:2 → ~80px line box, got {factor}");
        assert!((absolute - 70.0).abs() < 1.0, "line-height:70px → ~70px line box, got {absolute}");
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
        run_cascade(&doc, &mut styles, euclid::Size2D::new(800.0, 600.0), sheet, None);
        let images = ImagePlane::new();
        let viewport = taffy::Size {
            width: taffy::AvailableSpace::Definite(800.0),
            height: taffy::AvailableSpace::Definite(600.0),
        };
        let (fragments, built, text_ctx) = layout(&doc, &styles, &images, viewport);
        let p = find_p(&doc);

        let at0 = caret_rect(&doc, p, 0, &built, &text_ctx, &fragments, 2.0)
            .expect("caret at offset 0");
        let at3 = caret_rect(&doc, p, 3, &built, &text_ctx, &fragments, 2.0)
            .expect("caret at end of 'abc'");

        // Positive height (a line-tall bar) and the requested thickness.
        assert!(at0.height > 0.0, "caret has height: {at0:?}");
        assert!((at0.width - 2.0).abs() < 0.01, "caret width is the thickness");
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
        run_cascade(&doc, &mut styles, euclid::Size2D::new(800.0, 600.0), sheet, None);
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
        run_cascade(&doc, &mut styles, euclid::Size2D::new(800.0, 600.0), sheet, None);
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
        assert!(rect_at(19).y > start.y, "text wrapped to multiple visual lines");

        // Down one visual line lands on a later row (greater y) at a byte past
        // the first wrapped word.
        let down = caret_byte_vertical::<StaticDocument>(p, 0, &built, &text_ctx, 1).unwrap();
        assert!(down > 0, "down moved off byte 0: {down}");
        assert!(rect_at(down).y > start.y, "down moved to a lower visual line");

        // Up from there returns to the first row.
        let up = caret_byte_vertical::<StaticDocument>(p, down, &built, &text_ctx, -1).unwrap();
        assert!((rect_at(up).y - start.y).abs() < 0.5, "up returned to the first row");

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
        assert!((rect_at(hit).y - down_rect.y).abs() < 0.5, "click maps to the clicked row");

        // A node with no cached text layout yields None for both.
        let root = doc.document();
        assert!(caret_byte_vertical::<StaticDocument>(root, 0, &built, &text_ctx, 1).is_none());
        assert!(caret_byte_at_point(&doc, root, 1.0, 1.0, &built, &text_ctx, &fragments).is_none());
    }
}
