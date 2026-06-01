/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stacking-context paint ordering (z-index Tier 2).
//!
//! Tier 1 (in [`crate::paint_emit`]) painted in two flat passes: all in-flow
//! content in document order, then every out-of-flow positioned element on top,
//! sorted by one *global* `(z-index, document-order)`. That is wrong whenever
//! stacking nests: a child's `z-index` is meant to order it only *within its
//! parent's* stacking context, and a negative `z-index` is meant to paint
//! *behind* in-flow content, not on top.
//!
//! Tier 2 paints recursively, per CSS 2.1 [Appendix E]. Each stacking context
//! owns its layers: [`paint_context`] walks the context's own tree for its box +
//! in-flow content, collects the positioned / z-index descendants that belong to
//! *this* context (stopping at each, not descending — nested contexts recurse),
//! and emits them ordered
//!
//!   1. negative-z layers (most negative first), **behind** the content,
//!   2. this context's own box and in-flow descendants,
//!   3. zero / positive-z layers (least positive first), **on top**,
//!
//! each layer recursively as its own context, so nested z-orders stay
//! independent. A descendant that is positioned but *not* a context (no explicit
//! `z-index`) keeps flowing in this context, so its own positioned descendants
//! bubble up to here — the correct containing-context assignment.
//!
//! Two first-pass deviations from Appendix E, deferred until the reftest harness
//! demands them:
//!   * Negative-z layers paint behind the context root's own box too, not just
//!     its in-flow descendants (exact at the document root, whose box is the
//!     canvas; a minor inaccuracy for a nested context root with a visible
//!     background behind a negative child). Fixing it needs the box split out
//!     from the in-flow walk.
//!   * Floats and the block/inline split (Appendix E steps 3–5) are folded into
//!     document order rather than bucketed separately.
//!
//! [Appendix E]: https://www.w3.org/TR/CSS21/zindex.html

use std::hash::Hash;

use layout_dom_api::LayoutDom;
use paint_list_api::PaintCmd;

use crate::fragment::FragmentPlane;
use crate::paint_emit::{is_out_of_flow, walk, Deferred, Emitter};
use crate::style::StylePlane;

/// Paint `node` and its subtree as a stacking context rooted at absolute
/// `origin`, appending commands to `out`. See the module docs for the order.
pub(crate) fn paint_context<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    fragments: &FragmentPlane<D::NodeId>,
    em: &mut Emitter<'_, D::NodeId>,
    node: D::NodeId,
    origin: (f32, f32),
    out: &mut Vec<PaintCmd>,
) where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // This context's own box + in-flow descendants -> `body`; the positioned /
    // z-index layers belonging to this context -> `layers` (each recorded with
    // its absolute origin + bucket z; `walk` does not descend into them).
    let mut body = Vec::new();
    let mut layers: Vec<Deferred<D::NodeId>> = Vec::new();
    walk(dom, styles, fragments, em, node, origin, &mut body, &mut layers, true);
    // Stable sort by (z, document order): same-z layers keep document order, the
    // Appendix E tiebreak.
    layers.sort_by_key(|d| (d.z, d.seq));

    // Negative layers (z < 0) behind the content; the rest on top. `layers` is
    // sorted, so the split point is the first non-negative.
    let split = layers.iter().position(|d| d.z >= 0).unwrap_or(layers.len());
    for d in &layers[..split] {
        paint_context(dom, styles, fragments, em, d.node, d.origin, out);
    }
    // This context's own content. `walk` folded the absolute `origin` into the
    // root node's own transform (it entered with `is_root`), so `body` is already
    // in scene coordinates and appends directly. Each layer above/below is
    // likewise emitted on a clean stack at its own absolute origin, so no
    // transform nesting compounds across layers.
    out.append(&mut body);
    for d in &layers[split..] {
        paint_context(dom, styles, fragments, em, d.node, d.origin, out);
    }
}

/// Whether `id` is lifted out of its context's in-flow walk into a stacking
/// layer: any out-of-flow element (`absolute`/`fixed`), or an in-flow positioned
/// element (`relative`/`sticky`) carrying an explicit `z-index`. An unpositioned
/// element, or a positioned one with `z-index: auto`, keeps flowing.
pub(crate) fn defers_to_stacking<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> bool {
    is_out_of_flow(styles, id) || (is_positioned(styles, id) && z_index_value(styles, id).is_some())
}

/// The paint-bucket z-index for a lifted layer: its `z-index`, or `0` for `auto`
/// (an out-of-flow `z-index: auto` element paints in the zero group).
pub(crate) fn bucket_z<NodeId: Copy + Eq + Hash>(styles: &StylePlane<NodeId>, id: NodeId) -> i32 {
    z_index_value(styles, id).unwrap_or(0)
}

/// Whether `id`'s `position` is anything other than `static`.
fn is_positioned<NodeId: Copy + Eq + Hash>(styles: &StylePlane<NodeId>, id: NodeId) -> bool {
    use style::values::computed::PositionProperty;
    let Some(entry) = styles.get(id) else {
        return false;
    };
    let Some(data) = entry.borrow_data() else {
        return false;
    };
    !matches!(data.styles.primary().get_box().position, PositionProperty::Static)
}

/// The element's `z-index` as `Some(i32)`, or `None` for `auto`.
fn z_index_value<NodeId: Copy + Eq + Hash>(styles: &StylePlane<NodeId>, id: NodeId) -> Option<i32> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    let z = data.styles.primary().get_position().z_index;
    if z.is_auto() {
        None
    } else {
        Some(z.integer_or(0))
    }
}

#[cfg(test)]
mod tests {
    use paint_list_api::{DeviceIntSize, PaintCmd, PaintList};
    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::{AvailableSpace, Size};

    use crate::cascade::run_cascade;
    use crate::image_decode::ImagePlane;
    use crate::layout::layout;
    use crate::paint_emit::emit_paint_list;
    use crate::style::StylePlane;

    /// The command index of the first `DrawRect` whose colour matches `(r, g, b)`
    /// (each channel within 0.1), or panic. Backgrounds give each box a findable
    /// marker in the command stream.
    fn rect_index(cmds: &[PaintCmd], r: f32, g: f32, b: f32, label: &str) -> usize {
        cmds.iter()
            .position(|c| {
                matches!(c, PaintCmd::DrawRect(rect)
                    if (rect.color.r - r).abs() < 0.1
                        && (rect.color.g - g).abs() < 0.1
                        && (rect.color.b - b).abs() < 0.1)
            })
            .unwrap_or_else(|| panic!("no DrawRect for {label}"))
    }

    fn paint(html: &str, sheet: &[&str]) -> Vec<PaintCmd> {
        let document = StaticDocument::parse(html);
        let mut styles: StylePlane<StaticNodeId> = StylePlane::new();
        run_cascade(&document, &mut styles, euclid::Size2D::new(800.0, 600.0), sheet, None);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        emit_paint_list(&document, &styles, &fragments, DeviceIntSize::new(800, 600))
            .commands()
            .to_vec()
    }

    /// A negative `z-index` out-of-flow box paints *behind* later in-flow content
    /// (Tier 1 painted every out-of-flow box on top regardless of sign).
    #[test]
    fn negative_z_index_paints_behind_in_flow() {
        let cmds = paint(
            "<html><body>\
                <div class=\"behind\"></div>\
                <div class=\"flow\"></div>\
            </body></html>",
            &[
                "html, body, div { display: block; margin: 0; }",
                ".behind { position: absolute; top: 0; left: 0; width: 50px; height: 50px; \
                    z-index: -1; background-color: rgb(255, 0, 0); }",
                ".flow { width: 50px; height: 50px; background-color: rgb(0, 0, 255); }",
            ],
        );
        let red = rect_index(&cmds, 1.0, 0.0, 0.0, ".behind (z:-1)");
        let blue = rect_index(&cmds, 0.0, 0.0, 1.0, ".flow");
        assert!(red < blue, "z:-1 .behind (idx {red}) paints before in-flow .flow (idx {blue})");
    }

    /// z-index is scoped to each stacking context: a child's `z-index` orders it
    /// only within its parent context. Context A (`z:1`) holds a `z:100` child;
    /// sibling context B (`z:2`) holds a `z:5` child. A's whole subtree (incl. the
    /// z:100 child) paints below B's (incl. the z:5 child), because A < B at the
    /// parent level. A single global z-sort (Tier 1) would instead paint the z:5
    /// child below the z:100 child.
    #[test]
    fn z_index_is_scoped_to_its_stacking_context() {
        let cmds = paint(
            "<html><body>\
                <div class=\"a\"><div class=\"a1\"></div></div>\
                <div class=\"b\"><div class=\"b1\"></div></div>\
            </body></html>",
            &[
                "html, body, div { display: block; margin: 0; }",
                ".a { position: absolute; top: 0; left: 0; width: 80px; height: 80px; \
                    z-index: 1; background-color: rgb(40, 40, 40); }",
                ".a1 { position: absolute; top: 0; left: 0; width: 40px; height: 40px; \
                    z-index: 100; background-color: rgb(0, 255, 0); }",
                ".b { position: absolute; top: 0; left: 0; width: 80px; height: 80px; \
                    z-index: 2; background-color: rgb(60, 60, 60); }",
                ".b1 { position: absolute; top: 0; left: 0; width: 40px; height: 40px; \
                    z-index: 5; background-color: rgb(255, 255, 0); }",
            ],
        );
        let a1 = rect_index(&cmds, 0.0, 1.0, 0.0, ".a1 (z:100 in A)");
        let b1 = rect_index(&cmds, 1.0, 1.0, 0.0, ".b1 (z:5 in B)");
        assert!(
            a1 < b1,
            "A's z:100 child (idx {a1}) paints below B's z:5 child (idx {b1}) — z is context-scoped"
        );
    }
}
