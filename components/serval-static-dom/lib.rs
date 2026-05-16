/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use std::rc::Rc;

use html5ever::interface::ElemName;
use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, LocalName, Namespace, QualName, parse_document};

/// Stable identifier for a node in a [`StaticDocument`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StaticNodeId(usize);

/// A script-free HTML document tree.
#[derive(Clone, Debug)]
pub struct StaticDocument {
    nodes: Vec<StaticNode>,
    document: StaticNodeId,
    quirks_mode: StaticQuirksMode,
}

impl StaticDocument {
    /// Parse a full HTML document with html5ever.
    pub fn parse(input: &str) -> Self {
        parse_document(StaticTreeSink::new(), Default::default()).one(input)
    }

    /// Return the document node id.
    pub fn document_node(&self) -> StaticNodeId {
        self.document
    }

    /// Return the parser-selected quirks mode.
    pub fn quirks_mode(&self) -> StaticQuirksMode {
        self.quirks_mode
    }

    /// Return a node by id.
    pub fn node(&self, id: StaticNodeId) -> &StaticNode {
        &self.nodes[id.0]
    }

    /// Return the first element child of the document, normally `<html>`.
    pub fn document_element(&self) -> Option<StaticNodeId> {
        self.node(self.document)
            .children
            .iter()
            .copied()
            .find(|id| matches!(self.node(*id).kind, StaticNodeKind::Element { .. }))
    }
}

/// Static document quirks mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaticQuirksMode {
    /// Standards mode.
    NoQuirks,
    /// Limited quirks mode.
    LimitedQuirks,
    /// Full quirks mode.
    Quirks,
}

impl From<QuirksMode> for StaticQuirksMode {
    fn from(value: QuirksMode) -> Self {
        match value {
            QuirksMode::NoQuirks => Self::NoQuirks,
            QuirksMode::LimitedQuirks => Self::LimitedQuirks,
            QuirksMode::Quirks => Self::Quirks,
        }
    }
}

/// A node in a [`StaticDocument`].
#[derive(Clone, Debug)]
pub struct StaticNode {
    parent: Option<StaticNodeId>,
    children: Vec<StaticNodeId>,
    kind: StaticNodeKind,
}

impl StaticNode {
    /// Return this node's parent.
    pub fn parent(&self) -> Option<StaticNodeId> {
        self.parent
    }

    /// Return this node's children.
    pub fn children(&self) -> &[StaticNodeId] {
        &self.children
    }

    /// Return this node's kind.
    pub fn kind(&self) -> &StaticNodeKind {
        &self.kind
    }
}

/// Script-free static node payload.
#[derive(Clone, Debug)]
pub enum StaticNodeKind {
    /// The document root.
    Document,
    /// A doctype node.
    Doctype {
        /// Doctype name.
        name: String,
        /// Public identifier.
        public_id: String,
        /// System identifier.
        system_id: String,
    },
    /// A text node.
    Text(String),
    /// A comment node.
    Comment(String),
    /// An element node.
    Element {
        /// Qualified element name.
        name: QualName,
        /// Element attributes.
        attrs: Vec<Attribute>,
        /// Template content document fragment, if this is a template element.
        template_contents: Option<StaticNodeId>,
        /// Whether this is a MathML annotation-xml integration point.
        mathml_annotation_xml_integration_point: bool,
    },
    /// A processing instruction node.
    ProcessingInstruction {
        /// Processing instruction target.
        target: String,
        /// Processing instruction contents.
        contents: String,
    },
}

#[derive(Clone)]
struct StaticTree {
    nodes: Vec<StaticNode>,
    quirks_mode: StaticQuirksMode,
}

impl StaticTree {
    fn new() -> Self {
        Self {
            nodes: vec![StaticNode {
                parent: None,
                children: Vec::new(),
                kind: StaticNodeKind::Document,
            }],
            quirks_mode: StaticQuirksMode::NoQuirks,
        }
    }

    fn push(&mut self, kind: StaticNodeKind) -> StaticNodeId {
        let id = StaticNodeId(self.nodes.len());
        self.nodes.push(StaticNode {
            parent: None,
            children: Vec::new(),
            kind,
        });
        id
    }

    fn append(&mut self, parent: StaticNodeId, child: StaticNodeId) {
        self.detach(child);
        self.nodes[child.0].parent = Some(parent);
        self.nodes[parent.0].children.push(child);
    }

    fn insert_before(&mut self, sibling: StaticNodeId, child: StaticNodeId) {
        self.detach(child);
        let parent = self.nodes[sibling.0].parent;
        self.nodes[child.0].parent = parent;
        if let Some(parent) = parent {
            let siblings = &mut self.nodes[parent.0].children;
            let index = siblings
                .iter()
                .position(|candidate| *candidate == sibling)
                .unwrap_or(siblings.len());
            siblings.insert(index, child);
        }
    }

    fn detach(&mut self, target: StaticNodeId) {
        let Some(parent) = self.nodes[target.0].parent.take() else {
            return;
        };
        self.nodes[parent.0]
            .children
            .retain(|child| *child != target);
    }

    fn append_node_or_text(&mut self, parent: StaticNodeId, child: NodeOrText<StaticNodeId>) {
        match child {
            NodeOrText::AppendNode(node) => self.append(parent, node),
            NodeOrText::AppendText(text) => {
                let can_merge = self.nodes[parent.0]
                    .children
                    .last()
                    .copied()
                    .filter(|id| matches!(self.nodes[id.0].kind, StaticNodeKind::Text(_)));
                if let Some(id) = can_merge {
                    if let StaticNodeKind::Text(contents) = &mut self.nodes[id.0].kind {
                        contents.push_str(&text);
                    }
                    return;
                }
                let text = self.push(StaticNodeKind::Text(text.to_string()));
                self.append(parent, text);
            },
        }
    }
}

#[derive(Clone)]
struct StaticTreeSink {
    tree: Rc<RefCell<StaticTree>>,
}

impl StaticTreeSink {
    fn new() -> Self {
        Self {
            tree: Rc::new(RefCell::new(StaticTree::new())),
        }
    }
}

#[derive(Clone)]
struct StaticElemName(QualName);

impl fmt::Debug for StaticElemName {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl ElemName for StaticElemName {
    fn ns(&self) -> &Namespace {
        &self.0.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.0.local
    }
}

impl TreeSink for StaticTreeSink {
    type ElemName<'a> = StaticElemName;
    type Handle = StaticNodeId;
    type Output = StaticDocument;

    fn finish(self) -> StaticDocument {
        let tree = self.tree.borrow().clone();
        StaticDocument {
            nodes: tree.nodes,
            document: StaticNodeId(0),
            quirks_mode: tree.quirks_mode,
        }
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> StaticNodeId {
        StaticNodeId(0)
    }

    fn elem_name<'a>(&'a self, target: &'a StaticNodeId) -> StaticElemName {
        match &self.tree.borrow().nodes[target.0].kind {
            StaticNodeKind::Element { name, .. } => StaticElemName(name.clone()),
            _ => panic!("node is not an element"),
        }
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        flags: ElementFlags,
    ) -> StaticNodeId {
        let mut tree = self.tree.borrow_mut();
        let template_contents = flags.template.then(|| tree.push(StaticNodeKind::Document));
        tree.push(StaticNodeKind::Element {
            name,
            attrs,
            template_contents,
            mathml_annotation_xml_integration_point: flags.mathml_annotation_xml_integration_point,
        })
    }

    fn create_comment(&self, text: StrTendril) -> StaticNodeId {
        self.tree
            .borrow_mut()
            .push(StaticNodeKind::Comment(text.to_string()))
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> StaticNodeId {
        self.tree
            .borrow_mut()
            .push(StaticNodeKind::ProcessingInstruction {
                target: target.to_string(),
                contents: data.to_string(),
            })
    }

    fn append(&self, parent: &StaticNodeId, child: NodeOrText<StaticNodeId>) {
        self.tree.borrow_mut().append_node_or_text(*parent, child);
    }

    fn append_before_sibling(&self, sibling: &StaticNodeId, child: NodeOrText<StaticNodeId>) {
        let mut tree = self.tree.borrow_mut();
        match child {
            NodeOrText::AppendNode(node) => tree.insert_before(*sibling, node),
            NodeOrText::AppendText(text) => {
                let node = tree.push(StaticNodeKind::Text(text.to_string()));
                tree.insert_before(*sibling, node);
            },
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &StaticNodeId,
        prev_element: &StaticNodeId,
        child: NodeOrText<StaticNodeId>,
    ) {
        if self.tree.borrow().nodes[element.0].parent.is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let mut tree = self.tree.borrow_mut();
        let node = tree.push(StaticNodeKind::Doctype {
            name: name.to_string(),
            public_id: public_id.to_string(),
            system_id: system_id.to_string(),
        });
        tree.append(StaticNodeId(0), node);
    }

    fn get_template_contents(&self, target: &StaticNodeId) -> StaticNodeId {
        match self.tree.borrow().nodes[target.0].kind {
            StaticNodeKind::Element {
                template_contents: Some(contents),
                ..
            } => contents,
            _ => panic!("node is not a template element"),
        }
    }

    fn same_node(&self, x: &StaticNodeId, y: &StaticNodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.tree.borrow_mut().quirks_mode = mode.into();
    }

    fn add_attrs_if_missing(&self, target: &StaticNodeId, attrs: Vec<Attribute>) {
        let mut tree = self.tree.borrow_mut();
        let StaticNodeKind::Element {
            attrs: existing, ..
        } = &mut tree.nodes[target.0].kind
        else {
            panic!("node is not an element");
        };
        let names = existing
            .iter()
            .map(|attr| attr.name.clone())
            .collect::<HashSet<_>>();
        existing.extend(attrs.into_iter().filter(|attr| !names.contains(&attr.name)));
    }

    fn remove_from_parent(&self, target: &StaticNodeId) {
        self.tree.borrow_mut().detach(*target);
    }

    fn reparent_children(&self, node: &StaticNodeId, new_parent: &StaticNodeId) {
        let children = {
            let mut tree = self.tree.borrow_mut();
            std::mem::take(&mut tree.nodes[node.0].children)
        };
        for child in children {
            self.tree.borrow_mut().append(*new_parent, child);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &StaticNodeId) -> bool {
        matches!(
            self.tree.borrow().nodes[handle.0].kind,
            StaticNodeKind::Element {
                mathml_annotation_xml_integration_point: true,
                ..
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use html5ever::{local_name, ns};

    use super::*;

    #[test]
    fn parses_document_element() {
        let document =
            StaticDocument::parse("<!doctype html><html><body><p>Hello</p></body></html>");
        let html = document
            .document_element()
            .expect("missing document element");
        let StaticNodeKind::Element { name, .. } = document.node(html).kind() else {
            panic!("document element should be an element");
        };
        assert_eq!(name.ns, ns!(html));
        assert_eq!(name.local, local_name!("html"));
    }
}
