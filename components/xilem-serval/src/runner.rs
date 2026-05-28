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
//! No event/message-thunk wiring yet (that is Stage 2). "Messages" here are
//! just external state updates via [`ServalAppRunner::update`]; the timer tick
//! that drives them is the caller's concern.

use layout_dom_api::{LayoutDom, LayoutDomMut};
use serval_scripted_dom::NodeId;
use xilem_core::View;

use crate::{DomHandle, ServalCtx, ServalElement, ServalElementMut};

/// Owns the app state, the view-producing logic, and the retained view tree,
/// rebuilding the [`ScriptedDom`] whenever the state changes.
///
/// `Logic: FnMut(&State) -> V` is the app-logic closure (Xilem's `app_logic`,
/// minus the `&mut State` — Stage 1b has no message handlers mutating state
/// from inside a view, so the logic only *reads* state). `V` is the root view;
/// its element is always the uniform [`ServalElement`].
pub struct ServalAppRunner<State, Logic, V>
where
    State: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, (), ServalCtx, Element = ServalElement>,
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
}

impl<State, Logic, V> ServalAppRunner<State, Logic, V>
where
    State: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, (), ServalCtx, Element = ServalElement>,
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
        }
    }

    /// Apply a state update, then rebuild the view tree against the new state.
    ///
    /// `f` is the externally-driven "message": it mutates the owned state. The
    /// runner then reruns the logic and diffs the produced view against the
    /// retained one, emitting `DomMutation`s into the `ScriptedDom`.
    ///
    /// The disjoint-field borrows matter: `rebuild` needs `&prev_view`,
    /// `&mut view_state`, `&mut ctx`, the root `Mut`, and `&mut state` all at
    /// once, so the fields are destructured into separate `&mut`s. The root
    /// `ServalElementMut` is constructed exactly as the `Harness::rebuild` does
    /// — borrowing `root.node` so a view *could* swap the root node, and
    /// cloning the shared `dom` handle.
    pub fn update(&mut self, f: impl FnOnce(&mut State)) {
        f(&mut self.state);

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
