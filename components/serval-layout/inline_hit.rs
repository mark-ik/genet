/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Inline-box hit-testing: resolve a point inside an inline-formatting leaf to the
//! innermost inline element under it.
//!
//! The block fragment plane ([`crate::fragment::FragmentPlane`]) holds one box per
//! block-level / replaced / IFC-establishing element. A `display:inline` element
//! (`<a>`, `<span>`, `<label>`, …) establishes no box — it flows transparently into
//! its block's anonymous line boxes as parley runs — so the block hit-walk
//! ([`crate::serval_lane::walk_for_hit`]) can only ever resolve the containing
//! block. This module recovers the inline element by descending into the leaf's
//! cached `parley::Layout`: the same geometry paint emits from
//! ([`crate::paint_emit`]), so hit and paint mirror.
//!
//! Standards model (CSSOM View `elementFromPoint`; CSS2.2 §9.4.2; Appendix E.2):
//! an inline element broken across N lines is N boxes distributed across line
//! boxes. Its hittable area is therefore the **set** of its per-line run rects —
//! each line-box tall (`min_coord..max_coord`, leading included) and run-advance
//! wide — tested by **containment**, never a single union bounding box (which would
//! false-hit the inter-line gutter of a wrapped anchor). The host then maps the
//! resolved element to a navigation by walking up for an `<a href>`, exactly as
//! `elementFromPoint`'s caller would.

use std::ops::Range;

use parley::{Layout, PositionedLayoutItem};

use crate::text_measure::ColorBrush;

/// Resolve content-local point `(x, y)` within an inline-formatting leaf to the
/// source element of the glyph run it lands on, or `None` when it lands in the
/// leaf's empty / inter-run space (the caller keeps the block leaf itself).
///
/// `layout` is the leaf's cached parley layout; `sources` is its byte-range →
/// source-element index (document order, disjoint, in the same byte space as the
/// layout's concatenated run text). `(x, y)` is relative to the leaf's content-box
/// top-left, the space the layout positions runs in.
///
/// A glyph run maps to a source by its first byte. For the common case (a link is
/// styled distinctly from its surroundings, so it shapes into its own glyph run)
/// this is exact. A run that parley merges across same-styled adjacent source
/// elements attributes to the first; that only bites a link styled identically to
/// the text around it (visually indistinguishable anyway) and is a documented edge.
pub(crate) fn inline_source_at<NodeId: Copy>(
    layout: &Layout<ColorBrush>,
    sources: &[(Range<usize>, NodeId)],
    x: f32,
    y: f32,
) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for line in layout.lines() {
        let m = line.metrics();
        // The line box is the inline hit band (CSS: "a line box is always tall
        // enough for the boxes it contains"). Its block extent is the half-leading
        // model around the baseline — ascent + descent + leading = line_height — in
        // layout (content-box-relative) space, the same space `caret_band` reads.
        let half_leading = m.leading / 2.0;
        let top = m.baseline - m.ascent - half_leading;
        let bottom = m.baseline + m.descent + half_leading;
        if y < top || y >= bottom {
            continue;
        }
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else {
                continue;
            };
            let x0 = run.offset();
            // Containment, run-advance wide: a point past the line's text (or in its
            // trailing space) matches no run, so it does not hit — the wrapped-anchor
            // gutter correctness the union-rect approach loses.
            if x < x0 || x >= x0 + run.advance() {
                continue;
            }
            let byte = run.run().text_range().start;
            if let Some((_, src)) = sources.iter().find(|(r, _)| r.contains(&byte)) {
                // Later runs paint over earlier ones; the last containing run wins,
                // matching paint order (topmost). For non-overlapping inline content
                // a point is in at most one run, so this is just that run's source.
                hit = Some(*src);
            }
        }
    }
    hit
}
