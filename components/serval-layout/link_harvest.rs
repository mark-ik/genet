/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Link-rect harvest: every `<a href>`'s href + its absolute document-px hit
//! rect(s).
//!
//! The flat HTML/serval scene the content host composites is not a queryable
//! packet (unlike the document lane's retained `DocumentRenderPacket`, which the
//! host hit-tests directly). So a click on an HTML card resolves to a link via a
//! pre-harvested rect table instead. This builds that table.
//!
//! Two anchor shapes, both yielding rects in **full-document px, unscrolled** — the
//! same space the host adds the card's scroll into before hit-testing, never
//! band-relative (the band's `-band_y` shift is a paint-time translate only):
//!
//! - **Text anchors** (the common case: a link in body text or a nav list). A
//!   `display:inline` `<a>` establishes no box of its own; its geometry lives in
//!   its inline-formatting leaf's cached `parley::Layout` as per-line glyph runs.
//!   We map the anchor to its byte span within that leaf (via the leaf's
//!   byte-range → source-element map, [`BoxTree::inline_sources`]) and reuse
//!   [`crate::caret::selection_rects`] to get one rect per line box the anchor's
//!   text occupies — the same per-line geometry text selection highlights, so hit
//!   and paint mirror, and a wrapped link is N boxes rather than one union rect
//!   (which would false-hit the inter-line gutter).
//! - **Boxed anchors** (`display:block` / `inline-block`, or any `<a>` that itself
//!   establishes a box): its border-box rect from the fragment plane, so a
//!   full-width nav row is clickable across its whole box (in addition to any text
//!   rects). An `<a>` that wraps only a replaced element while staying `inline` (an
//!   image-only inline link) establishes no box and carries no text, so it is not
//!   yet harvested — a documented follow-on.

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use parley::PositionedLayoutItem;

use crate::box_tree::BoxTree;
use crate::caret::{absolute_origin, collect_text_leaves, selection_rects};
use crate::fragment::FragmentPlane;
use crate::incremental::anchor_href;
use crate::text_measure::TextMeasureCtx;

/// Every `<a href>`'s href paired with an absolute document-px hit rect
/// (`[x0, y0, x1, y1]`, top-left origin, Y-down). See the module docs for the two
/// anchor shapes and the coordinate space: rects are unscrolled document px; the
/// caller composites a band by subtracting `band_y`, and the host hit-tests after
/// subtracting the card origin and adding the card's scroll.
pub(crate) fn harvest_link_rects<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
) -> Vec<(String, [f32; 4])>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut out = Vec::new();

    // 1. Text anchors: per inline-formatting leaf, group the leaf's inline source
    //    runs by their enclosing `<a href>`, then emit that anchor's per-line rects.
    let mut leaves = Vec::new();
    collect_text_leaves(dom, built, text_ctx, dom.document(), &mut leaves);
    for leaf in leaves {
        let Some(sources) = built.inline_sources(leaf) else {
            continue;
        };
        // (anchor node, href, min byte start, max byte end) over this leaf. An
        // anchor's descendant runs are contiguous in document order, so the
        // [min, max) span is the anchor's whole text in the leaf.
        let mut spans: Vec<(D::NodeId, String, usize, usize)> = Vec::new();
        for (range, src) in sources {
            let Some((anchor, href)) = enclosing_anchor(dom, *src, leaf) else {
                continue;
            };
            match spans.iter_mut().find(|(a, ..)| *a == anchor) {
                Some(e) => {
                    e.2 = e.2.min(range.start);
                    e.3 = e.3.max(range.end);
                },
                None => spans.push((anchor, href, range.start, range.end)),
            }
        }
        for (_anchor, href, start, end) in spans {
            for r in selection_rects(dom, leaf, start, end, built, text_ctx, fragments) {
                out.push((href.clone(), [r.x, r.y, r.x + r.width, r.y + r.height]));
            }
        }
    }

    // 2. Boxed anchors: an `<a href>` that establishes its own box also contributes
    //    its border-box rect (full-width nav rows / block links), accumulating the
    //    parent-relative fragment origins down the tree to absolute document px.
    boxed_anchor_rects(dom, fragments, dom.document(), (0.0, 0.0), &mut out);

    // 3. Image-only inline anchors: an `<a href>` that wraps only a replaced `<img>`
    //    while staying `inline` establishes no box and carries no text, so neither
    //    pass above catches it. Harvest the image's positioned inline box instead.
    image_anchor_rects(dom, fragments, built, text_ctx, &mut out);

    out
}

/// Harvest the rect of an inline replaced `<img>` that is the content of an inline
/// `<a href>` (an image link — a logo / banner home-link). Such an anchor establishes
/// no fragment box and carries no text, so the text pass and the boxed-anchor pass
/// both miss it; its geometry is the image's positioned inline box in its formatting
/// leaf's `parley::Layout` (the same box paint emits the `DrawImage` from). Boxed
/// anchors are already covered by [`boxed_anchor_rects`], so only INLINE anchors (no
/// fragment box of their own) are handled here.
fn image_anchor_rects<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    built: &BoxTree<D::NodeId>,
    text_ctx: &TextMeasureCtx,
    out: &mut Vec<(String, [f32; 4])>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut leaves = Vec::new();
    collect_text_leaves(dom, built, text_ctx, dom.document(), &mut leaves);
    for leaf in leaves {
        let Some(&taffy_id) = built.node_map.get(&leaf) else {
            continue;
        };
        let (Some(layout), Some(content)) = (
            text_ctx.layouts.get(&taffy_id),
            built.get_node_context(taffy_id),
        ) else {
            continue;
        };
        let (Some((ox, oy)), Some(frame)) = (
            absolute_origin(dom, fragments, leaf),
            fragments.rect_of(leaf),
        ) else {
            continue;
        };
        // The leaf content-box origin — the space parley positions inline boxes in.
        let content_x = ox + frame.border.left + frame.padding.left;
        let content_y = oy + frame.border.top + frame.padding.top;
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::InlineBox(pbox) = item else {
                    continue;
                };
                let Some(box_item) = content.boxes.get(pbox.id as usize) else {
                    continue;
                };
                // Replaced `<img>` only (an inline-block is hit through its own
                // fragments; links inside an inline-block are a follow-on).
                if box_item.block.is_some() {
                    continue;
                }
                let Some((anchor, href)) = enclosing_anchor(dom, box_item.source, leaf) else {
                    continue;
                };
                // A boxed anchor's box already covers this (pass 2); only the
                // box-less inline anchor needs the image rect.
                if fragments.rect_of(anchor).is_some() {
                    continue;
                }
                out.push((
                    href,
                    [
                        content_x + pbox.x,
                        content_y + pbox.y,
                        content_x + pbox.x + pbox.width,
                        content_y + pbox.y + pbox.height,
                    ],
                ));
            }
        }
    }
}

/// The nearest `<a href>` at or above inline source `src` (stopping at the
/// containing leaf), with its href — the source run's owning link, if any.
fn enclosing_anchor<D>(dom: &D, src: D::NodeId, leaf: D::NodeId) -> Option<(D::NodeId, String)>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq,
{
    let mut cur = Some(src);
    while let Some(node) = cur {
        if let Some(href) = anchor_href(dom, node) {
            return Some((node, href));
        }
        if node == leaf {
            break;
        }
        cur = dom.parent(node);
    }
    None
}

/// Pre-order walk accumulating absolute border-box origins; at each `<a href>` that
/// owns a fragment box, push its absolute border-box rect. (`taffy::Layout.location`
/// is parent-relative, so the origin is the running sum down the tree — the same
/// accumulation [`crate::caret`]'s `absolute_origin` does.)
fn boxed_anchor_rects<D>(
    dom: &D,
    fragments: &FragmentPlane<D::NodeId>,
    id: D::NodeId,
    acc: (f32, f32),
    out: &mut Vec<(String, [f32; 4])>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let origin = match fragments.rect_of(id) {
        Some(l) => (acc.0 + l.location.x, acc.1 + l.location.y),
        None => acc,
    };
    if let (Some(href), Some(l)) = (anchor_href(dom, id), fragments.rect_of(id)) {
        out.push((
            href,
            [
                origin.0,
                origin.1,
                origin.0 + l.size.width,
                origin.1 + l.size.height,
            ],
        ));
    }
    for child in dom.dom_children(id) {
        boxed_anchor_rects(dom, fragments, child, origin, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::run_cascade;
    use crate::image_decode::ImagePlane;
    use crate::layout::layout;
    use crate::style::StylePlane;
    use serval_static_dom::{StaticDocument, StaticNodeId};

    fn lay(
        body: &str,
        sheet: &[&str],
    ) -> (
        StaticDocument,
        crate::fragment::FragmentPlane<StaticNodeId>,
        BoxTree<StaticNodeId>,
        TextMeasureCtx,
    ) {
        let doc = StaticDocument::parse(body);
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
        (doc, fragments, built, text_ctx)
    }

    /// An inline `<a href>` in body text harvests one rect (one line) carrying its
    /// href, with positive area, sitting inside the paragraph's band.
    #[test]
    fn inline_anchor_harvests_a_line_rect() {
        let (doc, fragments, built, text_ctx) = lay(
            "<html><body><p>see <a href=\"https://example.test/spec\">the spec</a> now</p></body></html>",
            &["html, body, p { display: block; margin: 0; font-size: 20px; }"],
        );
        let links = harvest_link_rects(&doc, &fragments, &built, &text_ctx);
        assert_eq!(
            links.len(),
            1,
            "one inline link -> one line rect, got {links:?}"
        );
        let (href, rect) = &links[0];
        assert_eq!(href, "https://example.test/spec");
        assert!(
            rect[2] > rect[0] && rect[3] > rect[1],
            "positive area, got {rect:?}"
        );
        // "the spec" follows "see " so the link rect starts a little right of x=0.
        assert!(
            rect[0] > 0.0,
            "link starts after the leading text, got x0={}",
            rect[0]
        );
    }

    /// A `display:block` `<a href>` contributes its full border-box rect (the whole
    /// clickable row), in addition to its text rect.
    #[test]
    fn block_anchor_harvests_its_box() {
        let (doc, fragments, built, text_ctx) = lay(
            "<html><body><a href=\"/home\">Home</a></body></html>",
            &[
                "html, body { display: block; margin: 0; } a { display: block; width: 200px; height: 40px; }",
            ],
        );
        let links = harvest_link_rects(&doc, &fragments, &built, &text_ctx);
        assert!(!links.is_empty(), "block anchor harvests rects");
        assert!(
            links.iter().all(|(h, _)| h == "/home"),
            "all rects carry the href"
        );
        // The border-box rect spans the declared 200x40 row.
        let has_box = links
            .iter()
            .any(|(_, r)| (r[2] - r[0] - 200.0).abs() < 1.0 && (r[3] - r[1] - 40.0).abs() < 1.0);
        assert!(has_box, "the 200x40 border box is harvested, got {links:?}");
    }

    /// An image-only inline `<a href>` (a logo / banner link, `<a><img></a>` with the
    /// anchor staying `inline`) harvests the image's box — the case neither the text
    /// pass nor the boxed-anchor pass catches.
    #[test]
    fn image_only_inline_anchor_harvests_the_image_box() {
        let (doc, fragments, built, text_ctx) = lay(
            "<html><body><p><a href=\"https://example.test/home\"><img width=\"40\" height=\"30\"></a></p></body></html>",
            &["html, body, p { display: block; margin: 0; } img { width: 40px; height: 30px; }"],
        );
        let links = harvest_link_rects(&doc, &fragments, &built, &text_ctx);
        let hit = links.iter().find(|(h, _)| h == "https://example.test/home");
        assert!(
            hit.is_some(),
            "an image-only inline link harvests a rect, got {links:?}"
        );
        let (_, rect) = hit.unwrap();
        assert!(
            (rect[2] - rect[0] - 40.0).abs() < 2.0,
            "~40px wide image box, got {}",
            rect[2] - rect[0]
        );
        assert!(
            (rect[3] - rect[1] - 30.0).abs() < 2.0,
            "~30px tall image box, got {}",
            rect[3] - rect[1]
        );
    }

    /// A page with no links harvests nothing.
    #[test]
    fn no_links_harvests_empty() {
        let (doc, fragments, built, text_ctx) = lay(
            "<html><body><p>plain text</p></body></html>",
            &["p { display: block; }"],
        );
        assert!(harvest_link_rects(&doc, &fragments, &built, &text_ctx).is_empty());
    }
}
