/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The view context for the Genet backend.
//!
//! It holds the `id_path` used for message routing, the [`Environment`], a
//! shared handle to the [`ScriptedDom`] every view mutates, event-handler
//! registries, focusability markers, and the portable-child nursery.
//!
//! Each click or key handler has a propagation phase ([`Handler::capture`]): a
//! listener registered with `capture == true` fires in
//! the `root → target` capture pass, one with `capture == false` (the
//! browser/`xilem_web` default) in the `target → root` bubble pass. A node may
//! carry *several* listeners of one kind (nested `on_click`s over one element, a
//! handler beside an instrumentation listener), so each registry maps a node to a
//! `Vec` of [`Handler`]s and dispatch routes every one — in registration order
//! within the matching phase — rather than letting a later listener silently
//! clobber an earlier one.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::hash::Hash;

use crate::DomHandle;
use crate::pod::{GenetElement, GenetElementMut};
use genet_scripted_dom::NodeId;
use layout_dom_api::{LayoutDom, LayoutDomMut};
use meristem::{Environment, View, ViewId, ViewPathTracker};

/// A registered event handler: its routing view path plus the propagation phase
/// it listens in.
///
/// A node maps to a `Vec` of these per event type — usually one, but several when
/// listeners stack on one element. `path` is the `view_path()` captured inside the
/// handler's `with_id` (so it ends in the handler's marker id and routes straight
/// to its `message`), and it uniquely identifies this listener for removal;
/// `capture` is the per-listener phase set by
/// [`OnClick::capture`](crate::OnClick::capture) /
/// [`OnKey::capture`](crate::OnKey::capture) — `true` = capture phase
/// (`root → target`), `false` (default) = bubble phase (`target → root`).
#[derive(Clone, Debug)]
pub struct Handler {
    /// The routing view path to the handler's `message`.
    pub path: Vec<ViewId>,
    /// The phase this listener fires in: `true` = capture, `false` = bubble.
    pub capture: bool,
    /// Whether this handler makes its node a Tab/click focus target. Click
    /// handlers set this to `false`; key handlers default to `true` and may opt
    /// out when they only observe keys bubbling from descendants.
    pub focusable: bool,
}

/// The [`ViewPathTracker`] context for all Genet views.
///
/// The context tracks the current Meristem view path, the shared DOM, and the
/// routing registries built by event views. [`OnClick`](crate::OnClick) and
/// [`OnKey`](crate::OnKey) record their view paths against the DOM node they
/// wrap. [`GenetAppRunner`](crate::GenetAppRunner) walks the hit or focused
/// node's ancestors and routes messages down those retained paths.
///
/// Key handlers and explicit [`focusable`](crate::focusable) markers together
/// define the focus traversal set. Pointer, hover, and wheel handlers use the
/// same context for drag capture, hover transitions, and scroll routing.
pub struct GenetCtx {
    id_path: Vec<ViewId>,
    environment: Environment,
    dom: DomHandle,
    /// `NodeId → `[`Handler`]s for click handlers, in registration order. Usually
    /// one, but a node can carry several stacked `on_click`s; each [`Handler::path`]
    /// is the `view_path()` captured inside that handler's `with_id` (ending in
    /// `ON_CLICK_ID`) and [`Handler::capture`] its propagation phase.
    click_handlers: HashMap<NodeId, Vec<Handler>>,
    /// `NodeId → `[`Handler`]s for key handlers, the parallel of
    /// [`click_handlers`](Self::click_handlers). Each path is the `view_path()`
    /// captured inside an [`OnKey`](crate::OnKey)'s `with_id`, ending in
    /// `ON_KEY_ID`. Handlers whose [`Handler::focusable`] bit is true contribute
    /// to the focus set regardless of phase; the other source is an explicit
    /// [`focusable`](crate::focusable) marker, tracked in
    /// [`focusable`](Self::focusable).
    key_handlers: HashMap<NodeId, Vec<Handler>>,
    /// Nodes marked focusable by an explicit [`focusable`](crate::focusable) view,
    /// independent of any key handler — so a plain `on_click` button (no `on_key`)
    /// can still be tab-reached and Enter/Space-activated. The `usize` is a
    /// refcount, so stacked markers and node-swap rebuilds stay balanced. Read
    /// (with [`key_handlers`](Self::key_handlers)) by [`is_focusable`](Self::is_focusable).
    focusable: HashMap<NodeId, usize>,
    /// `NodeId → routing path` for pointer-drag handlers
    /// ([`OnPointer`](crate::OnPointer)). Unlike click/key there is no
    /// capture/bubble phase: a drag routes straight to the captured target, so
    /// the value is just the path (ending in `ON_POINTER_ID`). The runner walks a
    /// hit node's ancestor chain through this on `pointerdown` to find the
    /// element that captures the drag.
    pointer_handlers: HashMap<NodeId, Vec<ViewId>>,
    /// `NodeId → routing path` for pointer-hover handlers
    /// ([`OnHover`](crate::OnHover)). Hover routes directly to the nearest
    /// registered ancestor selected by the host's hit-test transition.
    hover_handlers: HashMap<NodeId, Vec<ViewId>>,
    /// `NodeId → routing path` for wheel/scroll handlers
    /// ([`OnWheel`](crate::OnWheel)). Like pointer there is no capture/bubble
    /// phase: a wheel routes to the nearest scroll-handling ancestor of the hit
    /// node, so the value is just the path (ending in `ON_WHEEL_ID`). The runner
    /// walks the hit node's ancestor chain through this on a wheel event to find
    /// the element that handles the scroll.
    wheel_handlers: HashMap<NodeId, Vec<ViewId>>,
    /// The portable-child nursery (moveBefore plan S5, cross-parent): children a
    /// [`PortableKeyed`](crate::PortableKeyed) parked because their key left its
    /// list, waiting within the same rebuild pass to be claimed by the sequence
    /// the key arrived in. Buckets are per concrete `(K, V)` instantiation,
    /// type-erased; the runner drains unclaimed children at the end of every
    /// rebuild ([`drain_nursery`](Self::drain_nursery)), tearing them down for
    /// real. A parked child's DOM node stays attached under its former parent
    /// until adoption moves it or the drain removes it.
    nursery: HashMap<TypeId, Box<dyn NurseryBucket>>,
}

/// A parked portable child: the previous view (an owned clone), its retained
/// view state, and its element — everything an adoption's rebuild or a drain's
/// teardown needs.
struct ParkedChild<V, VS> {
    view: V,
    state: VS,
    element: GenetElement,
}

/// One nursery bucket per concrete `(K, V, V::ViewState)` instantiation. The
/// monomorphized `teardown` fn pointer is captured at park time, so the bucket
/// itself needs no `View` bounds and the drain no type knowledge.
struct Bucket<K, V, VS> {
    parked: HashMap<K, ParkedChild<V, VS>>,
    teardown: fn(V, VS, GenetElement, &mut GenetCtx),
}

/// Type-erased nursery bucket: `Any` for the typed claim, plus the drain hook.
trait NurseryBucket {
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn drain_teardowns(&mut self, ctx: &mut GenetCtx);
}

impl<K, V, VS> NurseryBucket for Bucket<K, V, VS>
where
    K: Eq + Hash + 'static,
    V: 'static,
    VS: 'static,
{
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn drain_teardowns(&mut self, ctx: &mut GenetCtx) {
        for (_key, parked) in self.parked.drain() {
            (self.teardown)(parked.view, parked.state, parked.element, ctx);
        }
    }
}

/// Tear down an unclaimed parked child for real: run the view teardown (which
/// unregisters its handlers by their stored paths), then remove the node — it
/// was left attached under its former parent by the park.
fn teardown_parked<State, Action, V>(
    view: V,
    mut state: V::ViewState,
    mut element: GenetElement,
    ctx: &mut GenetCtx,
) where
    State: 'static,
    Action: 'static,
    V: View<State, Action, GenetCtx, Element = GenetElement>,
{
    let node = element.node;
    let parent = ctx.dom.borrow().parent(node);
    let el = GenetElementMut {
        node: &mut element.node,
        dom: ctx.dom.clone(),
        parent,
    };
    view.teardown(&mut state, ctx, el);
    ctx.dom.borrow_mut().remove(node);
}

impl GenetCtx {
    /// Create a context over an existing document handle.
    pub fn new(dom: DomHandle) -> Self {
        Self {
            id_path: Vec::new(),
            environment: Environment::new(),
            dom,
            click_handlers: HashMap::new(),
            key_handlers: HashMap::new(),
            focusable: HashMap::new(),
            pointer_handlers: HashMap::new(),
            hover_handlers: HashMap::new(),
            wheel_handlers: HashMap::new(),
            nursery: HashMap::new(),
        }
    }

    /// Park a portable child whose key left its [`PortableKeyed`](crate::PortableKeyed)
    /// list: keep its (previous) view, view state, and element alive so the
    /// sequence its key arrives in — later in this same rebuild pass — can adopt
    /// it wholesale. The child's DOM node stays attached under its former parent
    /// until adoption moves it (one atomic `Moved`) or the end-of-rebuild
    /// [`drain_nursery`](Self::drain_nursery) tears it down. (moveBefore S5.)
    pub fn park_portable<State, Action, K, V>(
        &mut self,
        key: K,
        view: V,
        state: V::ViewState,
        element: GenetElement,
    ) where
        State: 'static,
        Action: 'static,
        K: Eq + Hash + 'static,
        V: View<State, Action, GenetCtx, Element = GenetElement> + 'static,
        V::ViewState: 'static,
    {
        let bucket = self
            .nursery
            .entry(TypeId::of::<Bucket<K, V, V::ViewState>>())
            .or_insert_with(|| {
                Box::new(Bucket::<K, V, V::ViewState> {
                    parked: HashMap::new(),
                    teardown: teardown_parked::<State, Action, V>,
                })
            });
        let bucket = bucket
            .as_any_mut()
            .downcast_mut::<Bucket<K, V, V::ViewState>>()
            .expect("nursery bucket keyed by its own TypeId");
        bucket.parked.insert(
            key,
            ParkedChild {
                view,
                state,
                element,
            },
        );
    }

    /// Claim a parked portable child by key, if one of this exact `(K, V)`
    /// instantiation was parked earlier in the current rebuild pass. Returns the
    /// previous view (for the rebuild diff), the retained view state, and the
    /// still-attached element.
    pub fn claim_portable<State, Action, K, V>(
        &mut self,
        key: &K,
    ) -> Option<(V, V::ViewState, GenetElement)>
    where
        State: 'static,
        Action: 'static,
        K: Eq + Hash + 'static,
        V: View<State, Action, GenetCtx, Element = GenetElement> + 'static,
        V::ViewState: 'static,
    {
        let bucket = self
            .nursery
            .get_mut(&TypeId::of::<Bucket<K, V, V::ViewState>>())?
            .as_any_mut()
            .downcast_mut::<Bucket<K, V, V::ViewState>>()?;
        let parked = bucket.parked.remove(key)?;
        Some((parked.view, parked.state, parked.element))
    }

    /// Tear down every still-parked child. The runner calls this at the end of
    /// each rebuild: a parked child not claimed within its own pass really is
    /// gone (its key left every portable list), so it gets an ordinary teardown
    /// and its node is removed. Loops until quiescent, since a teardown can
    /// itself park nested portable children.
    pub fn drain_nursery(&mut self) {
        while !self.nursery.is_empty() {
            let mut nursery = std::mem::take(&mut self.nursery);
            for bucket in nursery.values_mut() {
                bucket.drain_teardowns(self);
            }
        }
    }

    /// The document handle this context mutates.
    pub fn dom(&self) -> DomHandle {
        self.dom.clone()
    }

    /// Take the environment out (leaving a fresh empty one) to thread the *real*
    /// environment through the dispatch message cycle: hand it to
    /// [`MessageCtx::new`](meristem::MessageCtx::new), then return what
    /// `MessageCtx::finish` gives back via [`set_environment`](Self::set_environment).
    /// `Environment` is not `Clone`, so this take / restore is how dispatch shares
    /// one environment with build (which reads `self.environment` directly through
    /// the [`ViewPathTracker`] accessor) rather than routing against a throwaway
    /// `Environment::new()`. (Grab-bag G2.2.)
    pub fn take_environment(&mut self) -> Environment {
        std::mem::replace(&mut self.environment, Environment::new())
    }

    /// Restore the environment after a dispatch message cycle (see
    /// [`take_environment`](Self::take_environment)).
    pub fn set_environment(&mut self, environment: Environment) {
        self.environment = environment;
    }

    /// Register `path` (in phase `capture`) as *a* click handler for `node`,
    /// appended after any already there.
    ///
    /// Called by [`OnClick::build`](crate::OnClick) (and on rebuild when the
    /// wrapped node changes). `path` is the `view_path()` captured *inside* the
    /// handler's `with_id`, so it ends in `ON_CLICK_ID` and routes straight to
    /// the handler's `message`; `capture` is the per-listener phase
    /// ([`OnClick::capture`](crate::OnClick::capture), default `false` = bubble).
    /// Idempotent per `path`: re-registering the same path updates its phase in
    /// place rather than duplicating, so a redundant rebuild can't grow the list.
    pub fn register_click(&mut self, node: NodeId, path: Vec<ViewId>, capture: bool) {
        let handlers = self.click_handlers.entry(node).or_default();
        handlers.retain(|h| h.path != path);
        handlers.push(Handler {
            path,
            capture,
            focusable: false,
        });
    }

    /// Drop the click handler with `path` from `node` (teardown, or before a
    /// re-register onto a different node), leaving any sibling listeners. The
    /// node's entry is removed once its last handler goes.
    pub fn unregister_click(&mut self, node: NodeId, path: &[ViewId]) {
        if let Some(handlers) = self.click_handlers.get_mut(&node) {
            handlers.retain(|h| h.path != path);
            if handlers.is_empty() {
                self.click_handlers.remove(&node);
            }
        }
    }

    /// The click handlers on `node`, in registration order (empty if none).
    /// The runner's dispatch walk consults this per ancestor, in both the capture
    /// and bubble passes, routing every listener in the matching phase.
    pub fn click_handlers_at(&self, node: NodeId) -> &[Handler] {
        self.click_handlers.get(&node).map_or(&[], Vec::as_slice)
    }

    /// Register `path` (in phase `capture`) as *a* key handler for `node`.
    /// `focusable` controls whether this listener also puts the node in the
    /// focus traversal set. Appended after any already there.
    ///
    /// Called by [`OnKey::build`](crate::OnKey) (and on rebuild when the wrapped
    /// node changes). `path` is the `view_path()` captured *inside* the handler's
    /// `with_id`, so it ends in `ON_KEY_ID` and routes straight to the handler's
    /// `message`; `capture` is the per-listener phase
    /// ([`OnKey::capture`](crate::OnKey::capture), default `false` = bubble).
    /// Idempotent per `path`, like [`register_click`](Self::register_click).
    pub fn register_key(
        &mut self,
        node: NodeId,
        path: Vec<ViewId>,
        capture: bool,
        focusable: bool,
    ) {
        let handlers = self.key_handlers.entry(node).or_default();
        handlers.retain(|h| h.path != path);
        handlers.push(Handler {
            path,
            capture,
            focusable,
        });
    }

    /// Drop the key handler with `path` from `node`, leaving any siblings. When
    /// the last focusable key handler goes, `node` is no longer focusable via a
    /// handler (an explicit [`focusable`](crate::focusable) marker still counts).
    pub fn unregister_key(&mut self, node: NodeId, path: &[ViewId]) {
        if let Some(handlers) = self.key_handlers.get_mut(&node) {
            handlers.retain(|h| h.path != path);
            if handlers.is_empty() {
                self.key_handlers.remove(&node);
            }
        }
    }

    /// The key handlers on `node`, in registration order (empty if none).
    ///
    /// A handler whose [`Handler::focusable`] is true also makes `node`
    /// focusable, independent of phase. The runner's
    /// [`dispatch_click`](crate::GenetAppRunner::dispatch_click) uses
    /// [`is_focusable`](Self::is_focusable) to find the focus target (nearest
    /// focusable ancestor of a click), and
    /// [`dispatch_key`](crate::GenetAppRunner::dispatch_key) routes from the
    /// focused node up its ancestor chain through the per-phase passes.
    pub fn key_handlers_at(&self, node: NodeId) -> &[Handler] {
        self.key_handlers.get(&node).map_or(&[], Vec::as_slice)
    }

    /// Mark `node` focusable explicitly (an [`focusable`](crate::focusable)
    /// marker), independent of any key handler. Refcounted, so stacked markers
    /// and node-swap rebuilds balance.
    pub fn register_focusable(&mut self, node: NodeId) {
        *self.focusable.entry(node).or_insert(0) += 1;
    }

    /// Drop one explicit focusable mark from `node` (teardown / re-key on node
    /// swap); the node stays focusable while any mark or key handler remains.
    pub fn unregister_focusable(&mut self, node: NodeId) {
        if let Some(count) = self.focusable.get_mut(&node) {
            *count -= 1;
            if *count == 0 {
                self.focusable.remove(&node);
            }
        }
    }

    /// Whether `node` is *focusable*: it carries a focusable key handler (in
    /// either phase) or an explicit [`focusable`](crate::focusable) marker.
    pub fn is_focusable(&self, node: NodeId) -> bool {
        self.key_handlers
            .get(&node)
            .is_some_and(|handlers| handlers.iter().any(|handler| handler.focusable))
            || self.focusable.contains_key(&node)
    }

    /// Register `path` as the pointer-drag handler for `node`
    /// ([`OnPointer`](crate::OnPointer) build/rebuild). The path ends in
    /// `ON_POINTER_ID` and routes straight to the handler's `message`.
    pub fn register_pointer(&mut self, node: NodeId, path: Vec<ViewId>) {
        self.pointer_handlers.insert(node, path);
    }

    /// Drop the pointer handler for `node` (teardown / re-key on node swap).
    pub fn unregister_pointer(&mut self, node: NodeId) {
        self.pointer_handlers.remove(&node);
    }

    /// The pointer-drag routing path on `node`, if one is registered. The
    /// runner's `dispatch_pointer_down` walks the hit node's ancestors through
    /// this to find the drag-capturing element.
    pub fn pointer_handler(&self, node: NodeId) -> Option<&[ViewId]> {
        self.pointer_handlers.get(&node).map(Vec::as_slice)
    }

    /// Register `path` as the hover handler for `node`.
    pub fn register_hover(&mut self, node: NodeId, path: Vec<ViewId>) {
        self.hover_handlers.insert(node, path);
    }

    /// Drop the hover handler for `node` (teardown / re-key on node swap).
    pub fn unregister_hover(&mut self, node: NodeId) {
        self.hover_handlers.remove(&node);
    }

    /// The hover routing path on `node`, if one is registered.
    pub fn hover_handler(&self, node: NodeId) -> Option<&[ViewId]> {
        self.hover_handlers.get(&node).map(Vec::as_slice)
    }

    /// Register `path` as the wheel handler for `node`
    /// ([`OnWheel`](crate::OnWheel) build/rebuild). The path ends in
    /// `ON_WHEEL_ID` and routes straight to the handler's `message`.
    pub fn register_wheel(&mut self, node: NodeId, path: Vec<ViewId>) {
        self.wheel_handlers.insert(node, path);
    }

    /// Drop the wheel handler for `node` (teardown / re-key on node swap).
    pub fn unregister_wheel(&mut self, node: NodeId) {
        self.wheel_handlers.remove(&node);
    }

    /// The wheel routing path on `node`, if one is registered. The runner's
    /// `dispatch_wheel` walks the hit node's ancestors through this to find the
    /// scroll-handling element.
    pub fn wheel_handler(&self, node: NodeId) -> Option<&[ViewId]> {
        self.wheel_handlers.get(&node).map(Vec::as_slice)
    }
}

impl ViewPathTracker for GenetCtx {
    fn environment(&mut self) -> &mut Environment {
        &mut self.environment
    }

    fn push_id(&mut self, id: ViewId) {
        self.id_path.push(id);
    }

    fn pop_id(&mut self) {
        self.id_path.pop();
    }

    fn view_path(&mut self) -> &[ViewId] {
        &self.id_path
    }
}
