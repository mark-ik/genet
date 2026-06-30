/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `NodeRef<'a, D>` — the structural handle wrapping `(dom: &'a D, id:
//! D::NodeId)` for any `layout_dom_api::LayoutDom` impl. It exposes structural
//! navigation (parent, sibling navigation, children, kind), and is what
//! `construct.rs` walks to build the box tree.
//!
//! The Stylo foreign-trait firewall is its sibling `StyleNodeRef`
//! (`adapter_stylo.rs`), which implements Stylo's trait family (`TNode` /
//! `TElement` / `selectors::Element` / etc.) over a `(dom, id, plane)` triple.
//! `NodeRef` itself carries no Stylo impls and no side-table references: per the
//! planes architecture, computed style and atom storage live in
//! `serval-layout`-owned planes (`StylePlane`) keyed by `D::NodeId`, read via
//! plane accessors during the cascade rather than embedded on the handle.
//!
//! See `docs/2026-05-17_serval_layout_planes_architecture.md` for the
//! architectural context (planes architecture; `StyleNodeRef` as the single
//! Stylo adapter; no `layout_api` LayoutNode/Element bundle, the path-C lift
//! plan's original target shape superseded).

use layout_dom_api::LayoutDom;

/// A structural handle into a `LayoutDom`-backed DOM. Carries a borrow of the
/// DOM (`&'a D`) plus the node identity, and exposes structural navigation for
/// `construct.rs`.
///
/// Stylo's trait family is implemented on the sibling `StyleNodeRef`
/// (`adapter_stylo.rs`), not on `NodeRef`. Style-side state (computed style,
/// atomized id/class) lives in `serval-layout`-owned planes (`StylePlane`)
/// keyed by `D::NodeId`, read via plane accessors during the cascade rather
/// than embedded on the handle. See the planes doc for the rationale.
pub struct NodeRef<'a, D: LayoutDom> {
    pub(crate) dom: &'a D,
    pub(crate) id: D::NodeId,
}

impl<'a, D: LayoutDom> NodeRef<'a, D> {
    /// Construct an adapter rooted at a specific node.
    pub fn new(dom: &'a D, id: D::NodeId) -> Self {
        Self { dom, id }
    }

    /// Construct an adapter rooted at the document node.
    pub fn document(dom: &'a D) -> Self {
        Self {
            dom,
            id: dom.document(),
        }
    }

    /// Borrow the underlying DOM.
    pub fn dom(&self) -> &'a D {
        self.dom
    }

    /// The node ID this adapter points at.
    pub fn id(&self) -> D::NodeId {
        self.id
    }

    /// Move the adapter to a new node in the same DOM.
    pub fn with_id(&self, id: D::NodeId) -> Self {
        Self { dom: self.dom, id }
    }

    /// Parent node, or `None` if this is the document root.
    pub fn parent(&self) -> Option<Self> {
        self.dom.parent(self.id).map(|pid| self.with_id(pid))
    }

    /// Previous sibling in DOM order.
    pub fn prev_sibling(&self) -> Option<Self> {
        self.dom.prev_sibling(self.id).map(|s| self.with_id(s))
    }

    /// Next sibling in DOM order.
    pub fn next_sibling(&self) -> Option<Self> {
        self.dom.next_sibling(self.id).map(|s| self.with_id(s))
    }

    /// DOM-tree children as fresh adapters.
    pub fn dom_children(&self) -> impl Iterator<Item = Self> + '_ {
        let dom = self.dom;
        self.dom
            .dom_children(self.id)
            .map(move |id| Self { dom, id })
    }

    /// Flat-tree children (shadow-aware) as fresh adapters. Defaults to
    /// `dom_children` for backends without shadow DOM.
    pub fn flat_children(&self) -> impl Iterator<Item = Self> + '_ {
        let dom = self.dom;
        self.dom
            .flat_children(self.id)
            .map(move |id| Self { dom, id })
    }

    /// What kind of node this is.
    pub fn kind(&self) -> layout_dom_api::NodeKind {
        self.dom.kind(self.id)
    }
}

impl<'a, D: LayoutDom> Clone for NodeRef<'a, D> {
    fn clone(&self) -> Self {
        Self {
            dom: self.dom,
            id: self.id,
        }
    }
}

impl<'a, D: LayoutDom> Copy for NodeRef<'a, D> {}

impl<'a, D: LayoutDom> std::fmt::Debug for NodeRef<'a, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeRef")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl<'a, D: LayoutDom> PartialEq for NodeRef<'a, D> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.dom, other.dom) && self.id == other.id
    }
}

impl<'a, D: LayoutDom> Eq for NodeRef<'a, D> {}

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use serval_static_dom::StaticDocument;

    use super::*;

    fn find_element_descendant<'a, D: LayoutDom>(
        start: &NodeRef<'a, D>,
        local: html5ever::LocalName,
    ) -> Option<NodeRef<'a, D>> {
        // BFS through the subtree looking for an element with the given
        // local name. Tests don't care about traversal order; this avoids
        // depending on html5ever's auto-inserted <head> vs <body> ordering.
        let mut queue = vec![*start];
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

    #[test]
    fn adapter_walks_a_parsed_document() {
        let document = StaticDocument::parse("<html><body><p>Hello</p></body></html>");
        let root = NodeRef::document(&document);

        let body =
            find_element_descendant(&root, local_name!("body")).expect("body element exists");
        let p = find_element_descendant(&body, local_name!("p")).expect("p element under body");

        let text = p.dom_children().next().expect("text node under p");
        assert!(matches!(text.kind(), layout_dom_api::NodeKind::Text));
        assert_eq!(text.dom().text(text.id()), Some("Hello"));
    }

    #[test]
    fn adapter_sibling_navigation() {
        let document = StaticDocument::parse("<html><body><p>a</p><p>b</p><p>c</p></body></html>");
        let root = NodeRef::document(&document);
        let body =
            find_element_descendant(&root, local_name!("body")).expect("body element exists");

        let ps: Vec<_> = body
            .dom_children()
            .filter(|n| {
                n.dom()
                    .element_name(n.id())
                    .is_some_and(|q| q.local == local_name!("p"))
            })
            .collect();
        assert_eq!(ps.len(), 3);

        assert_eq!(ps[0].prev_sibling(), None);
        assert_eq!(ps[1].prev_sibling().map(|n| n.id()), Some(ps[0].id()));
        assert_eq!(ps[1].next_sibling().map(|n| n.id()), Some(ps[2].id()));
        assert_eq!(ps[2].next_sibling(), None);
    }

    #[test]
    fn adapter_round_trips_parent_child() {
        let document = StaticDocument::parse("<html><body><p>x</p></body></html>");
        let root = NodeRef::document(&document);
        let html = root.dom_children().next().expect("html");
        assert_eq!(html.parent().map(|p| p.id()), Some(root.id()));
    }
}
