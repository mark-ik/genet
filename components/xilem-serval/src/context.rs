/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The view context for the serval backend.
//!
//! Mirrors `xilem_web`'s `ViewCtx`, minus the browser-only state (document
//! fragment, hydration node stack, modifier size hints). It holds the `id_path`
//! used for message routing, the [`Environment`], and a shared handle to the
//! [`ScriptedDom`] every view mutates.

use crate::DomHandle;
use xilem_core::{Environment, ViewId, ViewPathTracker};

/// The [`ViewPathTracker`] context for all serval views.
///
/// Stage 1a carries no `AppRunner`/message-thunk wiring (that is Stage 1b's
/// `ServalAppRunner`); the context exists so the `View` traits can be driven
/// directly by a test.
pub struct ServalCtx {
    id_path: Vec<ViewId>,
    environment: Environment,
    dom: DomHandle,
}

impl ServalCtx {
    /// Create a context over an existing document handle.
    pub fn new(dom: DomHandle) -> Self {
        Self {
            id_path: Vec::new(),
            environment: Environment::new(),
            dom,
        }
    }

    /// The document handle this context mutates.
    pub fn dom(&self) -> DomHandle {
        self.dom.clone()
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
