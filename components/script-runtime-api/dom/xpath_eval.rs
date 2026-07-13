// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `Document.evaluate()` native half: adapt the live `ScriptedDom` to the
//! generic XPath 1.0 engine, then serialize the result for the JS `XPathResult`
//! wrapper in `bootstrap.js`.

use std::cmp::Ordering;
use std::rc::Rc;

use layout_dom_api::{LayoutDom, NodeKind};
use markup5ever::{LocalName, Namespace, Prefix};
use script_engine_api::{NativeFn, ScriptEngine};
use genet_scripted_dom::{NodeId, ScriptedDom};
use xpath::Node as _;

use super::*;

#[derive(Clone, Debug)]
struct XPathTree {
    nodes: Vec<XPathNodeData>,
    attrs: Vec<XPathAttrData>,
    root: usize,
}

#[derive(Clone, Debug)]
struct XPathNodeData {
    kind: NodeKind,
    name: Option<QualName>,
    attrs: Vec<usize>,
    text: Option<String>,
    parent: Option<usize>,
    children: Vec<usize>,
    raw: usize,
    order: usize,
}

#[derive(Clone, Debug)]
struct XPathAttrData {
    owner: usize,
    name: QualName,
    value: String,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum XPathKind {
    Node(usize),
    Attr(usize),
}

#[derive(Clone, Debug)]
struct XPathNode {
    tree: Rc<XPathTree>,
    kind: XPathKind,
}

#[derive(Clone, Debug)]
struct XPathElement {
    node: XPathNode,
}

#[derive(Clone, Debug)]
struct XPathAttr {
    node: XPathNode,
}

#[derive(Clone, Debug)]
struct XPathDocument {
    tree: Rc<XPathTree>,
}

#[derive(Clone)]
struct NoNamespaces;

struct XPathDom;

impl xpath::Dom for XPathDom {
    type Node = XPathNode;
    type NamespaceResolver = NoNamespaces;
}

impl xpath::NamespaceResolver for NoNamespaces {
    fn resolve_namespace_prefix(&self, _prefix: &str) -> Option<String> {
        None
    }
}

impl PartialEq for XPathNode {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for XPathNode {}

impl XPathTree {
    fn from_dom(dom: &ScriptedDom, root: NodeId) -> Rc<Self> {
        fn copy_node(
            dom: &ScriptedDom,
            id: NodeId,
            parent: Option<usize>,
            nodes: &mut Vec<XPathNodeData>,
            attrs: &mut Vec<XPathAttrData>,
            order: &mut usize,
        ) -> usize {
            let idx = nodes.len();
            let node_order = *order;
            *order += 1;
            let attr_ids = dom
                .attributes(id)
                .enumerate()
                .map(|(index, attr)| {
                    let attr_idx = attrs.len();
                    attrs.push(XPathAttrData {
                        owner: idx,
                        name: attr.name.clone(),
                        value: attr.value.to_string(),
                        index,
                    });
                    attr_idx
                })
                .collect::<Vec<_>>();

            nodes.push(XPathNodeData {
                kind: dom.kind(id),
                name: dom.element_name(id).cloned(),
                attrs: attr_ids,
                text: dom.text(id).map(str::to_string),
                parent,
                children: Vec::new(),
                raw: id.raw(),
                order: node_order,
            });

            let children = dom
                .dom_children(id)
                .map(|child| copy_node(dom, child, Some(idx), nodes, attrs, order))
                .collect::<Vec<_>>();
            nodes[idx].children = children;
            idx
        }

        let mut nodes = Vec::new();
        let mut attrs = Vec::new();
        let mut order = 0;
        let root = copy_node(dom, root, None, &mut nodes, &mut attrs, &mut order);
        Rc::new(Self { nodes, attrs, root })
    }

    fn node(self: &Rc<Self>, idx: usize) -> XPathNode {
        XPathNode {
            tree: self.clone(),
            kind: XPathKind::Node(idx),
        }
    }

    fn attr(self: &Rc<Self>, idx: usize) -> XPathNode {
        XPathNode {
            tree: self.clone(),
            kind: XPathKind::Attr(idx),
        }
    }
}

impl XPathNode {
    fn node_data(&self) -> Option<&XPathNodeData> {
        match self.kind {
            XPathKind::Node(idx) => self.tree.nodes.get(idx),
            XPathKind::Attr(_) => None,
        }
    }

    fn attr_data(&self) -> Option<&XPathAttrData> {
        match self.kind {
            XPathKind::Attr(idx) => self.tree.attrs.get(idx),
            XPathKind::Node(_) => None,
        }
    }

    fn order_key(&self) -> (usize, usize) {
        match self.kind {
            XPathKind::Node(idx) => (self.tree.nodes[idx].order * 2, 0),
            XPathKind::Attr(idx) => {
                let attr = &self.tree.attrs[idx];
                (self.tree.nodes[attr.owner].order * 2 + 1, attr.index)
            },
        }
    }

    fn collect_text(&self, out: &mut String) {
        match self.kind {
            XPathKind::Attr(idx) => out.push_str(&self.tree.attrs[idx].value),
            XPathKind::Node(idx) => {
                if let Some(text) = &self.tree.nodes[idx].text {
                    out.push_str(text);
                }
                for &child in &self.tree.nodes[idx].children {
                    self.tree.node(child).collect_text(out);
                }
            },
        }
    }

    fn collect_preorder(&self, out: &mut Vec<XPathNode>) {
        out.push(self.clone());
        if let XPathKind::Node(idx) = self.kind {
            for &child in &self.tree.nodes[idx].children {
                self.tree.node(child).collect_preorder(out);
            }
        }
    }

    fn root(&self) -> XPathNode {
        self.tree.node(self.tree.root)
    }

    fn raw_node(&self) -> Option<usize> {
        self.node_data().map(|d| d.raw)
    }

    fn is_ancestor_of(&self, other: &XPathNode) -> bool {
        let mut current = other.parent();
        while let Some(node) = current {
            if node == *self {
                return true;
            }
            current = node.parent();
        }
        false
    }
}

impl xpath::Node for XPathNode {
    type ProcessingInstruction = XPathNode;
    type Document = XPathDocument;
    type Attribute = XPathAttr;
    type Element = XPathElement;
    type Opaque = XPathKind;

    fn is_comment(&self) -> bool {
        self.node_data()
            .is_some_and(|d| d.kind == NodeKind::Comment)
    }

    fn is_text(&self) -> bool {
        self.node_data().is_some_and(|d| d.kind == NodeKind::Text)
    }

    fn text_content(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn language(&self) -> Option<String> {
        let mut current = Some(self.clone());
        while let Some(node) = current {
            if let XPathKind::Node(idx) = node.kind {
                for &attr in &node.tree.nodes[idx].attrs {
                    let attr = &node.tree.attrs[attr];
                    if attr.name.local.as_ref().eq_ignore_ascii_case("lang") {
                        return Some(attr.value.clone());
                    }
                }
            }
            current = node.parent();
        }
        None
    }

    fn parent(&self) -> Option<Self> {
        match self.kind {
            XPathKind::Node(idx) => self.tree.nodes[idx].parent.map(|p| self.tree.node(p)),
            XPathKind::Attr(idx) => Some(self.tree.node(self.tree.attrs[idx].owner)),
        }
    }

    fn children(&self) -> impl Iterator<Item = Self> {
        match self.kind {
            XPathKind::Node(idx) => self.tree.nodes[idx]
                .children
                .iter()
                .map(|&child| self.tree.node(child))
                .collect::<Vec<_>>()
                .into_iter(),
            XPathKind::Attr(_) => Vec::new().into_iter(),
        }
    }

    fn compare_tree_order(&self, other: &Self) -> Ordering {
        self.order_key().cmp(&other.order_key())
    }

    fn traverse_preorder(&self) -> impl Iterator<Item = Self> {
        let mut out = Vec::new();
        self.collect_preorder(&mut out);
        out.into_iter()
    }

    fn inclusive_ancestors(&self) -> impl Iterator<Item = Self> {
        let mut out = Vec::new();
        let mut current = Some(self.clone());
        while let Some(node) = current {
            current = node.parent();
            out.push(node);
        }
        out.into_iter()
    }

    fn preceding_nodes(&self) -> impl Iterator<Item = Self> {
        let mut nodes = self.root().traverse_preorder().collect::<Vec<_>>();
        nodes.retain(|node| {
            node == self || (node.compare_tree_order(self).is_lt() && !node.is_ancestor_of(self))
        });
        nodes.reverse();
        nodes.into_iter()
    }

    fn following_nodes(&self) -> impl Iterator<Item = Self> {
        let mut nodes = self.root().traverse_preorder().collect::<Vec<_>>();
        nodes.retain(|node| {
            node == self || (node.compare_tree_order(self).is_gt() && !self.is_ancestor_of(node))
        });
        nodes.into_iter()
    }

    fn preceding_siblings(&self) -> impl Iterator<Item = Self> {
        let Some(parent) = self.parent() else {
            return Vec::new().into_iter();
        };
        let mut out = Vec::new();
        for child in parent.children() {
            if child == *self {
                break;
            }
            out.push(child);
        }
        out.into_iter()
    }

    fn following_siblings(&self) -> impl Iterator<Item = Self> {
        let Some(parent) = self.parent() else {
            return Vec::new().into_iter();
        };
        let mut seen_self = false;
        let mut out = Vec::new();
        for child in parent.children() {
            if seen_self {
                out.push(child);
            } else if child == *self {
                seen_self = true;
            }
        }
        out.into_iter()
    }

    fn owner_document(&self) -> Self::Document {
        XPathDocument {
            tree: self.tree.clone(),
        }
    }

    fn to_opaque(&self) -> Self::Opaque {
        self.kind
    }

    fn as_processing_instruction(&self) -> Option<Self::ProcessingInstruction> {
        self.node_data()
            .is_some_and(|d| d.kind == NodeKind::ProcessingInstruction)
            .then(|| self.clone())
    }

    fn as_attribute(&self) -> Option<Self::Attribute> {
        matches!(self.kind, XPathKind::Attr(_)).then(|| XPathAttr { node: self.clone() })
    }

    fn as_element(&self) -> Option<Self::Element> {
        self.node_data()
            .is_some_and(|d| d.kind == NodeKind::Element)
            .then(|| XPathElement { node: self.clone() })
    }

    fn get_root_node(&self) -> Self {
        self.root()
    }
}

impl xpath::ProcessingInstruction for XPathNode {
    fn target(&self) -> String {
        String::new()
    }
}

impl xpath::Document for XPathDocument {
    type Node = XPathNode;

    fn get_elements_with_id(
        &self,
        id: &str,
    ) -> impl Iterator<Item = <Self::Node as xpath::Node>::Element> {
        self.tree
            .node(self.tree.root)
            .traverse_preorder()
            .filter_map(|node| {
                let XPathKind::Node(idx) = node.kind else {
                    return None;
                };
                let has_id = self.tree.nodes[idx].attrs.iter().any(|&attr| {
                    let attr = &self.tree.attrs[attr];
                    attr.name.local.as_ref() == "id" && attr.value == id
                });
                (has_id && self.tree.nodes[idx].kind == NodeKind::Element)
                    .then(|| XPathElement { node })
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

impl xpath::Element for XPathElement {
    type Node = XPathNode;
    type Attribute = XPathAttr;

    fn as_node(&self) -> Self::Node {
        self.node.clone()
    }

    fn prefix(&self) -> Option<Prefix> {
        self.node
            .node_data()
            .and_then(|d| d.name.as_ref()?.prefix.clone())
    }

    fn namespace(&self) -> Namespace {
        self.node
            .node_data()
            .and_then(|d| d.name.as_ref().map(|q| q.ns.clone()))
            .unwrap_or_default()
    }

    fn local_name(&self) -> LocalName {
        self.node
            .node_data()
            .and_then(|d| d.name.as_ref().map(|q| q.local.clone()))
            .unwrap_or_default()
    }

    fn attributes(&self) -> impl Iterator<Item = Self::Attribute> {
        match self.node.kind {
            XPathKind::Node(idx) => self.node.tree.nodes[idx]
                .attrs
                .iter()
                .map(|&attr| XPathAttr {
                    node: self.node.tree.attr(attr),
                })
                .collect::<Vec<_>>()
                .into_iter(),
            XPathKind::Attr(_) => Vec::new().into_iter(),
        }
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.namespace().as_ref() == XHTML_NS
    }
}

impl xpath::Attribute for XPathAttr {
    type Node = XPathNode;

    fn as_node(&self) -> Self::Node {
        self.node.clone()
    }

    fn prefix(&self) -> Option<Prefix> {
        self.node.attr_data().and_then(|d| d.name.prefix.clone())
    }

    fn namespace(&self) -> Namespace {
        self.node
            .attr_data()
            .map(|d| d.name.ns.clone())
            .unwrap_or_default()
    }

    fn local_name(&self) -> LocalName {
        self.node
            .attr_data()
            .map(|d| d.name.local.clone())
            .unwrap_or_default()
    }
}

/// `__xpathEvaluate(expression, contextNode)` -> a line-based record:
///
/// - `boolean\ntrue|false`
/// - `number\n...`
/// - `string\n...`
/// - `nodes\nraw,raw,...`
/// - `error\n...`
pub(crate) struct EvaluateXPath;

impl<E: ScriptEngine> NativeFn<E> for EvaluateXPath {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let expr_v = cx.arg(0);
        let expression = cx.value_to_string(&expr_v)?;
        let context_v = cx.arg(1);
        let Some(context_raw) = cx.reflector_data(&context_v) else {
            return cx.make_string("error\nDocument.evaluate requires a context node");
        };

        let record = with_dom::<E, _>(cx, |dom| {
            let context_id = NodeId::from_raw(context_raw as usize);
            if !dom.is_live(context_id) {
                return "error\nDocument.evaluate context node is not live".to_string();
            }
            let tree = XPathTree::from_dom(dom, dom.document());
            let Some(context_idx) = tree
                .nodes
                .iter()
                .position(|node| node.raw == context_id.raw())
            else {
                return "error\nDocument.evaluate context node is outside this document"
                    .to_string();
            };
            let parsed = match xpath::parse(&expression, None::<NoNamespaces>, true) {
                Ok(parsed) => parsed,
                Err(err) => return format!("error\n{err:?}"),
            };
            match xpath::evaluate_parsed_xpath::<XPathDom>(&parsed, tree.node(context_idx)) {
                Ok(xpath::Value::Boolean(value)) => format!("boolean\n{value}"),
                Ok(xpath::Value::Number(value)) => format!("number\n{value}"),
                Ok(xpath::Value::String(value)) => format!("string\n{value}"),
                Ok(xpath::Value::NodeSet(nodes)) => {
                    let ids = nodes
                        .into_iter()
                        .filter_map(|node| node.raw_node())
                        .map(|raw| raw.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("nodes\n{ids}")
                },
                Err(err) => format!("error\n{err:?}"),
            }
        })
        .unwrap_or_else(|| "error\nDocument.evaluate has no host DOM".to_string());

        cx.make_string(&record)
    }
}
