/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The serval-native runner that owns app state and the retained view tree.
//!
//! Stage 1b of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`. The plan
//! names the runner the real artifact: `xilem_core` provides view diffing and
//! (eventually) message thunks, but it *schedules nothing*. Something
//! serval-native has to own state, the root node, and the rebuild cadence —
//! the thing that turns "state changed" into "diff → DOM mutations" (and, in a
//! host, "→ relayout → netrender → present").
//!
//! [`ServalAppRunner`] is that owner, kept deliberately thin: it depends only
//! on `xilem_core` and this backend, never on serval-layout / netrender. A host
//! crate (`pelt-live`) drives the render side over the
//! [`ScriptedDom`](serval_scripted_dom::ScriptedDom) the runner mutates.
//!
//! Stage 2b adds native event dispatch: [`ServalAppRunner::dispatch_click`]
//! takes a hit [`NodeId`] (the `point → NodeId` half is the `pelt-live` host's
//! `hit_test_node`), walks the node's ancestor chain, and routes a
//! [`PointerClick`] down each registered click handler's path via the faithful
//! `xilem_core` message cycle, then rebuilds so handler state changes reach the
//! DOM. The timer tick that drives [`update`](ServalAppRunner::update) and the
//! window event that drives dispatch are the host's concern.

use core::marker::PhantomData;

use layout_dom_api::{LayoutDom, LayoutDomMut};
use serval_scripted_dom::{NodeId, ScriptedDom};
use xilem_core::{DynMessage, MessageCtx, MessageResult, View, ViewId};

use crate::{
    DomHandle, Key, KeyEvent, NamedKey, PointerClick, PointerEvent, ServalCtx, ServalElement,
    ServalElementMut, WheelEvent,
};

/// Owns the app state, the view-producing logic, and the retained view tree,
/// rebuilding the [`ScriptedDom`] whenever the state changes.
///
/// `Logic: FnMut(&State) -> V` is the app-logic closure (Xilem's `app_logic`,
/// minus the `&mut State` — Stage 1b has no message handlers mutating state
/// from inside a view, so the logic only *reads* state). `V` is the root view;
/// its element is always the uniform [`ServalElement`].
///
/// `Action` is the root view's action type. Stage 2b used `()` exclusively (a
/// handler mutates state and the runner rebuilds). Stage 3a generalizes it so an
/// action can *reach the root*: when a click handler returns an action that no
/// parent [`map_action`](xilem_core::map_action) consumes, it surfaces as a
/// [`MessageResult::Action`] which [`dispatch_click`](Self::dispatch_click)
/// collects and returns. The common `Action = ()` case is the no-op it was — a
/// `()` action carries nothing to observe.
pub struct ServalAppRunner<State, Logic, V, Action = ()>
where
    State: 'static,
    Action: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, Action, ServalCtx, Element = ServalElement>,
{
    dom: DomHandle,
    ctx: ServalCtx,
    state: State,
    logic: Logic,
    view: V,
    view_state: V::ViewState,
    /// The retained root element produced by the current `view`. Its `node`
    /// stays attached under the document root for the runner's lifetime.
    root: ServalElement,
    /// The currently focused node, if any. A click sets this to the nearest
    /// focusable (key-handler-bearing) ancestor of the click target, or clears
    /// it to `None` when the click lands outside any focusable element.
    /// [`dispatch_key`](Self::dispatch_key) walks from here. Stage 3b.
    focus: Option<NodeId>,
    /// The element capturing the current pointer drag, if any. Set by
    /// [`dispatch_pointer_down`](Self::dispatch_pointer_down) to the nearest
    /// pointer-handler ancestor of the press, consulted by
    /// [`dispatch_pointer_move`](Self::dispatch_pointer_move) to route moves, and
    /// cleared by [`dispatch_pointer_up`](Self::dispatch_pointer_up). So a drag
    /// keeps reaching the element it started on even if the cursor leaves it.
    pointer_capture: Option<NodeId>,
    /// Whether the most recent [`dispatch_click`](Self::dispatch_click) /
    /// [`dispatch_key`](Self::dispatch_key) had its default action prevented (a
    /// handler called `prevent_default` on the shared [`Propagation`](crate::Propagation)
    /// cell). Read via [`default_prevented`](Self::default_prevented) after
    /// dispatch to gate the host's own default action (navigate, caret move,
    /// form activation) — the host-consumption end of the converged
    /// native+JS cancellation contract.
    last_default_prevented: bool,
    phantom: PhantomData<fn() -> Action>,
}

impl<State, Logic, V, Action> ServalAppRunner<State, Logic, V, Action>
where
    State: 'static,
    Action: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, Action, ServalCtx, Element = ServalElement>,
{
    /// Build the initial tree from `state` and attach its root under the
    /// document root.
    ///
    /// Mirrors `xilem-serval/src/tests.rs`'s `Harness::build`: run the logic to
    /// get a view, `View::build` it into the `ScriptedDom`, then
    /// `insert_before(document, root, None)` to append it under the document.
    pub fn new(dom: DomHandle, mut logic: Logic, mut state: State) -> Self {
        let mut ctx = ServalCtx::new(dom.clone());
        let view = logic(&state);
        let (root, view_state) = view.build(&mut ctx, &mut state);

        // Attach the produced root under the document root (append).
        let doc_root = dom.borrow().document();
        dom.borrow_mut().insert_before(doc_root, root.node, None);

        Self {
            dom,
            ctx,
            state,
            logic,
            view,
            view_state,
            root,
            focus: None,
            pointer_capture: None,
            last_default_prevented: false,
            phantom: PhantomData,
        }
    }

    /// Apply a state update, then rebuild the view tree against the new state.
    ///
    /// `f` is the externally-driven "message": it mutates the owned state. The
    /// runner then reruns the logic and diffs the produced view against the
    /// retained one (via [`rebuild`](Self::rebuild)), emitting `DomMutation`s
    /// into the `ScriptedDom`.
    pub fn update(&mut self, f: impl FnOnce(&mut State)) {
        f(&mut self.state);
        self.rebuild();
    }

    /// Re-run the logic against the current state and diff the produced view
    /// into the retained one (the shared rebuild tail of [`update`](Self::update)
    /// and [`dispatch_click`](Self::dispatch_click)).
    ///
    /// The disjoint-field borrows matter: `rebuild` needs `&prev_view`,
    /// `&mut view_state`, `&mut ctx`, the root `Mut`, and `&mut state` all at
    /// once, so the fields are destructured into separate `&mut`s. The root
    /// `ServalElementMut` is constructed exactly as `Harness::rebuild` does —
    /// borrowing `root.node` so a view *could* swap the root node, and cloning
    /// the shared `dom` handle.
    fn rebuild(&mut self) {
        let next = (self.logic)(&self.state);

        // Disjoint borrows: each field separately so `rebuild`'s argument set
        // does not alias `self`.
        let Self {
            ctx,
            state,
            view,
            view_state,
            root,
            dom,
            ..
        } = self;

        let mut_ref = ServalElementMut {
            node: &mut root.node,
            dom: dom.clone(),
            // The root is attached under the document, so a type-changing
            // `AnyView` root can swap its node there via `replace_inner`.
            parent: Some(dom.borrow().document()),
        };
        next.rebuild(view, view_state, ctx, mut_ref, state);

        // The freshly diffed view becomes the retained `prev` for the next tick.
        *view = next;

        // Tear down any portable child parked during this rebuild and not
        // claimed within it — its key really left every portable list, so it
        // gets a real teardown and its (still-attached) node is removed.
        // (moveBefore plan S5, cross-parent.)
        self.ctx.drain_nursery();
    }

    /// Build the routing paths for `chain` in DOM propagation order
    /// (**capture → target → bubble**), the shared core of
    /// [`dispatch_click`](Self::dispatch_click) and
    /// [`dispatch_key`](Self::dispatch_key).
    ///
    /// `chain` is the ancestor list in `target → … → document` order. `lookup`
    /// resolves a node's [`Handler`](crate::context::Handler) for the relevant
    /// event type (click or key). The result interleaves the two phases:
    ///   * **Capture pass** — `chain` *reversed* (`root → target`), keeping only
    ///     handlers whose `capture == true`.
    ///   * **Bubble pass** — `chain` as given (`target → root`), keeping only
    ///     handlers whose `capture == false`.
    ///
    /// Each listener matches exactly one phase, so it contributes one path and
    /// never double-fires; a node carrying several listeners contributes each, in
    /// registration order within the phase. Collected entirely under a shared
    /// `&self` borrow (clones the paths) so the borrow releases before routing.
    fn phase_ordered_paths(
        &self,
        chain: &[NodeId],
        lookup: impl Fn(&ServalCtx, NodeId) -> &[crate::context::Handler],
    ) -> Vec<Vec<ViewId>> {
        let mut paths = Vec::new();
        // Capture pass: root → target (chain reversed), capture listeners only.
        for &node in chain.iter().rev() {
            for handler in lookup(&self.ctx, node) {
                if handler.capture {
                    paths.push(handler.path.clone());
                }
            }
        }
        // Bubble pass: target → root (chain order), bubble listeners only.
        for &node in chain {
            for handler in lookup(&self.ctx, node) {
                if !handler.capture {
                    paths.push(handler.path.clone());
                }
            }
        }
        paths
    }

    /// Dispatch a native pointer click that hit `target`.
    ///
    /// `target` is the node serval's hit-test resolved the pointer to (the
    /// `point → NodeId` half lives in the host's `hit_test_node`). This is the
    /// faithful-routing dispatch: no native handler registry of `Rc<dyn Fn>`,
    /// just the `xilem_core` message cycle the browser path also uses.
    ///
    /// Dispatch runs the full DOM propagation order, **capture → target →
    /// bubble**. The ancestor chain (`target → … → document`) is collected once;
    /// then the event is routed in two passes:
    ///   * **Capture pass** — the chain *reversed* (`root → target` order),
    ///     routing only handlers registered with `capture == true`.
    ///   * **Bubble pass** — the chain in its natural `target → root` order,
    ///     routing only handlers with `capture == false` (the default).
    ///
    /// A node's single click listener is registered in exactly one phase, so it
    /// appears in exactly one pass and never double-fires. A `.capture(true)`
    /// ancestor therefore fires before a default (bubble) descendant/target,
    /// yielding the browser/`xilem_web` ordering.
    ///
    /// Paths are collected **before** any routing so the immutable `ctx`/`dom`
    /// borrows are released before the `&mut self` message + rebuild borrows.
    ///
    /// Returns the [`Action`]s that bubbled all the way to the root — i.e. any
    /// handler action no parent [`map_action`](xilem_core::map_action) absorbed,
    /// surfacing as [`MessageResult::Action`]. With `Action = ()` (the Stage 2b
    /// path) handlers mutate state in place and this is an empty `Vec`, so old
    /// call sites can ignore the return; an action-bubbling app reads it to drive
    /// the next effect. This is the runner's minimal home for `Action` — no
    /// callback sink, just the collected results.
    ///
    /// Stage 3b — **click-to-focus.** After routing + rebuild, focus is set to
    /// the nearest ancestor of `target` (including `target` itself) that carries
    /// a key handler (is focusable, per
    /// [`ServalCtx::is_focusable`](crate::ServalCtx::is_focusable), in either
    /// phase); if none is found, focus is cleared to `None`. So clicking a
    /// focusable element focuses it and clicking elsewhere defocuses. This is
    /// independent of whether any *click* handler fired — a focusable element
    /// need not have an `on_click`.
    pub fn dispatch_click(&mut self, target: NodeId, event: PointerClick) -> Vec<Action> {
        // 1. Collect the ancestor chain (target → … → document) once, and — in
        //    the same walk — find the nearest focusable (key-handler-bearing)
        //    ancestor for click-to-focus. Done under a short-lived shared borrow
        //    of `dom`, fully released before routing.
        let (chain, new_focus): (Vec<NodeId>, Option<NodeId>) = {
            let dom = self.dom.borrow();
            let mut chain = Vec::new();
            let mut focus = None;
            let mut current = Some(target);
            while let Some(node) = current {
                chain.push(node);
                // The first (nearest) focusable ancestor wins; the walk runs
                // target → root, so the first hit is the deepest. Focusability
                // is phase-independent (presence in the key registry).
                if focus.is_none() && self.ctx.is_focusable(node) {
                    focus = Some(node);
                }
                current = dom.parent(node);
            }
            (chain, focus)
        };

        // Click-to-focus: focus the nearest focusable ancestor, or clear it when
        // the click landed outside any focusable element. Independent of whether
        // a click handler fired below.
        self.focus = new_focus;

        // 2. Build the routing paths in propagation order: capture pass first
        //    (chain reversed → root → target, capture==true handlers only), then
        //    bubble pass (chain as-is → target → root, capture==false only). A
        //    node's lone listener is in exactly one phase, so it routes once.
        let paths = self.phase_ordered_paths(&chain, |ctx, node| ctx.click_handlers_at(node));

        if paths.is_empty() {
            // No click handler anywhere on the chain: nothing routed, no rebuild.
            // Focus was still updated above; nothing could prevent the default.
            self.last_default_prevented = event.prop.default_prevented();
            return Vec::new();
        }

        // 3. Route the event down each collected path through the faithful
        //    message cycle. The disjoint-field destructure mirrors `rebuild`,
        //    so `View::message`'s borrow set does not alias `self`. Any action
        //    that reaches the root (a `MessageResult::Action`) is collected.
        // Thread the *real* environment through the message cycle so dispatch and
        // build share one: take it out, hand it to each path's `MessageCtx`, and
        // reclaim it via `finish` for the next path (and to restore below). (G2.2.)
        let mut env = self.ctx.take_environment();
        let mut actions = Vec::new();
        {
            // `ctx` is not needed for routing: the recorded path is the full
            // routing target, so `View::message` walks it without the context.
            let Self {
                state,
                view,
                view_state,
                root,
                dom,
                ..
            } = self;

            for path in paths {
                let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
                let mut_ref = ServalElementMut {
                    node: &mut root.node,
                    dom: dom.clone(),
                    parent: Some(dom.borrow().document()),
                };
                // Handlers may mutate state in place (a rebuild below reflects
                // that) and/or bubble an `Action` up to the root. A root-level
                // `MessageResult::Action(a)` is the runner's `Action` home: we
                // collect it for the caller. `Action = ()` collects nothing
                // meaningful (the Stage 2b path).
                if let MessageResult::Action(a) = view.message(view_state, &mut msg, mut_ref, state)
                {
                    actions.push(a);
                }
                // Reclaim the environment for the next path *before* any break, so
                // it is never left moved-out.
                env = msg.finish().0;
                // stopPropagation / stopImmediatePropagation: a handler that
                // canceled propagation halts the capture/bubble walk here, after
                // its path fired (every event clone shares one Propagation cell).
                // The native twin of dom.rs's per-node `__stop` check.
                if event.prop.stopped() {
                    break;
                }
            }
        }
        self.ctx.set_environment(env);

        // Record whether a handler prevented the default action — the host reads
        // this (default_prevented()) to gate its own default (the cancellation
        // seam's consumption point).
        self.last_default_prevented = event.prop.default_prevented();

        // 4. Rebuild so the handler's state mutation reaches the DOM — the same
        //    tail `update` runs.
        self.rebuild();

        actions
    }

    /// Whether the most recent [`dispatch_click`](Self::dispatch_click) or
    /// [`dispatch_key`](Self::dispatch_key) had its default action prevented by a
    /// handler (`prevent_default` on the event). The host calls this right after
    /// dispatch to decide whether to run its own default action — navigation,
    /// caret movement, form activation, drag start. This is the native side of
    /// the converged cancellation contract (the JS side returns it from
    /// `dispatchEvent`); a handler that does not prevent leaves it `false`.
    pub fn default_prevented(&self) -> bool {
        self.last_default_prevented
    }

    /// The currently focused node, if any.
    ///
    /// Set by [`dispatch_click`](Self::dispatch_click) (click-to-focus) or
    /// [`set_focus`](Self::set_focus); read by [`dispatch_key`](Self::dispatch_key)
    /// as the root of its bubble walk. Stage 3b.
    pub fn focus(&self) -> Option<NodeId> {
        self.focus
    }

    /// Set (or clear, with `None`) the focused node directly.
    ///
    /// The keyboard counterpart of a programmatic `element.focus()`. No
    /// validation that `node` is focusable: a test (or a host) may aim focus at
    /// any node, and [`dispatch_key`](Self::dispatch_key) simply finds no key
    /// handler to route to if it is not focusable. Stage 3b.
    pub fn set_focus(&mut self, node: Option<NodeId>) {
        self.focus = node;
    }

    /// Move focus to the next (`forward`) or previous focusable element in
    /// document order, wrapping. The Tab-traversal default: a document engine has
    /// no built-in tab order, so the runner provides one over the focusable set
    /// (elements carrying a key handler, per [`ServalCtx::is_focusable`]) in DOM
    /// pre-order. With nothing focused, `forward` focuses the first focusable and
    /// backward the last. Rebuilds after (focus may drive `:focus` styling
    /// later). No-op when there are no focusable elements.
    pub fn focus_traverse(&mut self, forward: bool) {
        let focusables: Vec<NodeId> = {
            let dom = self.dom.borrow();
            let mut out = Vec::new();
            collect_focusables(&dom, &self.ctx, dom.document(), &mut out);
            out
        };
        if focusables.is_empty() {
            return;
        }
        let next = match self
            .focus
            .and_then(|f| focusables.iter().position(|&n| n == f))
        {
            Some(i) => {
                let len = focusables.len();
                if forward {
                    (i + 1) % len
                } else {
                    (i + len - 1) % len
                }
            },
            None => {
                if forward {
                    0
                } else {
                    focusables.len() - 1
                }
            },
        };
        self.set_focus(Some(focusables[next]));
        self.rebuild();
    }

    /// Dispatch a native key event to the focused node, then apply the
    /// runner-level keyboard defaults (Tab traversal, Enter/Space activation) the
    /// model leaves to the host.
    ///
    /// With nothing focused this is a near-no-op: only Tab does anything (it enters
    /// the focusable set), since there is no element to route to. With a focused
    /// node it mirrors [`dispatch_click`](Self::dispatch_click) — collect the chain
    /// `focus → … → document`, route the [`KeyEvent`] in **capture → target →
    /// bubble** order through the faithful `MessageCtx`/`View::message` cycle — then
    /// applies the defaults and rebuilds, returning the actions that reached the
    /// root. A key handler on a parent fires when the focused descendant has none
    /// (DOM key bubbling); a capture handler on an ancestor fires first.
    ///
    /// **Keyboard-model escape hatches (G2.3), each overridable by a handler that
    /// calls `prevent_default`:**
    ///   * **Tab / Shift+Tab** is now delivered to the focused element's handlers
    ///     *first* (it used to be swallowed pre-routing), then — unless prevented —
    ///     traverses focus across the focusable set. So a `textarea` can insert a
    ///     tab character (handle Tab and `prevent_default`) or a view impose a
    ///     custom order, while the default stays free tab traversal.
    ///   * **Enter / Space** on a focusable control that carries a click handler but
    ///     no key handler of its own (a plain [`focusable`](crate::focusable)
    ///     button) synthesizes a click — the keyboard equivalent of a pointer
    ///     activation — unless a handler prevented it. A control with its own
    ///     `on_key` owns the key instead (so a text field's Space inserts a space,
    ///     never "clicks").
    ///
    /// As in `dispatch_click`, paths are collected **before** any routing so the
    /// immutable `ctx`/`dom` borrows release before the `&mut self` message +
    /// rebuild borrows.
    pub fn dispatch_key(&mut self, event: KeyEvent) -> Vec<Action> {
        // A fresh pass: the routing tail records the real cancellation value; a
        // pass that routes nothing leaves it false.
        self.last_default_prevented = false;

        // With nothing focused, the only key carrying a runner default is Tab,
        // which enters the focusable set (no element to route to). Everything else
        // no-ops — no focused node means nowhere to send keys.
        let Some(focus) = self.focus else {
            if matches!(event.key, Key::Named(NamedKey::Tab)) {
                self.focus_traverse(!event.mods.shift);
            }
            return Vec::new();
        };

        // 1. Collect the ancestor chain (focus → … → document) once.
        let chain: Vec<NodeId> = {
            let dom = self.dom.borrow();
            let mut chain = Vec::new();
            let mut current = Some(focus);
            while let Some(node) = current {
                chain.push(node);
                current = dom.parent(node);
            }
            chain
        };

        // 2. Route the key to the focused element's handlers in propagation order
        //    (capture then bubble), exactly as `dispatch_click`. Tab is delivered
        //    here too now (no longer swallowed pre-routing), so a view can handle
        //    it and `prevent_default` to override the traversal in step 3. `paths`
        //    is empty for a focusable node with no `on_key` — a plain
        //    `focusable(button(..))`.
        let paths = self.phase_ordered_paths(&chain, |ctx, node| ctx.key_handlers_at(node));
        let routed = !paths.is_empty();
        let mut actions = Vec::new();
        if routed {
            // Thread the real environment through the cycle (G2.2): take it, hand it
            // to each path, reclaim via `finish`, restore after.
            let mut env = self.ctx.take_environment();
            {
                let Self {
                    state,
                    view,
                    view_state,
                    root,
                    dom,
                    ..
                } = self;

                for path in paths {
                    let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
                    let mut_ref = ServalElementMut {
                        node: &mut root.node,
                        dom: dom.clone(),
                        parent: Some(dom.borrow().document()),
                    };
                    if let MessageResult::Action(a) =
                        view.message(view_state, &mut msg, mut_ref, state)
                    {
                        actions.push(a);
                    }
                    // Reclaim the environment for the next path before any break.
                    env = msg.finish().0;
                    // stopPropagation halts the bubble walk (shared Propagation
                    // cell), matching dispatch_click and the JS dispatcher.
                    if event.prop.stopped() {
                        break;
                    }
                }
            }
            self.ctx.set_environment(env);
            // Record whether a handler prevented the default (gates step 3 + host).
            self.last_default_prevented = event.prop.default_prevented();
        }

        // 3. Runner-level key defaults, each suppressed when a handler prevented it
        //    (the escape hatches, G2.3).
        if !event.prop.default_prevented() {
            match event.key {
                // Tab traverses focus across the focusable set, the runner-level
                // default a document engine otherwise lacks.
                Key::Named(NamedKey::Tab) => {
                    self.focus_traverse(!event.mods.shift);
                    return actions; // focus_traverse rebuilt
                },
                // Enter/Space activate a focusable control that has a click handler
                // but no key handler of its own (a plain button) by synthesizing a
                // click at the element-local origin. A control with an `on_key`
                // owns the key, so it is excluded — a text field's Space inserts a
                // space, it does not "click".
                Key::Named(NamedKey::Enter | NamedKey::Space)
                    if !self.ctx.click_handlers_at(focus).is_empty()
                        && self.ctx.key_handlers_at(focus).is_empty() =>
                {
                    actions.extend(self.dispatch_click(focus, PointerClick::at((0.0, 0.0))));
                    // The key was consumed as activation: tell the host not to run
                    // its own default (e.g. Space scrolling the page).
                    self.last_default_prevented = true;
                    return actions; // dispatch_click rebuilt
                },
                _ => {},
            }
        }

        // 4. A handler ran but no runner default fired: rebuild so the handler's
        //    state change reaches the DOM. When nothing routed and no default
        //    fired, nothing changed, so the rebuild is skipped.
        if routed {
            self.rebuild();
        }

        actions
    }

    /// Begin a pointer drag: the press hit `target`. Capture is set to the
    /// nearest ancestor of `target` (including itself) carrying an
    /// [`on_pointer`](crate::on_pointer) handler, and a `Down`
    /// [`PointerEvent`] is routed to it. Subsequent
    /// [`dispatch_pointer_move`](Self::dispatch_pointer_move) /
    /// [`dispatch_pointer_up`](Self::dispatch_pointer_up) go to that captured
    /// element until release. Returns the actions that bubbled to the root.
    ///
    /// `event.local` / `event.size` are the press point + element box in the
    /// captured element's coordinate space; the host computes them from the
    /// laid-out rect (the headless view layer has no layout).
    pub fn dispatch_pointer_down(&mut self, target: NodeId, event: PointerEvent) -> Vec<Action> {
        // A fresh pass: reset the cancellation flag; route_pointer records the
        // real value when a handler runs, and a no-capture press leaves it false.
        self.last_default_prevented = false;
        let captured = {
            let dom = self.dom.borrow();
            let mut current = Some(target);
            let mut found = None;
            while let Some(node) = current {
                if self.ctx.pointer_handler(node).is_some() {
                    found = Some(node);
                    break;
                }
                current = dom.parent(node);
            }
            found
        };
        self.pointer_capture = captured;
        match captured {
            Some(node) => self.route_pointer(node, event),
            None => Vec::new(),
        }
    }

    /// Route a `Move` to the element capturing the drag (if any). No-op when no
    /// drag is active.
    pub fn dispatch_pointer_move(&mut self, event: PointerEvent) -> Vec<Action> {
        self.last_default_prevented = false;
        match self.pointer_capture {
            Some(node) => self.route_pointer(node, event),
            None => Vec::new(),
        }
    }

    /// Route an `Up` to the capturing element and end the drag (clearing
    /// capture). No-op when no drag is active.
    pub fn dispatch_pointer_up(&mut self, event: PointerEvent) -> Vec<Action> {
        self.last_default_prevented = false;
        match self.pointer_capture.take() {
            Some(node) => self.route_pointer(node, event),
            None => Vec::new(),
        }
    }

    /// The element currently capturing a pointer drag, if any. The host reads
    /// this between [`dispatch_pointer_down`](Self::dispatch_pointer_down) and
    /// `up` to know which element's rect to measure for the move's local coords.
    pub fn pointer_capture(&self) -> Option<NodeId> {
        self.pointer_capture
    }

    /// The nearest ancestor of `hit` (including itself) carrying an
    /// [`on_pointer`](crate::on_pointer) handler — the element a press there
    /// would capture, resolved *without* starting a drag. The host calls this on
    /// a press to find the drag element so it can measure that element's rect for
    /// the press's local coords before [`dispatch_pointer_down`](Self::dispatch_pointer_down).
    pub fn pointer_target(&self, hit: NodeId) -> Option<NodeId> {
        let dom = self.dom.borrow();
        let mut current = Some(hit);
        while let Some(node) = current {
            if self.ctx.pointer_handler(node).is_some() {
                return Some(node);
            }
            current = dom.parent(node);
        }
        None
    }

    /// Route a [`PointerEvent`] down `node`'s registered pointer path through the
    /// faithful message cycle, then rebuild. The disjoint-field destructure
    /// mirrors [`dispatch_click`](Self::dispatch_click).
    fn route_pointer(&mut self, node: NodeId, event: PointerEvent) -> Vec<Action> {
        let Some(path) = self.ctx.pointer_handler(node).map(<[ViewId]>::to_vec) else {
            return Vec::new();
        };
        // Thread the *real* environment through the message cycle (build + dispatch
        // share one): take it out, hand it to `MessageCtx`, restore what `finish`
        // returns. (G2.2.)
        let env = self.ctx.take_environment();
        let mut actions = Vec::new();
        let env = {
            let Self {
                state,
                view,
                view_state,
                root,
                dom,
                ..
            } = self;
            // Clone into the message: the handler mutates its clone's shared
            // `Propagation` cell, and the original below reads back what it set.
            let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
            let mut_ref = ServalElementMut {
                node: &mut root.node,
                dom: dom.clone(),
                parent: Some(dom.borrow().document()),
            };
            if let MessageResult::Action(a) = view.message(view_state, &mut msg, mut_ref, state) {
                actions.push(a);
            }
            msg.finish().0
        };
        self.ctx.set_environment(env);
        // Record this pointer pass's own cancellation (the host gates its default
        // drag behavior on it), mirroring dispatch_click / dispatch_key — not the
        // stale value left by an earlier click/key.
        self.last_default_prevented = event.prop.default_prevented();
        self.rebuild();
        actions
    }

    /// Route a wheel/scroll notch to the nearest ancestor of `target` (including
    /// itself) carrying an [`on_wheel`](crate::on_wheel) handler. Unlike a
    /// pointer drag there is no capture: each notch resolves its own target by
    /// the ancestor walk (the scroll routes to the innermost scroll-handling
    /// element under the cursor). Returns the actions that bubbled to the root.
    ///
    /// `event.local` / `event.size` are the cursor point + element box in the
    /// resolved element's coordinate space; the host computes them from the
    /// laid-out rect (the headless view layer has no layout).
    pub fn dispatch_wheel(&mut self, target: NodeId, event: WheelEvent) -> Vec<Action> {
        self.last_default_prevented = false;
        let resolved = {
            let dom = self.dom.borrow();
            let mut current = Some(target);
            let mut found = None;
            while let Some(node) = current {
                if self.ctx.wheel_handler(node).is_some() {
                    found = Some(node);
                    break;
                }
                current = dom.parent(node);
            }
            found
        };
        match resolved {
            Some(node) => self.route_wheel(node, event),
            None => Vec::new(),
        }
    }

    /// The nearest ancestor of `hit` (including itself) carrying an
    /// [`on_wheel`](crate::on_wheel) handler — the element a wheel there would
    /// scroll, resolved *without* dispatching. The host calls this to measure
    /// that element's rect for the event's local coords before
    /// [`dispatch_wheel`](Self::dispatch_wheel).
    pub fn wheel_target(&self, hit: NodeId) -> Option<NodeId> {
        let dom = self.dom.borrow();
        let mut current = Some(hit);
        while let Some(node) = current {
            if self.ctx.wheel_handler(node).is_some() {
                return Some(node);
            }
            current = dom.parent(node);
        }
        None
    }

    /// Route a [`WheelEvent`] down `node`'s registered wheel path through the
    /// faithful message cycle, then rebuild. Mirrors
    /// [`route_pointer`](Self::route_pointer).
    fn route_wheel(&mut self, node: NodeId, event: WheelEvent) -> Vec<Action> {
        let Some(path) = self.ctx.wheel_handler(node).map(<[ViewId]>::to_vec) else {
            return Vec::new();
        };
        // Thread the real environment through the cycle (G2.2), like route_pointer.
        let env = self.ctx.take_environment();
        let mut actions = Vec::new();
        let env = {
            let Self {
                state,
                view,
                view_state,
                root,
                dom,
                ..
            } = self;
            let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
            let mut_ref = ServalElementMut {
                node: &mut root.node,
                dom: dom.clone(),
                parent: Some(dom.borrow().document()),
            };
            if let MessageResult::Action(a) = view.message(view_state, &mut msg, mut_ref, state) {
                actions.push(a);
            }
            msg.finish().0
        };
        self.ctx.set_environment(env);
        self.last_default_prevented = event.prop.default_prevented();
        self.rebuild();
        actions
    }

    /// The shared document handle the runner mutates.
    pub fn dom(&self) -> DomHandle {
        self.dom.clone()
    }

    /// The DOM node of the current root element (attached under the document
    /// root). Stable across `update`s unless a view swaps the root node.
    pub fn root(&self) -> NodeId {
        self.root.node
    }

    /// The current app state.
    pub fn state(&self) -> &State {
        &self.state
    }
}

/// Append `node`'s focusable descendants (including itself), in document
/// pre-order, to `out`. Focusable = carries a key handler ([`ServalCtx::is_focusable`]).
fn collect_focusables(dom: &ScriptedDom, ctx: &ServalCtx, node: NodeId, out: &mut Vec<NodeId>) {
    if ctx.is_focusable(node) {
        out.push(node);
    }
    for child in dom.dom_children(node) {
        collect_focusables(dom, ctx, child, out);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{DomMutation, LayoutDom, LayoutDomMut, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{DomHandle, El, el};

    use super::ServalAppRunner;

    /// A counter: the canonical Stage 1b app state.
    struct Counter {
        count: u32,
    }

    /// The app logic: a `<div>` holding the count as text. The element type is
    /// the uniform `ServalElement`, so the view type is concrete (no boxing).
    fn counter_view(s: &Counter) -> El<String, Counter, ()> {
        el::<_, Counter, ()>("div", s.count.to_string())
    }

    /// The text data of the (single) text child under `node`.
    fn text_child(dom: &ScriptedDom, node: NodeId) -> Option<String> {
        dom.dom_children(node)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
    }

    fn drain(dom: &DomHandle) -> Vec<DomMutation<NodeId>> {
        let mut out = Vec::new();
        dom.borrow_mut().drain_mutations(&mut out);
        out
    }

    #[test]
    fn counter_ticks_through_runner() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::new(dom.clone(), counter_view, Counter { count: 0 });

        // The initial tree: a <div> under the document root holding "0".
        let root = runner.root();
        {
            let dom = runner.dom();
            let dom = dom.borrow();
            assert_eq!(dom.kind(root), NodeKind::Element);
            assert_eq!(dom.element_name(root).unwrap().local.to_string(), "div");
            assert_eq!(text_child(&dom, root).as_deref(), Some("0"));
            // The root is attached under the document root.
            assert!(dom.dom_children(dom.document()).any(|c| c == root));
        }
        // The build recorded inserts (text under div, div under document) and
        // no removals.
        let build_muts = drain(&dom);
        assert!(
            build_muts
                .iter()
                .any(|m| matches!(m, DomMutation::Inserted { .. })),
            "build should record inserts: {build_muts:?}"
        );

        // Tick the counter a few times; each update diffs the new count into
        // the existing text node (a CharacterDataChanged, not a re-create).
        for expected in 1..=3u32 {
            runner.update(|s| s.count += 1);
            assert_eq!(runner.state().count, expected);

            {
                let dom = runner.dom();
                let dom = dom.borrow();
                assert_eq!(
                    text_child(&dom, runner.root()).as_deref(),
                    Some(expected.to_string().as_str()),
                    "text under div should read the latest count"
                );
            }

            let muts = drain(&dom);
            // The only structural mutation a count change produces is the text
            // node's character-data update — no node churn.
            assert!(
                muts.iter()
                    .any(|m| matches!(m, DomMutation::CharacterDataChanged { .. })),
                "tick to {expected} should record a CharacterDataChanged: {muts:?}"
            );
            assert!(
                !muts
                    .iter()
                    .any(|m| matches!(m, DomMutation::Inserted { .. })),
                "tick to {expected} should not insert nodes: {muts:?}"
            );
            assert!(
                !muts
                    .iter()
                    .any(|m| matches!(m, DomMutation::Removed { .. })),
                "tick to {expected} should not remove nodes: {muts:?}"
            );
        }
    }
}
