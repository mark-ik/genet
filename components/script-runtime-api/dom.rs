// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `document` / `Node` construction surface — the live-DOM rung of the host
//! layer (`pluggable_engines_testharness_plan` step 2).
//!
//! Same shape as the rest of the host surface: native sinks (here, mutators of the
//! [`ScriptedDom`] in host state) plus a JS bootstrap that assembles the ergonomic
//! `document` object and wraps node handles. The JS→DOM bridge is the **reflector**
//! — a JS-opaque value carrying a `NodeId` (proven by `serval-scripted`'s `setText`,
//! generalized here and made engine-neutral). Incoming nodes are recovered with
//! `CallCx::reflector_data`; outgoing nodes (`createElement`, `getElementById`) are
//! returned via `CallCx::reflector_for`, which mints **canonical** reflectors (one
//! per node), so the JS wrapper cache keyed on them gives identity
//! (`getElementById('x') === getElementById('x')`).
//!
//! Construction/mutation half: `createElement`, `createTextNode`, `appendChild`,
//! `setAttribute`, `textContent` (setter), `getElementById`. Read half
//! (`getAttribute`, `tagName`, `textContent` getter, `getElementsByTagName`,
//! `parentNode`), via `CallCx::make_string` / `make_null`. Generic over the
//! backend; tested on Boa + Nova like the rest of the host surface.
//!
//! Dispatch is prototype-based with an `Element` / `Text` / `Document` split over
//! `Node` (`instanceof`, `nodeType`); nodes are `EventTarget`s with real tree
//! propagation (capture → target → bubble over `parentNode`, `stopPropagation`).
//! A source document can be cloned in ([`clone_into`] /
//! [`crate::Runtime::load_dom`]), with `document.body` / `documentElement` /
//! `head` over it. The `Element` surface: `getAttribute` / `setAttribute` /
//! `hasAttribute` / `removeAttribute` / `toggleAttribute`, `id` / `className`
//! reflection, `classList`, and `querySelector` / `querySelectorAll` / `matches`
//! (via [`crate::selector`]). Node traversal: `childNodes` / `firstChild` /
//! siblings / element-filtered views, `nodeName` / `nodeValue`, the mutators
//! `removeChild` / `insertBefore` / `replaceChild` (throwing `NotFoundError`), and
//! the `ChildNode` mixin. Namespaces: `localName` / `namespaceURI` / `prefix`,
//! namespace-gated `tagName`, `createElementNS`. A **reflected IDL attribute**
//! layer on `Element.prototype` (DOMString / boolean / approximate-enumerated /
//! long kinds, table-driven) and `TreeWalker` / `NodeIterator` / `NodeFilter`.
//! Not yet: `Comment` / `DocumentFragment` node types, `cloneNode`, URL/tokenlist
//! reflected kinds, live HTMLCollection. See
//! `docs/2026-05-26_pluggable_engines_testharness_plan.md`.

use std::cell::RefCell;

use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, NodeKind, QualName};
use markup5ever::Prefix;
use script_engine_api::{CallCx, NativeFn, ScriptEngine};
use serval_scripted_dom::{NodeId, ScriptedDom};

use crate::HostState;

/// The XHTML namespace — HTML elements live here; `tagName` upper-cases only in it.
const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Clone `src`'s tree (elements with attributes + text) under `dst_parent` in the
/// scripted DOM, recursively. Backs [`crate::Runtime::load_dom`]: a test's parsed
/// HTML (any [`LayoutDom`]) becomes the live document scripts query. Comments /
/// doctypes / PIs are dropped (scripts rarely query them; the `Comment` node type
/// is later breadth).
pub(crate) fn clone_into<D: LayoutDom>(
    src: &D,
    src_node: D::NodeId,
    dst: &mut ScriptedDom,
    dst_parent: NodeId,
) {
    for child in src.dom_children(src_node) {
        match src.kind(child) {
            NodeKind::Element => {
                let Some(name) = src.element_name(child) else { continue };
                let el = dst.create_element(name.clone());
                for attr in src.attributes(child) {
                    dst.set_attribute(el, attr.name.clone(), attr.value);
                }
                dst.append_child(dst_parent, el);
                clone_into(src, child, dst, el);
            },
            NodeKind::Text => {
                let t = dst.create_text(src.text(child).unwrap_or(""));
                dst.append_child(dst_parent, t);
            },
            _ => {},
        }
    }
}

/// First element child of `node` (e.g. `<html>` under the document).
fn first_element_child(dom: &ScriptedDom, node: NodeId) -> Option<NodeId> {
    dom.dom_children(node).find(|&c| dom.element_name(c).is_some())
}

/// Install the `document`/`Node` surface: native sinks, then the JS bootstrap that
/// builds `document` and the node wrappers over them.
pub(crate) fn install_dom_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<DocumentRoot>("__documentRoot", 0)?;
    engine.set_function::<CreateElement>("__createElement", 1)?;
    engine.set_function::<CreateTextNode>("__createTextNode", 1)?;
    engine.set_function::<AppendChild>("__appendChild", 2)?;
    engine.set_function::<SetAttribute>("__setAttribute", 3)?;
    engine.set_function::<SetTextContent>("__setTextContent", 2)?;
    engine.set_function::<GetElementById>("__getElementById", 1)?;
    engine.set_function::<GetAttribute>("__getAttribute", 2)?;
    engine.set_function::<TagName>("__tagName", 1)?;
    engine.set_function::<GetTextContent>("__getTextContent", 1)?;
    engine.set_function::<ParentNode>("__parentNode", 1)?;
    engine.set_function::<ElementsByTagNameCount>("__elementsByTagNameCount", 1)?;
    engine.set_function::<ElementsByTagNameItem>("__elementsByTagNameItem", 2)?;
    engine.set_function::<DocumentElement>("__documentElement", 0)?;
    engine.set_function::<DocumentBody>("__documentBody", 0)?;
    engine.set_function::<DocumentHead>("__documentHead", 0)?;
    engine.set_function::<NodeType>("__nodeType", 1)?;
    engine.set_function::<RemoveAttribute>("__removeAttribute", 2)?;
    engine.set_function::<Matches>("__matches", 2)?;
    engine.set_function::<QuerySelector>("__querySelector", 2)?;
    engine.set_function::<QuerySelectorAllCount>("__querySelectorAllCount", 2)?;
    engine.set_function::<QuerySelectorAllItem>("__querySelectorAllItem", 3)?;
    engine.set_function::<FirstChild>("__firstChild", 1)?;
    engine.set_function::<LastChild>("__lastChild", 1)?;
    engine.set_function::<NextSibling>("__nextSibling", 1)?;
    engine.set_function::<PrevSibling>("__prevSibling", 1)?;
    engine.set_function::<ChildNodesCount>("__childNodesCount", 1)?;
    engine.set_function::<ChildNodesItem>("__childNodesItem", 2)?;
    engine.set_function::<NodeName>("__nodeName", 1)?;
    engine.set_function::<NodeValue>("__nodeValue", 1)?;
    engine.set_function::<RemoveChild>("__removeChild", 2)?;
    engine.set_function::<InsertBefore>("__insertBefore", 3)?;
    engine.set_function::<LocalNameOf>("__localName", 1)?;
    engine.set_function::<NamespaceUri>("__namespaceURI", 1)?;
    engine.set_function::<PrefixOf>("__prefix", 1)?;
    engine.set_function::<CreateElementNS>("__createElementNS", 2)?;
    engine.eval(DOM_BOOTSTRAP)?;
    Ok(())
}

/// An HTML-namespaced element name (matches serval-layout's cascade keying).
fn html_qual(local: &str) -> QualName {
    QualName::new(
        None,
        Namespace::from("http://www.w3.org/1999/xhtml"),
        LocalName::from(local),
    )
}

/// A null-namespace attribute name (the common case: `id`, `class`, …).
fn attr_qual(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}

/// Run `f` against the host's [`ScriptedDom`], recovered from the engine host-data
/// slot (a `RefCell<HostState>`). `None` if no host state is set.
fn with_dom<E: ScriptEngine, R>(
    cx: &mut E::CallCx<'_>,
    f: impl FnOnce(&mut ScriptedDom) -> R,
) -> Option<R> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let mut host = cell.borrow_mut();
    Some(f(&mut host.dom))
}

/// Depth-first search for the first element whose null-namespace `id` equals `target`.
fn find_by_id(dom: &ScriptedDom, target: &str) -> Option<NodeId> {
    fn walk(dom: &ScriptedDom, node: NodeId, target: &str) -> Option<NodeId> {
        if dom.attribute(node, &Namespace::from(""), &LocalName::from("id")) == Some(target) {
            return Some(node);
        }
        for child in dom.dom_children(node).collect::<Vec<_>>() {
            if let Some(found) = walk(dom, child, target) {
                return Some(found);
            }
        }
        None
    }
    walk(dom, dom.document(), target)
}

/// `__documentRoot()` → a reflector for the document node.
struct DocumentRoot;
impl<E: ScriptEngine> NativeFn<E> for DocumentRoot {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        match with_dom::<E, _>(cx, |dom| dom.document()) {
            Some(root) => cx.reflector_for(root.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__createElement(tag)` → a reflector for the new (unparented) element.
struct CreateElement;
impl<E: ScriptEngine> NativeFn<E> for CreateElement {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let arg = cx.arg(0);
        let tag = cx.value_to_string(&arg)?;
        match with_dom::<E, _>(cx, |dom| dom.create_element(html_qual(&tag))) {
            Some(id) => cx.reflector_for(id.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__createTextNode(data)` → a reflector for the new (unparented) text node.
struct CreateTextNode;
impl<E: ScriptEngine> NativeFn<E> for CreateTextNode {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let arg = cx.arg(0);
        let data = cx.value_to_string(&arg)?;
        match with_dom::<E, _>(cx, |dom| dom.create_text(&data)) {
            Some(id) => cx.reflector_for(id.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__appendChild(parent, child)` — both reflectors.
struct AppendChild;
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
struct SetAttribute;
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
struct SetTextContent;
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

/// `__getElementById(id)` → a reflector for the match, or `undefined`.
struct GetElementById;
impl<E: ScriptEngine> NativeFn<E> for GetElementById {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let arg = cx.arg(0);
        let id = cx.value_to_string(&arg)?;
        match with_dom::<E, _>(cx, |dom| find_by_id(dom, &id)).flatten() {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__getAttribute(element, name)` → the attribute string, or `null` if absent.
struct GetAttribute;
impl<E: ScriptEngine> NativeFn<E> for GetAttribute {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let name_v = cx.arg(1);
        let name = cx.value_to_string(&name_v)?;
        let value = with_dom::<E, _>(cx, |dom| {
            dom.attribute(NodeId::from_raw(id as usize), &Namespace::from(""), &LocalName::from(name.as_str()))
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
struct TagName;
impl<E: ScriptEngine> NativeFn<E> for TagName {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let name = with_dom::<E, _>(cx, |dom| {
            // `tagName` upper-cases only for HTML-namespaced elements.
            dom.element_name(NodeId::from_raw(id as usize)).map(|q| {
                if q.ns.as_ref() == XHTML_NS {
                    q.local.as_ref().to_ascii_uppercase()
                } else {
                    q.local.as_ref().to_string()
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
struct GetTextContent;
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

/// Collect, in document order, the elements whose local name matches `tag`
/// (ASCII case-insensitive). Shared by the `getElementsByTagName` count/item sinks.
fn collect_by_tag(dom: &ScriptedDom, tag: &str) -> Vec<NodeId> {
    fn walk(dom: &ScriptedDom, node: NodeId, tag: &str, out: &mut Vec<NodeId>) {
        if dom.element_name(node).is_some_and(|q| q.local.as_ref().eq_ignore_ascii_case(tag)) {
            out.push(node);
        }
        for child in dom.dom_children(node).collect::<Vec<_>>() {
            walk(dom, child, tag, out);
        }
    }
    let mut out = Vec::new();
    walk(dom, dom.document(), tag, &mut out);
    out
}

/// `__elementsByTagNameCount(tag)` → how many elements match. Paired with
/// `__elementsByTagNameItem` so the JS `getElementsByTagName` builds the list
/// without an array-minting primitive (re-walks per item; fine for load-time
/// `meta`/`script`/`title` queries, mostly empty).
struct ElementsByTagNameCount;
impl<E: ScriptEngine> NativeFn<E> for ElementsByTagNameCount {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let arg = cx.arg(0);
        let tag = cx.value_to_string(&arg)?;
        let n = with_dom::<E, _>(cx, |dom| collect_by_tag(dom, &tag).len()).unwrap_or(0);
        // Returned as a string; the JS wrapper coerces with `+` (avoids a
        // number-minting primitive for now).
        cx.make_string(&n.to_string())
    }
}

/// `__elementsByTagNameItem(tag, i)` → the i-th matching element's reflector, or
/// `undefined`.
struct ElementsByTagNameItem;
impl<E: ScriptEngine> NativeFn<E> for ElementsByTagNameItem {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let tag_v = cx.arg(0);
        let tag = cx.value_to_string(&tag_v)?;
        let i_v = cx.arg(1);
        let i = cx.value_to_string(&i_v)?.parse::<usize>().unwrap_or(usize::MAX);
        match with_dom::<E, _>(cx, |dom| collect_by_tag(dom, &tag).get(i).copied()).flatten() {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__parentNode(node)` → a reflector for the parent, or `undefined` if unparented.
/// Used by the `parentNode` getter and event propagation.
struct ParentNode;
impl<E: ScriptEngine> NativeFn<E> for ParentNode {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return Ok(cx.undefined());
        };
        match with_dom::<E, _>(cx, |dom| dom.parent(NodeId::from_raw(id as usize))).flatten() {
            Some(p) => cx.reflector_for(p.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__documentElement()` → the root element (`<html>`), or `undefined`.
struct DocumentElement;
impl<E: ScriptEngine> NativeFn<E> for DocumentElement {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        match with_dom::<E, _>(cx, |dom| first_element_child(dom, dom.document())).flatten() {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__documentBody()` → the first `<body>`, or `undefined`.
struct DocumentBody;
impl<E: ScriptEngine> NativeFn<E> for DocumentBody {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        match with_dom::<E, _>(cx, |dom| collect_by_tag(dom, "body").first().copied()).flatten() {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__documentHead()` → the first `<head>`, or `undefined`.
struct DocumentHead;
impl<E: ScriptEngine> NativeFn<E> for DocumentHead {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        match with_dom::<E, _>(cx, |dom| collect_by_tag(dom, "head").first().copied()).flatten() {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__nodeType(node)` → the DOM `nodeType` integer (as a string): 1 element,
/// 3 text, 8 comment, 9 document, 10 doctype, 7 processing-instruction. Drives the
/// JS `Element` / `Text` prototype split in `wrapNode`.
struct NodeType;
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
        })
        .unwrap_or(0);
        cx.make_string(&n.to_string())
    }
}

/// `__removeAttribute(element, name)`.
struct RemoveAttribute;
impl<E: ScriptEngine> NativeFn<E> for RemoveAttribute {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.undefined());
        };
        let name_v = cx.arg(1);
        let name = cx.value_to_string(&name_v)?;
        with_dom::<E, _>(cx, |dom| {
            dom.remove_attribute(NodeId::from_raw(id as usize), attr_qual(&name))
        });
        Ok(cx.undefined())
    }
}

/// `__matches(element, selector)` → `"true"`/`"false"`.
struct Matches;
impl<E: ScriptEngine> NativeFn<E> for Matches {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return cx.make_string("false");
        };
        let sel_v = cx.arg(1);
        let sel = cx.value_to_string(&sel_v)?;
        let matched = with_dom::<E, _>(cx, |dom| {
            crate::selector::parse(&sel).matches(dom, NodeId::from_raw(id as usize))
        })
        .unwrap_or(false);
        cx.make_string(if matched { "true" } else { "false" })
    }
}

/// `__querySelector(scope, selector)` → the first matching descendant's reflector,
/// or `null`. `scope` is an element or the document.
struct QuerySelector;
impl<E: ScriptEngine> NativeFn<E> for QuerySelector {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(id) = cx.reflector_data(&scope) else {
            return Ok(cx.make_null());
        };
        let sel_v = cx.arg(1);
        let sel = cx.value_to_string(&sel_v)?;
        match with_dom::<E, _>(cx, |dom| {
            crate::selector::parse(&sel).query_first(dom, NodeId::from_raw(id as usize))
        })
        .flatten()
        {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__querySelectorAllCount(scope, selector)` → match count (as a string). Paired
/// with `__querySelectorAllItem`, the count/item pattern used elsewhere.
struct QuerySelectorAllCount;
impl<E: ScriptEngine> NativeFn<E> for QuerySelectorAllCount {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(id) = cx.reflector_data(&scope) else {
            return cx.make_string("0");
        };
        let sel_v = cx.arg(1);
        let sel = cx.value_to_string(&sel_v)?;
        let n = with_dom::<E, _>(cx, |dom| {
            crate::selector::parse(&sel).query_all(dom, NodeId::from_raw(id as usize)).len()
        })
        .unwrap_or(0);
        cx.make_string(&n.to_string())
    }
}

/// `__querySelectorAllItem(scope, selector, i)` → the i-th match's reflector, or
/// `undefined`.
struct QuerySelectorAllItem;
impl<E: ScriptEngine> NativeFn<E> for QuerySelectorAllItem {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let scope = cx.arg(0);
        let Some(id) = cx.reflector_data(&scope) else {
            return Ok(cx.undefined());
        };
        let sel_v = cx.arg(1);
        let sel = cx.value_to_string(&sel_v)?;
        let i_v = cx.arg(2);
        let i = cx.value_to_string(&i_v)?.parse::<usize>().unwrap_or(usize::MAX);
        match with_dom::<E, _>(cx, |dom| {
            crate::selector::parse(&sel).query_all(dom, NodeId::from_raw(id as usize)).get(i).copied()
        })
        .flatten()
        {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__firstChild(node)` → first child reflector, or `undefined`.
struct FirstChild;
impl<E: ScriptEngine> NativeFn<E> for FirstChild {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        child_at::<E>(cx, |dom, node| dom.dom_children(node).next())
    }
}

/// `__lastChild(node)` → last child reflector, or `undefined`.
struct LastChild;
impl<E: ScriptEngine> NativeFn<E> for LastChild {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        child_at::<E>(cx, |dom, node| dom.dom_children(node).last())
    }
}

/// `__nextSibling(node)` → next sibling reflector, or `undefined`.
struct NextSibling;
impl<E: ScriptEngine> NativeFn<E> for NextSibling {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        child_at::<E>(cx, |dom, node| dom.next_sibling(node))
    }
}

/// `__prevSibling(node)` → previous sibling reflector, or `undefined`.
struct PrevSibling;
impl<E: ScriptEngine> NativeFn<E> for PrevSibling {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        child_at::<E>(cx, |dom, node| dom.prev_sibling(node))
    }
}

/// Shared helper for the single-node traversal sinks: recover the arg-0 node, run
/// `pick` against the DOM, reflect the result (or `undefined`).
fn child_at<E: ScriptEngine>(
    cx: &mut E::CallCx<'_>,
    pick: impl FnOnce(&ScriptedDom, NodeId) -> Option<NodeId>,
) -> Result<E::Value, E::Error> {
    let node = cx.arg(0);
    let Some(id) = cx.reflector_data(&node) else {
        return Ok(cx.undefined());
    };
    match with_dom::<E, _>(cx, |dom| pick(dom, NodeId::from_raw(id as usize))).flatten() {
        Some(n) => cx.reflector_for(n.raw() as u64),
        None => Ok(cx.undefined()),
    }
}

/// `__childNodesCount(node)` → child count (string). With `__childNodesItem`, backs
/// `childNodes` (and, JS-filtered, `children`).
struct ChildNodesCount;
impl<E: ScriptEngine> NativeFn<E> for ChildNodesCount {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return cx.make_string("0");
        };
        let n = with_dom::<E, _>(cx, |dom| dom.dom_children(NodeId::from_raw(id as usize)).count())
            .unwrap_or(0);
        cx.make_string(&n.to_string())
    }
}

/// `__childNodesItem(node, i)` → the i-th child's reflector, or `undefined`.
struct ChildNodesItem;
impl<E: ScriptEngine> NativeFn<E> for ChildNodesItem {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return Ok(cx.undefined());
        };
        let i_v = cx.arg(1);
        let i = cx.value_to_string(&i_v)?.parse::<usize>().unwrap_or(usize::MAX);
        match with_dom::<E, _>(cx, |dom| dom.dom_children(NodeId::from_raw(id as usize)).nth(i))
            .flatten()
        {
            Some(n) => cx.reflector_for(n.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__nodeName(node)`: element → uppercase tag; text → `#text`; comment →
/// `#comment`; document → `#document`; else the kind's conventional name.
struct NodeName;
impl<E: ScriptEngine> NativeFn<E> for NodeName {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return cx.make_string("");
        };
        let name = with_dom::<E, _>(cx, |dom| {
            let n = NodeId::from_raw(id as usize);
            match dom.kind(n) {
                NodeKind::Element => {
                    dom.element_name(n).map(|q| q.local.as_ref().to_ascii_uppercase()).unwrap_or_default()
                },
                NodeKind::Text => "#text".to_string(),
                NodeKind::Comment => "#comment".to_string(),
                NodeKind::Document => "#document".to_string(),
                NodeKind::Doctype => "html".to_string(),
                NodeKind::ProcessingInstruction => "#processing-instruction".to_string(),
            }
        })
        .unwrap_or_default();
        cx.make_string(&name)
    }
}

/// `__nodeValue(node)`: text/comment → its data; otherwise `null`.
struct NodeValue;
impl<E: ScriptEngine> NativeFn<E> for NodeValue {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return Ok(cx.make_null());
        };
        let value = with_dom::<E, _>(cx, |dom| {
            let n = NodeId::from_raw(id as usize);
            match dom.kind(n) {
                NodeKind::Text | NodeKind::Comment => Some(dom.text(n).unwrap_or("").to_string()),
                _ => None,
            }
        })
        .flatten();
        match value {
            Some(s) => cx.make_string(&s),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__removeChild(parent, child)` — detach `child` (the JS side has already checked
/// it is a child of `parent`).
struct RemoveChild;
impl<E: ScriptEngine> NativeFn<E> for RemoveChild {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let child = cx.arg(1);
        if let Some(c) = cx.reflector_data(&child) {
            // Orphan (keep alive + re-insertable), not drop — DOM `removeChild`.
            with_dom::<E, _>(cx, |dom| dom.remove_child(NodeId::from_raw(c as usize)));
        }
        Ok(cx.undefined())
    }
}

/// `__insertBefore(parent, node, ref)` — insert `node` before `ref` (a reflector),
/// or append when `ref` is not a reflector (undefined/null).
struct InsertBefore;
impl<E: ScriptEngine> NativeFn<E> for InsertBefore {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let parent = cx.arg(0);
        let node = cx.arg(1);
        let reference = cx.arg(2);
        if let (Some(p), Some(n)) = (cx.reflector_data(&parent), cx.reflector_data(&node)) {
            let r = cx.reflector_data(&reference).map(|r| NodeId::from_raw(r as usize));
            with_dom::<E, _>(cx, |dom| {
                dom.insert_before(NodeId::from_raw(p as usize), NodeId::from_raw(n as usize), r)
            });
        }
        Ok(cx.undefined())
    }
}

/// `__localName(element)` → the element's local name (as stored, lowercase for
/// HTML), or `null`.
struct LocalNameOf;
impl<E: ScriptEngine> NativeFn<E> for LocalNameOf {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let name = with_dom::<E, _>(cx, |dom| {
            dom.element_name(NodeId::from_raw(id as usize)).map(|q| q.local.as_ref().to_string())
        })
        .flatten();
        match name {
            Some(s) => cx.make_string(&s),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__namespaceURI(element)` → the element's namespace, or `null` when empty.
struct NamespaceUri;
impl<E: ScriptEngine> NativeFn<E> for NamespaceUri {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let ns = with_dom::<E, _>(cx, |dom| {
            dom.element_name(NodeId::from_raw(id as usize)).map(|q| q.ns.as_ref().to_string())
        })
        .flatten();
        match ns {
            Some(s) if !s.is_empty() => cx.make_string(&s),
            _ => Ok(cx.make_null()),
        }
    }
}

/// `__prefix(element)` → the element's namespace prefix, or `null`.
struct PrefixOf;
impl<E: ScriptEngine> NativeFn<E> for PrefixOf {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let prefix = with_dom::<E, _>(cx, |dom| {
            dom.element_name(NodeId::from_raw(id as usize))
                .and_then(|q| q.prefix.as_ref().map(|p| p.as_ref().to_string()))
        })
        .flatten();
        match prefix {
            Some(s) => cx.make_string(&s),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__createElementNS(ns, qualifiedName)` → a reflector for the new element. The
/// qualified name is split on `:` into prefix + local. (Strict name validation /
/// `InvalidCharacterError` is deferred; a malformed name still creates an element.)
struct CreateElementNS;
impl<E: ScriptEngine> NativeFn<E> for CreateElementNS {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ns_v = cx.arg(0);
        let qname_v = cx.arg(1);
        let ns = cx.value_to_string(&ns_v)?;
        let qname = cx.value_to_string(&qname_v)?;
        let (prefix, local) = match qname.split_once(':') {
            Some((p, l)) => (Some(Prefix::from(p)), l.to_string()),
            None => (None, qname),
        };
        let qual = QualName::new(prefix, Namespace::from(ns.as_str()), LocalName::from(local.as_str()));
        match with_dom::<E, _>(cx, |dom| dom.create_element(qual)) {
            Some(node) => cx.reflector_for(node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `document` plus node wrappers. A wrapper is a plain object carrying its reflector
/// (`__ref`) and the methods that drive the native sinks. ES5-style (no arrows /
/// classes / let) for the widest backend coverage, matching the other bootstraps.
const DOM_BOOTSTRAP: &str = r#"
(function() {
  // Wrapper cache keyed by the canonical reflector (engine-side `reflector_for`
  // returns the same reflector object per node), so the same node yields the same
  // wrapper: document.getElementById('x') === document.getElementById('x').
  var wrappers = new Map();

  // wrapNode is hoisted (function declaration), so the prototype methods defined
  // below may reference it before this point — they only run when called. The
  // prototype is chosen by nodeType, giving the Element / Text split (`instanceof
  // Element`, `node.nodeType`).
  function wrapNode(ref) {
    if (ref === undefined || ref === null) return null;
    if (wrappers.has(ref)) return wrappers.get(ref);
    var nt = +__nodeType(ref);
    var proto = nt === 1 ? Element.prototype
              : nt === 9 ? Document.prototype
              : nt === 3 ? Text.prototype
              : Node.prototype;
    var node = Object.create(proto);
    node.__ref = ref;
    node.nodeType = nt;
    wrappers.set(ref, node);
    return node;
  }

  // Node: the base every node shares (tree + events + textContent). Methods live
  // on the prototype (shared, instanceof-able), not per-object. `this.__ref` is
  // the node's reflector.
  function Node() {}
  Node.ELEMENT_NODE = Node.prototype.ELEMENT_NODE = 1;
  Node.TEXT_NODE = Node.prototype.TEXT_NODE = 3;
  Node.COMMENT_NODE = Node.prototype.COMMENT_NODE = 8;
  Node.DOCUMENT_NODE = Node.prototype.DOCUMENT_NODE = 9;
  Node.prototype.appendChild = function(child) { __appendChild(this.__ref, child.__ref); return child; };
  Object.defineProperty(Node.prototype, 'textContent', {
    configurable: true,
    get: function() { return __getTextContent(this.__ref); },
    set: function(v) { __setTextContent(this.__ref, String(v)); }
  });
  Object.defineProperty(Node.prototype, 'parentNode', {
    configurable: true,
    get: function() { return wrapNode(__parentNode(this.__ref)); }
  });
  Object.defineProperty(Node.prototype, 'parentElement', {
    configurable: true,
    get: function() { var p = this.parentNode; return (p && p.nodeType === 1) ? p : null; }
  });
  Object.defineProperty(Node.prototype, 'firstChild', {
    configurable: true, get: function() { return wrapNode(__firstChild(this.__ref)); }
  });
  Object.defineProperty(Node.prototype, 'lastChild', {
    configurable: true, get: function() { return wrapNode(__lastChild(this.__ref)); }
  });
  Object.defineProperty(Node.prototype, 'nextSibling', {
    configurable: true, get: function() { return wrapNode(__nextSibling(this.__ref)); }
  });
  Object.defineProperty(Node.prototype, 'previousSibling', {
    configurable: true, get: function() { return wrapNode(__prevSibling(this.__ref)); }
  });
  Object.defineProperty(Node.prototype, 'childNodes', {
    configurable: true,
    get: function() {
      var n = +__childNodesCount(this.__ref);
      var out = [];
      for (var i = 0; i < n; i++) { out.push(wrapNode(__childNodesItem(this.__ref, String(i)))); }
      return out;
    }
  });
  Object.defineProperty(Node.prototype, 'nodeName', {
    configurable: true, get: function() { return __nodeName(this.__ref); }
  });
  Object.defineProperty(Node.prototype, 'nodeValue', {
    configurable: true, get: function() { return __nodeValue(this.__ref); }
  });
  Node.prototype.hasChildNodes = function() { return +__childNodesCount(this.__ref) > 0; };
  Node.prototype.contains = function(other) {
    var n = other;
    while (n) { if (n === this) return true; n = n.parentNode; }
    return false;
  };
  Node.prototype.removeChild = function(child) {
    if (!child || child.parentNode !== this) {
      throw new DOMException("The node to be removed is not a child of this node.", "NotFoundError");
    }
    __removeChild(this.__ref, child.__ref);
    return child;
  };
  Node.prototype.insertBefore = function(node, ref) {
    if (ref !== null && ref !== undefined && ref.parentNode !== this) {
      throw new DOMException("The reference node is not a child of this node.", "NotFoundError");
    }
    __insertBefore(this.__ref, node.__ref, ref ? ref.__ref : undefined);
    return node;
  };
  Node.prototype.replaceChild = function(newChild, oldChild) {
    if (!oldChild || oldChild.parentNode !== this) {
      throw new DOMException("The node to be replaced is not a child of this node.", "NotFoundError");
    }
    this.insertBefore(newChild, oldChild);
    __removeChild(this.__ref, oldChild.__ref);
    return oldChild;
  };

  // Node-level EventTarget with real tree propagation (capture → target → bubble)
  // over the parentNode chain. Listeners live on the (cached) wrapper, keyed by
  // phase: 'c:'+type for capture, 'b:'+type for bubble/target.
  Node.prototype.addEventListener = function(type, cb, capture) {
    if (typeof cb !== 'function') return;
    if (!this.__listeners) this.__listeners = {};
    var key = (capture ? 'c:' : 'b:') + type;
    if (!this.__listeners[key]) this.__listeners[key] = [];
    this.__listeners[key].push(cb);
  };
  Node.prototype.removeEventListener = function(type, cb, capture) {
    if (!this.__listeners) return;
    var l = this.__listeners[(capture ? 'c:' : 'b:') + type];
    if (!l) return;
    var i = l.indexOf(cb);
    if (i !== -1) l.splice(i, 1);
  };
  function fire(node, event, key) {
    if (!node.__listeners) return;
    var l = node.__listeners[key];
    if (!l) return;
    event.currentTarget = node;
    var copy = l.slice();
    for (var i = 0; i < copy.length; i++) { copy[i].call(node, event); }
  }
  Node.prototype.dispatchEvent = function(event) {
    var path = [];
    var n = this;
    while (n) { path.push(n); n = n.parentNode; }
    event.target = this;
    event.__stop = false;
    // Capture: root → just above the target.
    for (var i = path.length - 1; i >= 1 && !event.__stop; i--) {
      fire(path[i], event, 'c:' + event.type);
    }
    // Target: capture- then bubble-registered listeners on the target itself.
    if (!event.__stop) { fire(this, event, 'c:' + event.type); }
    if (!event.__stop) { fire(this, event, 'b:' + event.type); }
    // Bubble: just above the target → root, when the event bubbles.
    if (event.bubbles) {
      for (var j = 1; j < path.length && !event.__stop; j++) {
        fire(path[j], event, 'b:' + event.type);
      }
    }
    return !event.__canceled;
  };
  // stopPropagation halts further nodes (the current node's other listeners still
  // run). Extends the shell's Event, installed before this bootstrap.
  if (globalThis.Event && globalThis.Event.prototype) {
    globalThis.Event.prototype.stopPropagation = function() { this.__stop = true; };
  }

  // Text : Node (no extra surface yet beyond textContent on Node).
  function Text() {}
  Text.prototype = Object.create(Node.prototype);

  // Element : Node — attributes, reflection, selectors.
  function Element() {}
  Element.prototype = Object.create(Node.prototype);
  Element.prototype.setAttribute = function(name, value) { __setAttribute(this.__ref, String(name), String(value)); };
  Element.prototype.getAttribute = function(name) { return __getAttribute(this.__ref, String(name)); };
  Element.prototype.hasAttribute = function(name) { return __getAttribute(this.__ref, String(name)) !== null; };
  Element.prototype.removeAttribute = function(name) { __removeAttribute(this.__ref, String(name)); };
  Element.prototype.toggleAttribute = function(name, force) {
    var has = this.hasAttribute(name);
    if (force === undefined) force = !has;
    if (force) { if (!has) this.setAttribute(name, ''); return true; }
    if (has) this.removeAttribute(name);
    return false;
  };
  Element.prototype.matches = function(sel) { return __matches(this.__ref, String(sel)) === 'true'; };
  Object.defineProperty(Element.prototype, 'tagName', {
    configurable: true, get: function() { return __tagName(this.__ref); }
  });
  Object.defineProperty(Element.prototype, 'id', {
    configurable: true,
    get: function() { return this.getAttribute('id') || ''; },
    set: function(v) { this.setAttribute('id', String(v)); }
  });
  Object.defineProperty(Element.prototype, 'className', {
    configurable: true,
    get: function() { return this.getAttribute('class') || ''; },
    set: function(v) { this.setAttribute('class', String(v)); }
  });
  Object.defineProperty(Element.prototype, 'localName', {
    configurable: true, get: function() { return __localName(this.__ref); }
  });
  Object.defineProperty(Element.prototype, 'namespaceURI', {
    configurable: true, get: function() { return __namespaceURI(this.__ref); }
  });
  Object.defineProperty(Element.prototype, 'prefix', {
    configurable: true, get: function() { return __prefix(this.__ref); }
  });
  Object.defineProperty(Element.prototype, 'classList', {
    configurable: true,
    get: function() {
      var el = this;
      function tokens() {
        var c = el.getAttribute('class') || '';
        return c.trim().split(/\s+/).filter(function(s) { return s.length; });
      }
      function write(arr) { el.setAttribute('class', arr.join(' ')); }
      return {
        get length() { return tokens().length; },
        item: function(i) { var t = tokens(); return i < t.length ? t[i] : null; },
        contains: function(tok) { return tokens().indexOf(tok) !== -1; },
        add: function() {
          var t = tokens();
          for (var i = 0; i < arguments.length; i++) { if (t.indexOf(arguments[i]) === -1) t.push(arguments[i]); }
          write(t);
        },
        remove: function() {
          var t = tokens();
          for (var i = 0; i < arguments.length; i++) { var x = t.indexOf(arguments[i]); if (x !== -1) t.splice(x, 1); }
          write(t);
        },
        toggle: function(tok, force) {
          var t = tokens(); var has = t.indexOf(tok) !== -1;
          if (force === true || (force === undefined && !has)) { if (!has) { t.push(tok); write(t); } return true; }
          if (has) { t.splice(t.indexOf(tok), 1); write(t); }
          return false;
        },
        toString: function() { return el.getAttribute('class') || ''; }
      };
    }
  });

  // querySelector / querySelectorAll, shared by Element and Document (scope is the
  // receiver). querySelectorAll returns an array (NodeList-approximate).
  function querySelector(sel) { return wrapNode(__querySelector(this.__ref, String(sel))); }
  function querySelectorAll(sel) {
    var n = +__querySelectorAllCount(this.__ref, String(sel));
    var out = [];
    for (var i = 0; i < n; i++) { out.push(wrapNode(__querySelectorAllItem(this.__ref, String(sel), String(i)))); }
    return out;
  }
  Element.prototype.querySelector = querySelector;
  Element.prototype.querySelectorAll = querySelectorAll;

  // Element-only tree views: children (elements), the element siblings, count.
  Object.defineProperty(Element.prototype, 'children', {
    configurable: true,
    get: function() { return this.childNodes.filter(function(n) { return n.nodeType === 1; }); }
  });
  Object.defineProperty(Element.prototype, 'firstElementChild', {
    configurable: true, get: function() { var c = this.children; return c.length ? c[0] : null; }
  });
  Object.defineProperty(Element.prototype, 'lastElementChild', {
    configurable: true, get: function() { var c = this.children; return c.length ? c[c.length - 1] : null; }
  });
  Object.defineProperty(Element.prototype, 'childElementCount', {
    configurable: true, get: function() { return this.children.length; }
  });
  Object.defineProperty(Element.prototype, 'nextElementSibling', {
    configurable: true,
    get: function() { var n = this.nextSibling; while (n) { if (n.nodeType === 1) return n; n = n.nextSibling; } return null; }
  });
  Object.defineProperty(Element.prototype, 'previousElementSibling', {
    configurable: true,
    get: function() { var n = this.previousSibling; while (n) { if (n.nodeType === 1) return n; n = n.previousSibling; } return null; }
  });

  // ChildNode mixin: remove / before / after / replaceWith. String arguments
  // become text nodes (per spec).
  function toNode(arg) { return (typeof arg === 'string') ? document.createTextNode(arg) : arg; }
  Element.prototype.remove = function() { var p = this.parentNode; if (p) p.removeChild(this); };
  Element.prototype.before = function() {
    var p = this.parentNode; if (!p) return;
    for (var i = 0; i < arguments.length; i++) { p.insertBefore(toNode(arguments[i]), this); }
  };
  Element.prototype.after = function() {
    var p = this.parentNode; if (!p) return;
    var ref = this.nextSibling;
    for (var i = 0; i < arguments.length; i++) { p.insertBefore(toNode(arguments[i]), ref); }
  };
  Element.prototype.replaceWith = function() {
    var p = this.parentNode; if (!p) return;
    var ref = this.nextSibling;
    p.removeChild(this);
    for (var i = 0; i < arguments.length; i++) { p.insertBefore(toNode(arguments[i]), ref); }
  };

  // Document : Node, with the construction/lookup methods.
  function Document() {}
  Document.prototype = Object.create(Node.prototype);
  Document.prototype.createElement = function(tag) { return wrapNode(__createElement(String(tag))); };
  Document.prototype.createElementNS = function(ns, qname) {
    return wrapNode(__createElementNS(ns === null ? '' : String(ns), String(qname)));
  };
  Document.prototype.createTextNode = function(data) { return wrapNode(__createTextNode(String(data))); };
  Document.prototype.getElementById = function(id) { return wrapNode(__getElementById(String(id))); };
  Document.prototype.getElementsByTagName = function(tag) {
    var n = +__elementsByTagNameCount(String(tag));
    var out = [];
    for (var i = 0; i < n; i++) { out.push(wrapNode(__elementsByTagNameItem(String(tag), String(i)))); }
    return out;
  };
  Document.prototype.querySelector = querySelector;
  Document.prototype.querySelectorAll = querySelectorAll;
  Object.defineProperty(Document.prototype, 'documentElement', {
    configurable: true, get: function() { return wrapNode(__documentElement()); }
  });
  Object.defineProperty(Document.prototype, 'body', {
    configurable: true,
    get: function() { return wrapNode(__documentBody()); },
    set: function(v) {
      var old = this.body;
      if (old) { old.parentNode.replaceChild(v, old); }
      else { var root = this.documentElement; if (root) root.appendChild(v); }
    }
  });
  Object.defineProperty(Document.prototype, 'head', {
    configurable: true, get: function() { return wrapNode(__documentHead()); }
  });
  // Document IDL accessors (Lever 10): title walks to <title> (whitespace-collapsed);
  // dir reflects documentElement's dir; compatMode/readyState are constants.
  Object.defineProperty(Document.prototype, 'title', {
    configurable: true,
    get: function() {
      var titles = this.getElementsByTagName('title');
      if (!titles.length) return '';
      return (titles[0].textContent || '').replace(/[ \t\n\f\r]+/g, ' ').replace(/^ | $/g, '');
    },
    set: function(v) {
      var titles = this.getElementsByTagName('title');
      var t = titles.length ? titles[0] : null;
      if (!t) {
        var head = this.head; if (!head) return;
        t = this.createElement('title'); head.appendChild(t);
      }
      t.textContent = String(v);
    }
  });
  Object.defineProperty(Document.prototype, 'dir', {
    configurable: true,
    get: function() { var r = this.documentElement; return r ? r.dir : ''; },
    set: function(v) { var r = this.documentElement; if (r) r.dir = v; }
  });
  Object.defineProperty(Document.prototype, 'compatMode', {
    configurable: true, get: function() { return 'CSS1Compat'; }
  });
  Object.defineProperty(Document.prototype, 'readyState', {
    configurable: true, get: function() { return 'complete'; }
  });

  globalThis.Node = Node;
  globalThis.Element = Element;
  globalThis.Text = Text;
  globalThis.Document = Document;

  installReflectedAttributes();
  installTraversal();

  // Reflected IDL attribute accessors on Element.prototype (Lever 1). Driven by a
  // table of [idlName, attr, kind]; kinds: s=DOMString, b=boolean, e=enumerated
  // (approximate: lowercased pass-through, '' default — keyword canonicalization
  // deferred), l=long. url/tokenlist/double are deferred (need URL parsing /
  // exotic objects). All over the existing get/set/has/toggle/removeAttribute.
  function installReflectedAttributes() {
    function parseHtmlLong(s) {
      if (s === null || s === undefined) return null;
      var m = /^[ \t\n\f\r]*([+-]?[0-9]+)/.exec(String(s));
      return m ? parseInt(m[1], 10) : null;
    }
    function toLong(v) {
      v = Number(v);
      if (!isFinite(v)) return 0;
      return (v < 0 ? Math.ceil(v) : Math.floor(v)) | 0;
    }
    function def(idl, kind, attr) {
      attr = attr || idl;
      var desc = { configurable: true, enumerable: true };
      if (kind === 's') {
        desc.get = function() { var v = this.getAttribute(attr); return v === null ? '' : v; };
        desc.set = function(v) { this.setAttribute(attr, String(v)); };
      } else if (kind === 'b') {
        desc.get = function() { return this.hasAttribute(attr); };
        desc.set = function(v) { this.toggleAttribute(attr, !!v); };
      } else if (kind === 'e') {
        desc.get = function() { var v = this.getAttribute(attr); return v === null ? '' : String(v).toLowerCase(); };
        desc.set = function(v) { this.setAttribute(attr, String(v)); };
      } else if (kind === 'l') {
        desc.get = function() { var n = parseHtmlLong(this.getAttribute(attr)); return n === null ? -1 : n; };
        desc.set = function(v) { this.setAttribute(attr, String(toLong(v))); };
      }
      Object.defineProperty(Element.prototype, idl, desc);
    }
    // Global attributes (tested on every element by the WPT reflection harness).
    def('title', 's'); def('lang', 's'); def('accessKey', 's');
    def('autofocus', 'b'); def('hidden', 'b'); def('dir', 'e'); def('tabIndex', 'l');
    // Per-element DOMString attributes (conflict-free union from the WPT metadata).
    var S = ['aLink','abbr','accept','align','alt','archive','axis','background','bgColor','border',
             'cellPadding','cellSpacing','charset','clear','code','codeType','color','content','coords',
             'dateTime','dirName','download','event','face','formTarget','frame','frameBorder','headers',
             'hreflang','integrity','label','link','marginHeight','marginWidth','media','name','nonce',
             'pattern','ping','placeholder','rel','rev','rules','scheme','scrolling','shape','sizes',
             'srcdoc','srclang','srcset','standby','step','summary','target','text','useMap','vAlign',
             'vLink','valueType','version','wrap'];
    for (var i = 0; i < S.length; i++) def(S[i], 's');
    // DOMString with a differing content-attribute name.
    def('acceptCharset', 's', 'accept-charset'); def('ch', 's', 'char'); def('chOff', 's', 'charoff');
    def('defaultValue', 's', 'value'); def('httpEquiv', 's', 'http-equiv');
    // Boolean attributes.
    var B = ['allowFullscreen','autoplay','compact','controls','declare','defer','disabled',
             'formNoValidate','isMap','loop','multiple','noHref','noModule','noResize','noShade',
             'noValidate','noWrap','open','playsInline','readOnly','required','reversed','trueSpeed'];
    for (var j = 0; j < B.length; j++) def(B[j], 'b');
    def('defaultChecked', 'b', 'checked'); def('defaultMuted', 'b', 'muted'); def('defaultSelected', 'b', 'selected');
  }

  // NodeFilter + createTreeWalker / createNodeIterator (Lever 3), pure JS over the
  // wrapNode tree (firstChild/nextSibling/parentNode). Implements the DOM filter
  // semantics (whatToShow bitmask + ACCEPT/REJECT/SKIP) and the spec traversal.
  function installTraversal() {
    var NodeFilter = {
      FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
      SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 0x1, SHOW_ATTRIBUTE: 0x2, SHOW_TEXT: 0x4,
      SHOW_CDATA_SECTION: 0x8, SHOW_PROCESSING_INSTRUCTION: 0x40, SHOW_COMMENT: 0x80,
      SHOW_DOCUMENT: 0x100, SHOW_DOCUMENT_TYPE: 0x200, SHOW_DOCUMENT_FRAGMENT: 0x400
    };
    globalThis.NodeFilter = NodeFilter;

    function filterNode(node, whatToShow, filter) {
      if (!((whatToShow >>> 0) & (1 << (node.nodeType - 1)))) return NodeFilter.FILTER_SKIP;
      if (!filter) return NodeFilter.FILTER_ACCEPT;
      return (typeof filter === 'function') ? filter(node) : filter.acceptNode(node);
    }

    function TreeWalker(root, whatToShow, filter) {
      this.root = root;
      this.whatToShow = whatToShow >>> 0;
      this.filter = filter || null;
      this.currentNode = root;
    }
    TreeWalker.prototype._f = function(n) { return filterNode(n, this.whatToShow, this.filter); };
    TreeWalker.prototype.parentNode = function() {
      var node = this.currentNode;
      while (node !== null && node !== this.root) {
        node = node.parentNode;
        if (node !== null && this._f(node) === 1) { this.currentNode = node; return node; }
      }
      return null;
    };
    TreeWalker.prototype._traverseChildren = function(first) {
      var node = first ? this.currentNode.firstChild : this.currentNode.lastChild;
      while (node !== null) {
        var result = this._f(node);
        if (result === 1) { this.currentNode = node; return node; }
        if (result === 3) {
          var child = first ? node.firstChild : node.lastChild;
          if (child !== null) { node = child; continue; }
        }
        while (node !== null) {
          var sibling = first ? node.nextSibling : node.previousSibling;
          if (sibling !== null) { node = sibling; break; }
          var parent = node.parentNode;
          if (parent === null || parent === this.root || parent === this.currentNode) return null;
          node = parent;
        }
      }
      return null;
    };
    TreeWalker.prototype.firstChild = function() { return this._traverseChildren(true); };
    TreeWalker.prototype.lastChild = function() { return this._traverseChildren(false); };
    TreeWalker.prototype._traverseSiblings = function(next) {
      var node = this.currentNode;
      if (node === this.root) return null;
      while (true) {
        var sibling = next ? node.nextSibling : node.previousSibling;
        while (sibling !== null) {
          node = sibling;
          var result = this._f(node);
          if (result === 1) { this.currentNode = node; return node; }
          sibling = next ? node.firstChild : node.lastChild;
          if (result === 2 || sibling === null) { sibling = next ? node.nextSibling : node.previousSibling; }
        }
        node = node.parentNode;
        if (node === null || node === this.root) return null;
        if (this._f(node) === 1) return null;
      }
    };
    TreeWalker.prototype.nextSibling = function() { return this._traverseSiblings(true); };
    TreeWalker.prototype.previousSibling = function() { return this._traverseSiblings(false); };
    TreeWalker.prototype.nextNode = function() {
      var node = this.currentNode;
      var result = 1;
      while (true) {
        while (result !== 2 && node.firstChild !== null) {
          node = node.firstChild;
          result = this._f(node);
          if (result === 1) { this.currentNode = node; return node; }
        }
        var temporary = node;
        var sibling = null;
        while (temporary !== null) {
          if (temporary === this.root) return null;
          sibling = temporary.nextSibling;
          if (sibling !== null) break;
          temporary = temporary.parentNode;
        }
        if (sibling === null) return null;
        node = sibling;
        result = this._f(node);
        if (result === 1) { this.currentNode = node; return node; }
      }
    };
    TreeWalker.prototype.previousNode = function() {
      var node = this.currentNode;
      while (node !== this.root) {
        var sibling = node.previousSibling;
        while (sibling !== null) {
          node = sibling;
          var result = this._f(node);
          while (result !== 2 && node.lastChild !== null) {
            node = node.lastChild;
            result = this._f(node);
          }
          if (result === 1) { this.currentNode = node; return node; }
          sibling = node.previousSibling;
        }
        if (node === this.root) return null;
        var parent = node.parentNode;
        if (parent === null) return null;
        node = parent;
        if (this._f(node) === 1) { this.currentNode = node; return node; }
      }
      return null;
    };
    globalThis.TreeWalker = TreeWalker;
    Document.prototype.createTreeWalker = function(root, whatToShow, filter) {
      return new TreeWalker(root, whatToShow === undefined ? 0xFFFFFFFF : whatToShow, filter);
    };

    // NodeIterator over document order within root's subtree.
    function following(node, root) {
      if (node.firstChild) return node.firstChild;
      var n = node;
      while (n) {
        if (n === root) return null;
        if (n.nextSibling) return n.nextSibling;
        n = n.parentNode;
      }
      return null;
    }
    function preceding(node, root) {
      if (node === root) return null;
      if (node.previousSibling) {
        var n = node.previousSibling;
        while (n.lastChild) n = n.lastChild;
        return n;
      }
      return node.parentNode === root ? null : node.parentNode;
    }
    function NodeIterator(root, whatToShow, filter) {
      this.root = root;
      this.whatToShow = whatToShow >>> 0;
      this.filter = filter || null;
      this.referenceNode = root;
      this.pointerBeforeReferenceNode = true;
    }
    NodeIterator.prototype._traverse = function(next) {
      var node = this.referenceNode;
      var beforeNode = this.pointerBeforeReferenceNode;
      while (true) {
        if (next) {
          if (!beforeNode) { node = following(node, this.root); if (node === null) return null; }
          else { beforeNode = false; }
        } else {
          if (beforeNode) { node = preceding(node, this.root); if (node === null) return null; }
          else { beforeNode = true; }
        }
        if (filterNode(node, this.whatToShow, this.filter) === 1) {
          this.referenceNode = node;
          this.pointerBeforeReferenceNode = beforeNode;
          return node;
        }
      }
    };
    NodeIterator.prototype.nextNode = function() { return this._traverse(true); };
    NodeIterator.prototype.previousNode = function() { return this._traverse(false); };
    NodeIterator.prototype.detach = function() {};
    globalThis.NodeIterator = NodeIterator;
    Document.prototype.createNodeIterator = function(root, whatToShow, filter) {
      return new NodeIterator(root, whatToShow === undefined ? 0xFFFFFFFF : whatToShow, filter);
    };
  }

  // The document is a Document instance over the root reflector, registered in the
  // wrapper cache so wrapNode(rootRef) returns this same object.
  var docRef = __documentRoot();
  var document = Object.create(Document.prototype);
  document.__ref = docRef;
  document.nodeType = 9;
  wrappers.set(docRef, document);
  globalThis.document = document;
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Runtime;

    /// JS builds and mutates a tree through `document`, exercised against any backend:
    /// `createElement`/`createTextNode` mint nodes, `appendChild` parents them,
    /// `setAttribute` + `textContent` mutate, and `getElementById` finds by id — all
    /// landing in the host `ScriptedDom`, with the changes recorded as mutations.
    fn dom_construction_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "var d = document.createElement('div');\
             d.setAttribute('id', 'main');\
             var t = document.createTextNode('hello');\
             d.appendChild(t);\
             document.appendChild(d);\
             var found = document.getElementById('main');\
             found.textContent = 'world';",
        )
        .expect("dom script");

        {
            let host = rt.host().borrow();
            let dom = &host.dom;

            // The document root has the one <div> we appended.
            let root = dom.document();
            let kids: Vec<_> = dom.dom_children(root).collect();
            assert_eq!(kids.len(), 1, "div appended under document");
            let div = kids[0];

            // The div: a <div> element, id=main, textContent set to 'world'.
            assert_eq!(dom.element_name(div).unwrap().local, LocalName::from("div"));
            assert_eq!(
                dom.attribute(div, &Namespace::from(""), &LocalName::from("id")),
                Some("main"),
                "getElementById found the div and setAttribute stuck",
            );
            assert_eq!(dom.text(div), Some("world"), "textContent setter ran");

            // Its text-node child still carries the original data.
            let div_kids: Vec<_> = dom.dom_children(div).collect();
            assert_eq!(div_kids.len(), 1);
            assert_eq!(dom.text(div_kids[0]), Some("hello"));
        }

        // The structural + attribute + character-data changes were recorded for
        // serval-layout: setAttribute, two appendChilds, textContent → 4 mutations.
        // (createElement / createTextNode record nothing until parented.)
        let mut muts = Vec::new();
        rt.host().borrow_mut().dom.drain_mutations(&mut muts);
        assert_eq!(muts.len(), 4, "one attr + two inserts + one char-data");
    }

    /// The read surface, exercised against any backend: `getAttribute` / `tagName` /
    /// `textContent` getter return strings, and a miss returns `null`
    /// (`getAttribute` on an absent attr, `getElementById` with no match).
    fn dom_read_surface_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "var d = document.createElement('div');\
             d.setAttribute('id', 'main');\
             d.textContent = 'hello';\
             document.appendChild(d);\
             var el = document.getElementById('main');\
             console.log(el.getAttribute('id'));\
             console.log(el.tagName);\
             console.log(el.textContent);\
             console.log(String(el.getAttribute('nope')));\
             console.log(String(document.getElementById('nope')));",
        )
        .expect("read script");

        assert_eq!(
            rt.host().borrow().console,
            vec!["main", "DIV", "hello", "null", "null"],
        );
    }

    /// Reflector identity, exercised against any backend: two lookups of the same
    /// node are `===` (canonical reflector + wrapper cache), distinct nodes are not,
    /// and `document` is stable.
    fn dom_identity_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "var d = document.createElement('div');\
             d.setAttribute('id', 'main');\
             document.appendChild(d);\
             console.log(String(document.getElementById('main') === document.getElementById('main')));\
             console.log(String(document.getElementById('main') === d));\
             console.log(String(document.createElement('div') === document.createElement('div')));\
             console.log(String(document === document));",
        )
        .expect("identity script");

        // same node: ===; created === found-by-id; two fresh elements: not ===; doc stable.
        assert_eq!(rt.host().borrow().console, vec!["true", "true", "false", "true"]);
    }

    /// Prototype dispatch, exercised against any backend: methods live on
    /// `Node.prototype` (shared, not per-object closures), `instanceof` works, the
    /// `Document : Node` chain holds, and `parentNode` walks the real tree.
    fn dom_prototype_dispatch_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "var d = document.createElement('div');\
             var e = document.createElement('span');\
             document.appendChild(d);\
             console.log(String(d instanceof Node));\
             console.log(String(document instanceof Document));\
             console.log(String(document instanceof Node));\
             console.log(String(d.appendChild === e.appendChild));\
             console.log(String(d.parentNode === document));",
        )
        .expect("prototype script");

        // element is a Node; document is a Document and a Node; the method is shared
        // (same prototype function); parentNode walks back to the document.
        assert_eq!(rt.host().borrow().console, vec!["true", "true", "true", "true", "true"]);
    }

    /// `load_dom`, against any backend: a parsed source document becomes the live
    /// DOM, so script sees `document.body`, `getElementById`, and tag queries over
    /// the pre-existing tree.
    fn load_dom_works<E: ScriptEngine>() {
        use serval_static_dom::StaticDocument;
        let mut rt = Runtime::<E>::new().expect("runtime");
        let src = StaticDocument::parse(
            "<html><head></head><body><div id='main'><p>hi</p></div></body></html>",
        );
        rt.load_dom(&src);

        rt.eval(
            "console.log(document.body ? document.body.tagName : 'no-body');\
             console.log(document.documentElement ? document.documentElement.tagName : 'no-root');\
             var m = document.getElementById('main');\
             console.log(m ? m.tagName : 'not-found');\
             console.log(String(document.getElementsByTagName('p').length));",
        )
        .expect("query script");

        assert_eq!(rt.host().borrow().console, vec!["BODY", "HTML", "DIV", "1"]);
    }

    /// The Element surface, against any backend: prototype split (`instanceof
    /// Element`, `nodeType`), attribute methods (`hasAttribute` / `removeAttribute`
    /// / `toggleAttribute`), reflection (`id` / `className`), `classList`, and
    /// `querySelector` / `querySelectorAll` / `matches` over a loaded tree.
    fn dom_element_surface_works<E: ScriptEngine>() {
        use serval_static_dom::StaticDocument;
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.load_dom(&StaticDocument::parse(
            "<html><body><div id='a' class='x y'><p class='x'>hi</p><span></span></div></body></html>",
        ));

        rt.eval(
            "var div = document.getElementById('a');\
             console.log(String(div instanceof Element));\
             console.log(String(div.nodeType));\
             console.log(div.id + ',' + div.className);\
             console.log(String(div.hasAttribute('class')) + ',' + String(div.hasAttribute('nope')));\
             div.classList.add('z'); div.classList.remove('y');\
             console.log(div.className + ',' + String(div.classList.contains('x')) + ',' + String(div.classList.length));\
             div.toggleAttribute('hidden');\
             console.log(String(div.hasAttribute('hidden')));\
             console.log(String(document.querySelectorAll('.x').length));\
             console.log(document.querySelector('div > p').textContent);\
             console.log(String(div.querySelectorAll('span').length));\
             console.log(String(document.querySelector('p').matches('.x')));",
        )
        .expect("element script");

        assert_eq!(
            rt.host().borrow().console,
            vec![
                "true",        // div instanceof Element
                "1",           // nodeType ELEMENT_NODE
                "a,x y",       // id, className
                "true,false",  // hasAttribute
                "x z,true,2",  // className after add('z')/remove('y'); classList has x; length 2
                "true",        // toggleAttribute added 'hidden'
                "2",           // .x matches div + p
                "hi",          // div > p textContent
                "1",           // div's span descendants
                "true",        // p matches .x
            ],
        );
    }

    #[test]
    fn dom_construction_on_boa() {
        dom_construction_works::<script_engine_boa::BoaEngine>();
    }

    /// Node/Element traversal + mutation, against any backend: child/sibling
    /// navigation (incl. element-filtered), `nodeName`/`nodeValue`, `childNodes`,
    /// `removeChild` / `insertBefore` / `replaceChild`, and the ChildNode mixin.
    fn dom_traversal_works<E: ScriptEngine>() {
        use serval_static_dom::StaticDocument;
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.load_dom(&StaticDocument::parse(
            "<html><body><div id='p'>text<span id='a'></span><span id='b'></span></div></body></html>",
        ));

        rt.eval(
            "var p = document.getElementById('p');\
             console.log(String(p.childNodes.length));\
             console.log(p.firstChild.nodeName + ',' + p.firstChild.nodeValue);\
             console.log(String(p.childElementCount));\
             console.log(p.firstElementChild.id + ',' + p.lastElementChild.id);\
             var a = document.getElementById('a');\
             console.log(a.nextElementSibling.id + ',' + String(a.previousElementSibling));\
             var c = document.createElement('span'); c.id = 'c';\
             p.insertBefore(c, document.getElementById('b'));\
             console.log(p.children.map(function(e){return e.id;}).join(','));\
             p.removeChild(a);\
             console.log(p.children.map(function(e){return e.id;}).join(','));\
             var d = document.createElement('span'); d.id = 'd'; c.after(d);\
             console.log(p.children.map(function(e){return e.id;}).join(','));\
             d.remove();\
             console.log(p.children.map(function(e){return e.id;}).join(','));\
             console.log(String(p.contains(c)) + ',' + String(p.contains(a)));",
        )
        .expect("traversal script");

        assert_eq!(
            rt.host().borrow().console,
            vec![
                "3",            // childNodes: text + span#a + span#b
                "#text,text",   // firstChild nodeName/nodeValue
                "2",            // childElementCount (two spans)
                "a,b",          // first/last element child ids
                "b,null",       // a.nextElementSibling=b, previousElementSibling=null
                "a,c,b",        // after insertBefore(c, b)
                "c,b",          // after removeChild(a)
                "c,d,b",        // after c.after(d)
                "c,b",          // after d.remove()
                "true,false",   // contains c (yes), a (removed, no)
            ],
        );
    }

    #[test]
    fn dom_element_surface_on_boa() {
        dom_element_surface_works::<script_engine_boa::BoaEngine>();
    }

    /// Reflected IDL attributes + namespace getters + createElementNS + tree
    /// walker + document.title, against any backend.
    fn dom_reflection_ns_works<E: ScriptEngine>() {
        use serval_static_dom::StaticDocument;
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.load_dom(&StaticDocument::parse(
            "<html><head><title>  Hi   there </title></head><body><a id='x'></a></body></html>",
        ));

        rt.eval(
            "var a = document.getElementById('x');\
             console.log(typeof a.title + ',' + typeof a.hidden + ',' + typeof a.tabIndex);\
             a.title = 'T'; console.log(a.title + ',' + a.getAttribute('title'));\
             a.hidden = true; console.log(String(a.hidden) + ',' + String(a.hasAttribute('hidden')));\
             a.hidden = false; console.log(String(a.hidden));\
             a.tabIndex = 3; console.log(String(a.tabIndex));\
             console.log(a.localName + ',' + a.namespaceURI + ',' + a.tagName);\
             var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg:rect');\
             console.log(svg.localName + ',' + svg.namespaceURI + ',' + svg.prefix + ',' + svg.tagName);\
             console.log(document.title);\
             console.log(typeof NodeFilter + ',' + NodeFilter.SHOW_ELEMENT);\
             var tw = document.createTreeWalker(document.body, NodeFilter.SHOW_ELEMENT);\
             var seen = []; var n; while ((n = tw.nextNode())) { seen.push(n.localName); }\
             console.log(seen.join(','));",
        )
        .expect("reflection/ns script");

        assert_eq!(
            rt.host().borrow().console,
            vec![
                "string,boolean,number",       // typeof reflected attrs
                "T,T",                          // title set reflects to attribute
                "true,true",                    // hidden boolean reflects
                "false",                        // hidden cleared
                "3",                            // tabIndex long roundtrip
                "a,http://www.w3.org/1999/xhtml,A", // localName/namespaceURI/tagName (HTML upper)
                "rect,http://www.w3.org/2000/svg,svg,rect", // createElementNS: not upper-cased, prefix kept
                "Hi there",                     // document.title whitespace-collapsed
                "object,1",                     // NodeFilter present
                "a",                            // tree walker over body finds the <a>
            ],
        );
    }

    #[test]
    fn dom_traversal_on_boa() {
        dom_traversal_works::<script_engine_boa::BoaEngine>();
    }

    #[test]
    fn dom_reflection_ns_on_boa() {
        dom_reflection_ns_works::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_reflection_ns_on_nova() {
        dom_reflection_ns_works::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_traversal_on_nova() {
        dom_traversal_works::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_element_surface_on_nova() {
        dom_element_surface_works::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn load_dom_on_boa() {
        load_dom_works::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn load_dom_on_nova() {
        load_dom_works::<script_engine_nova::NovaEngine>();
    }

    /// Node-level EventTarget with tree propagation, against any backend: a
    /// bubbling event fires on the target then ancestors (with `target` /
    /// `currentTarget` set); a non-bubbling event does not reach ancestors;
    /// `stopPropagation` halts the climb.
    fn dom_node_events_work<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "var parent = document.createElement('div');\
             var child = document.createElement('span');\
             parent.appendChild(child);\
             document.appendChild(parent);\
             child.addEventListener('ping', function(e){ console.log('child:' + e.target.tagName); });\
             parent.addEventListener('ping', function(e){ console.log('parent:' + e.currentTarget.tagName); });\
             child.dispatchEvent(new Event('ping', { bubbles: true }));\
             parent.addEventListener('solo', function(){ console.log('solo-bubbled-SHOULD-NOT'); });\
             child.dispatchEvent(new Event('solo'));\
             child.addEventListener('stop', function(e){ e.stopPropagation(); console.log('child-stop'); });\
             parent.addEventListener('stop', function(){ console.log('parent-stop-SHOULD-NOT'); });\
             child.dispatchEvent(new Event('stop', { bubbles: true }));",
        )
        .expect("events script");

        // ping bubbles child→parent; solo does not reach parent (no bubble); stop is
        // halted at the child.
        assert_eq!(rt.host().borrow().console, vec!["child:SPAN", "parent:DIV", "child-stop"]);
    }

    #[test]
    fn dom_prototype_dispatch_on_boa() {
        dom_prototype_dispatch_works::<script_engine_boa::BoaEngine>();
    }

    #[test]
    fn dom_node_events_on_boa() {
        dom_node_events_work::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_node_events_on_nova() {
        dom_node_events_work::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_prototype_dispatch_on_nova() {
        dom_prototype_dispatch_works::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn dom_identity_on_boa() {
        dom_identity_works::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_identity_on_nova() {
        dom_identity_works::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn dom_read_surface_on_boa() {
        dom_read_surface_works::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_construction_on_nova() {
        dom_construction_works::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn dom_read_surface_on_nova() {
        dom_read_surface_works::<script_engine_nova::NovaEngine>();
    }
}
