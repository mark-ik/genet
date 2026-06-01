/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The native key event view: [`on_key`]`(child, handler)`, plus the
//! serval-native [`KeyEvent`] it carries.
//!
//! Stage 3b of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`: the
//! keyboard/focus *foundation*. It mirrors [`OnClick`](crate::OnClick) (Stage
//! 2b) exactly, swapping the click payload for a [`KeyEvent`] and the click
//! registry for the parallel key registry on [`ServalCtx`]. A later slice maps
//! winit key events to [`KeyEvent`] and adds form controls on top; this layer
//! stays headless (no winit, no bin).
//!
//! As with [`on_click`](crate::on_click): there is no browser, so no
//! `addEventListener`. On build, the view records its routing path in
//! [`ServalCtx`]'s *key* registry, keyed by the DOM node it wraps. The runner
//! ([`dispatch_key`]) is the dispatch engine: it routes a [`KeyEvent`] from the
//! currently *focused* node up its ancestor chain through the same
//! `id_path`-routed `View::message` cycle the click path uses. A node is
//! "focusable" precisely because it carries a key handler (it is in the key
//! registry); [`dispatch_click`] sets focus to the nearest such ancestor of the
//! click target.
//!
//! The message-routing shape is identical to [`OnClick::message`]: the captured
//! `view_path()` ends in [`ON_KEY_ID`], so `message` does `take_first()` (==
//! [`ON_KEY_ID`]), then — if the remaining path is empty — `take_message`s the
//! [`KeyEvent`] and calls the handler; otherwise it forwards to the child.
//!
//! [`dispatch_key`]: crate::ServalAppRunner::dispatch_key
//! [`dispatch_click`]: crate::ServalAppRunner::dispatch_click
//! [`OnClick::message`]: crate::OnClick

use core::marker::PhantomData;

use serval_scripted_dom::NodeId;
use xilem_core::{
    MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker,
};

use crate::pod::ServalElement;
use crate::{ElementView, OptionalAction, ServalCtx};

// A distinctive number, mirroring [`OnClick`](crate::OnClick)'s `ON_CLICK_ID`,
// so a stray message routed here on a wrong path is caught rather than silently
// matching. This is a randomly generated 32-bit number — 2025976435 in decimal.
const ON_KEY_ID: ViewId = ViewId::new(0x78C7_6F33);

/// A keyboard key, decoupled from winit.
///
/// The bin maps a winit `Key` to this later; the headless backend only needs the
/// two cases text editing requires: a typed [`Character`](Key::Character) (the
/// resolved string the key produced, e.g. `"h"`, `"H"`, `"é"`) and a
/// [`Named`](Key::Named) non-character key.
#[derive(Clone, Debug)]
pub enum Key {
    /// A key that produced text: the resolved character string.
    Character(String),
    /// A named, non-text key (editing/navigation control).
    Named(NamedKey),
}

/// The named (non-character) keys the editing foundation needs.
///
/// Deliberately minimal but real for text editing; [`Other`](NamedKey::Other) is
/// the catch-all for any named key the winit mapping does not yet special-case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey {
    /// Delete the character before the cursor.
    Backspace,
    /// Commit / newline.
    Enter,
    /// Focus advance / tab character.
    Tab,
    /// Cancel / dismiss.
    Escape,
    /// The space bar (a named key here rather than `Character(" ")` so callers
    /// can treat it uniformly with the other navigation/editing keys).
    Space,
    /// Move the cursor left.
    ArrowLeft,
    /// Move the cursor right.
    ArrowRight,
    /// Move the cursor up.
    ArrowUp,
    /// Move the cursor down.
    ArrowDown,
    /// Delete the character after the cursor.
    Delete,
    /// Move the cursor to the start of the line.
    Home,
    /// Move the cursor to the end of the line.
    End,
    /// Any other named key not yet special-cased.
    Other,
}

/// Active keyboard modifiers at the time of a [`KeyEvent`].
///
/// Enough for chrome shortcuts and focus traversal (`Shift+Tab`). The host maps
/// its platform modifier state into this; handlers and the runner read it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The platform "command" key (⌘ on macOS, Super/Win elsewhere).
    pub meta: bool,
}

/// A native keyboard event payload.
///
/// [`Clone`] because one dispatch may fire it to multiple listeners up the
/// ancestor chain (bubble phase), and [`Debug`] so it satisfies
/// [`AnyDebug`](xilem_core::anymore::AnyDebug) as a
/// [`DynMessage`](xilem_core::DynMessage) body.
#[derive(Clone, Debug)]
pub struct KeyEvent {
    /// The key this event is for.
    pub key: Key,
    /// Modifiers held when the key was pressed.
    pub mods: Modifiers,
    /// Shared cancellation state (`stopPropagation` / `preventDefault`); clones
    /// share one cell. The native twin of the JS `Event`'s `__stop`/`__canceled`.
    /// See [`Propagation`](crate::Propagation).
    pub prop: crate::Propagation,
}

impl KeyEvent {
    /// A key event with no modifiers.
    pub fn new(key: Key) -> Self {
        Self { key, mods: Modifiers::default(), prop: crate::Propagation::new() }
    }

    /// A key event with explicit modifiers.
    pub fn with_mods(key: Key, mods: Modifiers) -> Self {
        Self { key, mods, prop: crate::Propagation::new() }
    }

    /// Stop the event reaching later nodes
    /// ([`Propagation::stop_propagation`](crate::Propagation::stop_propagation)).
    pub fn stop_propagation(&self) {
        self.prop.stop_propagation();
    }

    /// Stop the event reaching any later listener
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

/// Wraps a [`View`] `V` and registers a native key handler on its element,
/// marking the element focusable.
///
/// Construct with [`on_key`]. The wrapped child must produce a [`ServalElement`]
/// (so the view has a DOM node to key the registry on). The handler return type
/// is an [`OptionalAction`] (`OA`), exactly as [`OnClick`](crate::OnClick): a
/// unit handler is a [`MessageResult::Nop`], an action handler bubbles a
/// [`MessageResult::Action`] that composes up through
/// [`map_action`](xilem_core::map_action).
///
/// The propagation phase is the `capture` field, exactly as
/// [`OnClick`](crate::OnClick): `false` (default) = bubble (`focus → root`),
/// `true` (via [`OnKey::capture`]) = capture (`root → focus`). Registering a key
/// handler marks the element focusable in *either* phase.
pub struct OnKey<V, State, Action, F> {
    child: V,
    handler: F,
    /// The propagation phase: `true` = capture, `false` = bubble (default).
    capture: bool,
    phantom: PhantomData<fn() -> (State, Action)>,
}

impl<V, State, Action, F> OnKey<V, State, Action, F> {
    /// Set whether this listener fires in the **capture** phase (`root → focus`)
    /// instead of the default **bubble** phase (`focus → root`). Default
    /// `false`, mirroring [`OnClick::capture`](crate::OnClick::capture) and the
    /// browser.
    ///
    /// A capture key listener on an ancestor fires *before* a bubble listener on
    /// the focused node (or a descendant). A listener fires in exactly one
    /// phase, so switching this never double-fires the same handler.
    /// Focusability is unaffected: the element is focusable in either phase.
    pub fn capture(mut self, value: bool) -> Self {
        self.capture = value;
        self
    }
}

/// Attach a native key handler to `child`, making `child`'s element focusable.
///
/// `handler` runs when [`dispatch_key`](crate::ServalAppRunner::dispatch_key)
/// routes a [`KeyEvent`] to this view — i.e. when this view's node is the focus
/// (or an ancestor of the focus, via the bubble walk). It mutates the app state
/// and may return an action (anything implementing [`OptionalAction<Action>`] —
/// `()`, an `Action`, or `Option<Action>`); a returned action becomes a
/// [`MessageResult::Action`]. The runner rebuilds the view tree afterwards so
/// any state change reaches the DOM.
///
/// Registering a key handler is also what makes `child`'s node *focusable*
/// (in either phase): [`ServalCtx::is_focusable`](crate::ServalCtx::is_focusable)
/// returns `true` for it, and a click on it (or a descendant) focuses it.
pub fn on_key<V, State, Action, OA, F>(child: V, handler: F) -> OnKey<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, KeyEvent) -> OA + 'static,
{
    OnKey {
        child,
        handler,
        // Default to the bubble phase, matching the browser and `OnClick`.
        capture: false,
        phantom: PhantomData,
    }
}

/// Retained state for an [`OnKey`].
pub struct OnKeyState<S> {
    child_state: S,
    /// The wrapped child's DOM node, so teardown can unregister and rebuild can
    /// detect a node swap and re-key the registry.
    node: NodeId,
}

impl<V, State, Action, F> ViewMarker for OnKey<V, State, Action, F> {}

impl<V, State, Action, OA, F> View<State, Action, ServalCtx> for OnKey<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, KeyEvent) -> OA + 'static,
{
    type ViewState = OnKeyState<V::ViewState>;

    type Element = ServalElement;

    fn build(&self, ctx: &mut ServalCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        // Push our own id so the captured `view_path()` (and the message path the
        // runner routes) ends in `ON_KEY_ID` — mirrors `OnClick::build`.
        ctx.with_id(ON_KEY_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let node = element.node;
            // The routing path *to this handler*: it ends in `ON_KEY_ID`. The
            // phase (`self.capture`) is stored alongside it so dispatch routes
            // this listener in the matching pass.
            let path = ctx.view_path().to_vec();
            ctx.register_key(node, path, self.capture);
            (element, OnKeyState { child_state, node })
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
        ctx.with_id(ON_KEY_ID, |ctx| {
            let prev_node = view_state.node;
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            // The child may have swapped its node; if so, move the registry entry
            // to the new node. The captured path is structural (not
            // node-dependent), so re-registering the same node is a harmless
            // no-op we skip — exactly as `OnClick::rebuild`.
            let node = *element.node;
            if node != prev_node {
                ctx.unregister_key(prev_node);
                let path = ctx.view_path().to_vec();
                ctx.register_key(node, path, self.capture);
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
        ctx.with_id(ON_KEY_ID, |ctx| {
            ctx.unregister_key(view_state.node);
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
        // Identical shape to `OnClick::message`: consume our own id, then either
        // handle (path exhausted) or forward to the child.
        let Some(first) = message.take_first() else {
            // A parent routed an empty/short path here: stale, not ours.
            return MessageResult::Stale;
        };
        if first != ON_KEY_ID {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            match message.take_message::<KeyEvent>() {
                // The handler runs and may yield an action; `OptionalAction`
                // collapses `()`/`A`/`Option<A>` to `Option<A>` — `Some(a)`
                // bubbles as `MessageResult::Action(a)`, `None` (incl. every unit
                // handler) is a `Nop`.
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

// `OnKey` passes its child's element through, so a key-wrapped element is itself
// an `ElementView` (the twin of `OnClick`'s impl), letting handlers compose.
impl<V, State, Action, OA, F> ElementView<State, Action> for OnKey<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, KeyEvent) -> OA + 'static,
{
}
