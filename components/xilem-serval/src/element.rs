/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The element [`View`]: [`el`]`(name, children)`, with an [`El::attr`]
//! modifier.
//!
//! Modelled on `xilem_web`'s `define_element!`-generated views, collapsed to a
//! single generic `El` (serval has one element type, so there is no per-tag
//! `DomNode` interface to specialize). Children are any `xilem_core`
//! [`ViewSequence`] over [`ServalElement`]; attributes are applied eagerly to
//! the `ScriptedDom`.

use crate::pod::ServalElement;
use crate::{DomHandle, ServalCtx, ServalChildrenSplice, attr_qual, html_qual};
use layout_dom_api::LayoutDomMut;
use xilem_core::{
    AppendVec, MessageCtx, MessageResult, Mut, View, ViewMarker, ViewSequence,
};

/// Re-export alias so callers can name the backend element type as
/// `xilem_serval::Element`, matching `xilem_web`'s convention of an element
/// type per view. Here there is only one.
pub use crate::pod::ServalElement as Element;

/// Marker for views that produce an **element** node (not text or other
/// content) — the Element/Text type split.
///
/// Element-only operations — [`on_click`](crate::on_click),
/// [`on_key`](crate::on_key), and [`El::attr`] — require `ElementView`, so they
/// reject text views (`&str` / `String` / [`text`](crate::text)): a text node is
/// not a sensible click / key / attribute target. Every node in serval is still
/// one runtime type ([`ServalElement`]) — this is a *compile-time* distinction
/// over views, not a second element type, so the children sequence stays
/// heterogeneous (text and elements mix freely as children).
///
/// Implemented by [`El`] and the element-preserving wrappers
/// [`OnClick`](crate::OnClick) / [`OnKey`](crate::OnKey) (whose `Element` is
/// their child's), so handlers compose: `on_key(on_click(el(..), ..), ..)`.
///
/// The split in action — a text view cannot be a click target:
/// ```compile_fail
/// use xilem_serval::{on_click, text, PointerClick};
/// // `text(..)` is not an `ElementView`, so this fails to compile.
/// let _ = on_click::<_, (), (), (), _>(text("hi"), |_: &mut (), _: PointerClick| {});
/// ```
pub trait ElementView<State: 'static, Action>:
    View<State, Action, ServalCtx, Element = ServalElement>
{
}

/// An HTML element view: a tag name, a child [`ViewSequence`], and attributes.
///
/// Construct with [`el`]. Add attributes with [`El::attr`].
pub struct El<Seq, State, Action> {
    name: String,
    children: Seq,
    /// Ordered attribute modifiers: `(name, value)`. Diffed on rebuild.
    attrs: Vec<(String, String)>,
    phantom: core::marker::PhantomData<fn() -> (State, Action)>,
}

/// Create an element view named `name` with the given children sequence.
///
/// `children` is any `xilem_core` [`ViewSequence`] over [`ServalElement`]: a
/// single view, a tuple of views, a `Vec`, an `Option`, a string/number (text),
/// etc.
pub fn el<Seq, State, Action>(name: impl Into<String>, children: Seq) -> El<Seq, State, Action>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
    El {
        name: name.into(),
        children,
        attrs: Vec::new(),
        phantom: core::marker::PhantomData,
    }
}

impl<Seq, State, Action> El<Seq, State, Action> {
    /// Add (or override) an attribute. Builder-style; chainable.
    #[must_use]
    pub fn attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        let value = value.into();
        if let Some(existing) = self.attrs.iter_mut().find(|(n, _)| *n == name) {
            existing.1 = value;
        } else {
            self.attrs.push((name, value));
        }
        self
    }
}

/// Retained state for an [`El`].
pub struct ElementState<SeqState> {
    seq_state: SeqState,
    /// The retained child elements, in DOM order.
    children: Vec<ServalElement>,
    append_scratch: AppendVec<ServalElement>,
    vec_splice_scratch: Vec<ServalElement>,
}

/// Apply the attribute diff between `prev` and `next` eagerly to `node`.
fn apply_attr_diff(
    dom: &DomHandle,
    node: serval_scripted_dom::NodeId,
    prev: &[(String, String)],
    next: &[(String, String)],
) {
    let mut dom = dom.borrow_mut();
    // Set new or changed attributes.
    for (name, value) in next {
        let changed = match prev.iter().find(|(n, _)| n == name) {
            Some((_, old)) => old != value,
            None => true,
        };
        if changed {
            dom.set_attribute(node, attr_qual(name), value);
        }
    }
    // Remove attributes that were present before but are gone now.
    for (name, _) in prev {
        if !next.iter().any(|(n, _)| n == name) {
            dom.remove_attribute(node, attr_qual(name));
        }
    }
}

impl<Seq, State, Action> ViewMarker for El<Seq, State, Action> {}

impl<Seq, State, Action> View<State, Action, ServalCtx> for El<Seq, State, Action>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
    type Element = ServalElement;

    type ViewState = ElementState<Seq::SeqState>;

    fn build(&self, ctx: &mut ServalCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        let dom = ctx.dom();
        let node = dom.borrow_mut().create_element(html_qual(&self.name));

        // Build the children detached, then attach each under the new node in
        // order (append == insert_before reference None).
        let mut append_scratch = AppendVec::default();
        let seq_state = self.children.seq_build(ctx, &mut append_scratch, app_state);
        let children: Vec<ServalElement> = append_scratch.into_inner();
        {
            let mut dom_mut = dom.borrow_mut();
            for child in &children {
                dom_mut.insert_before(node, child.node, None);
            }
        }

        // Apply attributes (all "new" against an empty prev).
        apply_attr_diff(&dom, node, &[], &self.attrs);

        let state = ElementState {
            seq_state,
            children,
            append_scratch: AppendVec::default(),
            vec_splice_scratch: Vec::new(),
        };
        (ServalElement::new(node, dom), state)
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ServalCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        let node = *element.node;
        let dom = element.dom.clone();

        // Diff children through the splice.
        let mut splice = ServalChildrenSplice::new(
            &mut view_state.append_scratch,
            &mut view_state.children,
            &mut view_state.vec_splice_scratch,
            node,
            dom.clone(),
            false,
        );
        self.children.seq_rebuild(
            &prev.children,
            &mut view_state.seq_state,
            ctx,
            &mut splice,
            app_state,
        );

        // Diff attributes eagerly.
        apply_attr_diff(&dom, node, &prev.attrs, &self.attrs);
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ServalCtx,
        element: Mut<'_, Self::Element>,
    ) {
        let node = *element.node;
        let dom = element.dom.clone();
        let mut splice = ServalChildrenSplice::new(
            &mut view_state.append_scratch,
            &mut view_state.children,
            &mut view_state.vec_splice_scratch,
            node,
            dom,
            // The parent node itself is being removed by the caller; skip the
            // per-child DOM removals.
            true,
        );
        self.children
            .seq_teardown(&mut view_state.seq_state, ctx, &mut splice);
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        let node = *element.node;
        let dom = element.dom.clone();
        let mut splice = ServalChildrenSplice::new(
            &mut view_state.append_scratch,
            &mut view_state.children,
            &mut view_state.vec_splice_scratch,
            node,
            dom,
            false,
        );
        self.children
            .seq_message(&mut view_state.seq_state, message, &mut splice, app_state)
    }
}

// `El` produces an element node, so it is the canonical `ElementView` (same
// bounds as its `View` impl).
impl<Seq, State, Action> ElementView<State, Action> for El<Seq, State, Action>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
}
