/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! DOM walk → Taffy tree construction.
//!
//! Walks a `LayoutDom` via `NodeRef`'s structural primitives, attaches
//! the style entry from `StylePlane`, and builds a
//! `taffy::TaffyTree<TextLeaf>` ready for Taffy's
//! `compute_layout_with_measure`. Element nodes become Taffy nodes;
//! text nodes become Taffy leaves carrying a [`TextLeaf`] context that
//! the parley measure-function (`crate::text_measure`) consumes at
//! layout time.
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
use crate::text_measure::TextLeaf;

/// Default font size used for text leaves whose parent has no
/// cascaded `font-size`. 16 px matches CSS/UA-stylesheet convention
/// and parley's own default; lines up with `TextLeaf::new`.
const DEFAULT_FONT_SIZE: f32 = 16.0;

/// Output of construction: the Taffy tree, the root, and the DOM↔Taffy id
/// mapping for reading results back. Tree is parameterized by `TextLeaf`
/// so text leaves carry their content + font properties through to
/// the measure function.
pub struct ConstructedTree<NodeId: Copy + Eq + Hash> {
    pub tree: TaffyTree<TextLeaf>,
    pub root: taffy::NodeId,
    /// DOM NodeId → Taffy NodeId. Sparse only for nodes that don't get
    /// Taffy entries (e.g., comments, the document node when treated
    /// as a synthetic root wrapper); element and text nodes are both
    /// present.
    pub node_map: FxHashMap<NodeId, taffy::NodeId>,
}

/// Build a Taffy tree from a `LayoutDom` rooted at `dom.document()`,
/// reading style from `styles`. Element nodes become Taffy nodes with
/// child lists; text nodes become Taffy leaves carrying [`TextLeaf`]
/// context. Comment / processing-instruction nodes are still skipped.
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
    let mut tree: TaffyTree<TextLeaf> = TaffyTree::new();
    let mut node_map: FxHashMap<D::NodeId, taffy::NodeId> = FxHashMap::default();

    let root_ref = NodeRef::document(dom);
    let root_children = build_children(dom, styles, root_ref, &mut tree, &mut node_map);

    let root_style = styles.taffy_style(dom.document());
    let root = tree
        .new_with_children(root_style, &root_children)
        .expect("Taffy: failed to create root");

    ConstructedTree { tree, root, node_map }
}

/// Recursively build Taffy nodes for `parent`'s element + text
/// descendants and return the list of Taffy NodeIds for them in DOM
/// order.
fn build_children<'a, D>(
    dom: &'a D,
    styles: &StylePlane<D::NodeId>,
    parent: NodeRef<'a, D>,
    tree: &mut TaffyTree<TextLeaf>,
    node_map: &mut FxHashMap<D::NodeId, taffy::NodeId>,
) -> Vec<taffy::NodeId>
where
    D: LayoutDom,
    D::NodeId: Copy + Eq + Hash,
{
    // Inherit font_size from the parent element's cascaded
    // ComputedValues. Text nodes themselves don't have entries (only
    // elements do); their effective font-size comes from the nearest
    // ancestor element, which here is `parent`. When the cascade
    // hasn't been applied (hand-rolled style fixtures), fall back to
    // the default.
    let parent_font_size = font_size_of(styles, parent.id()).unwrap_or(DEFAULT_FONT_SIZE);

    let mut children = Vec::new();
    for child in parent.dom_children() {
        let taffy_id = match dom.kind(child.id()) {
            NodeKind::Element => {
                let style = styles.taffy_style(child.id());
                let grand = build_children(dom, styles, child, tree, node_map);
                tree.new_with_children(style, &grand)
                    .expect("Taffy: failed to create element node")
            }
            NodeKind::Text => {
                let text = dom.text(child.id()).unwrap_or("").to_string();
                let leaf = TextLeaf::with_font_size(text, parent_font_size);
                tree.new_leaf_with_context(taffy::Style::default(), leaf)
                    .expect("Taffy: failed to create text leaf")
            }
            // Comments / processing instructions / document fragments
            // don't render — skip.
            _ => continue,
        };
        node_map.insert(child.id(), taffy_id);
        children.push(taffy_id);
    }
    children
}

/// Read an element's cascaded `font-size` in CSS px. Returns `None`
/// when the cascade hasn't been applied to that element (hand-rolled
/// style fixtures); the caller defaults to `DEFAULT_FONT_SIZE`.
fn font_size_of<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    id: NodeId,
) -> Option<f32> {
    let entry = styles.get(id)?;
    let data = entry.borrow_data()?;
    Some(data.styles.primary().get_font().font_size.computed_size().px())
}
