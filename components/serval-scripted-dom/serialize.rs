//! HTML serialization of the scripted DOM via html5ever's serializer — the inverse
//! of the `TreeSink` parse path serval-static-dom already implements. Powers
//! `outerHTML` / `innerHTML` and the verso flip's DOM-snapshot layer.
//!
//! This side only walks the tree and names start/end elements, text, comments, and
//! doctype; html5ever owns the correctness it is famous for: void elements (no close
//! tag), attribute and text escaping, and raw-text elements (`<script>`/`<style>`).
//! Mirrors servo's `dom/node.rs` serialize.

use std::io;

use html5ever::serialize::{SerializeOpts, serialize as html5ever_serialize};
use layout_dom_api::NodeKind;
use markup5ever::serialize::{Serialize, Serializer, TraversalScope};

use crate::{NodeId, ScriptedDom};

/// A `(ScriptedDom, NodeId)` pair html5ever's serializer can walk.
struct SerializableNode<'a> {
    dom: &'a ScriptedDom,
    id: NodeId,
}

impl SerializableNode<'_> {
    fn child(&self, id: NodeId) -> SerializableNode<'_> {
        SerializableNode { dom: self.dom, id }
    }

    /// Serialize every child as its own included node (shared by element bodies,
    /// the document/fragment roots, and `ChildrenOnly`).
    fn serialize_children<S: Serializer>(&self, serializer: &mut S) -> io::Result<()> {
        for &child in &self.dom.node(self.id).children {
            self.child(child)
                .serialize(serializer, TraversalScope::IncludeNode)?;
        }
        Ok(())
    }
}

impl Serialize for SerializableNode<'_> {
    fn serialize<S: Serializer>(
        &self,
        serializer: &mut S,
        traversal_scope: TraversalScope,
    ) -> io::Result<()> {
        let node = self.dom.node(self.id);
        match traversal_scope {
            TraversalScope::IncludeNode => match node.kind {
                NodeKind::Element => match &node.name {
                    Some(name) => {
                        serializer.start_elem(
                            name.clone(),
                            node.attrs.iter().map(|(n, v)| (n, v.as_str())),
                        )?;
                        self.serialize_children(serializer)?;
                        serializer.end_elem(name.clone())?;
                    },
                    // A nameless "element" is malformed; emit its children rather
                    // than a bare tag.
                    None => self.serialize_children(serializer)?,
                },
                NodeKind::Text => {
                    if let Some(text) = &node.text {
                        serializer.write_text(text)?;
                    }
                },
                NodeKind::Comment => {
                    if let Some(text) = &node.text {
                        serializer.write_comment(text)?;
                    }
                },
                NodeKind::Doctype => {
                    serializer.write_doctype(node.text.as_deref().unwrap_or("html"))?;
                },
                NodeKind::Document | NodeKind::DocumentFragment => {
                    self.serialize_children(serializer)?;
                },
                // Processing instructions are not valid HTML (html5ever parses them
                // as bogus comments); the scripted DOM never holds one, so emit
                // nothing rather than invalid markup.
                NodeKind::ProcessingInstruction => {},
            },
            TraversalScope::ChildrenOnly(_) => self.serialize_children(serializer)?,
        }
        Ok(())
    }
}

impl ScriptedDom {
    /// Serialize `node` and its subtree to HTML (`outerHTML`).
    ///
    /// Routes through html5ever's serializer, so void elements, attribute/text
    /// escaping, and raw-text elements are handled by the engine, not here.
    pub fn outer_html(&self, node: NodeId) -> String {
        self.serialize_scope(node, TraversalScope::IncludeNode)
    }

    /// Serialize only `node`'s children (`innerHTML`).
    pub fn inner_html(&self, node: NodeId) -> String {
        self.serialize_scope(node, TraversalScope::ChildrenOnly(None))
    }

    fn serialize_scope(&self, node: NodeId, traversal_scope: TraversalScope) -> String {
        let mut buf = Vec::new();
        let opts = SerializeOpts {
            traversal_scope,
            ..SerializeOpts::default()
        };
        // Writing to an in-memory Vec is infallible; html5ever emits UTF-8.
        html5ever_serialize(
            &mut buf,
            &SerializableNode {
                dom: self,
                id: node,
            },
            opts,
        )
        .expect("serializing the scripted DOM to a Vec is infallible");
        String::from_utf8(buf).expect("html5ever emits valid UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html5ever::{local_name, ns};
    use layout_dom_api::{LayoutDomMut, QualName};

    fn html(local: markup5ever::LocalName) -> QualName {
        QualName::new(None, ns!(html), local)
    }

    #[test]
    fn outer_html_emits_tag_attrs_and_escaped_text() {
        let mut dom = ScriptedDom::new();
        let div = dom.create_element(html(local_name!("div")));
        dom.set_attribute(div, QualName::new(None, ns!(), local_name!("class")), "x");
        let text = dom.create_text("hi & bye");
        dom.append_child(div, text);
        // html5ever owns the `&` escaping.
        assert_eq!(dom.outer_html(div), r#"<div class="x">hi &amp; bye</div>"#);
    }

    #[test]
    fn inner_html_skips_the_node_itself() {
        let mut dom = ScriptedDom::new();
        let div = dom.create_element(html(local_name!("div")));
        let span = dom.create_element(html(local_name!("span")));
        dom.append_child(div, span);
        assert_eq!(dom.inner_html(div), "<span></span>");
    }

    #[test]
    fn void_elements_get_no_close_tag() {
        let mut dom = ScriptedDom::new();
        let br = dom.create_element(html(local_name!("br")));
        assert_eq!(dom.outer_html(br), "<br>");
    }
}
