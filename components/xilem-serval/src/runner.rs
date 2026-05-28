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
use serval_scripted_dom::NodeId;
use xilem_core::{DynMessage, Environment, MessageCtx, MessageResult, View, ViewId};

use crate::{DomHandle, KeyEvent, PointerClick, ServalCtx, ServalElement, ServalElementMut};

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
        };
        next.rebuild(view, view_state, ctx, mut_ref, state);

        // The freshly diffed view becomes the retained `prev` for the next tick.
        *view = next;
    }

    /// Dispatch a native pointer click that hit `target`.
    ///
    /// `target` is the node serval's hit-test resolved the pointer to (the
    /// `point → NodeId` half lives in the host's `hit_test_node`). This is the
    /// faithful-routing dispatch: no native handler registry of `Rc<dyn Fn>`,
    /// just the `xilem_core` message cycle the browser path also uses.
    ///
    /// The walk is **bubble phase** (target → root), the DOM default. For each
    /// ancestor, if [`ServalCtx::click_path`] has a handler, its routing path is
    /// collected; then each collected path is routed through `view.message`. A
    /// later refinement can add a capture phase (root → target before bubble)
    /// and per-listener phase flags — the walk is structured target-first so
    /// capture is a reversed pre-pass, not a rewrite. Bubble-only here avoids
    /// double-firing a single handler.
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
    /// a key handler (is focusable, per [`ServalCtx::key_path`]); if none is
    /// found, focus is cleared to `None`. So clicking a focusable element focuses
    /// it and clicking elsewhere defocuses. This is independent of whether any
    /// *click* handler fired — a focusable element need not have an `on_click`.
    pub fn dispatch_click(&mut self, target: NodeId, event: PointerClick) -> Vec<Action> {
        // 1. + 2. Bubble walk (target → … → document), collecting the routing
        //    path of every ancestor that carries a click handler, and — in the
        //    same walk — finding the nearest focusable (key-handler-bearing)
        //    ancestor for click-to-focus. Done under short-lived shared borrows
        //    of `dom` and `ctx`, fully released before routing.
        let (paths, new_focus): (Vec<Vec<ViewId>>, Option<NodeId>) = {
            let dom = self.dom.borrow();
            let mut paths = Vec::new();
            let mut focus = None;
            let mut current = Some(target);
            while let Some(node) = current {
                if let Some(path) = self.ctx.click_path(node) {
                    paths.push(path.to_vec());
                }
                // The first (nearest) focusable ancestor wins; the walk runs
                // target → root, so the first hit is the deepest.
                if focus.is_none() && self.ctx.key_path(node).is_some() {
                    focus = Some(node);
                }
                current = dom.parent(node);
            }
            (paths, focus)
        };

        // Click-to-focus: focus the nearest focusable ancestor, or clear it when
        // the click landed outside any focusable element. Independent of whether
        // a click handler fired below.
        self.focus = new_focus;

        if paths.is_empty() {
            // No click handler anywhere on the chain: nothing routed, no rebuild.
            // Focus was still updated above.
            return Vec::new();
        }

        // 3. Route the event down each collected path through the faithful
        //    message cycle. The disjoint-field destructure mirrors `rebuild`,
        //    so `View::message`'s borrow set does not alias `self`. Any action
        //    that reaches the root (a `MessageResult::Action`) is collected.
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
                let mut msg = MessageCtx::new(
                    Environment::new(),
                    path,
                    DynMessage::new(event.clone()),
                );
                let mut_ref = ServalElementMut {
                    node: &mut root.node,
                    dom: dom.clone(),
                };
                // Handlers may mutate state in place (a rebuild below reflects
                // that) and/or bubble an `Action` up to the root. A root-level
                // `MessageResult::Action(a)` is the runner's `Action` home: we
                // collect it for the caller. `Action = ()` collects nothing
                // meaningful (the Stage 2b path).
                if let MessageResult::Action(a) =
                    view.message(view_state, &mut msg, mut_ref, state)
                {
                    actions.push(a);
                }
            }
        }

        // 4. Rebuild so the handler's state mutation reaches the DOM — the same
        //    tail `update` runs.
        self.rebuild();

        actions
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

    /// Dispatch a native key event to the focused node.
    ///
    /// If [`focus`](Self::focus) is `None`, this is a no-op: it returns an empty
    /// `Vec` and runs no routing or rebuild (no focused node = nowhere to send
    /// keys). Otherwise it mirrors [`dispatch_click`](Self::dispatch_click)
    /// exactly, but rooted at the *focused* node rather than a hit-test target:
    /// it bubble-walks `focus → … → document` over `dom.parent`, collects each
    /// ancestor's [`key_path`](ServalCtx::key_path), routes a [`KeyEvent`] down
    /// each via the faithful `MessageCtx`/`View::message` cycle, then rebuilds —
    /// returning the actions that reached the root.
    ///
    /// The walk is **bubble phase** (focus → root): a key handler on a parent
    /// fires when the focused descendant has none, matching DOM key bubbling. A
    /// capture pre-pass and per-listener phase flags are the same later
    /// refinement noted on `dispatch_click`.
    ///
    /// As in `dispatch_click`, paths are collected **before** any routing so the
    /// immutable `ctx`/`dom` borrows release before the `&mut self` message +
    /// rebuild borrows.
    pub fn dispatch_key(&mut self, event: KeyEvent) -> Vec<Action> {
        // No focus: nothing to route to, nothing to do.
        let Some(focus) = self.focus else {
            return Vec::new();
        };

        // 1. + 2. Bubble walk (focus → … → document), collecting the routing
        //    path of every ancestor that carries a key handler.
        let paths: Vec<Vec<ViewId>> = {
            let dom = self.dom.borrow();
            let mut paths = Vec::new();
            let mut current = Some(focus);
            while let Some(node) = current {
                if let Some(path) = self.ctx.key_path(node) {
                    paths.push(path.to_vec());
                }
                current = dom.parent(node);
            }
            paths
        };

        if paths.is_empty() {
            // The focused node (and its ancestors) carry no key handler — e.g.
            // focus was aimed at a non-focusable node via `set_focus`. Nothing
            // routed, no rebuild.
            return Vec::new();
        }

        // 3. Route the event down each collected path through the faithful
        //    message cycle, the same disjoint-field destructure as `rebuild` /
        //    `dispatch_click`. Any action that reaches the root is collected.
        let mut actions = Vec::new();
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
                let mut msg = MessageCtx::new(
                    Environment::new(),
                    path,
                    DynMessage::new(event.clone()),
                );
                let mut_ref = ServalElementMut {
                    node: &mut root.node,
                    dom: dom.clone(),
                };
                if let MessageResult::Action(a) =
                    view.message(view_state, &mut msg, mut_ref, state)
                {
                    actions.push(a);
                }
            }
        }

        // 4. Rebuild so the handler's state mutation reaches the DOM.
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
        let mut runner =
            ServalAppRunner::new(dom.clone(), counter_view, Counter { count: 0 });

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
