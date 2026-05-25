/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Mutable scripted-DOM provider.
//!
//! `ScriptedDom` is the mutable sibling of `serval-static-dom`'s `StaticDocument`:
//! a `NodeId`-keyed arena that implements [`LayoutDom`] (read) and [`LayoutDomMut`]
//! (mutate), recording each structural change as a [`DomMutation`] for
//! serval-layout's scheduler to translate into invalidation. The arena owns the
//! node data; JS reflectors bridge back to it by `NodeId` (via
//! `script-engine-api`'s `make_reflector`/`reflector_data`), so the engine never
//! owns DOM data.
//!
//! Scope (2026-05-23): structural mutation + the mutation stream. The reflector
//! bridge wiring and the DomMutation → serval-layout invalidation loop are the next
//! pass (they need the `script-runtime-api` host layer and serval-layout's
//! scheduler). `set_inner_html` is deferred — it needs html5ever fragment parsing.

#![deny(unsafe_code)]

use layout_dom_api::{
    AttributeView, DomMutation, LayoutDom, LayoutDomMut, LocalName, Namespace, NodeKind, QualName,
};
use serval_static_dom::{StaticDocument, StaticNodeId};

/// Opaque node identity: a stable index into the arena (slots are never reused, so
/// ids stay valid for the document's lifetime).
// `usize`-backed (pointer-sized): serval-layout's Stylo style-sharing cache asserts
// `size_of::<NodeId>() == size_of::<usize>()` (it packs the id into a pointer-shaped
// `OpaqueElement`). A `u32` would fail that assertion.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(usize);

impl NodeId {
    /// The raw arena index. The reflector bridge packs this into a
    /// `script_engine_api::ReflectorData` (`u64`) so JS can carry it opaquely.
    pub fn raw(self) -> usize {
        self.0
    }

    /// Rebuild a `NodeId` from a raw index recovered from a reflector.
    pub fn from_raw(raw: usize) -> Self {
        Self(raw)
    }
}

struct Node {
    kind: NodeKind,
    name: Option<QualName>,
    attrs: Vec<(QualName, String)>,
    text: Option<String>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
}

impl Node {
    fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            name: None,
            attrs: Vec::new(),
            text: None,
            parent: None,
            children: Vec::new(),
        }
    }
}

/// A mutable DOM arena. `nodes[i]` is `None` once removed (ids are never reused).
pub struct ScriptedDom {
    nodes: Vec<Option<Node>>,
    root: NodeId,
    mutations: Vec<DomMutation<NodeId>>,
}

impl Default for ScriptedDom {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedDom {
    /// A fresh document with an empty `Document` root.
    pub fn new() -> Self {
        Self {
            nodes: vec![Some(Node::new(NodeKind::Document))],
            root: NodeId(0),
            mutations: Vec::new(),
        }
    }

    fn node(&self, id: NodeId) -> &Node {
        self.nodes[id.0]
            .as_ref()
            .expect("NodeId refers to a live node")
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes[id.0]
            .as_mut()
            .expect("NodeId refers to a live node")
    }

    fn push(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Some(node));
        id
    }

    fn sibling(&self, id: NodeId, delta: isize) -> Option<NodeId> {
        let parent = self.node(id).parent?;
        let kids = &self.node(parent).children;
        let pos = kids.iter().position(|&c| c == id)?;
        let target = pos as isize + delta;
        if target < 0 {
            return None;
        }
        kids.get(target as usize).copied()
    }

    /// Unlink `child` from its current parent (no mutation recorded).
    fn detach(&mut self, child: NodeId) {
        if let Some(parent) = self.node(child).parent {
            let kids = &mut self.node_mut(parent).children;
            if let Some(pos) = kids.iter().position(|&c| c == child) {
                kids.remove(pos);
            }
        }
        self.node_mut(child).parent = None;
    }

    /// Free a node and its whole subtree (slots become `None`).
    fn drop_subtree(&mut self, node: NodeId) {
        let children = std::mem::take(&mut self.node_mut(node).children);
        for child in children {
            self.drop_subtree(child);
        }
        self.nodes[node.0] = None;
    }

    /// Link `child` under `parent` without recording a mutation. Used while
    /// building a parsed subtree, which is covered by one `SubtreeReplaced`.
    fn attach_silent(&mut self, parent: NodeId, child: NodeId) {
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
    }

    /// Deep-copy a node from a parsed [`StaticDocument`] into this arena (silent),
    /// returning the new id.
    fn copy_fragment_node(&mut self, src: &StaticDocument, sid: StaticNodeId) -> NodeId {
        let new = match src.kind(sid) {
            NodeKind::Element => {
                let mut node = Node::new(NodeKind::Element);
                node.name = src.element_name(sid).cloned();
                for attr in src.attributes(sid) {
                    node.attrs.push((attr.name.clone(), attr.value.to_owned()));
                }
                self.push(node)
            }
            kind @ (NodeKind::Text | NodeKind::Comment) => {
                let mut node = Node::new(kind);
                node.text = src.text(sid).map(str::to_owned);
                self.push(node)
            }
            other => self.push(Node::new(other)),
        };
        let children: Vec<StaticNodeId> = src.dom_children(sid).collect();
        for child in children {
            let copied = self.copy_fragment_node(src, child);
            self.attach_silent(new, copied);
        }
        new
    }

    /// The `<body>` element of a `parse_document`-parsed fragment, if present.
    fn fragment_body(doc: &StaticDocument) -> Option<StaticNodeId> {
        let html = doc.document_element()?;
        doc.dom_children(html)
            .find(|&c| doc.element_name(c).is_some_and(|q| q.local == LocalName::from("body")))
    }
}

impl LayoutDom for ScriptedDom {
    type NodeId = NodeId;

    fn document(&self) -> NodeId {
        self.root
    }

    fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.sibling(id, -1)
    }

    fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.sibling(id, 1)
    }

    fn dom_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.node(id).children.iter().copied()
    }

    fn kind(&self, id: NodeId) -> NodeKind {
        self.node(id).kind
    }

    fn opaque_id(&self, id: NodeId) -> u64 {
        id.0 as u64
    }

    fn element_name(&self, id: NodeId) -> Option<&QualName> {
        self.node(id).name.as_ref()
    }

    fn attribute(&self, id: NodeId, ns: &Namespace, local: &LocalName) -> Option<&str> {
        self.node(id)
            .attrs
            .iter()
            .find(|(name, _)| &name.ns == ns && &name.local == local)
            .map(|(_, value)| value.as_str())
    }

    fn attributes(&self, id: NodeId) -> impl Iterator<Item = AttributeView<'_>> + '_ {
        self.node(id)
            .attrs
            .iter()
            .map(|(name, value)| AttributeView {
                name,
                value: value.as_str(),
            })
    }

    fn text(&self, id: NodeId) -> Option<&str> {
        self.node(id).text.as_deref()
    }
}

impl LayoutDomMut for ScriptedDom {
    fn create_element(&mut self, name: QualName) -> NodeId {
        let mut node = Node::new(NodeKind::Element);
        node.name = Some(name);
        self.push(node)
    }

    fn create_text(&mut self, data: &str) -> NodeId {
        let mut node = Node::new(NodeKind::Text);
        node.text = Some(data.to_owned());
        self.push(node)
    }

    fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
        self.mutations
            .push(DomMutation::Inserted { node: child, parent });
    }

    fn remove(&mut self, node: NodeId) {
        let former_parent = self.node(node).parent;
        self.detach(node);
        if let Some(former_parent) = former_parent {
            self.mutations.push(DomMutation::Removed {
                node,
                former_parent,
            });
        }
        self.drop_subtree(node);
    }

    fn set_attribute(&mut self, node: NodeId, name: QualName, value: &str) {
        let attrs = &mut self.node_mut(node).attrs;
        // Capture the prior value before overwriting — serval-layout needs
        // it to build the Stylo snapshot at restyle time (the old value is
        // gone from the live DOM once we mutate). `None` = newly added.
        let old_value;
        if let Some(existing) = attrs
            .iter_mut()
            .find(|(n, _)| n.ns == name.ns && n.local == name.local)
        {
            old_value = Some(std::mem::replace(&mut existing.1, value.to_owned()));
        } else {
            old_value = None;
            attrs.push((name.clone(), value.to_owned()));
        }
        self.mutations.push(DomMutation::AttributeChanged {
            node,
            name,
            old_value,
        });
    }

    fn set_text(&mut self, node: NodeId, data: &str) {
        self.node_mut(node).text = Some(data.to_owned());
        self.mutations
            .push(DomMutation::CharacterDataChanged { node });
    }

    fn set_inner_html(&mut self, node: NodeId, html: &str) {
        // Drop the current children silently — the single SubtreeReplaced covers it.
        let existing = std::mem::take(&mut self.node_mut(node).children);
        for child in existing {
            self.node_mut(child).parent = None;
            self.drop_subtree(child);
        }
        // Parse via the static parser (a LayoutDom) and copy the <body> children in.
        // (Simplification: uses `parse_document` + the body subtree rather than true
        // context-aware fragment parsing; fine for the common element-fragment case.)
        let fragment = StaticDocument::parse(html);
        if let Some(body) = Self::fragment_body(&fragment) {
            let body_children: Vec<StaticNodeId> = fragment.dom_children(body).collect();
            for child in body_children {
                let copied = self.copy_fragment_node(&fragment, child);
                self.attach_silent(node, copied);
            }
        }
        self.mutations.push(DomMutation::SubtreeReplaced { node });
    }

    fn drain_mutations(&mut self, out: &mut Vec<DomMutation<NodeId>>) {
        out.append(&mut self.mutations);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qual(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    #[test]
    fn mutate_read_and_record() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();

        let div = dom.create_element(qual("div"));
        dom.append_child(root, div);
        let text = dom.create_text("hello");
        dom.append_child(div, text);
        dom.set_attribute(div, qual("id"), "main");

        // Read surface reflects the mutations.
        assert_eq!(dom.kind(div), NodeKind::Element);
        assert_eq!(dom.element_name(div).unwrap().local, LocalName::from("div"));
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![div]);
        assert_eq!(dom.dom_children(div).collect::<Vec<_>>(), vec![text]);
        assert_eq!(dom.parent(text), Some(div));
        assert_eq!(dom.text(text), Some("hello"));
        assert_eq!(
            dom.attribute(div, &Namespace::from(""), &LocalName::from("id")),
            Some("main")
        );

        // Mutation stream: 2 inserts + 1 attribute change, then drained empty.
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert_eq!(muts.len(), 3);
        let mut again = Vec::new();
        dom.drain_mutations(&mut again);
        assert!(again.is_empty());
    }

    #[test]
    fn set_inner_html_builds_subtree() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(qual("div"));
        dom.append_child(root, div);
        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained); // clear the append

        dom.set_inner_html(div, "<p>hi</p><span>x</span>");

        let kids: Vec<_> = dom.dom_children(div).collect();
        assert_eq!(kids.len(), 2);
        assert_eq!(dom.element_name(kids[0]).unwrap().local, LocalName::from("p"));
        assert_eq!(
            dom.element_name(kids[1]).unwrap().local,
            LocalName::from("span")
        );
        // <p>hi</p> — the <p> has a single text child "hi".
        let p_kids: Vec<_> = dom.dom_children(kids[0]).collect();
        assert_eq!(p_kids.len(), 1);
        assert_eq!(dom.text(p_kids[0]), Some("hi"));

        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert!(matches!(muts.as_slice(), [DomMutation::SubtreeReplaced { .. }]));
    }

    #[test]
    fn siblings_and_remove() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let a = dom.create_element(qual("a"));
        let b = dom.create_element(qual("b"));
        dom.append_child(root, a);
        dom.append_child(root, b);

        assert_eq!(dom.next_sibling(a), Some(b));
        assert_eq!(dom.prev_sibling(b), Some(a));
        assert_eq!(dom.prev_sibling(a), None);

        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained); // clear the two inserts

        dom.remove(a);
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![b]);
        assert_eq!(dom.next_sibling(b), None);

        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert!(matches!(
            muts.as_slice(),
            [DomMutation::Removed { .. }]
        ));
    }
}
