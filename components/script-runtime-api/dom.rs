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
//! (via [`crate::selector`]). Not yet: namespaced creation (`createElementNS`),
//! `Node` traversal breadth (`childNodes` / `firstChild` / `removeChild` /
//! `insertBefore`), and DOM methods *throwing* `DOMException` on bad input. See
//! `docs/2026-05-26_pluggable_engines_testharness_plan.md`.

use std::cell::RefCell;

use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, NodeKind, QualName};
use script_engine_api::{CallCx, NativeFn, ScriptEngine};
use serval_scripted_dom::{NodeId, ScriptedDom};

use crate::HostState;

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
            dom.element_name(NodeId::from_raw(id as usize)).map(|q| q.local.as_ref().to_ascii_uppercase())
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

  // Document : Node, with the construction/lookup methods.
  function Document() {}
  Document.prototype = Object.create(Node.prototype);
  Document.prototype.createElement = function(tag) { return wrapNode(__createElement(String(tag))); };
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
    configurable: true, get: function() { return wrapNode(__documentBody()); }
  });
  Object.defineProperty(Document.prototype, 'head', {
    configurable: true, get: function() { return wrapNode(__documentHead()); }
  });

  globalThis.Node = Node;
  globalThis.Element = Element;
  globalThis.Text = Text;
  globalThis.Document = Document;

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

    #[test]
    fn dom_element_surface_on_boa() {
        dom_element_surface_works::<script_engine_boa::BoaEngine>();
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
