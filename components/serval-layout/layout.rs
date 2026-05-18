/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layout entry point.
//!
//! Runs the minimum end-to-end pipeline: construct a Taffy tree from a
//! `LayoutDom` + `StylePlane`, ask Taffy to compute layout against a
//! viewport (with parley measuring text leaves), then read per-node
//! results back into a `FragmentPlane`.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::hash::Hash;

use layout_dom_api::LayoutDom;

use crate::construct::{construct, ConstructedTree};
use crate::fragment::FragmentPlane;
use crate::style::StylePlane;
use crate::text_measure::{measure_text_leaf, TextMeasureCtx};

/// Run the layout pipeline against a viewport.
///
/// Steps:
/// 1. `construct(dom, styles, viewport)` — DOM walk → Taffy tree with
///    text leaves carrying [`crate::text_measure::TextLeaf`] context.
/// 2. `taffy::compute_layout_with_measure(...)` with a parley-backed
///    measure closure that resolves text leaves to natural sizes.
/// 3. Walk the node_map → populate `FragmentPlane` with per-node rects.
///
/// Returns the `FragmentPlane` (read-side observable) plus the
/// `ConstructedTree` itself for tests that want to inspect Taffy state.
pub fn layout<D>(
    dom: &D,
    styles: &StylePlane<D::NodeId>,
    viewport: taffy::Size<taffy::AvailableSpace>,
) -> (FragmentPlane<D::NodeId>, ConstructedTree<D::NodeId>)
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut built = construct(dom, styles, viewport);
    let mut text_ctx = TextMeasureCtx::new();

    built
        .tree
        .compute_layout_with_measure(
            built.root,
            viewport,
            |known, avail, _id, ctx, _style| match ctx {
                Some(leaf) => measure_text_leaf(&mut text_ctx, leaf, known, avail),
                None => taffy::Size::ZERO,
            },
        )
        .expect("taffy compute_layout failed");

    let mut fragments = FragmentPlane::new();
    for (dom_id, taffy_id) in built.node_map.iter() {
        if let Ok(layout) = built.tree.layout(*taffy_id) {
            fragments.insert(*dom_id, *layout);
        }
    }

    (fragments, built)
}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use serval_static_dom::{StaticDocument, StaticNodeId};
    use taffy::prelude::*;

    use super::*;
    use crate::adapter::NodeRef;
    use crate::style::StyleEntry;

    /// Find the first element descendant matching a local name, walking the
    /// subtree under `start`. Used to locate `<p>` etc. without depending on
    /// html5ever's auto-inserted `<head>` ordering.
    fn find_element<'a, D: LayoutDom>(
        start: NodeRef<'a, D>,
        local: html5ever::LocalName,
    ) -> Option<NodeRef<'a, D>> {
        let mut queue = vec![start];
        while let Some(node) = queue.pop() {
            if let Some(name) = node.dom().element_name(node.id()) {
                if name.local == local {
                    return Some(node);
                }
            }
            queue.extend(node.dom_children());
        }
        None
    }

    /// Build a minimal StylePlane by hand: assign every element a
    /// block-display Taffy style with a fixed width/height. Skips Stylo
    /// entirely for the probe.
    fn build_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        let root = NodeRef::document(document);
        let mut queue = vec![root];
        while let Some(node) = queue.pop() {
            if document.element_name(node.id()).is_some() {
                plane.insert(
                    node.id(),
                    StyleEntry {
                        taffy: Style {
                            display: Display::Block,
                            size: Size {
                                width: length(200.0),
                                height: length(50.0),
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            }
            queue.extend(node.dom_children());
        }
        plane
    }

    /// Same as `build_style_plane` but without forcing fixed sizes —
    /// elements get default style so Taffy computes their dimensions
    /// from the text children. Used by the parley-measurement test
    /// where hard-coded sizes would mask the text measurement.
    fn build_default_style_plane(document: &StaticDocument) -> StylePlane<StaticNodeId> {
        let mut plane: StylePlane<StaticNodeId> = StylePlane::new();
        let root = NodeRef::document(document);
        let mut queue = vec![root];
        while let Some(node) = queue.pop() {
            if document.element_name(node.id()).is_some() {
                plane.insert(
                    node.id(),
                    StyleEntry {
                        taffy: Style {
                            display: Display::Block,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                );
            }
            queue.extend(node.dom_children());
        }
        plane
    }

    #[test]
    fn probe_layout_assigns_nonzero_rect_to_p() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _built) = layout(&document, &styles, viewport);

        let root = NodeRef::document(&document);
        let p_node = find_element(root, local_name!("p")).expect("<p> exists");
        let rect = fragments.rect_of(p_node.id()).expect("<p> got laid out");

        assert!(
            rect.size.width > 0.0,
            "expected positive width, got {}",
            rect.size.width
        );
        assert!(
            rect.size.height > 0.0,
            "expected positive height, got {}",
            rect.size.height
        );

        // FragmentPlane should have entries for html, body, p — three elements,
        // plus the inline text leaf under <p>.
        assert!(
            fragments.len() >= 3,
            "expected at least 3 fragments, got {}",
            fragments.len()
        );
    }

    #[test]
    fn probe_layout_respects_height() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let styles = build_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _) = layout(&document, &styles, viewport);

        let root = NodeRef::document(&document);
        let p = find_element(root, local_name!("p")).unwrap();
        let rect = fragments.rect_of(p.id()).unwrap();

        assert_eq!(
            rect.size.height, 50.0,
            "expected height 50.0, got {}",
            rect.size.height
        );
    }

    /// Probe the parley measure path: with default-sized elements, a
    /// text node should give its containing `<p>` a non-zero width
    /// derived from parley's measurement of the text.
    #[test]
    fn parley_measures_inline_text() {
        let document =
            StaticDocument::parse("<html><body><p>Hello, world!</p></body></html>");
        let styles = build_default_style_plane(&document);
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, built) = layout(&document, &styles, viewport);

        // The text leaf isn't keyed in our node_map directly under the
        // <p>'s DOM id — it gets its own. The text node's fragment
        // should have positive dimensions. Find a text node by DOM
        // kind and check its fragment.
        let mut text_id = None;
        let mut queue = vec![document.document()];
        while let Some(id) = queue.pop() {
            if matches!(document.kind(id), layout_dom_api::NodeKind::Text) {
                text_id = Some(id);
                break;
            }
            queue.extend(document.dom_children(id));
        }
        let text_id = text_id.expect("document contains a text node");

        // Confirm the text leaf is in the node_map (construct.rs adds it).
        assert!(
            built.node_map.contains_key(&text_id),
            "expected text node in Taffy node_map after construct"
        );

        // Confirm the text rect has positive width — parley measured it.
        let rect = fragments.rect_of(text_id).expect("text node has a fragment");
        assert!(
            rect.size.width > 0.0,
            "expected positive width from parley measurement, got {}",
            rect.size.width
        );
        assert!(
            rect.size.height > 0.0,
            "expected positive height from parley measurement, got {}",
            rect.size.height
        );
    }
}
