/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The [`focusable`] marker view: make an element focusable on its own, with no
//! key handler attached.
//!
//! Focusability is otherwise *implied* by an [`on_key`](crate::on_key) handler (a
//! node is focusable because it is in the key registry). That leaves a plain
//! control — a [`button`](crate::button) or [`checkbox`](crate::checkbox), which
//! carry only an [`on_click`](crate::on_click) — keyboard-unreachable: it cannot be
//! Tab-focused and a screen-reader / keyboard user cannot activate it. Wrapping it
//! in [`focusable`] registers the node in [`ServalCtx`]'s explicit focusable set
//! (the keyboard-model escape hatch, grab-bag G2.3), so it joins the Tab order and
//! the runner activates it on Enter/Space by synthesizing a click (the keyboard
//! equivalent of a pointer click — see
//! [`dispatch_key`](crate::ServalAppRunner::dispatch_key)).
//!
//! The view is a transparent wrapper, structured like [`OnKey`](crate::OnKey) minus
//! the handler: it pushes its own [`ON_FOCUSABLE_ID`] so its routing position is
//! well-formed, registers/unregisters the focusable mark on build/teardown,
//! re-keys it on a node swap, and forwards every routed message to its child (it
//! never terminates a path — it owns no handler). It passes the child's element
//! through, so it is itself an [`ElementView`] and composes:
//! `focusable(button(..))` is a keyboard-operable button.

use core::marker::PhantomData;

use meristem::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use serval_scripted_dom::NodeId;

use crate::pod::ServalElement;
use crate::{ElementView, ServalCtx};

// A distinctive number, mirroring [`OnKey`](crate::OnKey)'s `ON_KEY_ID`, so a
// stray message routed here on a wrong path is caught rather than silently
// matching. This is a randomly generated 32-bit number — 1187631267 in decimal.
const ON_FOCUSABLE_ID: ViewId = ViewId::new(0x46C5_2A63);

/// Wraps a [`View`] `V` and marks its element focusable without attaching any
/// handler. Construct with [`focusable`].
///
/// Carries no `State`/`Action` of its own (it has no handler); the `View` impl is
/// generic over them through the wrapped [`ElementView`]'s bound, so the
/// constructor needs no turbofish.
pub struct Focusable<V> {
    child: V,
}

/// Retained state for a [`Focusable`].
pub struct FocusableState<S> {
    child_state: S,
    /// The wrapped child's DOM node, so teardown can unregister and rebuild can
    /// detect a node swap and re-key the focusable set.
    node: NodeId,
    phantom: PhantomData<()>,
}

/// Mark `child`'s element focusable explicitly, independent of any key handler.
///
/// Use it to make an [`on_click`](crate::on_click)-only control (a
/// [`button`](crate::button), a [`checkbox`](crate::checkbox)) keyboard-operable:
/// once focusable, it joins the Tab order and the runner activates it on
/// Enter/Space by synthesizing a click. A control that already carries an
/// [`on_key`](crate::on_key) is focusable without this. Composes over any element
/// view: `focusable(button("Go", go))` is a keyboard-operable button.
pub fn focusable<V>(child: V) -> Focusable<V> {
    Focusable { child }
}

impl<V> ViewMarker for Focusable<V> {}

impl<V, State, Action> View<State, Action, ServalCtx> for Focusable<V>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
{
    type ViewState = FocusableState<V::ViewState>;

    type Element = ServalElement;

    fn build(
        &self,
        ctx: &mut ServalCtx,
        app_state: &mut State,
    ) -> (Self::Element, Self::ViewState) {
        // Push our own id so the captured routing position (and any descendant
        // handler's path) is well-formed — mirrors `OnKey::build`.
        ctx.with_id(ON_FOCUSABLE_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let node = element.node;
            ctx.register_focusable(node);
            (
                element,
                FocusableState {
                    child_state,
                    node,
                    phantom: PhantomData,
                },
            )
        })
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut ServalCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        ctx.with_id(ON_FOCUSABLE_ID, |ctx| {
            let prev_node = view_state.node;
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            // The child may have swapped its node; if so, move the focusable mark
            // to the new node (the mark is just node membership, nothing to carry).
            let node = *element.node;
            if node != prev_node {
                ctx.unregister_focusable(prev_node);
                ctx.register_focusable(node);
                view_state.node = node;
            }
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ServalCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(ON_FOCUSABLE_ID, |ctx| {
            ctx.unregister_focusable(view_state.node);
            self.child
                .teardown(&mut view_state.child_state, ctx, element);
        });
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        // Consume our own id, then forward: a `Focusable` owns no handler, so a
        // routed message is always for a descendant (it never terminates a path).
        let Some(first) = message.take_first() else {
            return MessageResult::Stale;
        };
        if first != ON_FOCUSABLE_ID {
            return MessageResult::Stale;
        }
        self.child
            .message(&mut view_state.child_state, message, element, app_state)
    }
}

// `Focusable` passes its child's element through, so a focusable-wrapped element
// is itself an `ElementView` — letting it compose under / over the handler views.
impl<V, State, Action> ElementView<State, Action> for Focusable<V>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
{
}
