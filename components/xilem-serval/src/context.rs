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

use std::collections::HashMap;

use crate::DomHandle;
use serval_scripted_dom::NodeId;
use xilem_core::{Environment, ViewId, ViewPathTracker};

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
    /// `NodeId → routing view path` for click handlers. One handler per node is
    /// enough for Stage 2b (a node carries at most one `on_click`); the path is
    /// the `view_path()` captured inside the handler's `with_id`, ending in
    /// `ON_CLICK_ID`.
    click_handlers: HashMap<NodeId, Vec<ViewId>>,
    /// `NodeId → routing view path` for key handlers, the parallel of
    /// [`click_handlers`](Self::click_handlers). The path is the `view_path()`
    /// captured inside [`OnKey`](crate::OnKey)'s `with_id`, ending in
    /// `ON_KEY_ID`. Presence in this map is the definition of *focusable*.
    key_handlers: HashMap<NodeId, Vec<ViewId>>,
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
        }
    }

    /// The document handle this context mutates.
    pub fn dom(&self) -> DomHandle {
        self.dom.clone()
    }

    /// Register `path` as the routing path for click events targeting `node`.
    ///
    /// Called by [`OnClick::build`](crate::OnClick) (and on rebuild when the
    /// wrapped node changes). `path` is the `view_path()` captured *inside* the
    /// handler's `with_id`, so it ends in `ON_CLICK_ID` and routes straight to
    /// the handler's `message`.
    pub fn register_click(&mut self, node: NodeId, path: Vec<ViewId>) {
        self.click_handlers.insert(node, path);
    }

    /// Drop the click handler registered for `node` (teardown, or before a
    /// re-register onto a different node).
    pub fn unregister_click(&mut self, node: NodeId) {
        self.click_handlers.remove(&node);
    }

    /// The routing view path of the click handler on `node`, if one is
    /// registered. The runner's dispatch walk consults this per ancestor.
    pub fn click_path(&self, node: NodeId) -> Option<&[ViewId]> {
        self.click_handlers.get(&node).map(Vec::as_slice)
    }

    /// Register `path` as the routing path for key events targeting `node`,
    /// which also marks `node` focusable.
    ///
    /// Called by [`OnKey::build`](crate::OnKey) (and on rebuild when the wrapped
    /// node changes). `path` is the `view_path()` captured *inside* the handler's
    /// `with_id`, so it ends in `ON_KEY_ID` and routes straight to the handler's
    /// `message`.
    pub fn register_key(&mut self, node: NodeId, path: Vec<ViewId>) {
        self.key_handlers.insert(node, path);
    }

    /// Drop the key handler registered for `node` (teardown, or before a
    /// re-register onto a different node). This also un-marks `node` focusable.
    pub fn unregister_key(&mut self, node: NodeId) {
        self.key_handlers.remove(&node);
    }

    /// The routing view path of the key handler on `node`, if one is registered.
    ///
    /// `Some(_)` also means `node` is *focusable*: the runner's
    /// [`dispatch_click`](crate::ServalAppRunner::dispatch_click) uses this both
    /// to find the focus target (nearest focusable ancestor of a click) and, in
    /// [`dispatch_key`](crate::ServalAppRunner::dispatch_key), to route from the
    /// focused node up its ancestor chain.
    pub fn key_path(&self, node: NodeId) -> Option<&[ViewId]> {
        self.key_handlers.get(&node).map(Vec::as_slice)
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
