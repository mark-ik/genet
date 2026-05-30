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
use parley::layout::{Affinity, Cursor};

use crate::box_tree::BoxTree;
use crate::fragment::FragmentPlane;
use crate::text_measure::TextMeasureCtx;

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

    Some(CaretRect {
        x: content_x + bb.x0 as f32,
        y: content_y + bb.y0 as f32,
        width: (bb.x1 - bb.x0) as f32,
        height: (bb.y1 - bb.y0) as f32,
    })
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

    /// The caret advances along the text: at offset 0 it sits at the content
    /// left; at the end of "abc" it sits further right (≈ text width). No padding
    /// keeps the content origin at the box origin so the assertion is on the
    /// glyph-advance geometry, not insets.
    #[test]
    fn caret_advances_with_offset() {
        let doc = StaticDocument::parse("<html><body><p>abc</p></body></html>");
        let sheet = &["html, body, p { display: block; margin: 0; padding: 0; border: 0; }"];
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(&doc, &mut styles, euclid::Size2D::new(800.0, 600.0), sheet);
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
}
