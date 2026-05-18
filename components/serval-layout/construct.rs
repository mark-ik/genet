/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DOM walk → Taffy tree construction (probe slice).
//!
//! Walks a `LayoutDom` via `NodeRef`'s structural primitives, attaches the
//! style entry from `StylePlane`, and builds a `taffy::TaffyTree` ready for
//! `taffy::compute_layout`. Element nodes become Taffy nodes; text nodes
//! and other kinds are skipped for the probe (no inline layout yet —
//! parley wiring comes later).
//!
//! Returns the constructed Taffy tree, the root Taffy NodeId, and a
//! `NodeId → taffy::NodeId` mapping so callers can read layout results
//! back keyed by their DOM identity.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::hash::Hash;

use layout_dom_api::{LayoutDom, NodeKind};
use rustc_hash::FxHashMap;
use taffy::TaffyTree;

use crate::adapter::NodeRef;
use crate::style::StylePlane;

/// Output of construction: the Taffy tree, the root, and the DOM↔Taffy id
/// mapping for reading results back.
pub struct ConstructedTree<NodeId: Copy + Eq + Hash> {
    pub tree: TaffyTree<()>,
    pub root: taffy::NodeId,
    /// DOM NodeId → Taffy NodeId. Sparse because non-element nodes
    /// (text, comments, document) don't get Taffy entries in the probe.
    pub node_map: FxHashMap<NodeId, taffy::NodeId>,
}

/// Build a Taffy tree from a `LayoutDom` rooted at `dom.document()`,
/// reading style from `styles`. Elements become Taffy nodes; non-elements
/// are skipped (probe limitation — inline text layout requires parley wiring).
///
/// The Taffy root is a synthetic node wrapping the document; its Taffy
/// style defaults to a viewport-shaped block container.
pub fn construct<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    viewport: taffy::Size<taffy::AvailableSpace>,
) -> ConstructedTree<D::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let _ = viewport; // Used at layout time, not construct.
    let mut tree = TaffyTree::new();
    let mut node_map: FxHashMap<D::NodeId, taffy::NodeId> = FxHashMap::default();

    let root_ref = NodeRef::document(dom);
    // Build children list first so the Taffy root gets its children at construction.
    let root_children = build_children(dom, styles, root_ref, &mut tree, &mut node_map);

    let root_style = styles.taffy_style(dom.document());
    let root = tree
        .new_with_children(root_style, &root_children)
        .expect("Taffy: failed to create root");

    ConstructedTree { tree, root, node_map }
}

/// Recursively build Taffy nodes for `parent`'s element descendants and
/// return the list of Taffy NodeIds for them in DOM order.
fn build_children<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    parent: NodeRef<'a, D>,
    tree: &mut TaffyTree<()>,
    node_map: &mut FxHashMap<D::NodeId, taffy::NodeId>,
) -> Vec<taffy::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    let mut children = Vec::new();
    for child in parent.dom_children() {
        if !matches!(dom.kind(child.id()), NodeKind::Element) {
            // Skip text/comment/document/etc. for the probe.
            continue;
        }
        let style = styles.taffy_style(child.id());
        let grand = build_children(dom, styles, child, tree, node_map);
        let taffy_id = tree
            .new_with_children(style, &grand)
            .expect("Taffy: failed to create element node");
        node_map.insert(child.id(), taffy_id);
        children.push(taffy_id);
    }
    children
}
