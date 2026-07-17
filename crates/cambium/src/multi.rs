/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`GenetMultiRunner`]: one app state, N windows, each a projection
//! (one-state-N-windows design, step 2).
//!
//! One runner owns one `State`. Each **projection** is a
//! [`RunnerTree`](crate::runner) — its own `ScriptedDom` target, retained view
//! tree, focus, and pointer capture — plus its own view-producing logic (a lens
//! over the shared state). One [`update`](GenetMultiRunner::update) mutates
//! the state once and rebuilds every projection; a dispatch into any window
//! routes through that window's tree, then rebuilds every projection, so a
//! click in window A updates what window B shows in the same pass. Multi-window
//! synced panels stop being a sync feature: there is one state, so there is
//! nothing to synchronize.
//!
//! Projections rebuild in **insertion order**, which is therefore the parking
//! order for portable children: a `PortableKeyed` departure preserves across a
//! move only when its source projection rebuilds before the target
//! (`portable.rs`'s ordering caveat). Hosts arrange tear-out source windows
//! before targets, or accept the safe fresh-build degradation.
//!
//! At this step each projection still owns a distinct `ScriptedDom` (N doms):
//! per-tree nursery draining confines parked elements to their own dom, so a
//! cross-**window** move degrades to fresh-build until the forest dom (design
//! step 3) makes every window a subtree of one document — at which point
//! same-document `move_before` covers cross-window moves too.

use genet_scripted_dom::NodeId;
use meristem::View;

use crate::runner::RunnerTree;
use crate::{DomHandle, GenetCtx, GenetElement, KeyEvent, PointerClick, PointerEvent, WheelEvent};

/// A stable handle to one projection (one OS window's tree). Handles stay
/// valid across removals of *other* projections; slots are not reused, so a
/// stale handle after `remove_projection` simply resolves to nothing.
///
/// The wrapped value is the projection's **slot index** — [`push_projection`]
/// returns `ProjectionId(projections.len())`, and slots are append-only
/// (removed slots tombstone in place, never shift). It is `pub` so a host can
/// keep a parallel per-projection collection index-aligned with the runner's
/// projections (`app.windows[id.0]`), which is exactly how the one-state-N-windows
/// host routes per-window state.
///
/// [`push_projection`]: GenetMultiRunner::push_projection
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectionId(pub usize);

struct Projection<State, Logic, V, Action>
where
    State: 'static,
    Action: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    logic: Logic,
    tree: RunnerTree<State, V, Action>,
}

/// One app state, N per-window projections. See the module docs.
///
/// All projections share one `Logic`/`V` *type* (each window runs the same
/// shell view function over the shared state, parameterized by what it closes
/// over — its window identity, its lens); heterogeneous window types would
/// need boxing and have no consumer yet.
pub struct GenetMultiRunner<State, Logic, V, Action = ()>
where
    State: 'static,
    Action: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    state: State,
    /// Slot-per-projection; `None` = removed (slots are never reused, keeping
    /// [`ProjectionId`]s stable).
    projections: Vec<Option<Projection<State, Logic, V, Action>>>,
}

impl<State, Logic, V, Action> GenetMultiRunner<State, Logic, V, Action>
where
    State: 'static,
    Action: 'static,
    Logic: FnMut(&State) -> V,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    /// A runner over `state` with no projections yet.
    pub fn new(state: State) -> Self {
        Self {
            state,
            projections: Vec::new(),
        }
    }

    /// Add a projection: build `logic`'s view tree over the shared state into
    /// `dom` and attach it under that document's root.
    pub fn push_projection(&mut self, dom: DomHandle, mut logic: Logic) -> ProjectionId {
        let tree = RunnerTree::build(dom, &mut logic, &mut self.state);
        let id = ProjectionId(self.projections.len());
        self.projections.push(Some(Projection { logic, tree }));
        id
    }

    /// Remove a projection (a window closing): tear its view tree down and
    /// detach its root from its document. The shared state is untouched — the
    /// other projections keep rendering it.
    pub fn remove_projection(&mut self, id: ProjectionId) {
        if let Some(slot) = self.projections.get_mut(id.0)
            && let Some(projection) = slot.take()
        {
            projection.tree.teardown();
        }
    }

    /// How many projections are live.
    pub fn projection_count(&self) -> usize {
        self.projections.iter().flatten().count()
    }

    /// Apply a state update, then rebuild **every** projection against the new
    /// state — the one-state contract: no mirroring, no fan-out, each window's
    /// lens re-reads the single truth.
    pub fn update(&mut self, f: impl FnOnce(&mut State)) {
        f(&mut self.state);
        self.rebuild_all();
    }

    /// Apply a state update, then rebuild **only** projection `id`. The per-window
    /// path: a snapshot that belongs to one window (its orrery render, its pane
    /// rows) changes only that window's view, so rebuilding every other projection
    /// would diff to nothing at N x the cost. A change to *shared* state still uses
    /// [`update`](Self::update) so every window re-reads it. A stale `id` (removed
    /// projection) mutates the state but rebuilds nothing.
    pub fn update_local(&mut self, id: ProjectionId, f: impl FnOnce(&mut State)) {
        f(&mut self.state);
        let Self {
            state, projections, ..
        } = self;
        if let Some(Some(projection)) = projections.get_mut(id.0) {
            projection.tree.rebuild(&mut projection.logic, state);
        }
    }

    /// Rebuild every live projection in insertion order (the portable-parking
    /// order — see the module docs).
    fn rebuild_all(&mut self) {
        let Self {
            state, projections, ..
        } = self;
        for projection in projections.iter_mut().flatten() {
            projection.tree.rebuild(&mut projection.logic, state);
        }
    }

    /// Rebuild every live projection except `skip` (whose own dispatch already
    /// rebuilt it with the final state).
    fn rebuild_others(&mut self, skip: ProjectionId) {
        let Self {
            state, projections, ..
        } = self;
        for (index, projection) in projections.iter_mut().enumerate() {
            if index == skip.0 {
                continue;
            }
            if let Some(projection) = projection {
                projection.tree.rebuild(&mut projection.logic, state);
            }
        }
    }

    /// Dispatch a click that hit `target` in projection `id`'s window, then
    /// rebuild every other projection — a handler's state mutation is shared
    /// truth, so every window reflects it in the same pass.
    pub fn dispatch_click(
        &mut self,
        id: ProjectionId,
        target: NodeId,
        event: PointerClick,
    ) -> Vec<Action> {
        let actions = {
            let Self {
                state, projections, ..
            } = self;
            let Some(Some(projection)) = projections.get_mut(id.0) else {
                return Vec::new();
            };
            projection
                .tree
                .dispatch_click(&mut projection.logic, state, target, event)
        };
        self.rebuild_others(id);
        actions
    }

    /// Dispatch a key event to projection `id`'s focused node (focus is
    /// per-window), then rebuild every other projection.
    pub fn dispatch_key(&mut self, id: ProjectionId, event: KeyEvent) -> Vec<Action> {
        let actions = {
            let Self {
                state, projections, ..
            } = self;
            let Some(Some(projection)) = projections.get_mut(id.0) else {
                return Vec::new();
            };
            projection
                .tree
                .dispatch_key(&mut projection.logic, state, event)
        };
        self.rebuild_others(id);
        actions
    }

    /// Begin a pointer drag in projection `id`'s window (capture is
    /// per-window), then rebuild every other projection.
    pub fn dispatch_pointer_down(
        &mut self,
        id: ProjectionId,
        target: NodeId,
        event: PointerEvent,
    ) -> Vec<Action> {
        let actions = {
            let Self {
                state, projections, ..
            } = self;
            let Some(Some(projection)) = projections.get_mut(id.0) else {
                return Vec::new();
            };
            projection
                .tree
                .dispatch_pointer_down(&mut projection.logic, state, target, event)
        };
        self.rebuild_others(id);
        actions
    }

    /// Route a drag `Move` in projection `id`'s window.
    pub fn dispatch_pointer_move(&mut self, id: ProjectionId, event: PointerEvent) -> Vec<Action> {
        let actions = {
            let Self {
                state, projections, ..
            } = self;
            let Some(Some(projection)) = projections.get_mut(id.0) else {
                return Vec::new();
            };
            projection
                .tree
                .dispatch_pointer_move(&mut projection.logic, state, event)
        };
        self.rebuild_others(id);
        actions
    }

    /// Route a drag `Up` in projection `id`'s window and end its capture.
    pub fn dispatch_pointer_up(&mut self, id: ProjectionId, event: PointerEvent) -> Vec<Action> {
        let actions = {
            let Self {
                state, projections, ..
            } = self;
            let Some(Some(projection)) = projections.get_mut(id.0) else {
                return Vec::new();
            };
            projection
                .tree
                .dispatch_pointer_up(&mut projection.logic, state, event)
        };
        self.rebuild_others(id);
        actions
    }

    /// Route a wheel notch in projection `id`'s window.
    pub fn dispatch_wheel(
        &mut self,
        id: ProjectionId,
        target: NodeId,
        event: WheelEvent,
    ) -> Vec<Action> {
        let actions = {
            let Self {
                state, projections, ..
            } = self;
            let Some(Some(projection)) = projections.get_mut(id.0) else {
                return Vec::new();
            };
            projection
                .tree
                .dispatch_wheel(&mut projection.logic, state, target, event)
        };
        self.rebuild_others(id);
        actions
    }

    /// Projection `id`'s document handle.
    pub fn dom(&self, id: ProjectionId) -> Option<DomHandle> {
        self.projection(id).map(|p| p.tree.dom())
    }

    /// Projection `id`'s root node.
    pub fn root(&self, id: ProjectionId) -> Option<NodeId> {
        self.projection(id).map(|p| p.tree.root())
    }

    /// Projection `id`'s focused node (focus is per-window).
    pub fn focus(&self, id: ProjectionId) -> Option<NodeId> {
        self.projection(id).and_then(|p| p.tree.focus())
    }

    /// Set (or clear) projection `id`'s focused node.
    pub fn set_focus(&mut self, id: ProjectionId, node: Option<NodeId>) {
        let Self {
            state, projections, ..
        } = self;
        if let Some(Some(projection)) = projections.get_mut(id.0) {
            projection
                .tree
                .set_focus(&mut projection.logic, state, node);
        }
        self.rebuild_others(id);
    }

    /// Whether projection `id`'s most recent dispatch was default-prevented.
    pub fn default_prevented(&self, id: ProjectionId) -> bool {
        self.projection(id)
            .is_some_and(|p| p.tree.default_prevented())
    }

    /// The element a press on `hit` would capture in projection `id`.
    pub fn pointer_target(&self, id: ProjectionId, hit: NodeId) -> Option<NodeId> {
        self.projection(id).and_then(|p| p.tree.pointer_target(hit))
    }

    /// The element a wheel on `hit` would scroll in projection `id`.
    pub fn wheel_target(&self, id: ProjectionId, hit: NodeId) -> Option<NodeId> {
        self.projection(id).and_then(|p| p.tree.wheel_target(hit))
    }

    /// The element capturing projection `id`'s current drag, if any.
    pub fn pointer_capture(&self, id: ProjectionId) -> Option<NodeId> {
        self.projection(id).and_then(|p| p.tree.pointer_capture())
    }

    /// The current app state — the one truth every projection renders.
    pub fn state(&self) -> &State {
        &self.state
    }

    fn projection(&self, id: ProjectionId) -> Option<&Projection<State, Logic, V, Action>> {
        self.projections.get(id.0).and_then(Option::as_ref)
    }
}
