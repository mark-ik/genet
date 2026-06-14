/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Per-tag element-view helpers: `div(children)` for `el("div", children)`, etc.
//!
//! Thin ergonomic wrappers over [`el`](crate::el) for the common HTML tags, so
//! authoring reads as `div((p("hi"), button("+")))` rather than repeating
//! `el("div", ..)`. Each returns an [`El`], so it is an
//! [`ElementView`](crate::ElementView) and composes with `.attr` / `on_click` /
//! `on_key` exactly as `el` does. `xilem_web` generates a per-tag view per HTML
//! element; serval has one element type, so these are one-liners over `el`.

use crate::pod::ServalElement;
use crate::{El, ServalCtx, el};
use xilem_core::ViewSequence;

macro_rules! tag_fns {
    ($($(#[$doc:meta])* $name:ident => $tag:literal),* $(,)?) => {
        $(
            $(#[$doc])*
            pub fn $name<Seq, State, Action>(children: Seq) -> El<Seq, State, Action>
            where
                State: 'static,
                Action: 'static,
                Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
            {
                el($tag, children)
            }
        )*
    };
}

tag_fns! {
    /// A `<div>` element view.
    div => "div",
    /// A `<span>` element view.
    span => "span",
    /// A `<p>` (paragraph) element view.
    p => "p",
    // Note: no `button` tag helper — `controls::button(label, handler)` is the
    // button view (a button without a handler does nothing). For a `<button>`
    // with custom children, use `on_click(el("button", children), handler)`.
    /// An `<input>` element view.
    input => "input",
    /// A `<label>` element view.
    label => "label",
    /// An `<a>` (anchor) element view.
    a => "a",
    /// An `<h1>` heading element view.
    h1 => "h1",
    /// An `<h2>` heading element view.
    h2 => "h2",
    /// An `<h3>` heading element view.
    h3 => "h3",
    /// A `<ul>` (unordered list) element view.
    ul => "ul",
    /// An `<ol>` (ordered list) element view.
    ol => "ol",
    /// An `<li>` (list item) element view.
    li => "li",
}

/// An `<external-texture>` element view: a host-composited texture region.
///
/// The producer registers a `wgpu::Texture` with the renderer under `key` (a stable
/// `u64`); serval lays out a `width`×`height` block box, and paint emits a
/// `DrawExternalTexture` at it that the host composites the producer's texture into —
/// a constellation actor scene, a scrying WebView, or a pelt tile's external-content
/// lane. A leaf: it has no serval-painted children (the texture *is* its content).
/// The element sets its own `display:block` + intrinsic size via the `style`
/// attribute, so it needs no stylesheet rule; override the size with CSS as for any
/// replaced box.
pub fn external_texture<State, Action>(key: u64, width: u32, height: u32) -> El<(), State, Action>
where
    State: 'static,
    Action: 'static,
{
    el("external-texture", ())
        .attr("key", key.to_string())
        .attr("style", format!("display:block;width:{width}px;height:{height}px"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServalAppRunner;
    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::ScriptedDom;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// `div(span("hi"))` builds a `<div>` element with a `<span>` child —
    /// confirming the helpers name their tags and nest like `el`.
    #[test]
    fn tag_helpers_build_named_elements() {
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &()| div::<_, (), ()>(span::<_, (), ()>("hi")),
            (),
        );
        let d = dom.borrow();
        let root = runner.root();
        assert_eq!(d.kind(root), NodeKind::Element);
        assert_eq!(d.element_name(root).unwrap().local.as_ref(), "div");
        let child = d.dom_children(root).next().expect("div has a child");
        assert_eq!(d.element_name(child).unwrap().local.as_ref(), "span");
    }

    /// `external_texture(7, 320, 240)` builds an `<external-texture>` element carrying
    /// the host texture key and a block box sized via its `style` attribute — the
    /// element serval-layout paints as a `DrawExternalTexture` compositor pass.
    #[test]
    fn external_texture_builds_keyed_element() {
        use layout_dom_api::{LocalName, Namespace};
        let no_ns = Namespace::from("");
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &()| external_texture::<(), ()>(7, 320, 240),
            (),
        );
        let d = dom.borrow();
        let root = runner.root();
        assert_eq!(
            d.element_name(root).unwrap().local.as_ref(),
            "external-texture",
            "the view names the reserved element",
        );
        assert_eq!(d.attribute(root, &no_ns, &LocalName::from("key")), Some("7"), "carries the key");
        assert_eq!(
            d.attribute(root, &no_ns, &LocalName::from("style")),
            Some("display:block;width:320px;height:240px"),
            "sizes itself as a block box",
        );
    }
}
