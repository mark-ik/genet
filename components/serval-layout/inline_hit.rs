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
//! boxes. Its hittable area is therefore the **set** of its per-line glyph-cluster
//! rects — each line-box tall (leading included), tested by **containment**, never a
//! single union bounding box (which would false-hit the inter-line gutter of a
//! wrapped anchor). Within a line the point is resolved to the **glyph cluster**
//! under it, not the run (see [`inline_source_at`]). The host then maps the resolved
//! element to a navigation by walking up for an `<a href>`, exactly as
//! `elementFromPoint`'s caller would.

use std::ops::Range;

use parley::{Cluster, Layout};

use crate::text_measure::ColorBrush;

/// Resolve content-local point `(x, y)` within an inline-formatting leaf to the
/// source element of the glyph **cluster** it lands on, or `None` when it lands in
/// the leaf's empty / inter-run / padding space (the caller keeps the block leaf
/// itself).
///
/// `layout` is the leaf's cached parley layout; `sources` is its byte-range →
/// source-element index (document order, disjoint, in the same byte space as the
/// layout's concatenated run text). `(x, y)` is relative to the leaf's content-box
/// top-left, the space the layout positions runs in.
///
/// Resolution is at glyph-cluster granularity, not run granularity. parley splits
/// shaping runs on font / script / bidi boundaries but **not on colour** (colour is
/// a per-cluster `Brush`), and a glyph run's first byte is the start of its whole
/// shaping run — so attributing a point by the run's first byte hands the entire
/// run (a colour-only mid-paragraph `<a>` included) to whatever element owns the
/// run's start, leaving the link unhittable. Instead this delegates the
/// line → run → cluster descent to parley's own
/// [`Cluster::from_point_exact`](parley::Cluster::from_point_exact), which walks
/// clusters in **visual** order (RTL-correct, since it maps each on-screen position
/// back to its logical byte) and returns `None` for a point past the line's text —
/// the containment miss that keeps the block leaf. The cluster's own
/// `text_range().start` (its logical first byte) then maps to a source; a byte in no
/// source range (the block's own direct text) yields `None`.
///
/// Two guards wrap the delegation. `from_point_exact` resolves `y` through
/// `line_for_offset`, which **clamps** an out-of-range `y` to the nearest line; so a
/// click in the leaf's padding above/below the text would resolve to a line's
/// cluster. The resolved cluster's line band is re-checked against `y` (the
/// half-leading model) and a miss falls through to the block, where padding belongs.
/// And an **end-of-line trailing-whitespace** cluster is skipped: a soft/hard break
/// hangs its trailing space (CSS Text), which is not part of the inline box's hit
/// area, so a click there keeps the block leaf rather than the element that happens
/// to own the space byte.
pub(crate) fn inline_source_at<NodeId: Copy>(
    layout: &Layout<ColorBrush>,
    sources: &[(Range<usize>, NodeId)],
    x: f32,
    y: f32,
) -> Option<NodeId> {
    // Delegate the line / run / cluster descent to parley (visual-order cluster
    // walk, RTL-correct, ligature components included, exact = `None` past the text).
    let (cluster, _side) = Cluster::from_point_exact(layout, x, y)?;

    // Padding guard: `from_point_exact` clamps an out-of-band `y` to the nearest
    // line, so reject a point that does not actually fall in the resolved cluster's
    // line box (the half-leading model — ascent + descent + leading = line_height,
    // in the same content-box-relative space the layout positions lines in). A click
    // in the leaf's padding then falls through to the block leaf.
    let line = cluster.line();
    let m = line.metrics();
    let half_leading = m.leading / 2.0;
    let top = m.baseline - m.ascent - half_leading;
    let bottom = m.baseline + m.descent + half_leading;
    if y < top || y >= bottom {
        return None;
    }

    // Trailing whitespace at a line break is hung, not part of the inline box's hit
    // area: a click on it keeps the block leaf rather than the element owning the
    // space byte.
    if cluster.is_space_or_nbsp() && cluster.is_end_of_line() {
        return None;
    }

    // The cluster's own logical first byte (RTL-correct: `from_point_exact` already
    // mapped the visual hit position to the logical cluster). A ligature cluster
    // spanning an element boundary attributes to its first byte — the inherent
    // `elementFromPoint` ambiguity over an indivisible glyph; its zero-glyph
    // components carry their own bytes, so the trailing half still resolves to its
    // own source.
    let byte = cluster.text_range().start;
    sources.iter().find(|(r, _)| r.contains(&byte)).map(|(_, src)| *src)
}
