/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The native click event view: [`on_click`]`(child, handler)`.
//!
//! Stage 2b of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`. This is
//! `xilem_web`'s `OnEvent` view (in `crates/xilem/xilem_web/src/events.rs`),
//! collapsed to *click only* and adapted to serval: there is no browser, so
//! there is no `addEventListener`. Instead, on build, the view records its
//! routing path in [`ServalCtx`]'s click registry, keyed by the DOM node it
//! wraps. The serval-native runner ([`dispatch_click`]) is the dispatch
//! engine: it hit-tests a pointer event to a node, walks that node's ancestor
//! chain, and routes a [`PointerClick`] message down each registered path —
//! the same `id_path`-routed `View::message` cycle the browser path uses,
//! just driven by serval rather than `web_sys`.
//!
//! The message-routing shape is identical to `OnEvent::message`: the captured
//! `view_path()` ends in [`ON_CLICK_ID`], so `message` does `take_first()` (==
//! `ON_CLICK_ID`), then — if the remaining path is empty — `take_message`s the
//! [`PointerClick`] and calls the handler; otherwise it forwards to the child.
//!
//! [`dispatch_click`]: crate::ServalAppRunner::dispatch_click

use core::marker::PhantomData;

use serval_scripted_dom::NodeId;
use xilem_core::{
    MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker,
};

use crate::pod::ServalElement;
use crate::{El, ElementView, OptionalAction, ServalCtx};

// A distinctive number, mirroring `OnEvent`'s `ON_EVENT_VIEW_ID`, so a stray
// message routed here on a wrong path is caught rather than silently matching.
// This is a randomly generated 32-bit number — 1430470739 in decimal.
const ON_CLICK_ID: ViewId = ViewId::new(0x5546_2453);

/// A native pointer-click event payload.
///
/// Carries the element-local hit point. It is [`Clone`] because one dispatch may
/// fire it to multiple listeners up the ancestor chain (bubble phase), and
/// [`Debug`] so it satisfies [`AnyDebug`](xilem_core::anymore::AnyDebug) as a
/// [`DynMessage`](xilem_core::DynMessage) body.
#[derive(Clone, Debug)]
pub struct PointerClick {
    /// The hit point in the target element's local coordinate space.
    pub local: (f32, f32),
    /// Shared cancellation state (`stopPropagation` / `preventDefault`).
    /// Clones share one cell, so a handler's call is seen by the dispatch loop
    /// and the host — the native twin of the JS `Event`'s `__stop`/`__canceled`.
    /// See [`Propagation`](crate::Propagation).
    pub prop: crate::Propagation,
}

impl PointerClick {
    /// A click at `local` (element-local coords) with fresh propagation state.
    pub fn at(local: (f32, f32)) -> Self {
        Self { local, prop: crate::Propagation::new() }
    }

    /// Stop the event reaching later nodes in the propagation path
    /// ([`Propagation::stop_propagation`](crate::Propagation::stop_propagation)).
    pub fn stop_propagation(&self) {
        self.prop.stop_propagation();
    }

    /// Stop the event reaching any later listener, incl. the current node's
    /// ([`Propagation::stop_immediate_propagation`](crate::Propagation::stop_immediate_propagation)).
    pub fn stop_immediate_propagation(&self) {
        self.prop.stop_immediate_propagation();
    }

    /// Cancel the default action
    /// ([`Propagation::prevent_default`](crate::Propagation::prevent_default)).
    pub fn prevent_default(&self) {
        self.prop.prevent_default();
    }
}

/// Wraps a [`View`] `V` and registers a native click handler on its element.
///
/// Construct with [`on_click`]. The wrapped child must produce a
/// [`ServalElement`] (so the view has a DOM node to key the registry on).
///
/// Stage 3a's handler returns an [`OptionalAction`] (`OA`): it may mutate app
/// state and *also* bubble an `Action`. The two ends of that polymorphism are
///   * a **unit** handler (`Fn(&mut State, PointerClick)`, `OA = ()`), the
///     Stage 2b shape — `action()` is `None`, so `message` returns
///     [`MessageResult::Nop`] exactly as before; and
///   * an **action** handler (`Fn(&mut State, PointerClick) -> A`, `OA = A`) —
///     `action()` is `Some(a)`, so `message` returns
///     [`MessageResult::Action(a)`], which composes up through
///     [`map_action`](xilem_core::map_action), as `OnEvent` does.
///
/// `OA` is not a struct field; it is introduced by the `View` impl (mirroring
/// `xilem_web`'s `OnEvent`), so the wrapper type stays `OnClick<V, State,
/// Action, F>`.
///
/// The propagation phase is the `capture` field: `false` (the default, matching
/// the browser and `xilem_web`'s `OnEvent::capture`) means the listener fires in
/// the bubble pass (`target → root`); `true`, set via [`OnClick::capture`],
/// means it fires in the capture pass (`root → target`). A node's single click
/// listener fires in whichever one phase it registered, never both.
pub struct OnClick<V, State, Action, F> {
    child: V,
    handler: F,
    /// The propagation phase: `true` = capture, `false` = bubble (default).
    capture: bool,
    phantom: PhantomData<fn() -> (State, Action)>,
}

impl<V, State, Action, F> OnClick<V, State, Action, F> {
    /// Set whether this listener fires in the **capture** phase
    /// (`root → target`) instead of the default **bubble** phase
    /// (`target → root`). Default `false`, mirroring the browser and
    /// `xilem_web`'s [`OnEvent::capture`](https://docs.rs/xilem_web).
    ///
    /// A capture listener on an ancestor fires *before* a bubble listener on a
    /// descendant (or the target). A listener fires in exactly one phase, so
    /// switching this never double-fires the same handler.
    pub fn capture(mut self, value: bool) -> Self {
        self.capture = value;
        self
    }
}

impl<Seq, State, Action, F> OnClick<El<Seq, State, Action>, State, Action, F> {
    /// Set an attribute on the wrapped element, forwarding to [`El::attr`].
    ///
    /// This is what lets the ergonomic [`button`] (and any `on_click(el(..), h)`)
    /// carry a `class` (or any attribute) *after* wrapping, since `.attr` is
    /// otherwise inherent to [`El`] and hidden once the element is wrapped in an
    /// `OnClick`. Repeated names overwrite, matching `El::attr`.
    pub fn attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.child = self.child.attr(name, value);
        self
    }
}

/// Attach a native click handler to `child`.
///
/// `handler` runs when [`dispatch_click`](crate::ServalAppRunner::dispatch_click)
/// routes a [`PointerClick`] to this view (directly on `child`'s node, or via
/// the bubble walk from a descendant). It mutates the app state and may return
/// an action (anything implementing [`OptionalAction<Action>`] — `()`, an
/// `Action`, or `Option<Action>`); a returned action becomes a
/// [`MessageResult::Action`]. The runner rebuilds the view tree afterwards so
/// any state change reaches the DOM.
pub fn on_click<V, State, Action, OA, F>(
    child: V,
    handler: F,
) -> OnClick<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    // `ElementView` (not just `Element = ServalElement`): a click target must be
    // an element, so a text view is rejected at compile time.
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerClick) -> OA + 'static,
{
    OnClick {
        child,
        handler,
        // Default to the bubble phase, matching the browser and `xilem_web`.
        // `.capture(true)` opts into the capture phase.
        capture: false,
        phantom: PhantomData,
    }
}

/// Retained state for an [`OnClick`].
pub struct OnClickState<S> {
    child_state: S,
    /// The wrapped child's DOM node, so teardown can unregister and rebuild can
    /// detect a node swap and re-key the registry.
    node: NodeId,
}

impl<V, State, Action, F> ViewMarker for OnClick<V, State, Action, F> {}

impl<V, State, Action, OA, F> View<State, Action, ServalCtx> for OnClick<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerClick) -> OA + 'static,
{
    type ViewState = OnClickState<V::ViewState>;

    type Element = ServalElement;

    fn build(&self, ctx: &mut ServalCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        // Push our own id so the captured `view_path()` (and the message path
        // the runner routes) ends in `ON_CLICK_ID` — mirrors `OnEvent::build`.
        ctx.with_id(ON_CLICK_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let node = element.node;
            // The routing path *to this handler*: it ends in `ON_CLICK_ID`. The
            // phase (`self.capture`) is stored alongside it so dispatch routes
            // this listener in the matching pass.
            let path = ctx.view_path().to_vec();
            ctx.register_click(node, path, self.capture);
            (element, OnClickState { child_state, node })
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
        ctx.with_id(ON_CLICK_ID, |ctx| {
            let prev_node = view_state.node;
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            // The child may have swapped its node (analogous to `OnEvent`
            // re-creating its listener when the element `was_created`). If so,
            // move the registry entry to the new node. The captured path is
            // unchanged (the view path is structural, not node-dependent), so a
            // re-register on the same node is a harmless no-op we skip.
            let node = *element.node;
            if node != prev_node {
                // The captured path is the same for both nodes (it is structural,
                // not node-dependent), so it both identifies the old node's entry
                // to drop and keys the new one.
                let path = ctx.view_path().to_vec();
                ctx.unregister_click(prev_node, &path);
                ctx.register_click(node, path, self.capture);
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
        ctx.with_id(ON_CLICK_ID, |ctx| {
            let path = ctx.view_path().to_vec();
            ctx.unregister_click(view_state.node, &path);
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
        // Identical shape to `OnEvent::message`: consume our own id, then either
        // handle (path exhausted) or forward to the child.
        let Some(first) = message.take_first() else {
            // A parent routed an empty/short path here: stale, not ours.
            return MessageResult::Stale;
        };
        if first != ON_CLICK_ID {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            match message.take_message::<PointerClick>() {
                // The handler runs and may yield an action; `OptionalAction`
                // collapses `()`/`A`/`Option<A>` to `Option<A>` — `Some(a)`
                // bubbles as `MessageResult::Action(a)`, `None` (incl. every
                // unit handler) is a `Nop`, preserving Stage 2b behaviour.
                Some(event) => match (self.handler)(app_state, *event).action() {
                    Some(a) => MessageResult::Action(a),
                    None => MessageResult::Nop,
                },
                // Wrong message type routed to this path: be robust.
                None => MessageResult::Stale,
            }
        } else {
            self.child
                .message(&mut view_state.child_state, message, element, app_state)
        }
    }
}

// `OnClick` passes its child's element through (`Element = ServalElement`), so a
// click-wrapped element is itself an `ElementView` — letting handlers compose,
// e.g. `on_key(on_click(el(..), ..), ..)` for an element with both.
impl<V, State, Action, OA, F> ElementView<State, Action> for OnClick<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerClick) -> OA + 'static,
{
}
