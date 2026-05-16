/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Bridge from `layout_dom_api::LayoutDom` to `layout_api::LayoutNode<'dom>`.
//!
//! This is the consumer-side adapter the path-C design doc describes (see
//! `docs/2026-05-16_layout_dom_api_design.md` — "Foreign trait adapters").
//! `LayoutDomAdapter<'a, D>` wraps a `(dom: &'a D, id: D::NodeId)` pair plus
//! the side-table references Stylo needs (style storage, atom storage), and
//! the layout crate's internal types will eventually constrain over its
//! `layout_api::LayoutNode<'dom>` impl rather than naming a concrete script
//! type.
//!
//! ## What's here today (P2.3 step 0, 2026-05-16)
//!
//! - `LayoutDomAdapter<'a, D>` type with structural methods backed by
//!   `LayoutDom` primitives (parent, children, sibling navigation, kind).
//! - `LayoutDomBundle<D>` skeleton for the eventual `LayoutDomTypeBundle`
//!   impl.
//! - Construction helpers and a smoke test that round-trips structural
//!   navigation through a `serval-static-dom` `StaticDocument`.
//!
//! ## What's not here yet (deferred to next session)
//!
//! - `impl layout_api::LayoutNode<'dom>` for `LayoutDomAdapter`. The
//!   trait has ~32 methods; many can return None / unimplemented!() for
//!   the static profile, but the signatures need to be right.
//! - `impl layout_api::LayoutElement<'dom>`. ~19 methods, similar story.
//! - `impl layout_api::DangerousStyleNode<'dom>` + the underlying
//!   `style::dom::TNode` it requires. ~20 method stubs.
//! - `impl layout_api::DangerousStyleElement<'dom>` + the underlying
//!   `style::dom::TElement` (~40 methods) and `selectors::Element` (~15
//!   methods). The big one.
//! - `LayoutDomBundle` actually impl-ing `LayoutDomTypeBundle<'dom>` once
//!   the four trait impls above exist.
//! - Style storage and atom storage side-tables (per the Stylo paper-probe
//!   findings — `borrow_data()` / `id()` / `each_class()` demand them).
//!
//! These get implemented one trait at a time, starting with TNode +
//! DangerousStyleNode as the smaller surface, then expanding to TElement
//! and selectors::Element. Each can land as its own commit with the
//! audit canary as the load-bearing check.

use std::marker::PhantomData;

use layout_dom_api::LayoutDom;

/// A handle into a `LayoutDom`-backed DOM, suitable for the layout crate's
/// node/element-type expectations. Carries a borrow of the DOM (`&'a D`)
/// plus the node identity.
///
/// **Stateful Stylo data is not carried here yet.** When the trait impls
/// for `DangerousStyleElement` + `TElement` land, this adapter will grow
/// references to a style storage side-table (`&'a StyleStorage<D::NodeId>`)
/// and an atom-interned id/class storage (`&'a AtomStorage<D::NodeId>`),
/// per the Stylo paper-probe findings in
/// `docs/2026-05-16_layout_dom_api_design.md`.
pub struct LayoutDomAdapter<'a, D: LayoutDom> {
    pub(crate) dom: &'a D,
    pub(crate) id: D::NodeId,
}

impl<'a, D: LayoutDom> LayoutDomAdapter<'a, D> {
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
        self.dom.dom_children(self.id).map(move |id| Self { dom, id })
    }

    /// Flat-tree children (shadow-aware) as fresh adapters. Defaults to
    /// `dom_children` for backends without shadow DOM.
    pub fn flat_children(&self) -> impl Iterator<Item = Self> + '_ {
        let dom = self.dom;
        self.dom.flat_children(self.id).map(move |id| Self { dom, id })
    }

    /// What kind of node this is.
    pub fn kind(&self) -> layout_dom_api::NodeKind {
        self.dom.kind(self.id)
    }
}

impl<'a, D: LayoutDom> Clone for LayoutDomAdapter<'a, D> {
    fn clone(&self) -> Self {
        Self {
            dom: self.dom,
            id: self.id,
        }
    }
}

impl<'a, D: LayoutDom> Copy for LayoutDomAdapter<'a, D> {}

impl<'a, D: LayoutDom> std::fmt::Debug for LayoutDomAdapter<'a, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutDomAdapter")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl<'a, D: LayoutDom> PartialEq for LayoutDomAdapter<'a, D> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.dom, other.dom) && self.id == other.id
    }
}

impl<'a, D: LayoutDom> Eq for LayoutDomAdapter<'a, D> {}

/// `LayoutDomTypeBundle` impl placeholder. Once the four trait impls
/// (`LayoutNode`, `LayoutElement`, `DangerousStyleNode`,
/// `DangerousStyleElement`) land on `LayoutDomAdapter`, this struct
/// gets the actual `impl LayoutDomTypeBundle<'dom>` block pointing all
/// four concrete types at `LayoutDomAdapter<'dom, D>`.
///
/// Kept as a phantom-data marker today so the bundle "exists" without
/// committing to a partial trait impl.
pub struct LayoutDomBundle<D: LayoutDom>(PhantomData<D>);

#[cfg(test)]
mod tests {
    use html5ever::local_name;
    use layout_dom_api::LayoutDom;
    use serval_static_dom::StaticDocument;

    use super::*;

    fn find_element_descendant<'a, D: LayoutDom>(
        start: &LayoutDomAdapter<'a, D>,
        local: html5ever::LocalName,
    ) -> Option<LayoutDomAdapter<'a, D>> {
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
        let root = LayoutDomAdapter::document(&document);

        let body = find_element_descendant(&root, local_name!("body"))
            .expect("body element exists");
        let p = find_element_descendant(&body, local_name!("p"))
            .expect("p element under body");

        let text = p.dom_children().next().expect("text node under p");
        assert!(matches!(text.kind(), layout_dom_api::NodeKind::Text));
        assert_eq!(text.dom().text(text.id()), Some("Hello"));
    }

    #[test]
    fn adapter_sibling_navigation() {
        let document = StaticDocument::parse(
            "<html><body><p>a</p><p>b</p><p>c</p></body></html>",
        );
        let root = LayoutDomAdapter::document(&document);
        let body = find_element_descendant(&root, local_name!("body"))
            .expect("body element exists");

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
        let root = LayoutDomAdapter::document(&document);
        let html = root.dom_children().next().expect("html");
        assert_eq!(html.parent().map(|p| p.id()), Some(root.id()));
    }
}
