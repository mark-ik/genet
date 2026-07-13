/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`button`]: a `<button>` view with a click handler, and [`button_with`], the
//! same over an arbitrary child view (a chisel leaf, an icon, a row).

use crate::{El, OnClick, OptionalAction, PointerClick, ServalCtx, ServalElement, el, on_click};
use xilem_core::ViewSequence;

/// A `<button>` view: `label` text plus an `on_click` handler — the ergonomic
/// form of `on_click(el("button", label), handler)`. The handler may return an
/// action (it is an [`OptionalAction`]) exactly as [`on_click`](crate::on_click).
///
/// Add a `class` (or any attribute) with the fluent [`OnClick::attr`], e.g.
/// `button("Save", on_save).attr("class", "primary")`.
pub fn button<State, Action, OA, F>(
    label: impl Into<String>,
    handler: F,
) -> OnClick<El<String, State, Action>, State, Action, F>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerClick) -> OA + 'static,
{
    on_click(el::<_, State, Action>("button", label.into()), handler)
}

/// A `<button>` view over an arbitrary child sequence rather than a text label:
/// `on_click(el("button", child), handler)`. This is how a chisel leaf rides
/// inside a native control — `button_with(custom_leaf(key, w, h), on_press)` is
/// the catalog's "native `button` wrapping a tiny `GraphGlyph` leaf".
///
/// Composition rule: the leaf only paints; the interaction lives here in the view
/// layer (the button owns focus, keyboard activation, and the click handler).
///
/// The child leaf reaches paint at any button `display`. It takes the block
/// replaced-leaf path inside a block button, and rides as an `InlineBoxItem`
/// inside the `inline-block` button serval's UA sheet gives `<button>`. Pinned by
/// `a_chisel_leaf_inside_a_button_is_reported_at_every_button_display` in
/// serval-layout.
pub fn button_with<Seq, State, Action, OA, F>(
    child: Seq,
    handler: F,
) -> OnClick<El<Seq, State, Action>, State, Action, F>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerClick) -> OA + 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
    on_click(el::<_, State, Action>("button", child), handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::custom_leaf;
    use crate::{AnyView, DomHandle, ServalAppRunner};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};
    use serval_scripted_dom::{NodeId, ScriptedDom};
    use std::cell::RefCell;
    use std::rc::Rc;

    const GLYPH_KEY: u64 = 7;

    type BtnView = Box<dyn AnyView<u32, (), ServalCtx, ServalElement>>;

    /// The catalog's tier-1 + tier-2 composition: a native `<button>` whose only
    /// content is a `GraphGlyph` chisel leaf. The leaf paints; the button owns the
    /// interaction.
    fn glyph_button(_presses: &u32) -> BtnView {
        Box::new(button_with(
            custom_leaf::<u32, ()>(GLYPH_KEY, 20, 20),
            |presses: &mut u32, _click: PointerClick| {
                *presses += 1;
            },
        ))
    }

    fn child_named(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<NodeId> {
        dom.dom_children(node)
            .into_iter()
            .find(|&c| dom.element_name(c).is_some_and(|q| q.local.as_ref() == name))
    }

    #[test]
    fn graph_glyph_leaf_draws_inside_a_native_button_and_the_button_owns_the_click() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(dom.clone(), glyph_button, 0u32);
        let root = runner.root();

        {
            let d = dom.borrow();
            assert_eq!(
                d.element_name(root).map(|q| q.local.to_string()).as_deref(),
                Some("button"),
                "button_with yields a native <button> root",
            );
            let leaf = child_named(&d, root, "custom-leaf")
                .expect("the glyph leaf is a child of the button");
            assert_eq!(
                d.attribute(leaf, &Namespace::from(""), &LocalName::from("key"))
                    .map(|k| k.to_string())
                    .as_deref(),
                Some("7"),
                "the leaf carries its registry key, which is all layout stamps onto the box",
            );
        }

        // Interaction lives in the view layer, not in `Leaf::event`: the click
        // lands on the button, not the leaf.
        runner.dispatch_click(root, PointerClick::at((10.0, 10.0)));
        assert_eq!(*runner.state(), 1, "clicking the button runs the handler");
    }
}
