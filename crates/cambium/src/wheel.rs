/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The native wheel/scroll event view: [`on_wheel`]`(child, handler)`.
//!
//! The scroll foundation under scrollable content (and, later, the orrery's
//! wheel-pan / Ctrl-zoom). It is the [`on_pointer`](crate::on_pointer) pattern
//! for a discrete scroll notch: the view records its routing path in
//! [`ServalCtx`]'s wheel registry keyed by its DOM node, and the runner's
//! [`dispatch_wheel`](crate::ServalAppRunner::dispatch_wheel) routes a
//! [`WheelEvent`] down that path.
//!
//! Unlike pointer there is no **capture**: a wheel notch is one-shot, so each
//! event resolves its own target by the ancestor walk (the innermost
//! scroll-handling element under the cursor). A single registered handler per
//! node receives the event (no capture/bubble split — the wheel routes straight
//! to the resolved target).

use core::marker::PhantomData;

use meristem::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};
use serval_scripted_dom::NodeId;

use crate::pod::ServalElement;
use crate::{ElementView, OptionalAction, Propagation, ServalCtx};

// Distinctive marker id (randomly generated) so a stray message on a wrong path
// is caught rather than silently matching. 0x5748_4C45 == "WHLE".
const ON_WHEEL_ID: ViewId = ViewId::new(0x5748_4C45);

/// A native wheel/scroll event payload.
///
/// `delta` is the scroll amount in device px (`x`, `y`); a positive `y` is the
/// content moving down (a scroll *toward the top*), matching the host's wheel
/// convention. `local` is the cursor position in the handling element's local
/// coordinate space (its top-left is `(0, 0)`) and `size` is that element's box
/// size, so a handler can scroll-to-cursor (e.g. zoom anchored under the
/// pointer) without knowing layout. The host computes `local` / `size` from the
/// hit element's laid-out rect (the headless view layer has no layout).
#[derive(Clone, Debug)]
pub struct WheelEvent {
    pub delta: (f32, f32),
    pub local: (f32, f32),
    pub size: (f32, f32),
    /// Clone-through cancellation state. A wheel handler calls
    /// `prevent_default` to tell the host not to run its own scroll default for
    /// this notch.
    pub prop: Propagation,
}

impl WheelEvent {
    pub fn new(delta: (f32, f32), local: (f32, f32), size: (f32, f32)) -> Self {
        Self {
            delta,
            local,
            size,
            prop: Propagation::new(),
        }
    }

    pub fn prevent_default(&self) {
        self.prop.prevent_default();
    }
}

/// Wraps a [`View`] and registers a native wheel handler on its element.
/// Construct with [`on_wheel`].
pub struct OnWheel<V, State, Action, F> {
    child: V,
    handler: F,
    phantom: PhantomData<fn() -> (State, Action)>,
}

/// Attach a wheel handler to `child`. `handler` runs for each [`WheelEvent`]
/// the runner routes to this element (the nearest wheel-handling ancestor of the
/// hit node). It mutates app state and may return an [`OptionalAction`]; the
/// runner rebuilds afterward.
pub fn on_wheel<V, State, Action, OA, F>(child: V, handler: F) -> OnWheel<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, WheelEvent) -> OA + 'static,
{
    OnWheel {
        child,
        handler,
        phantom: PhantomData,
    }
}

/// Retained state for an [`OnWheel`].
pub struct OnWheelState<S> {
    child_state: S,
    node: NodeId,
    /// The routing path this handler registered under — rebuild reconciles the
    /// `(node, path)` pair (an adoption changes the path without recreating the
    /// element; moveBefore plan S5).
    path: Vec<ViewId>,
}

impl<V, State, Action, F> ViewMarker for OnWheel<V, State, Action, F> {}

impl<V, State, Action, OA, F> View<State, Action, ServalCtx> for OnWheel<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, WheelEvent) -> OA + 'static,
{
    type ViewState = OnWheelState<V::ViewState>;
    type Element = ServalElement;

    fn build(
        &self,
        ctx: &mut ServalCtx,
        app_state: &mut State,
    ) -> (Self::Element, Self::ViewState) {
        ctx.with_id(ON_WHEEL_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let node = element.node;
            let path = ctx.view_path().to_vec();
            ctx.register_wheel(node, path.clone());
            (
                element,
                OnWheelState {
                    child_state,
                    node,
                    path,
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
        ctx.with_id(ON_WHEEL_ID, |ctx| {
            let prev_node = view_state.node;
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            // Reconcile the `(node, path)` pair — the path changes when this
            // subtree was adopted into a different position. (moveBefore S5.)
            let node = *element.node;
            if node != prev_node || ctx.view_path() != view_state.path.as_slice() {
                ctx.unregister_wheel(prev_node);
                let path = ctx.view_path().to_vec();
                ctx.register_wheel(node, path.clone());
                view_state.node = node;
                view_state.path = path;
            }
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ServalCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(ON_WHEEL_ID, |ctx| {
            ctx.unregister_wheel(view_state.node);
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
        if first != ON_WHEEL_ID {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            match message.take_message::<WheelEvent>() {
                Some(event) => match (self.handler)(app_state, *event).action() {
                    Some(a) => MessageResult::Action(a),
                    None => MessageResult::Nop,
                },
                None => MessageResult::Stale,
            }
        } else {
            self.child
                .message(&mut view_state.child_state, message, element, app_state)
        }
    }
}

// Passes the child's element through, so a wheel-wrapped element is itself an
// `ElementView` and composes with on_click / on_key / on_pointer.
impl<V, State, Action, OA, F> ElementView<State, Action> for OnWheel<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, WheelEvent) -> OA + 'static,
{
}
