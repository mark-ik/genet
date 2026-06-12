/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The view context for the serval backend.
//!
//! Mirrors `xilem_web`'s `ViewCtx`, minus the browser-only state (document
//! fragment, hydration node stack, modifier size hints). It holds the `id_path`
//! used for message routing, the [`Environment`], a shared handle to the
//! [`ScriptedDom`] every view mutates, and the native click-handler registry
//! (Stage 2b's stand-in for the browser's `addEventListener`) plus the parallel
//! key-handler registry (Stage 3b, which also defines focusability).
//!
//! The capture-phase slice of Stage 3 gives each registered handler a *phase*
//! ([`Handler::capture`]): a listener registered with `capture == true` fires in
//! the `root → target` capture pass, one with `capture == false` (the
//! browser/`xilem_web` default) in the `target → root` bubble pass. A node still
//! has at most one click and one key handler, so the registry value carries the
//! phase alongside the routing path rather than holding a list.

use std::collections::HashMap;

use crate::DomHandle;
use serval_scripted_dom::NodeId;
use xilem_core::{Environment, ViewId, ViewPathTracker};

/// A registered event handler: its routing view path plus the propagation phase
/// it listens in.
///
/// One per node per event type (a node carries at most one `on_click` and one
/// `on_key`). `path` is the `view_path()` captured inside the handler's
/// `with_id` (so it ends in the handler's marker id and routes straight to its
/// `message`); `capture` is the per-listener phase set by
/// [`OnClick::capture`](crate::OnClick::capture) /
/// [`OnKey::capture`](crate::OnKey::capture) — `true` = capture phase
/// (`root → target`), `false` (default) = bubble phase (`target → root`).
#[derive(Clone, Debug)]
pub struct Handler {
    /// The routing view path to the handler's `message`.
    pub path: Vec<ViewId>,
    /// The phase this listener fires in: `true` = capture, `false` = bubble.
    pub capture: bool,
}

/// The [`ViewPathTracker`] context for all serval views.
///
/// Stage 1a carries no `AppRunner`/message-thunk wiring (that is Stage 1b's
/// `ServalAppRunner`); the context exists so the `View` traits can be driven
/// directly by a test.
///
/// Stage 2b adds the [`click_handlers`](Self::click_handlers) registry: the
/// faithful-routing replacement for `xilem_web`'s browser listener. There is no
/// `addEventListener` here; instead an [`OnClick`](crate::OnClick) view, on
/// build, records the routing **view path** to itself keyed by the DOM
/// [`NodeId`] it wraps. Native dispatch (the runner) walks the hit node's
/// ancestor chain, looks each node up here, and routes a message down the
/// recorded path — exactly the `id_path` Xilem's message cycle expects.
///
/// Stage 3b adds the parallel [`key_handlers`](Self::key_handlers) registry,
/// populated by [`OnKey`](crate::OnKey) the same way. It does double duty: it
/// is both the key-event routing table *and* the focusability set — a node is
/// focusable iff it carries a key handler (i.e. is present here). The runner's
/// [`dispatch_click`](crate::ServalAppRunner::dispatch_click) consults it to
/// move focus, and [`dispatch_key`](crate::ServalAppRunner::dispatch_key) walks
/// it from the focused node.
pub struct ServalCtx {
    id_path: Vec<ViewId>,
    environment: Environment,
    dom: DomHandle,
    /// `NodeId → `[`Handler`] for click handlers. One handler per node is
    /// enough (a node carries at most one `on_click`); the [`Handler::path`] is
    /// the `view_path()` captured inside the handler's `with_id`, ending in
    /// `ON_CLICK_ID`, and [`Handler::capture`] is its propagation phase.
    click_handlers: HashMap<NodeId, Handler>,
    /// `NodeId → `[`Handler`] for key handlers, the parallel of
    /// [`click_handlers`](Self::click_handlers). The path is the `view_path()`
    /// captured inside [`OnKey`](crate::OnKey)'s `with_id`, ending in
    /// `ON_KEY_ID`. Presence in this map is the definition of *focusable*,
    /// **regardless of the handler's phase**.
    key_handlers: HashMap<NodeId, Handler>,
    /// `NodeId → routing path` for pointer-drag handlers
    /// ([`OnPointer`](crate::OnPointer)). Unlike click/key there is no
    /// capture/bubble phase: a drag routes straight to the captured target, so
    /// the value is just the path (ending in `ON_POINTER_ID`). The runner walks a
    /// hit node's ancestor chain through this on `pointerdown` to find the
    /// element that captures the drag.
    pointer_handlers: HashMap<NodeId, Vec<ViewId>>,
    /// `NodeId → routing path` for wheel/scroll handlers
    /// ([`OnWheel`](crate::OnWheel)). Like pointer there is no capture/bubble
    /// phase: a wheel routes to the nearest scroll-handling ancestor of the hit
    /// node, so the value is just the path (ending in `ON_WHEEL_ID`). The runner
    /// walks the hit node's ancestor chain through this on a wheel event to find
    /// the element that handles the scroll.
    wheel_handlers: HashMap<NodeId, Vec<ViewId>>,
}

impl ServalCtx {
    /// Create a context over an existing document handle.
    pub fn new(dom: DomHandle) -> Self {
        Self {
            id_path: Vec::new(),
            environment: Environment::new(),
            dom,
            click_handlers: HashMap::new(),
            key_handlers: HashMap::new(),
            pointer_handlers: HashMap::new(),
            wheel_handlers: HashMap::new(),
        }
    }

    /// The document handle this context mutates.
    pub fn dom(&self) -> DomHandle {
        self.dom.clone()
    }

    /// Register `path` (in phase `capture`) as the click handler for `node`.
    ///
    /// Called by [`OnClick::build`](crate::OnClick) (and on rebuild when the
    /// wrapped node changes). `path` is the `view_path()` captured *inside* the
    /// handler's `with_id`, so it ends in `ON_CLICK_ID` and routes straight to
    /// the handler's `message`; `capture` is the per-listener phase
    /// ([`OnClick::capture`](crate::OnClick::capture), default `false` = bubble).
    pub fn register_click(&mut self, node: NodeId, path: Vec<ViewId>, capture: bool) {
        self.click_handlers.insert(node, Handler { path, capture });
    }

    /// Drop the click handler registered for `node` (teardown, or before a
    /// re-register onto a different node).
    pub fn unregister_click(&mut self, node: NodeId) {
        self.click_handlers.remove(&node);
    }

    /// The click [`Handler`] (routing path + phase) on `node`, if one is
    /// registered. The runner's dispatch walk consults this per ancestor, in
    /// both the capture and bubble passes.
    pub fn click_handler(&self, node: NodeId) -> Option<&Handler> {
        self.click_handlers.get(&node)
    }

    /// Register `path` (in phase `capture`) as the key handler for `node`, which
    /// also marks `node` focusable.
    ///
    /// Called by [`OnKey::build`](crate::OnKey) (and on rebuild when the wrapped
    /// node changes). `path` is the `view_path()` captured *inside* the handler's
    /// `with_id`, so it ends in `ON_KEY_ID` and routes straight to the handler's
    /// `message`; `capture` is the per-listener phase
    /// ([`OnKey::capture`](crate::OnKey::capture), default `false` = bubble).
    pub fn register_key(&mut self, node: NodeId, path: Vec<ViewId>, capture: bool) {
        self.key_handlers.insert(node, Handler { path, capture });
    }

    /// Drop the key handler registered for `node` (teardown, or before a
    /// re-register onto a different node). This also un-marks `node` focusable.
    pub fn unregister_key(&mut self, node: NodeId) {
        self.key_handlers.remove(&node);
    }

    /// The key [`Handler`] (routing path + phase) on `node`, if one is
    /// registered.
    ///
    /// `Some(_)` also means `node` is *focusable* — independent of the handler's
    /// phase: the runner's
    /// [`dispatch_click`](crate::ServalAppRunner::dispatch_click) uses
    /// [`is_focusable`](Self::is_focusable) to find the focus target (nearest
    /// focusable ancestor of a click), and
    /// [`dispatch_key`](crate::ServalAppRunner::dispatch_key) routes from the
    /// focused node up its ancestor chain through the per-phase passes.
    pub fn key_handler(&self, node: NodeId) -> Option<&Handler> {
        self.key_handlers.get(&node)
    }

    /// Whether `node` is *focusable*: it carries a key handler, in **either**
    /// phase. Focusability is "node is in the key registry" and must not depend
    /// on the handler's capture/bubble phase, so click-to-focus keeps working
    /// for a capture-phase key listener too.
    pub fn is_focusable(&self, node: NodeId) -> bool {
        self.key_handlers.contains_key(&node)
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

impl ViewPathTracker for ServalCtx {
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
