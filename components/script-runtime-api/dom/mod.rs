// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `document` / `Node` construction surface — the live-DOM rung of the host
//! layer (`pluggable_engines_testharness_plan` step 2).
//!
//! Same shape as the rest of the host surface: native sinks (here, mutators of the
//! [`ScriptedDom`] in host state) plus a JS bootstrap that assembles the ergonomic
//! `document` object and wraps node handles. The JS→DOM bridge is the **reflector**
//! — a JS-opaque value carrying a `NodeId` (proven by `genet-scripted`'s `setText`,
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
//! `Comment` / `DocumentFragment` node types (`createComment` /
//! `createDocumentFragment`, nodeType 8 / 11), `cloneNode` (shallow + deep), and
//! **live** `HTMLCollection`s — `getElementsByTagName` / `getElementsByClassName` /
//! `children` are Proxy-backed and re-walked per access, so they reflect later
//! mutations — plus `DOMTokenList` (`classList` / `relList`), `dataset`, and
//! `NodeList` (`childNodes`). The reflected-attribute table carries `tokenlist`
//! (`t`, e.g. `relList`) and `url` (`u`, e.g. `href` / `src`, resolved against the
//! document base URL) kinds alongside DOMString / boolean / enumerated / long.
//! Verified by `dom_fragment_clone`, `dom_collections_works`,
//! `dom_tokenlist_dataset_works`, `dom_url_reflection_works`. Only the `double`
//! reflected kind remains. See
//! `docs/2026-05-26_pluggable_engines_testharness_plan.md`.

use std::cell::RefCell;
use std::rc::Rc;

use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, NodeKind, QualName};
use markup5ever::Prefix;
use script_engine_api::{CallCx, NativeFn, ScriptEngine};

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
                let Some(name) = src.element_name(child) else {
                    continue;
                };
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
    dom.dom_children(node)
        .find(|&c| dom.element_name(c).is_some())
}

/// Install the `document`/`Node` surface: native sinks, then the JS bootstrap that
/// builds `document` and the node wrappers over them.
pub(crate) fn install_dom_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<DocumentRoot>("__documentRoot", 0)?;
    engine.set_function::<ReflectNode>("__reflectNode", 1)?;
    engine.set_function::<CreateElement>("__createElement", 1)?;
    engine.set_function::<CreateTextNode>("__createTextNode", 1)?;
    engine.set_function::<AppendChild>("__appendChild", 2)?;
    engine.set_function::<SetAttribute>("__setAttribute", 3)?;
    engine.set_function::<SetTextContent>("__setTextContent", 2)?;
    engine.set_function::<GetElementById>("__getElementById", 2)?;
    engine.set_function::<GetAttribute>("__getAttribute", 2)?;
    engine.set_function::<TagName>("__tagName", 1)?;
    engine.set_function::<GetTextContent>("__getTextContent", 1)?;
    engine.set_function::<CookieGet>("__cookieGet", 0)?;
    engine.set_function::<CookieSet>("__cookieSet", 1)?;
    engine.set_function::<ParentNode>("__parentNode", 1)?;
    engine.set_function::<ElementsByTagNameCount>("__elementsByTagNameCount", 2)?;
    engine.set_function::<ElementsByTagNameItem>("__elementsByTagNameItem", 3)?;
    engine.set_function::<DocumentElement>("__documentElement", 1)?;
    engine.set_function::<DocumentBody>("__documentBody", 1)?;
    engine.set_function::<DocumentHead>("__documentHead", 1)?;
    engine.set_function::<CreateDocument>("__createDocument", 0)?;
    engine.set_function::<CreateComment>("__createComment", 1)?;
    engine.set_function::<CreateFragment>("__createFragment", 0)?;
    engine.set_function::<NodeType>("__nodeType", 1)?;
    engine.set_function::<NodeRawId>("__nodeRawId", 1)?;
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
    engine.set_function::<MoveBefore>("__moveBefore", 3)?;
    engine.set_function::<LocalNameOf>("__localName", 1)?;
    engine.set_function::<NamespaceUri>("__namespaceURI", 1)?;
    engine.set_function::<PrefixOf>("__prefix", 1)?;
    engine.set_function::<CreateElementNS>("__createElementNS", 2)?;
    engine.set_function::<AttributeNames>("__attributeNames", 1)?;
    engine.set_function::<ComputedStyleValue>("__computedStyleValue", 2)?;
    engine.set_function::<MatchMedia>("__matchMedia", 1)?;
    engine.set_function::<EvaluateXPath>("__xpathEvaluate", 2)?;
    let html_interfaces = html_interfaces::bootstrap_script();
    engine.eval(&html_interfaces)?;
    engine.eval(DOM_BOOTSTRAP)?;
    Ok(())
}

/// An HTML-namespaced element name (matches genet-layout's cascade keying).
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

/// The host's computed-style seam for `getComputedStyle`. Mirrors
/// [`FetchHandler`](crate::FetchHandler): the runtime never links a layout
/// engine, so a host that has one (e.g. pelt's `ScriptedDocument` over
/// `IncrementalLayout`) implements this to serialize a node's **computed** value
/// for a CSS longhand. `node` is the reflector's raw id; `property` is a longhand
/// name. `None` (unstyled / unsupported / no handler) surfaces to script as `""`.
/// Install with [`Runtime::set_computed_style_handler`](crate::Runtime::set_computed_style_handler).
pub trait ComputedStyleHandler {
    fn computed_value(&self, node: u64, property: &str) -> Option<String>;
}

/// Clone the computed-style handler out of host state (so it is not borrowed
/// while invoked).
fn host_computed_style<E: ScriptEngine>(
    cx: &mut E::CallCx<'_>,
) -> Option<Rc<dyn ComputedStyleHandler>> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let handler = cell.borrow().computed_style.clone();
    handler
}

/// `__computedStyleValue(nodeRef, property)` -> the host's serialized computed
/// value for the longhand, or `null` when there is no value / no handler.
struct ComputedStyleValue;
impl<E: ScriptEngine> NativeFn<E> for ComputedStyleValue {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(node) = cx.reflector_data(&el) else {
            return Ok(cx.make_null());
        };
        let a1 = cx.arg(1);
        let property = cx.value_to_string(&a1)?;
        let value = host_computed_style::<E>(cx).and_then(|h| h.computed_value(node, &property));
        match value {
            Some(v) => cx.make_string(&v),
            None => Ok(cx.make_null()),
        }
    }
}

/// The host's media-query seam for `window.matchMedia`. Mirrors
/// [`ComputedStyleHandler`]: the runtime links no layout engine, so a host with
/// one (evaluating against its `IncrementalLayout` device) implements this to
/// parse + evaluate a media query string. Returns the *serialized* (normalized)
/// query and whether it currently matches. Install with
/// [`Runtime::set_media_query_handler`](crate::Runtime::set_media_query_handler).
pub trait MediaQueryHandler {
    fn evaluate(&self, query: &str) -> (String, bool);
}

/// Clone the media-query handler out of host state (not borrowed while invoked).
fn host_media_query<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Option<Rc<dyn MediaQueryHandler>> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let handler = cell.borrow().media_query.clone();
    handler
}

/// `__matchMedia(query)` -> `"<0|1>\n<serialized media>"` (matches flag + the
/// normalized query), which the `matchMedia` shim splits into a MediaQueryList.
/// With no handler bound, returns `"0\n<raw query>"`.
struct MatchMedia;
impl<E: ScriptEngine> NativeFn<E> for MatchMedia {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let query = cx.value_to_string(&a0)?;
        let (media, matches) = match host_media_query::<E>(cx) {
            Some(h) => h.evaluate(&query),
            None => (query.clone(), false),
        };
        cx.make_string(&format!("{}\n{}", u8::from(matches), media))
    }
}

/// The host's cookie store for `document.cookie` (e.g. meerkat's view over the
/// netfetcher session jar). The runtime owns no networking, so the host supplies the
/// document's cookies. [`get_cookies`](Self::get_cookies) returns the *script-visible*
/// cookies for the current document — HttpOnly excluded, the host's job — as a
/// `"n1=v1; n2=v2"` string; [`set_cookie`](Self::set_cookie) takes one
/// `Set-Cookie`-style assignment (`"name=value; Path=/; ..."`). `None` = no store, so
/// `document.cookie` reads `""` and a write is a no-op. Install with
/// [`Runtime::set_cookie_provider`](crate::Runtime::set_cookie_provider).
pub trait CookieProvider {
    fn get_cookies(&self) -> String;
    fn set_cookie(&self, cookie: &str);
}

/// Clone the cookie provider out of host state (so it is not borrowed while invoked).
fn host_cookies<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Option<Rc<dyn CookieProvider>> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let provider = cell.borrow().cookies.clone();
    provider
}

/// `__cookieGet()` -> the document's script-visible cookies as a header string
/// (`""` when there is no provider).
struct CookieGet;
impl<E: ScriptEngine> NativeFn<E> for CookieGet {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let cookies = host_cookies::<E>(cx)
            .map(|h| h.get_cookies())
            .unwrap_or_default();
        cx.make_string(&cookies)
    }
}

/// `__cookieSet(value)` -> record one `Set-Cookie`-style assignment (no-op without a
/// provider).
struct CookieSet;
impl<E: ScriptEngine> NativeFn<E> for CookieSet {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let cookie = cx.value_to_string(&a0)?;
        if let Some(provider) = host_cookies::<E>(cx) {
            provider.set_cookie(&cookie);
        }
        Ok(cx.undefined())
    }
}

/// Hand script the **canonical** reflector for the node with raw id `raw`, and
/// **pin** that node (G1/G3). Every place a binding returns a node to script
/// routes through here: the node is now script-reachable, so the host pins it
/// until [`Runtime::collect_garbage`](crate::Runtime::collect_garbage) sees the
/// engine report the reflector dead. Pinning must be complete — a node handed
/// out unpinned could be swept while script still holds it — which is why this
/// is the sole node-handoff path (no binding calls `reflector_for` directly).
fn reflect_pinned<E: ScriptEngine>(cx: &mut E::CallCx<'_>, raw: u64) -> Result<E::Value, E::Error> {
    if let Some(data) = cx.host_data() {
        if let Some(cell) = data.downcast_ref::<RefCell<HostState>>() {
            cell.borrow_mut().pins.pin(NodeId::from_raw(raw as usize));
        }
    }
    cx.reflector_for(raw)
}

/// Depth-first search under `root` for the first element whose null-namespace `id`
/// equals `target`. `root` lets queries scope to a created document, not just the
/// primary one.
fn find_by_id(dom: &ScriptedDom, root: NodeId, target: &str) -> Option<NodeId> {
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
    walk(dom, root, target)
}

mod html_interfaces;
mod query_traverse;
mod tree;
mod xpath_eval;

#[cfg(test)]
mod tests;

use query_traverse::*;
use tree::*;
use xpath_eval::*;

/// `document` plus node wrappers. A wrapper is a plain object carrying its reflector
/// (`__ref`) and the methods that drive the native sinks. ES5-style (no arrows /
/// classes / let) for the widest backend coverage, matching the other bootstraps.
const DOM_BOOTSTRAP: &str = include_str!("bootstrap.js");
