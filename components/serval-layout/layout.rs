/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Probe-slice layout entry point.
//!
//! Runs the minimum pipeline: construct a Taffy tree from a `LayoutDom`
//! + `StylePlane`, ask Taffy to compute layout against a viewport, then
//! read per-node results back into a `FragmentPlane`.
//!
//! This is the smallest end-to-end that validates the planes
//! architecture's plumbing (NodeRef → construct → Taffy → FragmentPlane).
//! Not the full pipeline: no Stylo cascade, no inline text (parley
//! wiring deferred), no paint emission.

use std::hash::Hash;

use layout_dom_api::LayoutDom;

use crate::construct::{construct, ConstructedTree};
use crate::fragment::FragmentPlane;
use crate::style::StylePlane;

/// Run the probe layout pipeline.
///
/// Steps:
/// 1. `construct(dom, styles, viewport)` — DOM walk → Taffy tree.
/// 2. `taffy::compute_layout(...)` — Taffy lays out the tree.
/// 3. Walk the node_map → populate FragmentPlane with per-node rects.
///
/// Returns the FragmentPlane (read-side observable) plus the Taffy tree
/// itself for tests that want to inspect lower-level state.
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

    built
        .tree
        .compute_layout(built.root, viewport)
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
        // Walk all element nodes and give them a block style.
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

    #[test]
    fn probe_layout_assigns_nonzero_rect_to_p() {
        // Parse a trivial document.
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");

        // Build a hand-rolled StylePlane: every element is a block,
        // width 200, height 50.
        let styles = build_style_plane(&document);

        // Run layout against an 800×600 viewport.
        let viewport = Size {
            width: AvailableSpace::Definite(800.0),
            height: AvailableSpace::Definite(600.0),
        };
        let (fragments, _built) = layout(&document, &styles, viewport);

        // Find the <p> and assert it got a non-zero rect.
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

        // FragmentPlane should have entries for html, body, p — three elements.
        assert!(
            fragments.len() >= 3,
            "expected at least 3 element fragments, got {}",
            fragments.len()
        );
    }

    #[test]
    fn probe_layout_respects_height() {
        // Parse a single-element document and verify the height we set
        // round-trips through Taffy.
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

        // We set height: 50px; Taffy should respect that.
        assert_eq!(
            rect.size.height, 50.0,
            "expected height 50.0, got {}",
            rect.size.height
        );
    }
}

