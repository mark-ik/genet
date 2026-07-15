/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Genet-native runner that owns app state and the retained view tree.
//!
//! Meristem provides view diffing and message routing but does not schedule
//! work. [`GenetAppRunner`] owns state, the root node, and the rebuild cadence
//! that turns a state change into DOM mutations.
//!
//! [`GenetAppRunner`] is that owner, kept deliberately thin: it depends only
//! on `meristem` and this backend, never on genet-layout / genet-render. A host
//! crate such as `pelt-desktop` drives the render side over the
//! [`ScriptedDom`](genet_scripted_dom::ScriptedDom) the runner mutates.
//!
//! Native event dispatch takes a hit [`NodeId`] (the `point → NodeId` half is
//! the host's responsibility), walks the node's ancestor chain, and routes
//! events through Meristem's message cycle. The timer tick that drives
//! [`update`](GenetAppRunner::update), platform input, relayout, rendering, and
//! presentation remain host concerns.
//!
//! **One state, N windows** (one-state-N-windows design, step 2): the per-tree
//! half of the runner — the dom, ctx, retained view, focus, and pointer
//! capture — lives in the crate-internal [`RunnerTree`], with app state and
//! the view-producing logic threaded in per call. [`GenetAppRunner`] is one
//! state + one tree (this file, public API unchanged);
//! [`GenetMultiRunner`](crate::GenetMultiRunner) is one state + N trees,
//! each a projection of the same state into its own `ScriptedDom`.

use core::marker::PhantomData;

use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace};
use meristem::{DynMessage, MessageCtx, MessageResult, View, ViewId};

use crate::{
    DomHandle, GenetCtx, GenetElement, GenetElementMut, HoverEvent, Key, KeyEvent, NamedKey,
    PointerClick, PointerEvent, WheelEvent,
};

/// The per-tree half of a runner: one `ScriptedDom` target, its [`GenetCtx`]
/// (handler registries are keyed by this dom's `NodeId`s), the retained view
/// tree, and the per-window interaction state (focus, pointer capture, the
/// cancellation flag). App state and the view-producing logic are *not* here —
/// they are threaded into every method, so one state can drive one tree
/// ([`GenetAppRunner`]) or many ([`GenetMultiRunner`](crate::GenetMultiRunner)).
pub(crate) struct RunnerTree<State, V, Action>
where
    State: 'static,
    Action: 'static,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    dom: DomHandle,
    ctx: GenetCtx,
    view: V,
    view_state: V::ViewState,
    /// The retained root element produced by the current `view`. Its `node`
    /// stays attached under the document root for the tree's lifetime.
    root: GenetElement,
    /// The currently focused node, if any. A click sets this to the nearest
    /// focusable (key-handler-bearing) ancestor of the click target, or clears
    /// it to `None` when the click lands outside any focusable element.
    /// [`dispatch_key`](Self::dispatch_key) walks from here.
    focus: Option<NodeId>,
    /// The element capturing the current pointer drag, if any. Set by
    /// [`dispatch_pointer_down`](Self::dispatch_pointer_down) to the nearest
    /// pointer-handler ancestor of the press, consulted by
    /// [`dispatch_pointer_move`](Self::dispatch_pointer_move) to route moves, and
    /// cleared by [`dispatch_pointer_up`](Self::dispatch_pointer_up). So a drag
    /// keeps reaching the element it started on even if the cursor leaves it.
    pointer_capture: Option<NodeId>,
    /// Whether the most recent dispatch had its default action prevented (a
    /// handler called `prevent_default` on the shared [`Propagation`](crate::Propagation)
    /// cell). Read via `default_prevented` after dispatch to gate the host's
    /// own default action.
    last_default_prevented: bool,
    phantom: PhantomData<fn() -> (State, Action)>,
}

impl<State, V, Action> RunnerTree<State, V, Action>
where
    State: 'static,
    Action: 'static,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    /// Build the initial tree from `state` and attach its root under the
    /// document root of `dom`.
    pub(crate) fn build(
        dom: DomHandle,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
    ) -> Self {
        let mut ctx = GenetCtx::new(dom.clone());
        let view = logic(state);
        let (root, view_state) = view.build(&mut ctx, state);

        // Attach the produced root under the document root (append).
        let doc_root = dom.borrow().document();
        dom.borrow_mut().insert_before(doc_root, root.node, None);
        let focus = ctx
            .take_focus_request()
            .filter(|&node| dom.borrow().is_live(node));

        Self {
            dom,
            ctx,
            view,
            view_state,
            root,
            focus,
            pointer_capture: None,
            last_default_prevented: false,
            phantom: PhantomData,
        }
    }

    /// Re-run the logic against the current state and diff the produced view
    /// into the retained one.
    ///
    /// The disjoint-field borrows matter: `rebuild` needs `&prev_view`,
    /// `&mut view_state`, `&mut ctx`, the root `Mut`, and `&mut state` all at
    /// once, so the fields are destructured into separate `&mut`s. The root
    /// `GenetElementMut` is constructed exactly as `Harness::rebuild` does —
    /// borrowing `root.node` so a view *could* swap the root node, and cloning
    /// the shared `dom` handle.
    pub(crate) fn rebuild(&mut self, logic: &mut impl FnMut(&State) -> V, state: &mut State) {
        let next = logic(state);

        // Disjoint borrows: each field separately so `rebuild`'s argument set
        // does not alias `self`.
        let Self {
            ctx,
            view,
            view_state,
            root,
            dom,
            ..
        } = self;

        let mut_ref = GenetElementMut {
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
        // claimed within it — its key really left every portable list in this
        // tree, so it gets a real teardown and its (still-attached) node is
        // removed. Per-tree draining also confines parked elements to their
        // own dom: a cross-dom adoption is impossible by construction.
        // (moveBefore plan S5.)
        self.ctx.drain_nursery();

        // Focus and pointer capture are retained NodeIds, so a rebuild that
        // replaces or removes their element can retire either handle. The DOM
        // dangle contract makes `is_live` the only safe read in that state.
        // Clear them at the publication boundary before the host can ask for
        // the next dispatch target or captured-element rect.
        self.scrub_dead_interaction_handles();
        if let Some(node) = self.ctx.take_focus_request()
            && self.node_is_live(node)
        {
            self.focus = Some(node);
        }
    }

    fn node_is_live(&self, node: NodeId) -> bool {
        self.dom.borrow().is_live(node)
    }

    fn scrub_dead_interaction_handles(&mut self) {
        let dom = self.dom.borrow();
        if self.focus.is_some_and(|node| !dom.is_live(node)) {
            self.focus = None;
        }
        if self.pointer_capture.is_some_and(|node| !dom.is_live(node)) {
            self.pointer_capture = None;
        }
    }

    /// Tear the whole tree down (a projection being removed): run the retained
    /// view's teardown, then remove the root node from the document.
    pub(crate) fn teardown(mut self) {
        let Self {
            ctx,
            view,
            view_state,
            root,
            dom,
            ..
        } = &mut self;
        let mut_ref = GenetElementMut {
            node: &mut root.node,
            dom: dom.clone(),
            parent: Some(dom.borrow().document()),
        };
        view.teardown(view_state, ctx, mut_ref);
        let root_node = root.node;
        dom.borrow_mut().remove(root_node);
        // A teardown can park portable children; nothing can ever claim them
        // (the tree is gone), so drain immediately.
        ctx.drain_nursery();
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
        lookup: impl Fn(&GenetCtx, NodeId) -> &[crate::context::Handler],
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

    /// Dispatch a native pointer click that hit `target`. See
    /// [`GenetAppRunner::dispatch_click`] for the full contract; this is the
    /// per-tree implementation with state and logic threaded in.
    pub(crate) fn dispatch_click(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        target: NodeId,
        event: PointerClick,
    ) -> Vec<Action> {
        if !self.node_is_live(target) {
            self.last_default_prevented = false;
            return Vec::new();
        }

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
                view,
                view_state,
                root,
                dom,
                ..
            } = self;

            for path in paths {
                let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
                let mut_ref = GenetElementMut {
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
        self.rebuild(logic, state);

        actions
    }

    pub(crate) fn default_prevented(&self) -> bool {
        self.last_default_prevented
    }

    pub(crate) fn focus(&self) -> Option<NodeId> {
        self.focus
    }

    pub(crate) fn set_focus(&mut self, node: Option<NodeId>) {
        self.focus = node.filter(|&node| self.node_is_live(node));
    }

    /// Move focus to the next (`forward`) or previous focusable element in
    /// document order, wrapping. The Tab-traversal default: a document engine has
    /// no built-in tab order, so the runner provides one over the focusable set
    /// (elements carrying a key handler, per [`GenetCtx::is_focusable`]) in DOM
    /// pre-order. With nothing focused, `forward` focuses the first focusable and
    /// backward the last. Rebuilds after (focus may drive `:focus` styling
    /// later). No-op when there are no focusable elements.
    pub(crate) fn focus_traverse(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        forward: bool,
    ) {
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
            }
            None => {
                if forward {
                    0
                } else {
                    focusables.len() - 1
                }
            }
        };
        self.set_focus(Some(focusables[next]));
        self.rebuild(logic, state);
    }

    /// Dispatch a native key event to the focused node, then apply the
    /// runner-level keyboard defaults (Tab traversal, Enter/Space activation).
    /// See [`GenetAppRunner::dispatch_key`] for the full contract.
    pub(crate) fn dispatch_key(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        event: KeyEvent,
    ) -> Vec<Action> {
        // A fresh pass: the routing tail records the real cancellation value; a
        // pass that routes nothing leaves it false.
        self.last_default_prevented = false;
        self.scrub_dead_interaction_handles();

        // With nothing focused, the only key carrying a runner default is Tab,
        // which enters the focusable set (no element to route to). Everything else
        // no-ops — no focused node means nowhere to send keys.
        let Some(focus) = self.focus else {
            if matches!(event.key, Key::Named(NamedKey::Tab)) {
                self.focus_traverse(logic, state, !event.mods.shift);
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
        //    here too (no longer swallowed pre-routing), so a view can handle
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
                    view,
                    view_state,
                    root,
                    dom,
                    ..
                } = self;

                for path in paths {
                    let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
                    let mut_ref = GenetElementMut {
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
            let radio_target = {
                let dom = self.dom.borrow();
                radio_navigation_target(&dom, focus, &event.key)
            };
            let activates = {
                let dom = self.dom.borrow();
                semantic_activation_matches(&dom, focus, &event.key)
            };
            match event.key {
                // Tab traverses focus across the focusable set, the runner-level
                // default a document engine otherwise lacks.
                Key::Named(NamedKey::Tab) => {
                    self.focus_traverse(logic, state, !event.mods.shift);
                    return actions; // focus_traverse rebuilt
                }
                // Radio groups use one Tab stop. Arrow keys move to the next
                // radio, select it through the same click path as a pointer,
                // and keep focus on the newly selected item.
                Key::Named(
                    NamedKey::ArrowLeft
                    | NamedKey::ArrowUp
                    | NamedKey::ArrowRight
                    | NamedKey::ArrowDown
                    | NamedKey::Home
                    | NamedKey::End,
                ) if radio_target.is_some() && self.ctx.key_handlers_at(focus).is_empty() => {
                    let target = radio_target.expect("guarded above");
                    actions.extend(self.dispatch_click(
                        logic,
                        state,
                        target,
                        PointerClick::at((0.0, 0.0)),
                    ));
                    self.focus = Some(target);
                    self.last_default_prevented = true;
                    return actions;
                }
                // Enter/Space activate a focusable control that has a click handler
                // according to its native tag or ARIA role. Checkbox, switch,
                // and radio roles use Space; buttons use Enter and Space.
                Key::Named(NamedKey::Enter | NamedKey::Space)
                    if !self.ctx.click_handlers_at(focus).is_empty()
                        && self.ctx.key_handlers_at(focus).is_empty()
                        && activates =>
                {
                    actions.extend(self.dispatch_click(
                        logic,
                        state,
                        focus,
                        PointerClick::at((0.0, 0.0)),
                    ));
                    // The key was consumed as activation: tell the host not to run
                    // its own default (e.g. Space scrolling the page).
                    self.last_default_prevented = true;
                    return actions; // dispatch_click rebuilt
                }
                _ => {}
            }
        }

        // 4. A handler ran but no runner default fired: rebuild so the handler's
        //    state change reaches the DOM. When nothing routed and no default
        //    fired, nothing changed, so the rebuild is skipped.
        if routed {
            self.rebuild(logic, state);
        }

        actions
    }

    /// Begin a pointer drag: the press hit `target`. See
    /// [`GenetAppRunner::dispatch_pointer_down`].
    pub(crate) fn dispatch_pointer_down(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        target: NodeId,
        event: PointerEvent,
    ) -> Vec<Action> {
        // A fresh pass: reset the cancellation flag; route_pointer records the
        // real value when a handler runs, and a no-capture press leaves it false.
        self.last_default_prevented = false;
        if !self.node_is_live(target) {
            return Vec::new();
        }
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
            Some(node) => self.route_pointer(logic, state, node, event),
            None => Vec::new(),
        }
    }

    /// Route a `Move` to the element capturing the drag (if any).
    pub(crate) fn dispatch_pointer_move(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        event: PointerEvent,
    ) -> Vec<Action> {
        self.last_default_prevented = false;
        self.scrub_dead_interaction_handles();
        match self.pointer_capture {
            Some(node) => self.route_pointer(logic, state, node, event),
            None => Vec::new(),
        }
    }

    /// Route an `Up` to the capturing element and end the drag.
    pub(crate) fn dispatch_pointer_up(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        event: PointerEvent,
    ) -> Vec<Action> {
        self.last_default_prevented = false;
        self.scrub_dead_interaction_handles();
        match self.pointer_capture.take() {
            Some(node) => self.route_pointer(logic, state, node, event),
            None => Vec::new(),
        }
    }

    pub(crate) fn pointer_capture(&self) -> Option<NodeId> {
        self.pointer_capture
    }

    /// The nearest ancestor of `hit` (including itself) carrying an
    /// [`on_pointer`](crate::on_pointer) handler.
    pub(crate) fn pointer_target(&self, hit: NodeId) -> Option<NodeId> {
        if !self.node_is_live(hit) {
            return None;
        }
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

    /// Route one host-computed hover transition to the nearest registered
    /// ancestor of `target`.
    pub(crate) fn dispatch_hover(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        target: NodeId,
        event: HoverEvent,
    ) -> Vec<Action> {
        self.last_default_prevented = false;
        if !self.node_is_live(target) {
            return Vec::new();
        }
        match self.hover_target(target) {
            Some(node) => self.route_hover(logic, state, node, event),
            None => Vec::new(),
        }
    }

    /// The nearest ancestor of `hit` (including itself) carrying a hover
    /// handler.
    pub(crate) fn hover_target(&self, hit: NodeId) -> Option<NodeId> {
        if !self.node_is_live(hit) {
            return None;
        }
        let dom = self.dom.borrow();
        let mut current = Some(hit);
        while let Some(node) = current {
            if self.ctx.hover_handler(node).is_some() {
                return Some(node);
            }
            current = dom.parent(node);
        }
        None
    }

    /// Route a hover event down one registered view path, then rebuild.
    fn route_hover(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        node: NodeId,
        event: HoverEvent,
    ) -> Vec<Action> {
        let Some(path) = self.ctx.hover_handler(node).map(<[ViewId]>::to_vec) else {
            return Vec::new();
        };
        let env = self.ctx.take_environment();
        let mut actions = Vec::new();
        let env = {
            let Self {
                view,
                view_state,
                root,
                dom,
                ..
            } = self;
            let mut message = MessageCtx::new(env, path, DynMessage::new(event.clone()));
            let element = GenetElementMut {
                node: &mut root.node,
                dom: dom.clone(),
                parent: Some(dom.borrow().document()),
            };
            if let MessageResult::Action(action) =
                view.message(view_state, &mut message, element, state)
            {
                actions.push(action);
            }
            message.finish().0
        };
        self.ctx.set_environment(env);
        self.last_default_prevented = event.prop.default_prevented();
        self.rebuild(logic, state);
        actions
    }

    /// Route a [`PointerEvent`] down `node`'s registered pointer path through the
    /// faithful message cycle, then rebuild.
    fn route_pointer(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        node: NodeId,
        event: PointerEvent,
    ) -> Vec<Action> {
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
                view,
                view_state,
                root,
                dom,
                ..
            } = self;
            // Clone into the message: the handler mutates its clone's shared
            // `Propagation` cell, and the original below reads back what it set.
            let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
            let mut_ref = GenetElementMut {
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
        self.rebuild(logic, state);
        actions
    }

    /// Route a wheel/scroll notch to the nearest ancestor of `target` (including
    /// itself) carrying an [`on_wheel`](crate::on_wheel) handler.
    pub(crate) fn dispatch_wheel(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        target: NodeId,
        event: WheelEvent,
    ) -> Vec<Action> {
        self.last_default_prevented = false;
        if !self.node_is_live(target) {
            return Vec::new();
        }
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
            Some(node) => self.route_wheel(logic, state, node, event),
            None => Vec::new(),
        }
    }

    /// The nearest ancestor of `hit` (including itself) carrying an
    /// [`on_wheel`](crate::on_wheel) handler.
    pub(crate) fn wheel_target(&self, hit: NodeId) -> Option<NodeId> {
        if !self.node_is_live(hit) {
            return None;
        }
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
    fn route_wheel(
        &mut self,
        logic: &mut impl FnMut(&State) -> V,
        state: &mut State,
        node: NodeId,
        event: WheelEvent,
    ) -> Vec<Action> {
        let Some(path) = self.ctx.wheel_handler(node).map(<[ViewId]>::to_vec) else {
            return Vec::new();
        };
        // Thread the real environment through the cycle (G2.2), like route_pointer.
        let env = self.ctx.take_environment();
        let mut actions = Vec::new();
        let env = {
            let Self {
                view,
                view_state,
                root,
                dom,
                ..
            } = self;
            let mut msg = MessageCtx::new(env, path, DynMessage::new(event.clone()));
            let mut_ref = GenetElementMut {
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
        self.rebuild(logic, state);
        actions
    }

    /// The shared document handle the tree mutates.
    pub(crate) fn dom(&self) -> DomHandle {
        self.dom.clone()
    }

    /// The DOM node of the current root element.
    pub(crate) fn root(&self) -> NodeId {
        self.root.node
    }
}

/// Owns the app state, the view-producing logic, and one retained view tree,
/// rebuilding the [`ScriptedDom`] whenever the state changes.
///
/// `Logic: FnMut(&State) -> V` reads state and produces the root view. Its
/// element is always the uniform [`GenetElement`].
///
/// `Action` is the root view's action type. When a handler returns an action that no
/// parent [`map_action`](meristem::map_action) consumes, it surfaces as a
/// [`MessageResult::Action`] which [`dispatch_click`](Self::dispatch_click)
/// collects and returns. The common `Action = ()` case is the no-op it was — a
/// `()` action carries nothing to observe.
pub struct GenetAppRunner<State, Logic, V, Action = ()>
where
    State: 'static,
    Action: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    state: State,
    logic: Logic,
    tree: RunnerTree<State, V, Action>,
}

impl<State, Logic, V, Action> GenetAppRunner<State, Logic, V, Action>
where
    State: 'static,
    Action: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    /// Build the initial tree from `state` and attach its root under the
    /// document root.
    ///
    /// Mirrors Cambium's test harness: run the logic to
    /// get a view, `View::build` it into the `ScriptedDom`, then
    /// `insert_before(document, root, None)` to append it under the document.
    pub fn new(dom: DomHandle, mut logic: Logic, mut state: State) -> Self {
        let tree = RunnerTree::build(dom, &mut logic, &mut state);
        Self { state, logic, tree }
    }

    /// Apply a state update, then rebuild the view tree against the new state.
    ///
    /// `f` is the externally-driven "message": it mutates the owned state. The
    /// runner then reruns the logic and diffs the produced view against the
    /// retained one, emitting `DomMutation`s into the `ScriptedDom`.
    pub fn update(&mut self, f: impl FnOnce(&mut State)) {
        f(&mut self.state);
        self.tree.rebuild(&mut self.logic, &mut self.state);
    }

    /// Dispatch a native pointer click that hit `target`.
    ///
    /// `target` is the node Genet's hit-test resolved the pointer to (the
    /// `point → NodeId` half lives in the host's `hit_test_node`). This is the
    /// faithful-routing dispatch: no native handler registry of `Rc<dyn Fn>`,
    /// just the `meristem` message cycle the browser path also uses.
    ///
    /// Dispatch runs the full DOM propagation order, **capture → target →
    /// bubble**; a `.capture(true)` ancestor fires before a default (bubble)
    /// descendant/target, yielding the browser/`xilem_web` ordering.
    ///
    /// Returns the `Action`s that bubbled all the way to the root — i.e. any
    /// handler action no parent [`map_action`](meristem::map_action) absorbed.
    ///
    /// After routing and rebuilding, focus is set to the nearest ancestor of
    /// `target` (including `target` itself) that carries
    /// a key handler (is focusable); if none is found, focus is cleared. So
    /// clicking a focusable element focuses it and clicking elsewhere defocuses,
    /// independent of whether any *click* handler fired.
    pub fn dispatch_click(&mut self, target: NodeId, event: PointerClick) -> Vec<Action> {
        self.tree
            .dispatch_click(&mut self.logic, &mut self.state, target, event)
    }

    /// Whether the most recent dispatch had its default action prevented by a
    /// handler (`prevent_default` on the event). The host calls this right after
    /// dispatch to decide whether to run its own default action — navigation,
    /// caret movement, form activation, drag start. This is the native side of
    /// the converged cancellation contract (the JS side returns it from
    /// `dispatchEvent`); a handler that does not prevent leaves it `false`.
    pub fn default_prevented(&self) -> bool {
        self.tree.default_prevented()
    }

    /// The currently focused node, if any.
    ///
    /// Set by [`dispatch_click`](Self::dispatch_click) (click-to-focus) or
    /// [`set_focus`](Self::set_focus); read by [`dispatch_key`](Self::dispatch_key)
    /// as the root of its bubble walk.
    pub fn focus(&self) -> Option<NodeId> {
        self.tree.focus()
    }

    /// Set (or clear, with `None`) the focused node directly.
    ///
    /// The keyboard counterpart of a programmatic `element.focus()`. No
    /// validation that a live `node` is focusable: a test (or a host) may aim
    /// focus at any live node, and [`dispatch_key`](Self::dispatch_key) simply
    /// finds no key handler to route to if it is not focusable. A retired node
    /// clears focus under the DOM dangle contract.
    pub fn set_focus(&mut self, node: Option<NodeId>) {
        self.tree.set_focus(node);
    }

    /// Move focus to the next (`forward`) or previous focusable element in
    /// document order, wrapping.
    pub fn focus_traverse(&mut self, forward: bool) {
        self.tree
            .focus_traverse(&mut self.logic, &mut self.state, forward);
    }

    /// Dispatch a native key event to the focused node, then apply the
    /// runner-level keyboard defaults (Tab traversal, Enter/Space activation) the
    /// model leaves to the host — each overridable by a handler that calls
    /// `prevent_default` (the G2.3 escape hatches).
    pub fn dispatch_key(&mut self, event: KeyEvent) -> Vec<Action> {
        self.tree
            .dispatch_key(&mut self.logic, &mut self.state, event)
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
        self.tree
            .dispatch_pointer_down(&mut self.logic, &mut self.state, target, event)
    }

    /// Route a `Move` to the element capturing the drag (if any). No-op when no
    /// drag is active.
    pub fn dispatch_pointer_move(&mut self, event: PointerEvent) -> Vec<Action> {
        self.tree
            .dispatch_pointer_move(&mut self.logic, &mut self.state, event)
    }

    /// Route an `Up` to the capturing element and end the drag (clearing
    /// capture). No-op when no drag is active.
    pub fn dispatch_pointer_up(&mut self, event: PointerEvent) -> Vec<Action> {
        self.tree
            .dispatch_pointer_up(&mut self.logic, &mut self.state, event)
    }

    /// The element currently capturing a pointer drag, if any. The host reads
    /// this between [`dispatch_pointer_down`](Self::dispatch_pointer_down) and
    /// `up` to know which element's rect to measure for the move's local coords.
    pub fn pointer_capture(&self) -> Option<NodeId> {
        self.tree.pointer_capture()
    }

    /// The nearest ancestor of `hit` (including itself) carrying an
    /// [`on_pointer`](crate::on_pointer) handler — the element a press there
    /// would capture, resolved *without* starting a drag.
    pub fn pointer_target(&self, hit: NodeId) -> Option<NodeId> {
        self.tree.pointer_target(hit)
    }

    /// Route a hover Enter, Leave, or Move event from `target` to the nearest
    /// ancestor carrying [`on_hover`](crate::on_hover).
    ///
    /// The host owns hit testing and transition detection. When its hovered
    /// node changes it calls this once for Leave on the old hit and once for
    /// Enter on the new hit; Move uses the current hit.
    pub fn dispatch_hover(&mut self, target: NodeId, event: HoverEvent) -> Vec<Action> {
        self.tree
            .dispatch_hover(&mut self.logic, &mut self.state, target, event)
    }

    /// The nearest ancestor of `hit` (including itself) carrying an
    /// [`on_hover`](crate::on_hover) handler.
    pub fn hover_target(&self, hit: NodeId) -> Option<NodeId> {
        self.tree.hover_target(hit)
    }

    /// Route a wheel/scroll notch to the nearest ancestor of `target` (including
    /// itself) carrying an [`on_wheel`](crate::on_wheel) handler. Unlike a
    /// pointer drag there is no capture: each notch resolves its own target by
    /// the ancestor walk. Returns the actions that bubbled to the root.
    pub fn dispatch_wheel(&mut self, target: NodeId, event: WheelEvent) -> Vec<Action> {
        self.tree
            .dispatch_wheel(&mut self.logic, &mut self.state, target, event)
    }

    /// The nearest ancestor of `hit` (including itself) carrying an
    /// [`on_wheel`](crate::on_wheel) handler — the element a wheel there would
    /// scroll, resolved *without* dispatching.
    pub fn wheel_target(&self, hit: NodeId) -> Option<NodeId> {
        self.tree.wheel_target(hit)
    }

    /// The shared document handle the runner mutates.
    pub fn dom(&self) -> DomHandle {
        self.tree.dom()
    }

    /// The DOM node of the current root element (attached under the document
    /// root). Stable across `update`s unless a view swaps the root node.
    pub fn root(&self) -> NodeId {
        self.tree.root()
    }

    /// The current app state.
    pub fn state(&self) -> &State {
        &self.state
    }
}

/// Append `node`'s focusable descendants (including itself), in document
/// pre-order, to `out`. Focusable = carries a key handler ([`GenetCtx::is_focusable`]).
fn collect_focusables(dom: &ScriptedDom, ctx: &GenetCtx, node: NodeId, out: &mut Vec<NodeId>) {
    if ctx.is_focusable(node) {
        out.push(node);
    }
    for child in dom.dom_children(node) {
        collect_focusables(dom, ctx, child, out);
    }
}

fn attr<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
    dom.attribute(node, &Namespace::from(""), &LocalName::from(name))
}

fn semantic_activation_matches(dom: &ScriptedDom, node: NodeId, key: &Key) -> bool {
    let role = attr(dom, node, "role");
    match (role, key) {
        (Some("checkbox" | "switch" | "radio"), Key::Named(NamedKey::Space)) => true,
        (Some("button"), Key::Named(NamedKey::Enter | NamedKey::Space)) => true,
        (None, Key::Named(NamedKey::Enter | NamedKey::Space)) => {
            dom.element_name(node)
                .is_some_and(|name| name.local.as_ref() == "button")
                || !dom
                    .element_name(node)
                    .is_some_and(|name| matches!(name.local.as_ref(), "input" | "textarea"))
        }
        _ => false,
    }
}

fn radio_navigation_target(dom: &ScriptedDom, focus: NodeId, key: &Key) -> Option<NodeId> {
    if attr(dom, focus, "role") != Some("radio") {
        return None;
    }
    let parent = dom.parent(focus)?;
    let radios: Vec<_> = dom
        .dom_children(parent)
        .filter(|&node| attr(dom, node, "role") == Some("radio"))
        .collect();
    let current = radios.iter().position(|&node| node == focus)?;
    let target = match key {
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
            (current + radios.len() - 1) % radios.len()
        }
        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => (current + 1) % radios.len(),
        Key::Named(NamedKey::Home) => 0,
        Key::Named(NamedKey::End) => radios.len() - 1,
        _ => return None,
    };
    radios.get(target).copied()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{DomMutation, LayoutDom, LayoutDomMut, NodeKind};

    use crate::{DomHandle, El, el};

    use super::GenetAppRunner;

    /// A counter: the canonical Stage 1b app state.
    struct Counter {
        count: u32,
    }

    /// The app logic: a `<div>` holding the count as text. The element type is
    /// the uniform `GenetElement`, so the view type is concrete (no boxing).
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
        let mut runner = GenetAppRunner::new(dom.clone(), counter_view, Counter { count: 0 });

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
