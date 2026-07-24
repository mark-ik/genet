/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! One-shot programmatic focus for retained view transitions.

use genet_scripted_dom::NodeId;
use meristem::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};

use crate::{ElementView, GenetCtx, GenetElement};

const FOCUS_REQUEST_ID: ViewId = ViewId::new(0x464F_4352);

/// Transparent wrapper returned by [`request_focus`].
pub struct FocusRequest<V> {
    child: V,
    requested: bool,
}

/// Retained edge state for a [`FocusRequest`].
pub struct FocusRequestState<S> {
    child_state: S,
    node: NodeId,
    requested: bool,
}

/// Request focus on `child` when `requested` changes from false to true.
///
/// A node replacement while the request remains active also focuses the new
/// node. Holding `requested` at true does not steal focus on later rebuilds.
pub fn request_focus<V>(child: V, requested: bool) -> FocusRequest<V> {
    FocusRequest { child, requested }
}

impl<V> ViewMarker for FocusRequest<V> {}

impl<V, State, Action> View<State, Action, GenetCtx> for FocusRequest<V>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
{
    type ViewState = FocusRequestState<V::ViewState>;
    type Element = GenetElement;

    fn build(&self, ctx: &mut GenetCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        ctx.with_id(FOCUS_REQUEST_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            if self.requested {
                ctx.request_focus(element.node);
            }
            let node = element.node;
            (
                element,
                FocusRequestState {
                    child_state,
                    node,
                    requested: self.requested,
                },
            )
        })
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut GenetCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        ctx.with_id(FOCUS_REQUEST_ID, |ctx| {
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            let node = *element.node;
            if self.requested && (!view_state.requested || node != view_state.node) {
                ctx.request_focus(node);
            }
            view_state.node = node;
            view_state.requested = self.requested;
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut GenetCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(FOCUS_REQUEST_ID, |ctx| {
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
        let Some(first) = message.take_first() else {
            return MessageResult::Stale;
        };
        if first != FOCUS_REQUEST_ID {
            return MessageResult::Stale;
        }
        self.child
            .message(&mut view_state.child_state, message, element, app_state)
    }
}

impl<V, State, Action> ElementView<State, Action> for FocusRequest<V>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
{
}
