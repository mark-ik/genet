// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! DOM query (selectors / collections), tree traversal, mutation, and
//! node-info command sinks.

use super::*;

/// `__removeAttribute(element, name)`.
pub(crate) struct RemoveAttribute;
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
pub(crate) struct Matches;
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
pub(crate) struct QuerySelector;
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
            Some(node) => reflect_pinned::<E>(cx,node.raw() as u64),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__querySelectorAllCount(scope, selector)` → match count (as a string). Paired
/// with `__querySelectorAllItem`, the count/item pattern used elsewhere.
pub(crate) struct QuerySelectorAllCount;
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
pub(crate) struct QuerySelectorAllItem;
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
            Some(node) => reflect_pinned::<E>(cx,node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__firstChild(node)` → first child reflector, or `undefined`.
pub(crate) struct FirstChild;
impl<E: ScriptEngine> NativeFn<E> for FirstChild {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        child_at::<E>(cx, |dom, node| dom.dom_children(node).next())
    }
}

/// `__lastChild(node)` → last child reflector, or `undefined`.
pub(crate) struct LastChild;
impl<E: ScriptEngine> NativeFn<E> for LastChild {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        child_at::<E>(cx, |dom, node| dom.dom_children(node).last())
    }
}

/// `__nextSibling(node)` → next sibling reflector, or `undefined`.
pub(crate) struct NextSibling;
impl<E: ScriptEngine> NativeFn<E> for NextSibling {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        child_at::<E>(cx, |dom, node| dom.next_sibling(node))
    }
}

/// `__prevSibling(node)` → previous sibling reflector, or `undefined`.
pub(crate) struct PrevSibling;
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
        Some(n) => reflect_pinned::<E>(cx,n.raw() as u64),
        None => Ok(cx.undefined()),
    }
}

/// `__childNodesCount(node)` → child count (string). With `__childNodesItem`, backs
/// `childNodes` (and, JS-filtered, `children`).
pub(crate) struct ChildNodesCount;
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
pub(crate) struct ChildNodesItem;
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
            Some(n) => reflect_pinned::<E>(cx,n.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__nodeName(node)`: element → uppercase tag; text → `#text`; comment →
/// `#comment`; document → `#document`; else the kind's conventional name.
pub(crate) struct NodeName;
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
                NodeKind::DocumentFragment => "#document-fragment".to_string(),
            }
        })
        .unwrap_or_default();
        cx.make_string(&name)
    }
}

/// `__nodeValue(node)`: text/comment → its data; otherwise `null`.
pub(crate) struct NodeValue;
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
pub(crate) struct RemoveChild;
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
pub(crate) struct InsertBefore;
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
pub(crate) struct LocalNameOf;
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
pub(crate) struct NamespaceUri;
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
pub(crate) struct PrefixOf;
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
pub(crate) struct CreateElementNS;
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
            Some(node) => reflect_pinned::<E>(cx,node.raw() as u64),
            None => Ok(cx.undefined()),
        }
    }
}

/// `__attributeNames(element)` → the element's attribute local names, space-joined
/// (attribute names contain no spaces). Backs `dataset` ownKeys / enumeration.
pub(crate) struct AttributeNames;
impl<E: ScriptEngine> NativeFn<E> for AttributeNames {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        let Some(id) = cx.reflector_data(&el) else {
            return cx.make_string("");
        };
        let names = with_dom::<E, _>(cx, |dom| {
            dom.attributes(NodeId::from_raw(id as usize))
                .map(|a| a.name.local.as_ref().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
        cx.make_string(&names)
    }
}
