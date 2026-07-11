// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! DOM creation, document-structure, attribute-write, and read command sinks.

use super::*;

/// `__documentRoot()` → a reflector for the document node.
pub(crate) struct DocumentRoot;
impl<E: ScriptEngine> NativeFn<E> for DocumentRoot {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        match with_dom::<E, _>(cx, |dom| dom.document()) {
            Some(root) => reflect_pinned::<E>(cx, root.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__reflectNode(rawId)` → the canonical reflector for an **already-existing**
/// node identified by its raw id (a host-side `NodeId::raw()`), pinned like any
/// node handed to script. This is the inbound counterpart to the outbound
/// node-returning natives: the host (e.g. a hit-test that yields a `NodeId`)
/// needs a JS handle for a node it found in Rust, with no DOM query to reach it.
/// `null` if the argument is not a parseable raw id. Paired in the bootstrap with
/// `wrapNode(...)` and exposed to the host through `__dispatchSynthetic`.
pub(crate) struct ReflectNode;
impl<E: ScriptEngine> NativeFn<E> for ReflectNode {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        match cx.value_to_string(&a0)?.parse::<u64>() {
            Ok(raw) => reflect_pinned::<E>(cx, raw),
            Err(_) => Ok(cx.make_null()),
        }
    }
}

/// `__createElement(tag)` → a reflector for the new (unparented) element.
pub(crate) struct CreateElement;
impl<E: ScriptEngine> NativeFn<E> for CreateElement {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let arg = cx.arg(0);
        let tag = cx.value_to_string(&arg)?;
        match with_dom::<E, _>(cx, |dom| dom.create_element(html_qual(&tag))) {
            Some(id) => reflect_pinned::<E>(cx, id.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__createTextNode(data)` → a reflector for the new (unparented) text node.
pub(crate) struct CreateTextNode;
impl<E: ScriptEngine> NativeFn<E> for CreateTextNode {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let arg = cx.arg(0);
        let data = cx.value_to_string(&arg)?;
        match with_dom::<E, _>(cx, |dom| dom.create_text(&data)) {
            Some(id) => reflect_pinned::<E>(cx, id.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__appendChild(parent, child)` — both reflectors.
pub(crate) struct AppendChild;
impl<E: ScriptEngine> NativeFn<E> for AppendChild {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let parent = cx.arg(0);
        let child = cx.arg(1);
        if let (Some(p), Some(c)) = (cx.reflector_data(&parent), cx.reflector_data(&child)) {
            with_dom::<E, _>(cx, |dom| {
                dom.append_child(NodeId::from_raw(p as usize), NodeId::from_raw(c as usize))
            });
        }
        Ok(cx.undefined())
    }
}

/// `__setAttribute(element, name, value)` — element reflector + two strings.
pub(crate) struct SetAttribute;
impl<E: ScriptEngine> NativeFn<E> for SetAttribute {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.undefined());
        };
        let name_v = cx.arg(1);
        let value_v = cx.arg(2);
        let name = cx.value_to_string(&name_v)?;
        let value = cx.value_to_string(&value_v)?;
        with_dom::<E, _>(cx, |dom| {
            dom.set_attribute(NodeId::from_raw(id as usize), attr_qual(&name), &value)
        });
        Ok(cx.undefined())
    }
}

/// `__setTextContent(node, text)` — the `textContent` setter sink.
pub(crate) struct SetTextContent;
impl<E: ScriptEngine> NativeFn<E> for SetTextContent {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return Ok(cx.undefined());
        };
        let text_v = cx.arg(1);
        let text = cx.value_to_string(&text_v)?;
        with_dom::<E, _>(cx, |dom| dom.set_text(NodeId::from_raw(id as usize), &text));
        Ok(cx.undefined())
    }
}

/// `__getElementById(scope, id)` → a reflector for the match under `scope`, or
/// `undefined`.
pub(crate) struct GetElementById;
impl<E: ScriptEngine> NativeFn<E> for GetElementById {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(root) = cx.reflector_data(&scope) else {
            return Ok(cx.undefined());
        };
        let arg = cx.arg(1);
        let id = cx.value_to_string(&arg)?;
        match with_dom::<E, _>(cx, |dom| {
            find_by_id(dom, NodeId::from_raw(root as usize), &id)
        })
        .flatten()
        {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__getAttribute(element, name)` → the attribute string, or `null` if absent.
pub(crate) struct GetAttribute;
impl<E: ScriptEngine> NativeFn<E> for GetAttribute {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let name_v = cx.arg(1);
        let name = cx.value_to_string(&name_v)?;
        let value = with_dom::<E, _>(cx, |dom| {
            dom.attribute(
                NodeId::from_raw(id as usize),
                &Namespace::from(""),
                &LocalName::from(name.as_str()),
            )
            .map(str::to_string)
        })
        .flatten();
        match value {
            Some(s) => cx.make_string(&s),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__tagName(element)` → the uppercased tag name (HTML), or `null` for non-elements.
pub(crate) struct TagName;
impl<E: ScriptEngine> NativeFn<E> for TagName {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let name = with_dom::<E, _>(cx, |dom| {
            dom.element_name(NodeId::from_raw(id as usize)).map(|q| {
                // `tagName` is the qualified name (`prefix:local`), upper-cased
                // only for HTML-namespaced elements.
                let qualified = match q.prefix.as_ref() {
                    Some(p) => format!("{}:{}", p.as_ref(), q.local.as_ref()),
                    None => q.local.as_ref().to_string(),
                };
                if q.ns.as_ref() == XHTML_NS {
                    qualified.to_ascii_uppercase()
                } else {
                    qualified
                }
            })
        })
        .flatten();
        match name {
            Some(s) => cx.make_string(&s),
            None => Ok(cx.make_null()),
        }
    }
}

/// `node.textContent`: for a text/comment node its own data; for an element the
/// concatenation of all descendant text nodes, in document order (per the DOM).
fn text_content(dom: &ScriptedDom, node: NodeId) -> String {
    match dom.kind(node) {
        NodeKind::Text | NodeKind::Comment => dom.text(node).unwrap_or("").to_string(),
        _ => {
            fn collect(dom: &ScriptedDom, node: NodeId, out: &mut String) {
                for child in dom.dom_children(node).collect::<Vec<_>>() {
                    if dom.kind(child) == NodeKind::Text {
                        out.push_str(dom.text(child).unwrap_or(""));
                    }
                    collect(dom, child, out);
                }
            }
            // An element may carry text directly (the `set_text` / `textContent`
            // setter representation) or via text-node children (parsed / appended);
            // include both.
            let mut s = dom.text(node).unwrap_or("").to_string();
            collect(dom, node, &mut s);
            s
        },
    }
}

/// `__getTextContent(node)` → the node's text content (empty string if none).
pub(crate) struct GetTextContent;
impl<E: ScriptEngine> NativeFn<E> for GetTextContent {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return Ok(cx.make_null());
        };
        let text = with_dom::<E, _>(cx, |dom| text_content(dom, NodeId::from_raw(id as usize)))
            .unwrap_or_default();
        cx.make_string(&text)
    }
}

/// Collect, in document order under `root`, the elements whose local name matches
/// `tag` (ASCII case-insensitive; `*` matches all). Shared by the
/// `getElementsByTagName` count/item sinks. `root` itself is not included (the
/// document/element receiver is the scope, descendants are the result).
fn collect_by_tag(dom: &ScriptedDom, root: NodeId, tag: &str) -> Vec<NodeId> {
    fn walk(dom: &ScriptedDom, node: NodeId, tag: &str, out: &mut Vec<NodeId>) {
        for child in dom.dom_children(node).collect::<Vec<_>>() {
            if dom
                .element_name(child)
                .is_some_and(|q| tag == "*" || q.local.as_ref().eq_ignore_ascii_case(tag))
            {
                out.push(child);
            }
            walk(dom, child, tag, out);
        }
    }
    let mut out = Vec::new();
    walk(dom, root, tag, &mut out);
    out
}

/// `__elementsByTagNameCount(scope, tag)` → how many descendant elements of `scope`
/// match. Paired with `__elementsByTagNameItem` so JS `getElementsByTagName` builds
/// the list without an array-minting primitive (re-walks per item).
pub(crate) struct ElementsByTagNameCount;
impl<E: ScriptEngine> NativeFn<E> for ElementsByTagNameCount {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(root) = cx.reflector_data(&scope) else {
            return cx.make_string("0");
        };
        let tag_v = cx.arg(1);
        let tag = cx.value_to_string(&tag_v)?;
        let n = with_dom::<E, _>(cx, |dom| {
            collect_by_tag(dom, NodeId::from_raw(root as usize), &tag).len()
        })
        .unwrap_or(0);
        cx.make_string(&n.to_string())
    }
}

/// `__elementsByTagNameItem(scope, tag, i)` → the i-th matching descendant's
/// reflector, or `undefined`.
pub(crate) struct ElementsByTagNameItem;
impl<E: ScriptEngine> NativeFn<E> for ElementsByTagNameItem {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(root) = cx.reflector_data(&scope) else {
            return Ok(cx.undefined());
        };
        let tag_v = cx.arg(1);
        let tag = cx.value_to_string(&tag_v)?;
        let i_v = cx.arg(2);
        let i = cx
            .value_to_string(&i_v)?
            .parse::<usize>()
            .unwrap_or(usize::MAX);
        match with_dom::<E, _>(cx, |dom| {
            collect_by_tag(dom, NodeId::from_raw(root as usize), &tag)
                .get(i)
                .copied()
        })
        .flatten()
        {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__parentNode(node)` → a reflector for the parent, or `undefined` if unparented.
/// Used by the `parentNode` getter and event propagation.
pub(crate) struct ParentNode;
impl<E: ScriptEngine> NativeFn<E> for ParentNode {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return Ok(cx.undefined());
        };
        match with_dom::<E, _>(cx, |dom| dom.parent(NodeId::from_raw(id as usize))).flatten() {
            Some(p) => reflect_pinned::<E>(cx, p.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__documentElement(scope)` → the root element child of `scope` (the document),
/// or `undefined`.
pub(crate) struct DocumentElement;
impl<E: ScriptEngine> NativeFn<E> for DocumentElement {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(root) = cx.reflector_data(&scope) else {
            return Ok(cx.undefined());
        };
        match with_dom::<E, _>(cx, |dom| {
            first_element_child(dom, NodeId::from_raw(root as usize))
        })
        .flatten()
        {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__documentBody(scope)` → the first `<body>` under `scope`, or `undefined`.
pub(crate) struct DocumentBody;
impl<E: ScriptEngine> NativeFn<E> for DocumentBody {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(root) = cx.reflector_data(&scope) else {
            return Ok(cx.undefined());
        };
        match with_dom::<E, _>(cx, |dom| {
            collect_by_tag(dom, NodeId::from_raw(root as usize), "body")
                .first()
                .copied()
        })
        .flatten()
        {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__documentHead(scope)` → the first `<head>` under `scope`, or `undefined`.
pub(crate) struct DocumentHead;
impl<E: ScriptEngine> NativeFn<E> for DocumentHead {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(root) = cx.reflector_data(&scope) else {
            return Ok(cx.undefined());
        };
        match with_dom::<E, _>(cx, |dom| {
            collect_by_tag(dom, NodeId::from_raw(root as usize), "head")
                .first()
                .copied()
        })
        .flatten()
        {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__createDocument()` → a reflector for a fresh detached `Document` node (for
/// `DOMImplementation.createDocument` / `createHTMLDocument`).
pub(crate) struct CreateDocument;
impl<E: ScriptEngine> NativeFn<E> for CreateDocument {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        match with_dom::<E, _>(cx, |dom| dom.create_document()) {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__createComment(data)` → a reflector for a fresh detached `Comment` node.
pub(crate) struct CreateComment;
impl<E: ScriptEngine> NativeFn<E> for CreateComment {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let arg = cx.arg(0);
        let data = cx.value_to_string(&arg)?;
        match with_dom::<E, _>(cx, |dom| dom.create_comment(&data)) {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__createFragment()` → a reflector for a fresh detached `DocumentFragment`.
pub(crate) struct CreateFragment;
impl<E: ScriptEngine> NativeFn<E> for CreateFragment {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        match with_dom::<E, _>(cx, |dom| dom.create_fragment()) {
            Some(node) => reflect_pinned::<E>(cx, node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__nodeRawId(node)` → the reflector's raw `NodeId` as a decimal string, or
/// `""` for a non-reflector. The reverse of `__reflectNode`: a host bridge that
/// hands a node *back* to Rust (the testdriver Actions element origin, for one)
/// carries this id, since the reflector itself is JS-opaque.
pub(crate) struct NodeRawId;
impl<E: ScriptEngine> NativeFn<E> for NodeRawId {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        match cx.reflector_data(&node) {
            Some(id) => cx.make_string(&id.to_string()),
            None => cx.make_string(""),
        }
    }
}

/// `__nodeType(node)` → the DOM `nodeType` integer (as a string): 1 element,
/// 3 text, 8 comment, 9 document, 10 doctype, 7 processing-instruction. Drives the
/// JS `Element` / `Text` prototype split in `wrapNode`.
pub(crate) struct NodeType;
impl<E: ScriptEngine> NativeFn<E> for NodeType {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return cx.make_string("0");
        };
        let n = with_dom::<E, _>(cx, |dom| match dom.kind(NodeId::from_raw(id as usize)) {
            NodeKind::Element => 1,
            NodeKind::Text => 3,
            NodeKind::ProcessingInstruction => 7,
            NodeKind::Comment => 8,
            NodeKind::Document => 9,
            NodeKind::Doctype => 10,
            NodeKind::DocumentFragment => 11,
        })
        .unwrap_or(0);
        cx.make_string(&n.to_string())
    }
}
