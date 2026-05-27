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
//! `CallCx::reflector_data`; outgoing nodes (`createElement`) are minted with
//! `CallCx::make_reflector`.
//!
//! Construction/mutation half: `createElement`, `createTextNode`, `appendChild`,
//! `setAttribute`, `textContent` (setter), `getElementById`. Read half
//! (`getAttribute`, `tagName`, `textContent` getter), via `CallCx::make_string` /
//! `make_null`. Generic over the backend; tested on Boa + Nova like the rest of the
//! host surface.
//!
//! Not yet (true-W0 remaining): reflector *identity* (`document.body ===
//! document.body`) needs an engine-side `NodeId → reflector` cache, since a cached
//! reflector is an engine-native value and cannot live in neutral host state;
//! prototype-based dispatch instead of the per-object closures `wrapNode` builds;
//! and node-level `EventTarget` with tree propagation. See
//! `docs/2026-05-26_pluggable_engines_testharness_plan.md`.

use std::cell::RefCell;

use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};
use script_engine_api::{CallCx, NativeFn, ScriptEngine};
use serval_scripted_dom::{NodeId, ScriptedDom};

use crate::HostState;

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
            Some(root) => cx.make_reflector(root.raw() as u64),
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
            Some(id) => cx.make_reflector(id.raw() as u64),
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
            Some(id) => cx.make_reflector(id.raw() as u64),
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
            Some(node) => cx.make_reflector(node.raw() as u64),
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

/// `__getTextContent(node)` → the node's text content (empty string if none).
struct GetTextContent;
impl<E: ScriptEngine> NativeFn<E> for GetTextContent {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let node = cx.arg(0);
        let Some(id) = cx.reflector_data(&node) else {
            return Ok(cx.make_null());
        };
        let text = with_dom::<E, _>(cx, |dom| dom.text(NodeId::from_raw(id as usize)).map(str::to_string))
            .flatten()
            .unwrap_or_default();
        cx.make_string(&text)
    }
}

/// `document` plus node wrappers. A wrapper is a plain object carrying its reflector
/// (`__ref`) and the methods that drive the native sinks. ES5-style (no arrows /
/// classes / let) for the widest backend coverage, matching the other bootstraps.
const DOM_BOOTSTRAP: &str = r#"
(function() {
  function wrapNode(ref) {
    var node = { __ref: ref };
    node.appendChild = function(child) { __appendChild(ref, child.__ref); return child; };
    node.setAttribute = function(name, value) { __setAttribute(ref, String(name), String(value)); };
    node.getAttribute = function(name) { return __getAttribute(ref, String(name)); };
    Object.defineProperty(node, 'tagName', {
      configurable: true,
      get: function() { return __tagName(ref); }
    });
    Object.defineProperty(node, 'textContent', {
      configurable: true,
      get: function() { return __getTextContent(ref); },
      set: function(v) { __setTextContent(ref, String(v)); }
    });
    return node;
  }
  var document = wrapNode(__documentRoot());
  document.createElement = function(tag) { return wrapNode(__createElement(String(tag))); };
  document.createTextNode = function(data) { return wrapNode(__createTextNode(String(data))); };
  document.getElementById = function(id) {
    var r = __getElementById(String(id));
    return (r === undefined) ? null : wrapNode(r);
  };
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

    #[test]
    fn dom_construction_on_boa() {
        dom_construction_works::<script_engine_boa::BoaEngine>();
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
