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

use paint_list_api::specs::TransformKind;
use paint_list_api::{LayoutPoint, LayoutTransform, PaintCmd, TransformSpec};
use style::properties::ComputedValues;

use crate::box_tree::BoxTree;
use crate::paint_emit::{is_out_of_flow, walk, Deferred, Emitter};
use crate::text_measure::TextMeasureCtx;

/// Paint the box at arena index `node` and its subtree as a stacking context
/// rooted at absolute `origin`, appending commands to `out`. See the module docs
/// for the order.
pub(crate) fn paint_context<Id>(
    em: &mut Emitter<'_, Id>,
    tree: &BoxTree<Id>,
    text_ctx: Option<&TextMeasureCtx>,
    node: usize,
    origin: (f32, f32),
    out: &mut Vec<PaintCmd>,
) where
    Id: Copy + Eq + Hash,
{
    // This context's own box + in-flow descendants -> `body`; the positioned /
    // z-index layers belonging to this context -> `layers` (each recorded with
    // its absolute origin + bucket z; `walk` does not descend into them).
    let mut body = Vec::new();
    let mut layers: Vec<Deferred> = Vec::new();
    // Each context root starts a fresh ancestor-transform *and* ancestor-scroll
    // accumulation: any transform / scroll on an ancestor *of this context* is already
    // folded into this context's placement (the `paint_layer` wrap and the deferred
    // `origin`), so both restart at zero here and compose rather than double.
    walk(
        em, tree, text_ctx, node, origin, &mut body, &mut layers, true,
        LayoutTransform::identity(), (0.0, 0.0),
    );
    // Stable sort by (z, document order): same-z layers keep document order, the
    // Appendix E tiebreak.
    layers.sort_by_key(|d| (d.z, d.seq));

    // Negative layers (z < 0) behind the content; the rest on top. `layers` is
    // sorted, so the split point is the first non-negative.
    let split = layers.iter().position(|d| d.z >= 0).unwrap_or(layers.len());
    for d in &layers[..split] {
        paint_layer(em, tree, text_ctx, d, out);
    }
    // This context's own content. `walk` folded the absolute `origin` into the
    // root node's own transform (it entered with `is_root`), so `body` is already
    // in scene coordinates and appends directly. Each layer above/below is emitted
    // at its own absolute origin, wrapped (by `paint_layer`) in the cumulative CSS
    // transform of its transform-bearing ancestors so an abs-pos child of a
    // `transform`ed element (the orrery camera container) paints transformed.
    out.append(&mut body);
    for d in &layers[split..] {
        paint_layer(em, tree, text_ctx, d, out);
    }
}

/// Paint one lifted stacking layer, re-establishing its ancestors' cumulative CSS
/// transform around it. Without the wrap a lifted abs-pos layer paints on a clean
/// stack at its absolute layout origin, dropping any `transform` on an ancestor
/// (e.g. the orrery's camera container) — see [`Deferred::ancestor_transform`].
/// Identity ancestor transform (the common case) paints with no extra wrapper.
fn paint_layer<Id>(
    em: &mut Emitter<'_, Id>,
    tree: &BoxTree<Id>,
    text_ctx: Option<&TextMeasureCtx>,
    d: &Deferred,
    out: &mut Vec<PaintCmd>,
) where
    Id: Copy + Eq + Hash,
{
    // A `position: fixed` layer attaches to the viewport: counter the document
    // scroll (applied as the outermost wrap in `paint_emit::emit_inner`) so the
    // layer stays pinned while in-flow + absolute content scrolls under it (scope
    // doc rule 3, the Fixed≠Absolute distinction). The counter is the outermost
    // wrap here, so it cancels the document translate regardless of any ancestor
    // transform re-established below. `absolute` layers carry
    // `attaches_to_viewport == false` and scroll with the document.
    let (vsx, vsy) = em.viewport_scroll();
    let counter = d.attaches_to_viewport && (vsx != 0.0 || vsy != 0.0);
    if counter {
        out.push(PaintCmd::PushTransform(TransformSpec {
            origin: LayoutPoint::new(vsx, vsy),
            transform: LayoutTransform::identity(),
            kind: TransformKind::Standard,
        }));
    }
    let wrap = d.ancestor_transform != LayoutTransform::identity();
    if wrap {
        out.push(PaintCmd::PushTransform(TransformSpec {
            origin: LayoutPoint::new(0.0, 0.0),
            transform: d.ancestor_transform,
            kind: TransformKind::Standard,
        }));
    }
    paint_context(em, tree, text_ctx, d.node, d.origin, out);
    if wrap {
        out.push(PaintCmd::PopTransform);
    }
    if counter {
        out.push(PaintCmd::PopTransform);
    }
}

/// Whether a box is lifted out of its context's in-flow walk into a stacking
/// layer: any out-of-flow box (`absolute`/`fixed`), or an in-flow positioned box
/// (`relative`/`sticky`) carrying an explicit `z-index`. An unpositioned box, or a
/// positioned one with `z-index: auto`, keeps flowing.
pub(crate) fn defers_to_stacking(cv: &ComputedValues) -> bool {
    is_out_of_flow(cv) || (is_positioned(cv) && z_index_value(cv).is_some())
}

/// The paint-bucket z-index for a lifted layer: its `z-index`, or `0` for `auto`
/// (an out-of-flow `z-index: auto` element paints in the zero group).
pub(crate) fn bucket_z(cv: &ComputedValues) -> i32 {
    z_index_value(cv).unwrap_or(0)
}

/// Whether the box's `position` is anything other than `static`.
fn is_positioned(cv: &ComputedValues) -> bool {
    use style::values::computed::PositionProperty;
    !matches!(cv.get_box().position, PositionProperty::Static)
}

/// The box's `z-index` as `Some(i32)`, or `None` for `auto`.
fn z_index_value(cv: &ComputedValues) -> Option<i32> {
    let z = cv.get_position().z_index;
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
        let (fragments, built, _) = layout(&document, &styles, &ImagePlane::new(), viewport);
        emit_paint_list(&document, &styles, &fragments, &built, DeviceIntSize::new(800, 600))
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

    /// The slider shape — an `absolute` thumb inside a `relative` track — paints
    /// the thumb exactly once. (Regression guard: the live demo showed a "split"
    /// thumb on a fast drag; a DOM-node probe proved the tree holds one thumb, so
    /// this rules out the paint walk emitting the single deferred layer twice and
    /// pins the artefact to the GPU present path rather than emission.)
    #[test]
    fn absolute_thumb_in_relative_track_paints_once() {
        let cmds = paint(
            "<html><body><div class=\"track\"><div class=\"thumb\"></div></div></body></html>",
            &[
                "html, body, div { display: block; margin: 0; }",
                "div { position: relative; }",
                ".track { width: 200px; height: 28px; background-color: rgb(190, 194, 208); }",
                ".thumb { position: absolute; left: 50px; width: 18px; height: 28px; \
                    background-color: rgb(60, 100, 200); }",
            ],
        );
        // rgb(60, 100, 200) ≈ (0.235, 0.392, 0.784).
        let thumbs = cmds
            .iter()
            .filter(|c| {
                matches!(c, PaintCmd::DrawRect(r)
                    if (r.color.r - 0.235).abs() < 0.05
                        && (r.color.g - 0.392).abs() < 0.05
                        && (r.color.b - 0.784).abs() < 0.05)
            })
            .count();
        assert_eq!(thumbs, 1, "the absolute thumb paints exactly once, not split");
    }

    /// An abs-pos child of a `transform`ed ancestor inherits that transform (the
    /// orrery camera-container model). The container is in-flow (`relative` +
    /// `transform`, no `z-index`), so it paints in place with its own
    /// `translate(40,40)` push; its abs-pos `.node` child is lifted into a stacking
    /// layer. Before the fix the layer painted on a clean stack at its absolute
    /// origin, dropping the ancestor transform — so `translate(40,40)` appeared
    /// ONCE (the container). Now `paint_layer` re-establishes the ancestor
    /// transform around the lifted child, so it appears TWICE: the container's own
    /// push and the child's ancestor wrap. (A pure translate conjugates to itself,
    /// so the wrap matrix equals the container's regardless of its position.)
    #[test]
    fn abs_pos_child_inherits_ancestor_transform() {
        let cmds = paint(
            "<html><body><div class=\"cam\"><div class=\"node\"></div></div></body></html>",
            &[
                "html, body { margin: 0; } div { display: block; }",
                ".cam { position: relative; transform: translate(40px, 40px); \
                    width: 100px; height: 100px; }",
                ".node { position: absolute; left: 0; top: 0; width: 10px; height: 10px; \
                    background-color: rgb(0, 200, 0); }",
            ],
        );
        // The lifted abs-pos child is painted.
        let _ = rect_index(&cmds, 0.0, 0.78, 0.0, ".node");
        // translate(40,40) appears for the container's in-flow push AND the child's
        // ancestor wrap → twice. (Without the fix: once.)
        let n = cmds
            .iter()
            .filter(|c| {
                matches!(c, PaintCmd::PushTransform(t)
                    if (t.transform.m41 - 40.0).abs() < 0.5 && (t.transform.m42 - 40.0).abs() < 0.5)
            })
            .count();
        assert_eq!(
            n, 2,
            "abs-pos child must inherit the container transform (wrap), so translate(40,40) \
             appears twice (container push + child wrap); got {n}",
        );
    }
}
